//! Core-owned serial-device resources.
//!
//! A protected native surface resolves a short-lived candidate into a
//! FrozenSerialPlan before this module is called. The driver never accepts a
//! guest path, candidate, configuration, permission decision, or identity.

use std::{
    io::{self, Write as _},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
#[cfg(windows)]
use norishell_core_api::MAX_PLUGIN_SERIAL_LABEL_BYTES;
use norishell_core_api::{
    FrozenSerialPlan, MAX_PLUGIN_SERIAL_METADATA_BYTES, MAX_PLUGIN_SERIAL_SEND_BYTES,
    PluginApiErrorCode, PluginApiResourceEventKind, PluginSerialDataBits,
    PluginSerialDeviceCandidate, PluginSerialEvent, PluginSerialFlowControl, PluginSerialParity,
    PluginSerialPortKind, PluginSerialPortMetadata, PluginSerialSendRequest, PluginSerialSettings,
    PluginSerialStopBits,
};
use sha2::{Digest as _, Sha256};
use tokio::sync::watch;
use uuid::Uuid;

use super::{
    ResourceCommandReceiver, ResourceEventWriter, ResourceFence, ResourceOwner, ResourceRegistry,
};

const SERIAL_READ_TIMEOUT: Duration = Duration::from_millis(50);
const MAX_SERIAL_READ_BYTES: usize = 8 * 1024;
const SERIAL_COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// A private discovery record. Only the DTO can cross a Core UI boundary; the
/// exact path and identity stay in Core until native confirmation freezes them.
#[derive(Clone)]
pub(crate) struct DiscoveredSerialCandidate {
    pub candidate: PluginSerialDeviceCandidate,
    canonical_path: PathBuf,
    platform_identity: String,
}

impl DiscoveredSerialCandidate {
    pub(crate) fn freeze(
        &self,
        candidate_id: &str,
        settings: PluginSerialSettings,
    ) -> Result<FrozenSerialPlan, PluginApiErrorCode> {
        if candidate_id != self.candidate.candidate_id {
            return Err(PluginApiErrorCode::NotFound);
        }
        FrozenSerialPlan::new(
            self.canonical_path.clone(),
            self.platform_identity.clone(),
            settings,
        )
    }
}

pub(crate) struct SerialDriver;

impl SerialDriver {
    /// Enumerates only device metadata and opaque candidate IDs. Enumeration is
    /// advisory; availability and identity are checked again when opening.
    pub(crate) fn discover() -> Result<Vec<DiscoveredSerialCandidate>, PluginApiErrorCode> {
        let ports = serialport::available_ports().map_err(|_| PluginApiErrorCode::Unavailable)?;
        Ok(ports
            .into_iter()
            .filter_map(|port| discovered_candidate(port).ok())
            .collect())
    }

    /// Registers an owner-scoped duplex resource for an already confirmed plan.
    pub(crate) fn start(
        resources: &ResourceRegistry,
        owner: ResourceOwner,
        plan: FrozenSerialPlan,
        fence: ResourceFence,
    ) -> Result<String, PluginApiErrorCode> {
        plan.validate_bounds()?;
        if !fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        resources.spawn_with_commands(
            owner,
            "serial",
            move |cancel, events, commands| async move {
                tokio::task::spawn_blocking(move || {
                    run_serial(plan, cancel, events, commands, fence)
                })
                .await
                .unwrap_or(Err(PluginApiErrorCode::CleanupIncomplete))
            },
        )
    }

    /// The registry validates the exact resource owner and waits until the
    /// driver has accepted bytes into the operating-system serial writer.
    pub(crate) async fn send(
        resources: &ResourceRegistry,
        owner: &ResourceOwner,
        request: PluginSerialSendRequest,
        fence: &ResourceFence,
    ) -> Result<(), PluginApiErrorCode> {
        if !fence() {
            return Err(PluginApiErrorCode::Revoked);
        }
        if request.data_base64.is_empty()
            || request.data_base64.len() > MAX_PLUGIN_SERIAL_SEND_BYTES.saturating_mul(2)
        {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
        let bytes = BASE64
            .decode(request.data_base64)
            .map_err(|_| PluginApiErrorCode::InvalidRequest)?;
        if bytes.is_empty() || bytes.len() > MAX_PLUGIN_SERIAL_SEND_BYTES {
            return Err(PluginApiErrorCode::QuotaExceeded);
        }
        resources
            .send(owner, &request.handle, bytes, fence.as_ref())
            .await
    }
}

fn discovered_candidate(
    port: serialport::SerialPortInfo,
) -> Result<DiscoveredSerialCandidate, PluginApiErrorCode> {
    let canonical_path = canonical_serial_path(&port.port_name)?;
    let platform_identity = platform_identity(&canonical_path, &port);
    let metadata = metadata_for(&port);
    Ok(DiscoveredSerialCandidate {
        candidate: PluginSerialDeviceCandidate {
            candidate_id: Uuid::new_v4().to_string(),
            metadata,
        },
        canonical_path,
        platform_identity,
    })
}

fn canonical_serial_path(value: &str) -> Result<PathBuf, PluginApiErrorCode> {
    #[cfg(windows)]
    {
        let normalized = value.strip_prefix(r#"\\.\"#).unwrap_or(value).trim();
        if normalized.is_empty()
            || normalized.len() > MAX_PLUGIN_SERIAL_LABEL_BYTES
            || !normalized
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(PluginApiErrorCode::InvalidRequest);
        }
        return Ok(PathBuf::from(format!(r#"\\.\{normalized}"#)));
    }
    #[cfg(not(windows))]
    {
        std::fs::canonicalize(value).map_err(|_| PluginApiErrorCode::Unavailable)
    }
}

fn platform_identity(path: &Path, port: &serialport::SerialPortInfo) -> String {
    let mut identity = path.to_string_lossy().into_owned();
    identity.push('\n');
    match &port.port_type {
        serialport::SerialPortType::UsbPort(usb) => {
            identity.push_str("usb\n");
            identity.push_str(&usb.vid.to_string());
            identity.push('\n');
            identity.push_str(&usb.pid.to_string());
            identity.push('\n');
            identity.push_str(usb.serial_number.as_deref().unwrap_or_default());
            identity.push('\n');
            identity.push_str(usb.manufacturer.as_deref().unwrap_or_default());
            identity.push('\n');
            identity.push_str(usb.product.as_deref().unwrap_or_default());
        }
        serialport::SerialPortType::PciPort => identity.push_str("pci"),
        serialport::SerialPortType::BluetoothPort => identity.push_str("bluetooth"),
        serialport::SerialPortType::Unknown => identity.push_str("unknown"),
    }
    hex::encode(Sha256::digest(identity.as_bytes()))
}

fn metadata_for(port: &serialport::SerialPortInfo) -> PluginSerialPortMetadata {
    let label = Path::new(&port.port_name)
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(public_text)
        .unwrap_or_else(|| "Serial device".to_owned());
    match &port.port_type {
        serialport::SerialPortType::UsbPort(usb) => PluginSerialPortMetadata {
            label,
            kind: PluginSerialPortKind::Usb,
            manufacturer: usb.manufacturer.as_deref().and_then(public_text),
            product: usb.product.as_deref().and_then(public_text),
            usb_vendor_id: Some(usb.vid),
            usb_product_id: Some(usb.pid),
        },
        serialport::SerialPortType::PciPort => PluginSerialPortMetadata {
            label,
            kind: PluginSerialPortKind::Pci,
            manufacturer: None,
            product: None,
            usb_vendor_id: None,
            usb_product_id: None,
        },
        serialport::SerialPortType::BluetoothPort => PluginSerialPortMetadata {
            label,
            kind: PluginSerialPortKind::Bluetooth,
            manufacturer: None,
            product: None,
            usb_vendor_id: None,
            usb_product_id: None,
        },
        serialport::SerialPortType::Unknown => PluginSerialPortMetadata {
            label,
            kind: PluginSerialPortKind::Unknown,
            manufacturer: None,
            product: None,
            usb_vendor_id: None,
            usb_product_id: None,
        },
    }
}

fn public_text(value: &str) -> Option<String> {
    (!value.trim().is_empty()
        && value.len() <= MAX_PLUGIN_SERIAL_METADATA_BYTES
        && !value.chars().any(char::is_control))
    .then(|| value.to_owned())
}

fn plan_matches_present_device(plan: &FrozenSerialPlan) -> bool {
    serialport::available_ports()
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|port| {
            let canonical_path = canonical_serial_path(&port.port_name).ok()?;
            let identity = platform_identity(&canonical_path, &port);
            Some((canonical_path, identity))
        })
        .any(|(path, identity)| {
            path == plan.canonical_path() && identity == plan.platform_identity()
        })
}

fn run_serial(
    plan: FrozenSerialPlan,
    cancel: watch::Receiver<bool>,
    events: ResourceEventWriter,
    mut commands: ResourceCommandReceiver,
    fence: ResourceFence,
) -> Result<(), PluginApiErrorCode> {
    if !fence() {
        finish_commands(&mut commands, PluginApiErrorCode::Revoked);
        return Ok(());
    }
    if !plan_matches_present_device(&plan) {
        emit_error(&events, PluginApiErrorCode::Unavailable);
        finish_commands(&mut commands, PluginApiErrorCode::Unavailable);
        return Ok(());
    }
    let port = match open_serial(&plan) {
        Ok(port) => port,
        Err(code) => {
            emit_error(&events, code);
            finish_commands(&mut commands, code);
            return Ok(());
        }
    };
    run_open_serial(port, cancel, events, commands, fence)
}

fn run_open_serial(
    mut port: Box<dyn serialport::SerialPort>,
    mut cancel: watch::Receiver<bool>,
    events: ResourceEventWriter,
    mut commands: ResourceCommandReceiver,
    fence: ResourceFence,
) -> Result<(), PluginApiErrorCode> {
    // The cloned reader and writer preserve independent, bounded serial I/O:
    // a pending read cannot delay a Core write acknowledgement.
    let reader_port = port.try_clone().and_then(|mut reader| {
        reader.set_timeout(SERIAL_READ_TIMEOUT)?;
        Ok(reader)
    });
    let mut reader_port = match reader_port {
        Ok(reader) => reader,
        Err(_) => {
            // Setup failure releases both native handles here. Only an actual failed join
            // may remain registered as incomplete cleanup.
            emit_error(&events, PluginApiErrorCode::Unavailable);
            finish_commands(&mut commands, PluginApiErrorCode::Unavailable);
            return Ok(());
        }
    };
    if events
        .emit(PluginApiResourceEventKind::Serial {
            event: PluginSerialEvent::Opened {},
        })
        .is_err()
    {
        return Ok(());
    }
    let stop = Arc::new(AtomicBool::new(false));
    let (reader_result_sender, reader_result_receiver) = mpsc::channel();
    let reader_stop = stop.clone();
    let reader_events = events.clone();
    let reader_fence = fence.clone();
    let reader_cancel = cancel.clone();
    let reader = thread::spawn(move || {
        let result = read_serial(
            &mut *reader_port,
            reader_cancel,
            reader_events,
            reader_fence,
            reader_stop,
        );
        let _ = reader_result_sender.send(result);
    });
    loop {
        if *cancel.borrow_and_update() || !fence() {
            let code = if fence() {
                PluginApiErrorCode::Cancelled
            } else {
                PluginApiErrorCode::Revoked
            };
            finish_commands(&mut commands, code);
            finish_reader(&stop, reader)?;
            emit_terminal_serial(
                &events,
                &mut cancel,
                &fence,
                &mut commands,
                PluginSerialEvent::Closed {},
            );
            return Ok(());
        }
        if let Ok(result) = reader_result_receiver.try_recv() {
            let _ = reader.join();
            match result {
                Ok(()) => {
                    let code = if fence() {
                        PluginApiErrorCode::Cancelled
                    } else {
                        PluginApiErrorCode::Revoked
                    };
                    finish_commands(&mut commands, code);
                    emit_terminal_serial(
                        &events,
                        &mut cancel,
                        &fence,
                        &mut commands,
                        PluginSerialEvent::Closed {},
                    );
                }
                Err(code) => {
                    finish_commands(&mut commands, code);
                    emit_terminal_serial(
                        &events,
                        &mut cancel,
                        &fence,
                        &mut commands,
                        PluginSerialEvent::Error { stable_code: code },
                    );
                    emit_terminal_serial(
                        &events,
                        &mut cancel,
                        &fence,
                        &mut commands,
                        PluginSerialEvent::Closed {},
                    );
                }
            }
            return Ok(());
        }
        while let Some(command) = commands.try_recv() {
            if !fence() {
                command.finish(Err(PluginApiErrorCode::Revoked));
                finish_commands(&mut commands, PluginApiErrorCode::Revoked);
                finish_reader(&stop, reader)?;
                emit_terminal_serial(
                    &events,
                    &mut cancel,
                    &fence,
                    &mut commands,
                    PluginSerialEvent::Closed {},
                );
                return Ok(());
            }
            // A successful write_all is the concrete OS-writer handoff. Do
            // not call tcdrain here: it waits for a physical peer to consume
            // bytes and would incorrectly turn this local ACK into a remote
            // delivery claim.
            match port.write_all(command.payload()) {
                Ok(()) => command.finish(Ok(())),
                Err(_) => {
                    command.finish(Err(PluginApiErrorCode::OutcomeUnknown));
                    finish_commands(&mut commands, PluginApiErrorCode::OutcomeUnknown);
                    finish_reader(&stop, reader)?;
                    emit_terminal_serial(
                        &events,
                        &mut cancel,
                        &fence,
                        &mut commands,
                        PluginSerialEvent::Error {
                            stable_code: PluginApiErrorCode::OutcomeUnknown,
                        },
                    );
                    return Ok(());
                }
            }
        }
        thread::sleep(SERIAL_COMMAND_POLL_INTERVAL);
    }
}

fn emit_terminal_serial(
    events: &ResourceEventWriter,
    cancel: &mut watch::Receiver<bool>,
    fence: &ResourceFence,
    commands: &mut ResourceCommandReceiver,
    event: PluginSerialEvent,
) {
    let event = PluginApiResourceEventKind::Serial { event };
    loop {
        finish_commands(commands, PluginApiErrorCode::Unavailable);
        if *cancel.borrow_and_update() || !fence() {
            return;
        }
        match events.emit(event.clone()) {
            Ok(()) => return,
            Err(PluginApiErrorCode::Busy) => thread::sleep(SERIAL_COMMAND_POLL_INTERVAL),
            Err(_) => return,
        }
    }
}

fn read_serial(
    port: &mut dyn serialport::SerialPort,
    mut cancel: watch::Receiver<bool>,
    events: ResourceEventWriter,
    fence: ResourceFence,
    stop: Arc<AtomicBool>,
) -> Result<(), PluginApiErrorCode> {
    let mut buffer = [0_u8; MAX_SERIAL_READ_BYTES];
    while !stop.load(Ordering::Acquire) {
        if *cancel.borrow_and_update() || !fence() {
            return Ok(());
        }
        match port.read(&mut buffer) {
            Ok(0) => return Err(PluginApiErrorCode::Unavailable),
            Ok(read) => {
                let event = PluginApiResourceEventKind::Serial {
                    event: PluginSerialEvent::Data {
                        data_base64: BASE64.encode(&buffer[..read]),
                    },
                };
                // Retain this chunk while the consumer is behind. Cancellation still releases
                // the native reader without silently losing bytes or a terminal close event.
                loop {
                    if stop.load(Ordering::Acquire) || *cancel.borrow_and_update() || !fence() {
                        return Ok(());
                    }
                    match events.emit(event.clone()) {
                        Ok(()) => break,
                        Err(PluginApiErrorCode::Busy) => {
                            thread::sleep(SERIAL_COMMAND_POLL_INTERVAL)
                        }
                        Err(error) => return Err(error),
                    }
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) => {}
            Err(_) => return Err(PluginApiErrorCode::Unavailable),
        }
    }
    Ok(())
}

fn finish_reader(
    stop: &AtomicBool,
    reader: thread::JoinHandle<()>,
) -> Result<(), PluginApiErrorCode> {
    stop.store(true, Ordering::Release);
    reader
        .join()
        .map_err(|_| PluginApiErrorCode::CleanupIncomplete)
}

fn open_serial(
    plan: &FrozenSerialPlan,
) -> Result<Box<dyn serialport::SerialPort>, PluginApiErrorCode> {
    let settings = plan.settings();
    serialport::new(plan.canonical_path().to_string_lossy(), settings.baud_rate)
        .data_bits(match settings.data_bits {
            PluginSerialDataBits::Five => serialport::DataBits::Five,
            PluginSerialDataBits::Six => serialport::DataBits::Six,
            PluginSerialDataBits::Seven => serialport::DataBits::Seven,
            PluginSerialDataBits::Eight => serialport::DataBits::Eight,
        })
        .parity(match settings.parity {
            PluginSerialParity::None => serialport::Parity::None,
            PluginSerialParity::Odd => serialport::Parity::Odd,
            PluginSerialParity::Even => serialport::Parity::Even,
        })
        .stop_bits(match settings.stop_bits {
            PluginSerialStopBits::One => serialport::StopBits::One,
            PluginSerialStopBits::Two => serialport::StopBits::Two,
        })
        .flow_control(match settings.flow_control {
            PluginSerialFlowControl::None => serialport::FlowControl::None,
            PluginSerialFlowControl::Software => serialport::FlowControl::Software,
            PluginSerialFlowControl::Hardware => serialport::FlowControl::Hardware,
        })
        .timeout(SERIAL_READ_TIMEOUT)
        .open()
        .map_err(|_| PluginApiErrorCode::Unavailable)
}

fn finish_commands(commands: &mut ResourceCommandReceiver, code: PluginApiErrorCode) {
    while let Some(command) = commands.try_recv() {
        command.finish(Err(code));
    }
}

fn emit_error(events: &ResourceEventWriter, code: PluginApiErrorCode) {
    let _ = events.emit(PluginApiResourceEventKind::Serial {
        event: PluginSerialEvent::Error { stable_code: code },
    });
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use serialport::SerialPort as _;
    #[cfg(unix)]
    use std::{
        io::{Read as _, Write as _},
        sync::Arc,
    };

    use super::*;

    /// Exercises the only recoverable reader-setup failure without relying on a physical serial
    /// device that happens to reject cloning. The wrapped PTY still gives every unrelated trait
    /// method its normal platform behavior.
    #[cfg(unix)]
    struct CloneFailPort(serialport::TTYPort);

    #[cfg(unix)]
    impl io::Read for CloneFailPort {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.0.read(buffer)
        }
    }

    #[cfg(unix)]
    impl io::Write for CloneFailPort {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.0.write(buffer)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }

    #[cfg(unix)]
    impl serialport::SerialPort for CloneFailPort {
        fn name(&self) -> Option<String> {
            self.0.name()
        }
        fn baud_rate(&self) -> serialport::Result<u32> {
            self.0.baud_rate()
        }
        fn data_bits(&self) -> serialport::Result<serialport::DataBits> {
            self.0.data_bits()
        }
        fn flow_control(&self) -> serialport::Result<serialport::FlowControl> {
            self.0.flow_control()
        }
        fn parity(&self) -> serialport::Result<serialport::Parity> {
            self.0.parity()
        }
        fn stop_bits(&self) -> serialport::Result<serialport::StopBits> {
            self.0.stop_bits()
        }
        fn timeout(&self) -> Duration {
            self.0.timeout()
        }
        fn set_baud_rate(&mut self, value: u32) -> serialport::Result<()> {
            self.0.set_baud_rate(value)
        }
        fn set_data_bits(&mut self, value: serialport::DataBits) -> serialport::Result<()> {
            self.0.set_data_bits(value)
        }
        fn set_flow_control(&mut self, value: serialport::FlowControl) -> serialport::Result<()> {
            self.0.set_flow_control(value)
        }
        fn set_parity(&mut self, value: serialport::Parity) -> serialport::Result<()> {
            self.0.set_parity(value)
        }
        fn set_stop_bits(&mut self, value: serialport::StopBits) -> serialport::Result<()> {
            self.0.set_stop_bits(value)
        }
        fn set_timeout(&mut self, value: Duration) -> serialport::Result<()> {
            self.0.set_timeout(value)
        }
        fn write_request_to_send(&mut self, value: bool) -> serialport::Result<()> {
            self.0.write_request_to_send(value)
        }
        fn write_data_terminal_ready(&mut self, value: bool) -> serialport::Result<()> {
            self.0.write_data_terminal_ready(value)
        }
        fn read_clear_to_send(&mut self) -> serialport::Result<bool> {
            self.0.read_clear_to_send()
        }
        fn read_data_set_ready(&mut self) -> serialport::Result<bool> {
            self.0.read_data_set_ready()
        }
        fn read_ring_indicator(&mut self) -> serialport::Result<bool> {
            self.0.read_ring_indicator()
        }
        fn read_carrier_detect(&mut self) -> serialport::Result<bool> {
            self.0.read_carrier_detect()
        }
        fn bytes_to_read(&self) -> serialport::Result<u32> {
            self.0.bytes_to_read()
        }
        fn bytes_to_write(&self) -> serialport::Result<u32> {
            self.0.bytes_to_write()
        }
        fn clear(&self, value: serialport::ClearBuffer) -> serialport::Result<()> {
            self.0.clear(value)
        }
        fn try_clone(&self) -> serialport::Result<Box<dyn serialport::SerialPort>> {
            Err(serialport::Error::new(
                serialport::ErrorKind::Unknown,
                "test clone failure",
            ))
        }
        fn set_break(&self) -> serialport::Result<()> {
            self.0.set_break()
        }
        fn clear_break(&self) -> serialport::Result<()> {
            self.0.clear_break()
        }
    }

    #[test]
    fn candidate_metadata_never_exposes_a_path_or_secret_serial_number() {
        let port = serialport::SerialPortInfo {
            port_name: "/dev/ttyUSB0".to_owned(),
            port_type: serialport::SerialPortType::UsbPort(serialport::UsbPortInfo {
                vid: 0x1234,
                pid: 0x5678,
                serial_number: Some("private-device-number".to_owned()),
                manufacturer: Some("Nori Devices".to_owned()),
                product: Some("Serial Bridge".to_owned()),
            }),
        };
        let metadata = metadata_for(&port);
        let wire = serde_json::to_string(&metadata).expect("metadata wire");
        assert!(!wire.contains("/dev/ttyUSB0"));
        assert!(!wire.contains("private-device-number"));
        assert_eq!(metadata.label, "ttyUSB0");
    }

    #[test]
    fn rejected_settings_never_reach_a_port_builder() {
        let settings = PluginSerialSettings {
            baud_rate: 0,
            data_bits: PluginSerialDataBits::Eight,
            parity: PluginSerialParity::None,
            stop_bits: PluginSerialStopBits::One,
            flow_control: PluginSerialFlowControl::None,
        };
        assert!(settings.validate().is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_pty_driver_acknowledges_real_writes_and_releases_the_handle() {
        let (mut master, mut port) = serialport::TTYPort::pair().expect("serial PTY pair");
        let slave_name = port.name().expect("PTY slave name");
        port.set_timeout(SERIAL_READ_TIMEOUT).expect("PTY timeout");
        let resources = ResourceRegistry::default();
        let owner = ResourceOwner {
            plugin_id: norishell_core_api::PluginId::parse("org.norishell.serial-test")
                .expect("plugin id"),
            signer: "a".repeat(64),
            package: "b".repeat(64),
            generation: norishell_core_api::WireSequence::new(1),
        };
        let fence: ResourceFence = Arc::new(|| true);
        let handle = resources
            .spawn_with_commands(
                owner.clone(),
                "serial",
                move |cancel, events, commands| async move {
                    tokio::task::spawn_blocking(move || {
                        run_open_serial(Box::new(port), cancel, events, commands, fence)
                    })
                    .await
                    .unwrap_or(Err(PluginApiErrorCode::CleanupIncomplete))
                },
            )
            .expect("register serial");
        let opened = wait_for_serial_event(&resources, &owner, &handle, |event| {
            matches!(event, PluginSerialEvent::Opened {})
        })
        .await;
        assert!(opened, "PTY driver must publish opened");

        master.write_all(b"from-device").expect("device write");
        let data = wait_for_serial_event(&resources, &owner, &handle, |event| {
            matches!(event, PluginSerialEvent::Data { data_base64 }
                if BASE64.decode(data_base64).ok().as_deref() == Some(b"from-device"))
        })
        .await;
        assert!(data, "PTY read must become a bounded data event");

        let send_fence: ResourceFence = Arc::new(|| true);
        SerialDriver::send(
            &resources,
            &owner,
            PluginSerialSendRequest {
                handle: handle.clone(),
                data_base64: BASE64.encode(b"to-device"),
            },
            &send_fence,
        )
        .await
        .expect("write acknowledgement");
        let mut received = [0_u8; 9];
        master.read_exact(&mut received).expect("device read");
        assert_eq!(&received, b"to-device");

        // Keep the PTY master alive: dropping it removes the slave device itself.
        // The read timeout and cancellation must release both driver handles independently.
        resources
            .close(&owner, &handle)
            .await
            .expect("release serial");
        // macOS PTYs do not implement the hardware baud-rate ioctl used by TTYPort::open.
        // Reopen the device directly so this checks release, not unsupported hardware setup.
        let reopened = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(slave_name);
        assert!(
            reopened.is_ok(),
            "close must release the native device handle: {reopened:?}"
        );
        drop(master);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reader_setup_failure_emits_error_and_releases_the_resource() {
        let (_master, port) = serialport::TTYPort::pair().expect("serial PTY pair");
        let resources = ResourceRegistry::default();
        let owner = ResourceOwner {
            plugin_id: norishell_core_api::PluginId::parse("org.norishell.serial-test")
                .expect("plugin id"),
            signer: "a".repeat(64),
            package: "b".repeat(64),
            generation: norishell_core_api::WireSequence::new(1),
        };
        let handle = resources
            .spawn_with_commands(
                owner.clone(),
                "serial",
                move |cancel, events, commands| async move {
                    tokio::task::spawn_blocking(move || {
                        run_open_serial(
                            Box::new(CloneFailPort(port)),
                            cancel,
                            events,
                            commands,
                            Arc::new(|| true),
                        )
                    })
                    .await
                    .unwrap_or(Err(PluginApiErrorCode::CleanupIncomplete))
                },
            )
            .expect("register serial");
        assert!(
            wait_for_serial_event(&resources, &owner, &handle, |event| {
                matches!(
                    event,
                    PluginSerialEvent::Error {
                        stable_code: PluginApiErrorCode::Unavailable
                    }
                )
            })
            .await,
            "reader setup failure must be visible to the caller"
        );
        resources
            .close(&owner, &handle)
            .await
            .expect("released handles must not become cleanup blockers");
        assert!(resources.list(&owner).is_empty());
    }

    #[cfg(unix)]
    async fn wait_for_serial_event(
        resources: &ResourceRegistry,
        owner: &ResourceOwner,
        handle: &str,
        matches_event: impl Fn(&PluginSerialEvent) -> bool,
    ) -> bool {
        for _ in 0..100 {
            let (events, _) = resources
                .take_events(owner, handle, 32)
                .expect("resource events");
            if events.iter().any(|event| {
                matches!(
                    &event.kind,
                    PluginApiResourceEventKind::Serial { event } if matches_event(event)
                )
            }) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }
}
