use super::*;
use std::sync::{Arc, Mutex};

#[derive(Debug, Default)]
struct Observed {
    waves: Vec<(usize, u32, Vec<u8>)>,
    closed: usize,
    errors: usize,
}

#[derive(Debug)]
struct Backend {
    observed: Arc<Mutex<Observed>>,
    formats: Vec<AudioFormat>,
}

impl RdpsndClientHandler for Backend {
    fn get_formats(&self) -> &[AudioFormat] {
        &self.formats
    }
    fn wave(&mut self, index: usize, timestamp: u32, data: Cow<'_, [u8]>) {
        self.observed
            .lock()
            .unwrap()
            .waves
            .push((index, timestamp, data.into_owned()));
    }
    fn set_volume(&mut self, _: VolumePdu) {}
    fn set_pitch(&mut self, _: PitchPdu) {}
    fn close(&mut self) {
        self.observed.lock().unwrap().closed += 1;
    }
    fn protocol_error(&mut self) {
        self.observed.lock().unwrap().errors += 1;
        self.close();
    }
}

fn pcm() -> AudioFormat {
    AudioFormat {
        format: pdu::WaveFormat::PCM,
        n_channels: 2,
        n_samples_per_sec: 44100,
        n_avg_bytes_per_sec: 176400,
        n_block_align: 4,
        bits_per_sample: 16,
        data: None,
    }
}

fn formats() -> Vec<u8> {
    ironrdp_core::encode_vec(&pdu::ServerAudioOutputPdu::AudioFormat(
        ServerAudioFormatPdu {
            version: pdu::Version::V6,
            formats: vec![pcm()],
        },
    ))
    .unwrap()
}

fn client() -> (Rdpsnd, Arc<Mutex<Observed>>) {
    let observed = Arc::new(Mutex::new(Observed::default()));
    let mut client = Rdpsnd::new(Box::new(Backend {
        observed: observed.clone(),
        formats: vec![pcm()],
    }));
    assert_eq!(client.process(&formats()).unwrap().len(), 2);
    assert_eq!(client.process(&[6, 0, 4, 0, 42, 0, 0, 0]).unwrap().len(), 1);
    assert_eq!(client.state, RdpsndState::Ready);
    (client, observed)
}

fn info() -> [u8; 16] {
    // 8-byte sample; nonzero arbitrary WaveInfo padding must be ignored.
    [2, 0, 16, 0, 42, 0, 0, 0, 255, 9, 8, 7, 11, 12, 13, 14]
}

#[test]
fn legacy_split_reassembles_and_confirms_only_complete_sample() {
    let (mut client, observed) = client();
    assert!(client.process(&info()).unwrap().is_empty());
    assert!(observed.lock().unwrap().waves.is_empty());
    let replies = client.process(&[0, 0, 0, 0, 15, 16, 17, 18]).unwrap();
    assert_eq!(replies.len(), 1);
    assert_eq!(
        replies[0].encode_unframed_pdu().unwrap(),
        [5, 0, 4, 0, 42, 0, 255, 0]
    );
    assert_eq!(
        observed.lock().unwrap().waves,
        [(0, 42, vec![11, 12, 13, 14, 15, 16, 17, 18])]
    );
    let mut next = info();
    next[8] = 0;
    assert!(client.process(&next).unwrap().is_empty());
    assert_eq!(client.process(&[0, 0, 0, 0, 1, 2, 3, 4]).unwrap().len(), 1);
}

#[test]
fn malformed_continuations_stop_only_audio_and_discard_pending() {
    for data in [
        vec![],
        vec![0; 7],
        vec![0; 9],
        vec![1; 8],
        info().to_vec(),
        vec![1, 0, 0, 0],
        formats(),
    ] {
        let (mut client, observed) = client();
        client.process(&info()).unwrap();
        assert!(client.process(&data).unwrap().is_empty());
        assert_eq!(client.state, RdpsndState::Stop);
        assert!(client.pending_wave.is_none());
        assert!(client.process(&[0; 8]).unwrap().is_empty());
        let observed = observed.lock().unwrap();
        assert_eq!(observed.errors, 1);
        assert_eq!(observed.closed, 1);
        assert!(observed.waves.is_empty());
    }
}

#[test]
fn invalid_lengths_and_format_indices_are_rejected_without_buffering() {
    for size in [0_u16, 8, 12] {
        let (mut client, observed) = client();
        let mut packet = info();
        packet[2..4].copy_from_slice(&size.to_le_bytes());
        assert!(client.process(&packet).unwrap().is_empty());
        assert!(client.pending_wave.is_none());
        assert_eq!(observed.lock().unwrap().errors, 1);
    }
    let (mut client, observed) = client();
    let mut packet = info();
    packet[6] = 1;
    client.process(&packet).unwrap();
    assert_eq!(observed.lock().unwrap().errors, 1);
}

#[test]
fn maximum_length_is_bounded_by_wire_u16_and_requires_exact_continuation() {
    let (mut client, observed) = client();
    let mut packet = info();
    packet[2..4].copy_from_slice(&u16::MAX.to_le_bytes());
    client.process(&packet).unwrap();
    assert_eq!(client.pending_wave.as_ref().unwrap().sample_len, 65527);
    assert!(!format!("{client:?}").contains("[11, 12, 13, 14]"));
    client.process(&[0; 8]).unwrap();
    assert_eq!(observed.lock().unwrap().errors, 1);
}

#[test]
fn close_and_format_renegotiation_reset_backend_without_stale_wave() {
    let (mut client, observed) = client();
    client.process(&[1, 0, 0, 0]).unwrap();
    assert_eq!(observed.lock().unwrap().closed, 1);
    assert!(client.pending_wave.is_none());
    client.process(&formats()).unwrap();
    assert_eq!(observed.lock().unwrap().closed, 2);
    assert_eq!(client.state, RdpsndState::WaitingForTraining);
    client.process(&info()).unwrap();
    assert_eq!(observed.lock().unwrap().errors, 1);
}

#[test]
fn unsupported_formats_and_truncated_pdus_notify_protocol_error() {
    let observed = Arc::new(Mutex::new(Observed::default()));
    let mut client = Rdpsnd::new(Box::new(Backend {
        observed: observed.clone(),
        formats: vec![],
    }));
    assert!(client.process(&formats()).unwrap().is_empty());
    assert_eq!(observed.lock().unwrap().errors, 1);
    let (mut client, observed) = self::client();
    client.process(&[6]).unwrap();
    assert_eq!(observed.lock().unwrap().errors, 1);
}

#[test]
fn wave2_still_plays_and_confirms() {
    let (mut client, observed) = client();
    let packet = ironrdp_core::encode_vec(&pdu::ServerAudioOutputPdu::Wave2(pdu::Wave2Pdu {
        timestamp: 42,
        format_no: 0,
        block_no: 1,
        audio_timestamp: 4200,
        data: vec![1, 2, 3, 4].into(),
    }))
    .unwrap();
    assert_eq!(client.process(&packet).unwrap().len(), 1);
    assert_eq!(
        observed.lock().unwrap().waves,
        [(0, 4200, vec![1, 2, 3, 4])]
    );
}
