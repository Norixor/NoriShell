//! Smoke test against a real xrdp container, used only by qa/remote-desktop/smoke.sh.
//! Approval of a temporary self-signed certificate is restricted to the exact caller-supplied SHA-256 fingerprint.

use norishell_desktop_protocol::{
    AudioMuteState, AudioPlaybackState, DesktopInput, EngineCommand, EngineControl, EngineError,
    EngineEvent, EventSink,
};
use norishell_rdp_client::{CertificateApproval, RdpOptions, run};
use std::{env, error::Error, fs, path::Path, sync::Arc, time::Duration};
use tokio::{
    net::TcpStream,
    sync::{mpsc, oneshot, watch},
    time::timeout,
};
use zeroize::Zeroizing;

const WAIT: Duration = Duration::from_secs(40);

#[derive(Clone)]
enum Observation {
    Ready,
    Frame(Arc<norishell_desktop_protocol::DesktopFrame>),
    Audio(AudioPlaybackState),
}

fn required(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("{name} must be set"))
}

fn exact_approval(expected: String) -> CertificateApproval {
    Arc::new(move |challenge| {
        let accepted = challenge.server_name == "127.0.0.1"
            && challenge.sha256_fingerprint.eq_ignore_ascii_case(&expected);
        eprintln!(
            "RDP certificate approval: target={} observed={} exact_pin={accepted}",
            challenge.server_name, challenge.sha256_fingerprint
        );
        Box::pin(async move { accepted })
    })
}

fn options(password: String, expected_fingerprint: String) -> RdpOptions {
    RdpOptions {
        // This must remain the TLS identity in the certificate, never the
        // address of a future SSH tunnel.
        server_name: "127.0.0.1".into(),
        username: required("QA_USER"),
        domain: None,
        password: Zeroizing::new(password),
        width: 1024,
        height: 768,
        clipboard_enabled: false,
        audio_playback_enabled: false,
        audio_muted: watch::channel(AudioMuteState::default()).1,
        certificate_approval: Some(exact_approval(expected_fingerprint)),
    }
}

fn audio_options(
    password: String,
    expected_fingerprint: String,
    audio_muted: watch::Receiver<AudioMuteState>,
) -> RdpOptions {
    let mut configured = options(password, expected_fingerprint);
    configured.audio_playback_enabled = true;
    configured.audio_muted = audio_muted;
    configured
}

async fn observe_failed_authentication(
    address: &str,
    password: String,
    expected_fingerprint: String,
) -> Result<&'static str, Box<dyn Error>> {
    let stream = TcpStream::connect(address).await?;
    let (_commands, receiver) = mpsc::channel(1);
    let (stop_tx, stop) = watch::channel(false);
    let (_epoch_tx, focus_epoch) = watch::channel(0_u64);
    let (observed_tx, mut observed_rx) = mpsc::unbounded_channel();
    let sink: EventSink = Arc::new(move |event| match event {
        EngineEvent::Ready => {
            let _ = observed_tx.send(Observation::Ready);
        }
        EngineEvent::Frame(frame) => {
            let _ = observed_tx.send(Observation::Frame(frame));
        }
        EngineEvent::Clipboard(_) => {}
        EngineEvent::AudioState(state) => {
            let _ = observed_tx.send(Observation::Audio(state));
        }
    });
    let mut task = tokio::spawn(run(
        Box::new(stream),
        options(password, expected_fingerprint),
        receiver,
        EngineControl { stop, focus_epoch },
        sink,
    ));
    enum Outcome {
        Engine(norishell_desktop_protocol::Result<()>),
        Active,
        TimedOut,
    }
    let outcome = match timeout(Duration::from_secs(12), async {
        loop {
            tokio::select! {
                result = &mut task => return Outcome::Engine(match result {
                    Ok(value) => value,
                    Err(_) => Err(EngineError::Protocol),
                }),
                event = observed_rx.recv() => match event {
                Some(Observation::Ready) => {}
                Some(Observation::Frame(_)) => return Outcome::Active,
                Some(Observation::Audio(_)) => {}
                    None => return Outcome::Engine(Err(EngineError::ConnectionLost)),
                }
            }
        }
    })
    .await
    {
        Ok(outcome) => outcome,
        Err(_) => Outcome::TimedOut,
    };
    let reached_frame = matches!(&outcome, Outcome::Active);
    let message = match outcome {
        Outcome::Engine(Err(EngineError::AuthenticationRejected)) => {
            "engine authenticationRejected"
        }
        Outcome::Engine(other) => {
            return Err(format!("RDP wrong-password engine result was {other:?}").into());
        }
        Outcome::Active => {
            "RDP reached a framebuffer; xrdp uses post-TLS login, inspect xrdp login result"
        }
        Outcome::TimedOut => {
            "no protocol rejection within 12 seconds; xrdp login result requires server evidence"
        }
    };
    if reached_frame {
        // xrdp's TLS mode authenticates through its session manager after the
        // first RDP framebuffer exists. Leave that server-side exchange time
        // to finish so its login log records the wrong-password outcome.
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    let _ = stop_tx.send(true);
    if !task.is_finished() {
        let _ = timeout(Duration::from_secs(3), &mut task).await;
    }
    Ok(message)
}

