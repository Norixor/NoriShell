//! Length fences before IronRDP fragment reassembly prevent unbounded accumulation of unfinished fragments.
use ironrdp::{
    core::{ReadCursor, decode, decode_cursor},
    dvc::pdu::{DrdynvcDataPdu, DrdynvcServerPdu},
    pdu::{
        Action,
        fast_path::{FastPathHeader, FastPathUpdatePdu, Fragmentation},
    },
};
use norishell_desktop_protocol::{EngineError, Result};
use std::collections::BTreeMap;
const MAX_FRAGMENT: usize = 16 * 1024 * 1024;
const MAX_CHANNEL: usize = 4 * 1024 * 1024;
pub(crate) struct Limits {
    fast: usize,
    channels: BTreeMap<u16, Vec<u8>>,
    dynamic: Option<u16>,
}
impl Limits {
    pub fn new(channels: impl Iterator<Item = u16>, dynamic: Option<u16>) -> Self {
        Self {
            fast: 0,
            channels: channels.map(|id| (id, Vec::new())).collect(),
            dynamic,
        }
    }
    pub fn check(&mut self, action: Action, bytes: &[u8]) -> Result<()> {
        match action {
            Action::FastPath => {
                let mut cursor = ReadCursor::new(bytes);
                let _: FastPathHeader =
                    decode_cursor(&mut cursor).map_err(|_| EngineError::Protocol)?;
                while !cursor.is_empty() {
                    let update: FastPathUpdatePdu<'_> =
                        decode_cursor(&mut cursor).map_err(|_| EngineError::Protocol)?;
                    if update.compression_flags.is_some() {
                        return Err(EngineError::UnsupportedOperation);
                    }
                    if matches!(
                        update.fragmentation,
                        Fragmentation::Single | Fragmentation::First
                    ) {
                        self.fast = 0;
                    }
                    self.fast = self
                        .fast
                        .checked_add(update.data.len())
                        .ok_or(EngineError::ResourceLimit)?;
                    if self.fast > MAX_FRAGMENT {
                        return Err(EngineError::ResourceLimit);
                    }
                    if matches!(
                        update.fragmentation,
                        Fragmentation::Single | Fragmentation::Last
                    ) {
                        self.fast = 0;
                    }
                }
            }
            Action::X224 => {
                // Let the engine handle disconnect PDUs that are not SendData.
                let Ok(ctx) = ironrdp::pdu::mcs::decode_send_data_indication(bytes) else {
                    return Ok(());
                };
                if let Some(pending) = self.channels.get_mut(&ctx.channel_id) {
                    if ctx.user_data.len() < 8 {
                        return Err(EngineError::Protocol);
                    }
                    let length = u32::from_le_bytes(
                        ctx.user_data[..4]
                            .try_into()
                            .map_err(|_| EngineError::Protocol)?,
                    ) as usize;
                    let flags = u32::from_le_bytes(
                        ctx.user_data[4..8]
                            .try_into()
                            .map_err(|_| EngineError::Protocol)?,
                    );
                    if length > MAX_CHANNEL || pending.len() + ctx.user_data.len() - 8 > MAX_CHANNEL
                    {
                        return Err(EngineError::ResourceLimit);
                    }
                    pending.extend_from_slice(&ctx.user_data[8..]);
                    if flags & 2 != 0 {
                        if self.dynamic == Some(ctx.channel_id)
                            && let DrdynvcServerPdu::Data(DrdynvcDataPdu::DataFirst(first)) =
                                decode(pending).map_err(|_| EngineError::Protocol)?
                            && first.length() as usize > MAX_CHANNEL
                        {
                            return Err(EngineError::ResourceLimit);
                        }
                        pending.clear();
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironrdp::{
        core::encode_vec,
        pdu::fast_path::{EncryptionFlags, UpdateCode},
    };
    #[test]
    fn fragmented_graphics_cannot_grow_without_bound() {
        let update = FastPathUpdatePdu {
            fragmentation: Fragmentation::Next,
            update_code: UpdateCode::Bitmap,
            compression_flags: None,
            compression_type: None,
            data: &[1],
        };
        let payload = encode_vec(&update).unwrap();
        let mut packet = encode_vec(&FastPathHeader::new(
            EncryptionFlags::empty(),
            payload.len(),
        ))
        .unwrap();
        packet.extend(payload);
        let mut limits = Limits::new(std::iter::empty(), None);
        limits.fast = MAX_FRAGMENT;
        assert_eq!(
            limits.check(Action::FastPath, &packet),
            Err(EngineError::ResourceLimit)
        );
    }
}
