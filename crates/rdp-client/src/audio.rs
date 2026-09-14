//! RDP audio flows to the local device through a bounded Core-owned PCM queue; samples never enter UI, logs, or persistence.
use cpal::{
    Device, FromSample, I24, Sample, SampleFormat, Stream, StreamConfig, U24,
    traits::{DeviceTrait as _, HostTrait as _, StreamTrait as _},
};
use ironrdp::{
    core::impl_as_any,
    rdpsnd::{
        client::RdpsndClientHandler,
        pdu::{AudioFormat, PitchPdu, VolumePdu, WaveFormat},
    },
};
use norishell_desktop_protocol::{AudioMuteState, AudioPlaybackState, EngineEvent, EventSink};
use std::{
    borrow::Cow,
    collections::VecDeque,
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

// About 250 ms of 48 kHz stereo PCM. Drop new packets when full so slow devices cannot backpressure RDP reads.
const MAX_QUEUED_SAMPLES: usize = 24_000;

// IronRDP 0.17 collects the format intersection into a HashSet when constructing ClientAudioFormat,
// then uses the Wave index to index the handler's original array. Multiple formats can therefore mismatch.
// Advertise one stable format for this version; local output still converts to the device's sample rate and channels.
const RDP_AUDIO_FORMATS: [AudioFormat; 1] = [AudioFormat {
    format: WaveFormat::PCM,
    n_channels: 2,
    n_samples_per_sec: 44_100,
    n_avg_bytes_per_sec: 176_400,
    n_block_align: 4,
    bits_per_sample: 16,
    data: None,
}];

#[derive(Clone, Copy)]
struct InputFormat {
    channels: u16,
    sample_rate: u32,
}

const NEGOTIATED_FORMAT: InputFormat = InputFormat {
    channels: 2,
    sample_rate: 44_100,
};

struct Packet {
    epoch: u64,
    channels: u16,
    samples: Vec<f32>,
    offset: usize,
}

struct QueueState {
    packets: VecDeque<Packet>,
    queued_samples: usize,
}

struct AudioQueue {
    state: Mutex<QueueState>,
    muted: AtomicBool,
    closed: AtomicBool,
    epoch: AtomicU64,
}

impl AudioQueue {
    fn new() -> Self {
        Self {
            state: Mutex::new(QueueState {
                packets: VecDeque::new(),
                queued_samples: 0,
            }),
            muted: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            epoch: AtomicU64::new(0),
        }
    }

    fn apply_mute_state(&self, muted: bool) {
        self.muted.store(muted, Ordering::Release);
        // The caller invokes this for every new Core revision, even if the final state is
        // unmuted. This prevents an unobserved mute→unmute pair from replaying old samples.
        self.flush();
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.flush();
    }

    fn flush(&self) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut state) = self.state.try_lock() {
            state.packets.clear();
            state.queued_samples = 0;
        }
    }

    fn enqueue_pcm16(&self, channels: u16, data: &[u8]) {
        if self.closed.load(Ordering::Acquire) || self.muted.load(Ordering::Acquire) {
            return;
        }
        if !(1..=2).contains(&channels) {
            return;
        }
        // Capture the fence before decoding. A mute/unmute race while parsing a large
        // packet must not relabel old samples with the new epoch.
        let epoch = self.epoch.load(Ordering::Acquire);

        let max_samples = MAX_QUEUED_SAMPLES - (MAX_QUEUED_SAMPLES % usize::from(channels));
        let mut samples = Vec::with_capacity(max_samples.min(data.len() / 2));
        for pair in data.chunks_exact(2).take(max_samples) {
            samples.push(f32::from(i16::from_le_bytes([pair[0], pair[1]])) / 32_768.0);
        }
        let complete = samples.len() - samples.len() % usize::from(channels);
        samples.truncate(complete);
        if samples.is_empty() {
            return;
        }

        let Ok(mut state) = self.state.try_lock() else {
            return;
        };
        if self.closed.load(Ordering::Acquire)
            || self.muted.load(Ordering::Acquire)
            || epoch != self.epoch.load(Ordering::Acquire)
        {
            return;
        }
        let available = MAX_QUEUED_SAMPLES.saturating_sub(state.queued_samples);
        let accepted = available - available % usize::from(channels);
        if accepted == 0 {
            return;
        }
        samples.truncate(samples.len().min(accepted));
        state.queued_samples += samples.len();
        state.packets.push_back(Packet {
            epoch,
            channels,
            samples,
            offset: 0,
        });
    }

    fn pop_frame(&self, channels: u16) -> Option<[f32; 2]> {
        if self.closed.load(Ordering::Acquire) || self.muted.load(Ordering::Acquire) {
            return None;
        }
        let epoch = self.epoch.load(Ordering::Acquire);
        let mut state = self.state.try_lock().ok()?;
        loop {
            let mut discarded = None;
            let mut frame = None;
            let mut finished = false;
            {
                let packet = state.packets.front_mut()?;
                if packet.epoch != epoch || packet.channels != channels {
                    discarded = Some(packet.samples.len().saturating_sub(packet.offset));
                } else if let Some(next) = packet.offset.checked_add(usize::from(channels)) {
                    if next > packet.samples.len() {
                        discarded = Some(packet.samples.len().saturating_sub(packet.offset));
                    } else {
                        let left = packet.samples[packet.offset];
                        let right = if channels == 1 {
                            left
                        } else {
                            packet.samples[packet.offset + 1]
                        };
                        packet.offset = next;
                        finished = packet.offset == packet.samples.len();
                        frame = Some([left, right]);
                    }
                } else {
                    discarded = Some(packet.samples.len().saturating_sub(packet.offset));
                }
            }
            if let Some(discarded) = discarded {
                state.queued_samples = state.queued_samples.saturating_sub(discarded);
                state.packets.pop_front();
                continue;
            }
            let frame = frame?;
            state.queued_samples = state.queued_samples.saturating_sub(usize::from(channels));
            if finished {
                state.packets.pop_front();
            }
            return Some(frame);
        }
    }

    #[cfg(test)]
    fn queued_samples(&self) -> usize {
        self.state.lock().expect("test queue mutex").queued_samples
    }
}

