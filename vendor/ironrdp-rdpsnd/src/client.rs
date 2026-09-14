// Modified for NoriShell; see vendor/README.md at the repository root for upstream provenance.
use std::borrow::Cow;
use std::collections::HashSet;

use ironrdp_core::{Decode as _, EncodeResult, ReadCursor, cast_length, impl_as_any};
use ironrdp_pdu::gcc::ChannelName;
use ironrdp_pdu::{PduResult, decode_err, encode_err, pdu_other_err};
use ironrdp_svc::{CompressionCondition, SvcClientProcessor, SvcMessage, SvcProcessor};
use tracing::{debug, error};

use crate::pdu::{self, AudioFormat, PitchPdu, ServerAudioFormatPdu, TrainingPdu, VolumePdu};
use crate::server::RdpsndSvcMessages;

pub trait RdpsndClientHandler: Send + core::fmt::Debug {
    fn get_flags(&self) -> pdu::AudioFormatFlags {
        pdu::AudioFormatFlags::empty()
    }

    fn get_formats(&self) -> &[AudioFormat];

    fn wave(&mut self, format_no: usize, ts: u32, data: Cow<'_, [u8]>);

    fn set_volume(&mut self, volume: VolumePdu);

    fn set_pitch(&mut self, pitch: PitchPdu);

    fn close(&mut self);

    /// Audio protocol failure; the desktop connection remains usable.
    fn protocol_error(&mut self) {
        self.close();
    }
}

#[derive(Debug)]
pub struct NoopRdpsndBackend;

impl RdpsndClientHandler for NoopRdpsndBackend {
    fn get_formats(&self) -> &[AudioFormat] {
        &[]
    }

    fn wave(&mut self, _format_no: usize, _ts: u32, _data: Cow<'_, [u8]>) {}

    fn set_volume(&mut self, _volume: VolumePdu) {}

    fn set_pitch(&mut self, _pitch: PitchPdu) {}

    fn close(&mut self) {}
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum RdpsndState {
    Start,
    WaitingForTraining,
    Ready,
    Stop,
}

// The legacy protocol sends metadata and raw data in separate SVC messages.
// Keep only fixed-size metadata while waiting, never an unbounded audio buffer.
struct PendingWave {
    info: pdu::WaveInfoPdu,
    sample_len: usize,
}

impl core::fmt::Debug for PendingWave {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PendingWave")
            .field("sample_len", &self.sample_len)
            .finish_non_exhaustive()
    }
}

/// Required for rdpdr to work: [\[MS-RDPEFS\] Appendix A<1>]
///
/// [\[MS-RDPEFS\] Appendix A<1>]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpefs/fd28bfd9-dae2-4a78-abe1-b4efa208b7aa#Appendix_A_1
#[derive(Debug)]
pub struct Rdpsnd {
    handler: Box<dyn RdpsndClientHandler>,
    state: RdpsndState,
    server_format: Option<ServerAudioFormatPdu>,
    pending_wave: Option<PendingWave>,
    negotiated_format_count: usize,
}

impl Rdpsnd {
    pub const NAME: ChannelName = ChannelName::from_static(b"rdpsnd\0\0");

    pub fn new(handler: Box<dyn RdpsndClientHandler>) -> Self {
        Self {
            handler,
            state: RdpsndState::Start,
            server_format: None,
            pending_wave: None,
            negotiated_format_count: 0,
        }
    }

    pub fn get_format(&self, format_no: u16) -> PduResult<&AudioFormat> {
        let server_format = self
            .server_format
            .as_ref()
            .ok_or_else(|| pdu_other_err!("invalid state - no format"))?;

        server_format
            .formats
            .get(usize::from(format_no))
            .ok_or_else(|| pdu_other_err!("invalid format"))
    }

    pub fn version(&self) -> PduResult<pdu::Version> {
        let server_format = self
            .server_format
            .as_ref()
            .ok_or_else(|| pdu_other_err!("invalid state - no version"))?;

        Ok(server_format.version)
    }