async fn send_key(
    commands: &mpsc::Sender<EngineCommand>,
    down: bool,
) -> Result<(), Box<dyn Error>> {
    let (completion, complete) = oneshot::channel();
    commands
        .send(EngineCommand {
            input: DesktopInput::Key {
                scan_code: 0x1e,
                keysym: u32::from(b'a'),
                down,
            },
            focus_epoch: 0,
            completion,
        })
        .await?;
    match timeout(WAIT, complete).await?? {
        Ok(()) => Ok(()),
        Err(error) => Err(format!("RDP input was rejected: {error}").into()),
    }
}

fn write_ppm(
    path: &Path,
    frame: &norishell_desktop_protocol::DesktopFrame,
) -> Result<(), Box<dyn Error>> {
    let mut image = format!("P6\n{} {}\n255\n", frame.width, frame.height).into_bytes();
    for pixel in frame.rgba.chunks_exact(4) {
        image.extend_from_slice(&pixel[..3]);
    }
    fs::write(path, image)?;
    Ok(())
}

// The fixture has a blue background and a light marker window; a black first frame with only a cursor is insufficient.
// This check does not replace screenshot review or server-side authentication logs.
fn has_fixture_content(frame: &norishell_desktop_protocol::DesktopFrame) -> bool {
    let mut colored = 0;
    let mut bright = 0;
    for pixel in frame.rgba.chunks_exact(4) {
        if pixel[2] > pixel[0].saturating_add(20) && pixel[2] > 40 {
            colored += 1;
        }
        if pixel[..3].iter().all(|channel| *channel > 180) {
            bright += 1;
        }
    }
    let pixels = usize::from(frame.width) * usize::from(frame.height);
    colored > pixels / 10 && bright > pixels / 100
}

async fn successful_session(
    address: &str,
    password: String,
    expected_fingerprint: String,
) -> Result<(), Box<dyn Error>> {
    let stream = TcpStream::connect(address).await?;
    let (commands, receiver) = mpsc::channel(4);
    let (stop_tx, stop) = watch::channel(false);
    let (_epoch_tx, focus_epoch) = watch::channel(0_u64);
    let (observed_tx, mut observed_rx) = mpsc::unbounded_channel();
    let sink: EventSink = Arc::new(move |event| match event {
        EngineEvent::Ready => {
            let _ = observed_tx.send(Observation::Ready);
        }
        EngineEvent::Frame(frame) => {
            let _ = observed_tx.send(Observation::Frame(frame));
        }
        EngineEvent::Clipboard(_) => {}
        EngineEvent::AudioState(state) => {
            let _ = observed_tx.send(Observation::Audio(state));
        }
    });
    let task = tokio::spawn(run(
        Box::new(stream),
        options(password, expected_fingerprint),
        receiver,
        EngineControl { stop, focus_epoch },
        sink,
    ));

    let first_frame = timeout(WAIT, async {
        let mut ready = false;
        let mut frame = None;
        while !ready || frame.is_none() {
            match observed_rx.recv().await {
                Some(Observation::Ready) => ready = true,
                Some(Observation::Frame(next)) => frame = Some(next),
                Some(Observation::Audio(_)) => {}
                None => return Err("RDP event stream closed before ready/frame"),
            }
        }
        Ok::<_, &'static str>(frame.expect("checked above"))
    })
    .await??;

    // xrdp can use an RDP login UI when its security layer does not perform
    // CredSSP. Keep the newest post-settle frame, not merely the transport
    // Ready event, so a reviewer can distinguish that screen from the marked
    // authenticated Openbox session provided by the fixture.
    let settle = Duration::from_secs(
        env::var("QA_RDP_SETTLE_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(6)
            .clamp(1, 120),
    );
    let settled_frame = timeout(settle + Duration::from_secs(2), async {
        let mut newest = first_frame;
        let deadline = tokio::time::Instant::now() + settle;
        while tokio::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            match timeout(
                remaining.min(Duration::from_millis(600)),
                observed_rx.recv(),
            )
            .await
            {
                Ok(Some(Observation::Frame(next))) => newest = next,
                Ok(Some(Observation::Ready)) => {}
                Ok(Some(Observation::Audio(_))) => {}
                Ok(None) => break,
                Err(_) => {}
            }
        }
        newest
    })
    .await?;
    if let Ok(path) = env::var("QA_RDP_FRAME") {
        write_ppm(Path::new(&path), &settled_frame)?;
        println!("RDP frame retained at {path}");
    }

    let content_visible = has_fixture_content(&settled_frame);
    send_key(&commands, true).await?;
    send_key(&commands, false).await?;
    stop_tx.send(true)?;
    match timeout(WAIT, task).await?? {
        Err(EngineError::Cancelled) if content_visible => Ok(()),
        Err(EngineError::Cancelled) => Err(
            "RDP frame lacks the fixture desktop content (black/pointer-only is not a pass)".into(),
        ),
        other => Err(format!("RDP close returned {other:?}").into()),
    }
}