/// One backend per session; its dedicated OS thread exclusively owns the CPAL Stream.
pub(crate) struct AudioPlayback {
    queue: Arc<AudioQueue>,
    events: EventSink,
    worker: Mutex<Option<JoinHandle<()>>>,
    started: AtomicBool,
    published_state: AtomicU8,
    audio_muted: Mutex<tokio::sync::watch::Receiver<AudioMuteState>>,
    muted_revision: AtomicU64,
}

impl AudioPlayback {
    pub(crate) fn new(
        events: EventSink,
        audio_muted: tokio::sync::watch::Receiver<AudioMuteState>,
    ) -> Arc<Self> {
        let playback = Arc::new(Self {
            queue: Arc::new(AudioQueue::new()),
            events,
            worker: Mutex::new(None),
            started: AtomicBool::new(false),
            published_state: AtomicU8::new(u8::MAX),
            audio_muted: Mutex::new(audio_muted),
            muted_revision: AtomicU64::new(u64::MAX),
        });
        playback.publish(AudioPlaybackState::Waiting);
        playback
    }

    fn sync_mute_state(&self) {
        let Ok(receiver) = self.audio_muted.try_lock() else {
            return;
        };
        let state = *receiver.borrow();
        if self.muted_revision.swap(state.revision, Ordering::AcqRel) != state.revision {
            self.queue.apply_mute_state(state.muted);
        }
    }

    pub(crate) fn close(&self) {
        self.queue.close();
        if let Ok(mut worker) = self.worker.lock()
            && let Some(worker) = worker.take()
            && worker.thread().id() != thread::current().id()
        {
            let _ = worker.join();
        }
    }

    fn publish(&self, state: AudioPlaybackState) {
        let value = match state {
            AudioPlaybackState::Waiting => 0,
            AudioPlaybackState::Ready => 1,
            AudioPlaybackState::Unavailable => 2,
            AudioPlaybackState::Unsupported => 3,
        };
        if self.published_state.swap(value, Ordering::AcqRel) != value {
            (self.events)(EngineEvent::AudioState(state));
        }
    }

    fn start(self: &Arc<Self>) {
        if self.started.swap(true, Ordering::AcqRel) || self.queue.closed.load(Ordering::Acquire) {
            return;
        }
        let playback = Arc::clone(self);
        let worker = thread::spawn(move || playback.run_device());
        if let Ok(mut slot) = self.worker.lock() {
            *slot = Some(worker);
        }
    }

    fn run_device(self: Arc<Self>) {
        let host = cpal::default_host();
        let Some(device) = host.default_output_device() else {
            self.publish(AudioPlaybackState::Unavailable);
            return;
        };
        let Ok(supported) = device.default_output_config() else {
            self.publish(AudioPlaybackState::Unavailable);
            return;
        };
        let config = supported.config();
        if config.channels == 0 || config.sample_rate == 0 {
            self.publish(AudioPlaybackState::Unsupported);
            return;
        }
        let renderer = Renderer::new(Arc::clone(&self.queue), NEGOTIATED_FORMAT, &config);
        let stream = match build_stream(
            &device,
            &config,
            supported.sample_format(),
            renderer,
            Arc::clone(&self),
        ) {
            Ok(stream) => stream,
            Err(()) => return,
        };
        if stream.play().is_err() {
            self.publish(AudioPlaybackState::Unavailable);
            return;
        }
        self.publish(AudioPlaybackState::Ready);
        while !self.queue.closed.load(Ordering::Acquire) {
            self.sync_mute_state();
            thread::sleep(Duration::from_millis(20));
        }
        drop(stream);
    }
}