    pub fn client_formats(&mut self) -> PduResult<RdpsndSvcMessages> {
        // Windows seems to be confused if the client replies with more formats, or unknown formats (e.g.: opus).
        // We ensure to only send supported formats in common with the server.
        let server_format: HashSet<_> = self
            .server_format
            .as_ref()
            .ok_or_else(|| pdu_other_err!("invalid state - no server format"))?
            .formats
            .iter()
            .collect();
        let formats: HashSet<_> = self.handler.get_formats().iter().collect();
        let formats: Vec<_> = formats
            .intersection(&server_format)
            .map(|&x| x.clone())
            .collect();
        self.negotiated_format_count = formats.len();
        if formats.is_empty() {
            return Err(pdu_other_err!("no common audio format"));
        }

        let pdu = pdu::ClientAudioFormatPdu {
            version: self.version()?,
            flags: self.handler.get_flags() | pdu::AudioFormatFlags::ALIVE,
            formats,
            volume_left: 0xFFFF,
            volume_right: 0xFFFF,
            pitch: 0x00010000,
            dgram_port: 0,
        };
        Ok(RdpsndSvcMessages::new(vec![
            pdu::ClientAudioOutputPdu::AudioFormat(pdu).into(),
        ]))
    }

    pub fn quality_mode(&mut self) -> PduResult<RdpsndSvcMessages> {
        let pdu = pdu::QualityModePdu {
            quality_mode: pdu::QualityMode::High,
        };
        Ok(RdpsndSvcMessages::new(vec![
            pdu::ClientAudioOutputPdu::QualityMode(pdu).into(),
        ]))
    }

    pub fn training_confirm(&mut self, pdu: &TrainingPdu) -> PduResult<RdpsndSvcMessages> {
        let pack_size: EncodeResult<_> = cast_length!("wPackSize", pdu.data.len());
        let pack_size = pack_size.map_err(|e| encode_err!(e))?;
        let pdu = pdu::TrainingConfirmPdu {
            timestamp: pdu.timestamp,
            pack_size,
        };
        Ok(RdpsndSvcMessages::new(vec![
            pdu::ClientAudioOutputPdu::TrainingConfirm(pdu).into(),
        ]))
    }

    pub fn wave_confirm(&mut self, timestamp: u16, block_no: u8) -> PduResult<RdpsndSvcMessages> {
        let pdu = pdu::WaveConfirmPdu {
            timestamp,
            block_no,
        };
        Ok(RdpsndSvcMessages::new(vec![
            pdu::ClientAudioOutputPdu::WaveConfirm(pdu).into(),
        ]))
    }
}

impl_as_any!(Rdpsnd);

impl SvcProcessor for Rdpsnd {
    fn channel_name(&self) -> ChannelName {
        Self::NAME
    }

    fn compression_condition(&self) -> CompressionCondition {
        CompressionCondition::Never
    }