fn audio_state_name(state: AudioPlaybackState) -> &'static str {
    match state {
        AudioPlaybackState::Waiting => "waiting",
        AudioPlaybackState::Ready => "ready",
        AudioPlaybackState::Unavailable => "unavailable",
        AudioPlaybackState::Unsupported => "unsupported",
    }
}

async fn audio_playback_session(
    address: &str,
    password: String,
    expected_fingerprint: String,
) -> Result<(), Box<dyn Error>> {
    let stream = TcpStream::connect(address).await?;
    let (_commands, receiver) = mpsc::channel(1);
    let (stop_tx, stop) = watch::channel(false);
    let (_epoch_tx, focus_epoch) = watch::channel(0_u64);
    let (mute_tx, audio_muted) = watch::channel(AudioMuteState::default());
    let (observed_tx, mut observed_rx) = mpsc::unbounded_channel();
    let sink: EventSink = Arc::new(move |event| match event {
        EngineEvent::Ready => {
            let _ = observed_tx.send(Observation::Ready);
        }
        EngineEvent::Frame(frame) => {
            let _ = observed_tx.send(Observation::Frame(frame));
        }
        EngineEvent::Clipboard(_) => {}
        EngineEvent::AudioState(state) => {
            let _ = observed_tx.send(Observation::Audio(state));
        }
    });
    let mut task = tokio::spawn(run(
        Box::new(stream),
        audio_options(password, expected_fingerprint, audio_muted),
        receiver,
        EngineControl { stop, focus_epoch },
        sink,
    ));

    let readiness = timeout(WAIT, async {
        let mut desktop_ready = false;
        let mut audio_ready = false;
        loop {
            match observed_rx.recv().await {
                Some(Observation::Ready) => {
                    desktop_ready = true;
                    if audio_ready {
                        return Ok::<_, Box<dyn Error>>(());
                    }
                }
                Some(Observation::Frame(_)) => {}
                Some(Observation::Audio(state)) => {
                    println!("RDP audio state: {}", audio_state_name(state));
                    if state == AudioPlaybackState::Ready {
                        audio_ready = true;
                        if desktop_ready {
                            return Ok::<_, Box<dyn Error>>(());
                        }
                    }
                    if matches!(
                        state,
                        AudioPlaybackState::Unavailable | AudioPlaybackState::Unsupported
                    ) {
                        return Err(format!("RDP audio became {}", audio_state_name(state)).into());
                    }
                }
                None => {
                    return Err("RDP audio event stream closed before playback became ready".into());
                }
            }
        }
    })
    .await;
    if !matches!(readiness, Ok(Ok(()))) {
        let _ = stop_tx.send(true);
        let outcome = timeout(WAIT, &mut task).await;
        return Err(format!("audio readiness={readiness:?}; engine={outcome:?}").into());
    }

    let total = Duration::from_secs(
        env::var("QA_AUDIO_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(60)
            .clamp(25, 300),
    );
    tokio::time::sleep(Duration::from_secs(10)).await;
    mute_tx.send(AudioMuteState {
        muted: true,
        revision: 1,
    })?;
    println!("RDP audio muted for QA");
    tokio::time::sleep(Duration::from_secs(15)).await;
    mute_tx.send(AudioMuteState {
        muted: false,
        revision: 2,
    })?;
    println!("RDP audio unmuted for QA");
    tokio::time::sleep(total.saturating_sub(Duration::from_secs(25))).await;

    stop_tx.send(true)?;
    match timeout(WAIT, &mut task).await?? {
        Err(EngineError::Cancelled) => Ok(()),
        other => Err(format!("RDP audio QA close returned {other:?}").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_frame_with_pointer_does_not_pass_as_authenticated_desktop() {
        let mut frame = norishell_desktop_protocol::DesktopFrame {
            width: 100,
            height: 100,
            rgba: vec![0; 100 * 100 * 4],
        };
        frame.rgba[..32 * 4].fill(255);
        assert!(!has_fixture_content(&frame));
        for pixel in frame.rgba.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[23, 48, 75, 255]);
        }
        assert!(!has_fixture_content(&frame));
        frame.rgba[..1000 * 4].fill(240);
        assert!(has_fixture_content(&frame));
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let address = required("RDP_ADDR");
    let password = required("QA_PASSWORD");
    let expected_fingerprint = required("RDP_CERT_SHA256");
    let wrong_password = format!("{password}-wrong");

    let failed_auth = if env::var_os("QA_SKIP_FAILED_AUTH").is_none() {
        observe_failed_authentication(&address, wrong_password, expected_fingerprint.clone())
            .await?
    } else {
        "failed-auth observation skipped"
    };
    if env::var_os("QA_AUDIO_PLAYBACK").is_some() {
        audio_playback_session(&address, password, expected_fingerprint).await?;
        println!(
            "RDP audio QA passed: {failed_auth}; exact certificate pin, native device ready, mute requests, clean cancellation"
        );
    } else {
        successful_session(&address, password, expected_fingerprint).await?;
        println!("RDP QA smoke passed: {failed_auth}; exact certificate pin, frame, input, close");
    }
    Ok(())
}