pub(crate) struct Backend {
    playback: Arc<AudioPlayback>,
}

impl Backend {
    pub(crate) fn new(playback: Arc<AudioPlayback>) -> Self {
        Self { playback }
    }
}

impl fmt::Debug for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NoriShellRdpsndBackend")
    }
}

impl_as_any!(Backend);

impl RdpsndClientHandler for Backend {
    fn get_formats(&self) -> &[AudioFormat] {
        &RDP_AUDIO_FORMATS
    }

    fn wave(&mut self, format_no: usize, _timestamp: u32, data: Cow<'_, [u8]>) {
        if format_no != 0 {
            self.playback.publish(AudioPlaybackState::Unsupported);
            return;
        }
        self.playback.sync_mute_state();
        self.playback.start();
        self.playback
            .queue
            .enqueue_pcm16(NEGOTIATED_FORMAT.channels, data.as_ref());
    }

    fn protocol_error(&mut self) {
        // The channel is permanently stopped. Join device initialization/callback cleanup
        // before publishing its terminal state so a late Ready cannot replace it.
        self.playback.close();
        self.playback.publish(AudioPlaybackState::Unsupported);
    }

    fn set_volume(&mut self, _volume: VolumePdu) {}

    fn set_pitch(&mut self, _pitch: PitchPdu) {}

    fn close(&mut self) {
        // RDPSND also invokes close when a server changes its format list. Keep the
        // session-owned device thread alive and fence old samples; the RDP engine's
        // outer cleanup performs the permanent close.
        self.playback.queue.flush();
    }
}

struct Renderer {
    queue: Arc<AudioQueue>,
    source: InputFormat,
    output_channels: usize,
    output_rate: u32,
    current: Option<[f32; 2]>,
    current_epoch: u64,
    phase: u32,
}

impl Renderer {
    fn new(queue: Arc<AudioQueue>, source: InputFormat, output: &StreamConfig) -> Self {
        let current_epoch = queue.epoch.load(Ordering::Acquire);
        Self {
            queue,
            source,
            output_channels: usize::from(output.channels),
            output_rate: output.sample_rate,
            current: None,
            current_epoch,
            phase: 0,
        }
    }

    fn fill<T: Sample + FromSample<f32>>(&mut self, data: &mut [T]) {
        for frame in data.chunks_mut(self.output_channels) {
            let [left, right] = self.next_frame();
            for (index, sample) in frame.iter_mut().enumerate() {
                let value = match index {
                    0 if self.output_channels == 1 => (left + right) * 0.5,
                    0 => left,
                    1 => right,
                    _ => 0.0,
                };
                *sample = T::from_sample(value.clamp(-1.0, 1.0));
            }
        }
    }

    fn next_frame(&mut self) -> [f32; 2] {
        let epoch = self.queue.epoch.load(Ordering::Acquire);
        if self.current_epoch != epoch {
            self.current_epoch = epoch;
            self.current = None;
            self.phase = 0;
        }
        if self.current.is_none() {
            self.current = self.queue.pop_frame(self.source.channels);
        }
        let frame = self.current.unwrap_or([0.0, 0.0]);
        self.phase = self.phase.saturating_add(self.source.sample_rate);
        while self.phase >= self.output_rate {
            self.phase -= self.output_rate;
            self.current = self.queue.pop_frame(self.source.channels);
            if self.current.is_none() {
                break;
            }
        }
        frame
    }
}