    fn process(&mut self, payload: &[u8]) -> PduResult<Vec<SvcMessage>> {
        if self.state == RdpsndState::Stop {
            return Ok(vec![]);
        }
        match self.process_audio(payload) {
            Ok(messages) => Ok(messages),
            Err(_) => {
                // Optional audio must not tear down the desktop or log sample contents.
                self.pending_wave = None;
                self.state = RdpsndState::Stop;
                self.handler.protocol_error();
                error!("Audio channel stopped after invalid protocol message");
                Ok(vec![])
            }
        }
    }
}

impl Rdpsnd {
    fn process_audio(&mut self, payload: &[u8]) -> PduResult<Vec<SvcMessage>> {
        if let Some(pending) = self.pending_wave.take() {
            // MS-RDPEA 2.2.3.4: raw Wave starts with four zero padding bytes.
            // Any intervening control PDU is an invalid sequence and clears pending state.
            if payload.len() != pending.sample_len || payload.get(..4) != Some(&[0, 0, 0, 0]) {
                return Err(pdu_other_err!("invalid legacy audio continuation"));
            }
            let mut data = Vec::with_capacity(pending.sample_len);
            data.extend_from_slice(&pending.info.data);
            data.extend_from_slice(&payload[4..]);
            self.handler.wave(
                usize::from(pending.info.format_no),
                u32::from(pending.info.timestamp),
                data.into(),
            );
            return Ok(self
                .wave_confirm(pending.info.timestamp, pending.info.block_no)?
                .into());
        }
        if payload.first() == Some(&0x02) {
            // BodySize = complete sample length + 8, while this message is exactly 16 bytes.
            if self.state != RdpsndState::Ready || payload.len() != 16 {
                return Err(pdu_other_err!("invalid legacy audio info"));
            }
            let sample_len = usize::from(u16::from_le_bytes([payload[2], payload[3]]))
                .checked_sub(8)
                .filter(|length| *length > 4)
                .ok_or_else(|| pdu_other_err!("invalid legacy audio length"))?;
            let info = pdu::WaveInfoPdu::decode(&mut ReadCursor::new(&payload[4..]))
                .map_err(|e| decode_err!(e))?;
            if usize::from(info.format_no) >= self.negotiated_format_count {
                return Err(pdu_other_err!("invalid audio format index"));
            }
            self.pending_wave = Some(PendingWave { info, sample_len });
            return Ok(vec![]);
        }
        if payload.len() < 4
            || usize::from(u16::from_le_bytes([payload[2], payload[3]])) != payload.len() - 4
        {
            return Err(pdu_other_err!("invalid audio PDU length"));
        }
        let pdu = pdu::ServerAudioOutputPdu::decode(&mut ReadCursor::new(payload))
            .map_err(|e| decode_err!(e))?;

        debug!(state = ?self.state, message_type = payload[0], "Audio control message");
        let msg = match self.state {
            RdpsndState::Start => {
                let pdu::ServerAudioOutputPdu::AudioFormat(af) = pdu else {
                    error!("Invalid pdu");
                    return Err(pdu_other_err!("unexpected audio PDU"));
                };
                self.server_format = Some(af);
                self.state = RdpsndState::WaitingForTraining;
                let mut msgs: Vec<SvcMessage> = self.client_formats()?.into();
                if self.version()? >= pdu::Version::V6 {
                    let mut m = self.quality_mode()?.into();
                    msgs.append(&mut m);
                }
                msgs
            }
            RdpsndState::WaitingForTraining => {
                let pdu::ServerAudioOutputPdu::Training(pdu) = pdu else {
                    error!("Invalid PDU");
                    return Err(pdu_other_err!("unexpected audio PDU"));
                };
                self.state = RdpsndState::Ready;
                self.training_confirm(&pdu)?.into()
            }
            RdpsndState::Ready => {
                match pdu {
                    pdu::ServerAudioOutputPdu::Wave2(pdu) => {
                        let format_no = usize::from(pdu.format_no);
                        if format_no >= self.negotiated_format_count {
                            return Err(pdu_other_err!("invalid audio format index"));
                        }
                        let ts = pdu.audio_timestamp;
                        self.handler.wave(format_no, ts, pdu.data);
                        return Ok(self.wave_confirm(pdu.timestamp, pdu.block_no)?.into());
                    }
                    pdu::ServerAudioOutputPdu::Volume(pdu) => {
                        self.handler.set_volume(pdu);
                    }
                    pdu::ServerAudioOutputPdu::Pitch(pdu) => {
                        self.handler.set_pitch(pdu);
                    }
                    pdu::ServerAudioOutputPdu::Close => {
                        self.handler.close();
                    }
                    pdu::ServerAudioOutputPdu::Training(pdu) => {
                        return Ok(self.training_confirm(&pdu)?.into());
                    }
                    pdu::ServerAudioOutputPdu::AudioFormat(af) => {
                        self.handler.close();
                        self.server_format = Some(af);
                        self.state = RdpsndState::WaitingForTraining;
                        let mut msgs: Vec<SvcMessage> = self.client_formats()?.into();
                        if self.version()? >= pdu::Version::V6 {
                            let mut m = self.quality_mode()?.into();
                            msgs.append(&mut m);
                        }
                        return Ok(msgs);
                    }
                    _ => {
                        error!("Invalid PDU");
                        return Err(pdu_other_err!("unexpected audio PDU"));
                    }
                }
                vec![]
            }
            state => {
                error!(?state, "Invalid state");
                vec![]
            }
        };

        Ok(msg)
    }
}

impl Drop for Rdpsnd {
    fn drop(&mut self) {
        self.handler.close();
    }
}

impl SvcClientProcessor for Rdpsnd {}

#[cfg(test)]
mod tests;
