use std::collections::BTreeMap;

use super::*;

#[derive(Default)]
pub(crate) struct SharedRemoteRouter {
    next_id: u64,
    entries: BTreeMap<u64, SharedRemoteEntry>,
}

struct SharedRemoteEntry {
    binding: RemoteForwardBinding,
    sender: mpsc::Sender<ForwardedTcpipChannel>,
}

impl SharedRemoteRouter {
    pub(crate) fn sender_for(
        &self,
        address: &str,
        port: u32,
        originator: &str,
        originator_port: u32,
    ) -> Option<mpsc::Sender<ForwardedTcpipChannel>> {
        self.entries
            .values()
            .find(|entry| {
                validated_forwarded_tcpip_metadata(
                    Some(&entry.binding),
                    address,
                    port,
                    originator,
                    originator_port,
                )
                .is_some()
            })
            .map(|entry| entry.sender.clone())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RegistrationState {
    Pending,
    Ready(u16),
    Rejected,
    ParentClosed,
    Cancelled,
    CleanupUncertain(u16),
}

impl SharedSessionChannels {
    /// Retains ownership before sending the SSH request, so timeout and cancellation cannot lose a
    /// pending remote listener or its eventual server-assigned port.
    pub fn begin_remote_forward(&self, address: &str, port: u16) -> Result<SharedRemoteForward> {
        if self.is_closed()
            || address.is_empty()
            || address.len() > MAX_FORWARD_ADDRESS_BYTES
            || address.chars().any(char::is_control)
        {
            return Err(TransportError::Protocol);
        }
        let (sender, receiver) = mpsc::channel(MAX_PENDING_FORWARDED_CHANNELS);
        let id = {
            let mut remote = self
                .remote
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if remote.entries.len() >= 32
                || remote
                    .entries
                    .values()
                    .any(|entry| entry.binding.address == address && entry.binding.port == port)
            {
                return Err(TransportError::ForwardRequestRejected);
            }
            remote.next_id = remote.next_id.saturating_add(1);
            let id = remote.next_id;
            remote.entries.insert(
                id,
                SharedRemoteEntry {
                    binding: RemoteForwardBinding {
                        address: address.to_owned(),
                        port,
                        active: false,
                    },
                    sender,
                },
            );
            id
        };

        let (state_tx, state_rx) = watch::channel(RegistrationState::Pending);
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let parent = self.clone();
        let address = address.to_owned();
        tokio::spawn(run_registration(
            parent.clone(),
            id,
            address.clone(),
            port,
            state_tx,
            cancel_rx,
        ));
        Ok(SharedRemoteForward {
            id,
            address,
            receiver,
            parent,
            state: state_rx,
            cancel: cancel_tx,
            cancellation_confirmed: false,
        })
    }
}

async fn run_registration(
    parent: SharedSessionChannels,
    id: u64,
    address: String,
    requested_port: u16,
    state: watch::Sender<RegistrationState>,
    mut cancel: watch::Receiver<bool>,
) {
    let registration = tokio::select! {
        () = parent.wait_closed() => {
            remove_registration(&parent, id);
            state.send_replace(RegistrationState::ParentClosed);
            return;
        }
        result = parent.handle.tcpip_forward(&address, u32::from(requested_port)) => result,
    };
    let actual_port = match registration {
        Ok(_) if requested_port != 0 => requested_port,
        Ok(assigned) => match u16::try_from(assigned).ok().filter(|port| *port != 0) {
            Some(port) => port,
            None => {
                deactivate_registration(&parent, id);
                state.send_replace(RegistrationState::CleanupUncertain(0));
                return;
            }
        },
        Err(russh::Error::RequestDenied) => {
            remove_registration(&parent, id);
            state.send_replace(RegistrationState::Rejected);
            return;
        }
        Err(_) if parent.is_closed() => {
            remove_registration(&parent, id);
            state.send_replace(RegistrationState::ParentClosed);
            return;
        }
        Err(_) => {
            deactivate_registration(&parent, id);
            state.send_replace(RegistrationState::CleanupUncertain(requested_port));
            return;
        }
    };

    {
        let mut remote = parent
            .remote
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(entry) = remote.entries.get_mut(&id) else {
            return;
        };
        entry.binding.port = actual_port;
        entry.binding.active = !*cancel.borrow();
    }
    if *cancel.borrow() {
        finish_cancellation(&parent, id, &address, actual_port, &state).await;
        return;
    }
    state.send_replace(RegistrationState::Ready(actual_port));

    tokio::select! {
        () = parent.wait_closed() => {
            remove_registration(&parent, id);
            state.send_replace(RegistrationState::ParentClosed);
        }
        () = wait_true(&mut cancel) => {
            deactivate_registration(&parent, id);
            finish_cancellation(&parent, id, &address, actual_port, &state).await;
        }
    }
}

async fn wait_true(receiver: &mut watch::Receiver<bool>) {
    while !*receiver.borrow() {
        if receiver.changed().await.is_err() {
            return;
        }
    }
}

async fn finish_cancellation(
    parent: &SharedSessionChannels,
    id: u64,
    address: &str,
    port: u16,
    state: &watch::Sender<RegistrationState>,
) {
    if parent.is_closed() {
        remove_registration(parent, id);
        state.send_replace(RegistrationState::ParentClosed);
        return;
    }
    let cancelled = timeout(
        parent.timeouts.channel_request,
        parent.handle.cancel_tcpip_forward(address, u32::from(port)),
    )
    .await;
    match cancelled {
        Ok(Ok(())) => {
            remove_registration(parent, id);
            state.send_replace(RegistrationState::Cancelled);
        }
        _ if parent.is_closed() => {
            remove_registration(parent, id);
            state.send_replace(RegistrationState::ParentClosed);
        }
        _ => {
            deactivate_registration(parent, id);
            state.send_replace(RegistrationState::CleanupUncertain(port));
        }
    }
}

fn deactivate_registration(parent: &SharedSessionChannels, id: u64) {
    if let Some(entry) = parent
        .remote
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entries
        .get_mut(&id)
    {
        entry.binding.active = false;
    }
}

fn remove_registration(parent: &SharedSessionChannels, id: u64) {
    parent
        .remote
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entries
        .remove(&id);
}

pub struct SharedRemoteForward {
    id: u64,
    address: String,
    receiver: mpsc::Receiver<ForwardedTcpipChannel>,
    parent: SharedSessionChannels,
    state: watch::Receiver<RegistrationState>,
    cancel: watch::Sender<bool>,
    cancellation_confirmed: bool,
}

impl SharedRemoteForward {
    /// A timeout leaves this ticket owned and its registration task alive, allowing cancellation
    /// to fence a late successful reply.
    pub async fn wait_ready(&mut self) -> Result<u16> {
        timeout(self.parent.timeouts.channel_request, async {
            loop {
                match *self.state.borrow() {
                    RegistrationState::Pending => {}
                    RegistrationState::Ready(port) => return Ok(port),
                    RegistrationState::Rejected | RegistrationState::Cancelled => {
                        return Err(TransportError::ForwardRequestRejected);
                    }
                    RegistrationState::ParentClosed => {
                        return Err(TransportError::ConnectionLost);
                    }
                    RegistrationState::CleanupUncertain(_) => {
                        return Err(TransportError::ForwardRequestTimeout);
                    }
                }
                self.state
                    .changed()
                    .await
                    .map_err(|_| TransportError::ConnectionLost)?;
            }
        })
        .await
        .map_err(|_| TransportError::ForwardRequestTimeout)?
    }

    pub fn actual_bind(&self) -> Option<(&str, u16)> {
        match *self.state.borrow() {
            RegistrationState::Ready(port) | RegistrationState::CleanupUncertain(port)
                if port != 0 =>
            {
                Some((&self.address, port))
            }
            _ => None,
        }
    }

    pub async fn next_channel(&mut self) -> Option<ForwardedTcpipChannel> {
        tokio::select! {
            () = self.parent.wait_closed() => None,
            channel = self.receiver.recv() => channel,
        }
    }

    /// Failure keeps this ticket and router entry available for an exact retry.
    pub async fn cancel(&mut self) -> Result<()> {
        if self.cancellation_confirmed {
            return Ok(());
        }
        self.cancel.send_replace(true);
        let observed_state = *self.state.borrow();
        if let RegistrationState::CleanupUncertain(port) = observed_state
            && port != 0
        {
            let (retry_state, _) = watch::channel(RegistrationState::CleanupUncertain(port));
            finish_cancellation(&self.parent, self.id, &self.address, port, &retry_state).await;
            self.state = retry_state.subscribe();
        }
        let result = timeout(self.parent.timeouts.channel_request, async {
            loop {
                match *self.state.borrow() {
                    RegistrationState::Cancelled | RegistrationState::ParentClosed => return Ok(()),
                    RegistrationState::Rejected => {
                        remove_registration(&self.parent, self.id);
                        return Ok(());
                    }
                    RegistrationState::CleanupUncertain(_) => {
                        return Err(TransportError::ForwardRequestTimeout);
                    }
                    RegistrationState::Pending | RegistrationState::Ready(_) => {}
                }
                self.state
                    .changed()
                    .await
                    .map_err(|_| TransportError::ConnectionLost)?;
            }
        })
        .await
        .map_err(|_| TransportError::ForwardRequestTimeout)?;
        if result.is_ok() {
            self.cancellation_confirmed = true;
        }
        result
    }
}

impl Drop for SharedRemoteForward {
    fn drop(&mut self) {
        if !self.cancellation_confirmed {
            self.cancel.send_replace(true);
        }
    }
}