fn build_stream(
    device: &Device,
    config: &StreamConfig,
    sample_format: SampleFormat,
    mut renderer: Renderer,
    playback: Arc<AudioPlayback>,
) -> Result<Stream, ()> {
    macro_rules! build {
        ($sample:ty) => {{
            let failed = Arc::clone(&playback);
            device
                .build_output_stream::<$sample, _, _>(
                    config,
                    move |data, _| renderer.fill(data),
                    move |_| failed.publish(AudioPlaybackState::Unavailable),
                    None,
                )
                .map_err(|_| ())
        }};
    }
    let result = match sample_format {
        SampleFormat::I8 => build!(i8),
        SampleFormat::I16 => build!(i16),
        SampleFormat::I24 => build!(I24),
        SampleFormat::I32 => build!(i32),
        SampleFormat::I64 => build!(i64),
        SampleFormat::U8 => build!(u8),
        SampleFormat::U16 => build!(u16),
        SampleFormat::U24 => build!(U24),
        SampleFormat::U32 => build!(u32),
        SampleFormat::U64 => build!(u64),
        SampleFormat::F32 => build!(f32),
        SampleFormat::F64 => build!(f64),
        _ => {
            playback.publish(AudioPlaybackState::Unsupported);
            return Err(());
        }
    };
    if result.is_err() {
        playback.publish(AudioPlaybackState::Unavailable);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn pcm16_is_decoded_and_queue_stays_bounded() {
        let queue = AudioQueue::new();
        let data = vec![0xff; MAX_QUEUED_SAMPLES * 4];
        queue.enqueue_pcm16(2, &data);
        assert!(queue.queued_samples() <= MAX_QUEUED_SAMPLES);
        let frame = queue.pop_frame(2).expect("PCM frame");
        assert!(frame[0] < 0.0 && frame[1] < 0.0);
    }

    #[test]
    fn muting_discards_old_audio_and_accepts_only_new_audio_after_unmute() {
        let queue = AudioQueue::new();
        queue.enqueue_pcm16(2, &[1, 0, 2, 0]);
        queue.apply_mute_state(true);
        queue.apply_mute_state(false);
        assert!(queue.pop_frame(2).is_none());
        queue.enqueue_pcm16(2, &[3, 0, 4, 0]);
        assert!(queue.pop_frame(2).is_some());
    }

    #[test]
    fn closing_discards_audio() {
        let queue = AudioQueue::new();
        queue.enqueue_pcm16(2, &[1, 0, 2, 0]);
        queue.close();
        assert_eq!(queue.queued_samples(), 0);
        assert!(queue.pop_frame(2).is_none());
    }

    #[test]
    fn merged_mute_revision_discards_queue_and_renderer_held_frame() {
        let config = StreamConfig {
            channels: 2,
            sample_rate: 44_100,
            buffer_size: cpal::BufferSize::Default,
        };
        // The worker can observe only the latest revision after this quick toggle.
        // Revision two still forces a flush even though the final value is unmuted.
        let (mute_tx, _) = tokio::sync::watch::channel(AudioMuteState::default());
        let mute_rx = mute_tx.subscribe();
        let merged = AudioPlayback::new(Arc::new(|_| {}), mute_rx);
        merged.queue.enqueue_pcm16(2, &[1, 0, 2, 0, 3, 0, 4, 0]);
        let mut merged_renderer =
            Renderer::new(Arc::clone(&merged.queue), NEGOTIATED_FORMAT, &config);
        assert_ne!(merged_renderer.next_frame(), [0.0, 0.0]);
        mute_tx
            .send(AudioMuteState {
                muted: true,
                revision: 1,
            })
            .expect("test mute sender");
        mute_tx
            .send(AudioMuteState {
                muted: false,
                revision: 2,
            })
            .expect("test unmute sender");
        merged.sync_mute_state();
        assert_eq!(merged.queue.queued_samples(), 0);
        assert_eq!(merged_renderer.next_frame(), [0.0, 0.0]);
    }

    #[test]
    fn protocol_error_waits_for_late_device_ready_before_terminal_state() {
        let (_mute_tx, mute_rx) = tokio::sync::watch::channel(AudioMuteState::default());
        let states = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&states);
        let playback = AudioPlayback::new(
            Arc::new(move |event| {
                if let EngineEvent::AudioState(state) = event {
                    observed.lock().unwrap().push(state);
                }
            }),
            mute_rx,
        );
        let pending_device = Arc::clone(&playback);
        let worker = thread::spawn(move || {
            while !pending_device.queue.closed.load(Ordering::Acquire) {
                thread::yield_now();
            }
            pending_device.publish(AudioPlaybackState::Ready);
        });
        *playback.worker.lock().unwrap() = Some(worker);
        Backend::new(Arc::clone(&playback)).protocol_error();
        assert_eq!(
            states.lock().unwrap().last(),
            Some(&AudioPlaybackState::Unsupported)
        );
        assert!(playback.queue.closed.load(Ordering::Acquire));
        assert!(playback.worker.lock().unwrap().is_none());
    }

    #[test]
    fn audio_guard_drop_closes_and_joins_controlled_worker() {
        let (_mute_tx, mute_rx) = tokio::sync::watch::channel(AudioMuteState::default());
        let playback = AudioPlayback::new(Arc::new(|_| {}), mute_rx);
        let queue = Arc::clone(&playback.queue);
        let (done_tx, done_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            while !queue.closed.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(1));
            }
            done_tx.send(()).expect("test completion receiver");
        });
        *playback.worker.lock().expect("test worker mutex") = Some(worker);

        drop(crate::AudioGuard(Some(Arc::clone(&playback))));
        done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("AudioGuard joins worker after closing its queue");
        assert!(playback.queue.closed.load(Ordering::Acquire));
        assert!(playback.worker.lock().expect("test worker mutex").is_none());
    }
}
