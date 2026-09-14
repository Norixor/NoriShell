//! Smoke test against a real TigerVNC container, used only by qa/remote-desktop/smoke.sh.

use norishell_desktop_protocol::{
    DesktopInput, EngineCommand, EngineControl, EngineError, EngineEvent, EventSink,
};
use norishell_vnc_client::{VncOptions, run};
use std::{env, error::Error, fs, path::Path, sync::Arc, time::Duration};
use tokio::{
    net::TcpStream,
    sync::{mpsc, oneshot, watch},
    time::timeout,
};
use zeroize::Zeroizing;

const WAIT: Duration = Duration::from_secs(30);

#[derive(Clone)]
enum Observation {
    Ready,
    Frame(Arc<norishell_desktop_protocol::DesktopFrame>),
}

fn required(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("{name} must be set"))
}

fn options(password: String) -> VncOptions {
    VncOptions {
        password: Zeroizing::new(password),
        allow_unauthenticated: false,
        clipboard_enabled: false,
    }
}

async fn authentication_rejects(address: &str, password: String) -> Result<(), Box<dyn Error>> {
    let stream = TcpStream::connect(address).await?;
    let (_commands, receiver) = mpsc::channel(1);
    let (_stop_tx, stop) = watch::channel(false);
    let (_epoch_tx, focus_epoch) = watch::channel(0_u64);
    match timeout(
        WAIT,
        run(
            Box::new(stream),
            options(password),
            receiver,
            EngineControl { stop, focus_epoch },
            Arc::new(|_| {}),
        ),
    )
    .await?
    {
        Err(EngineError::AuthenticationRejected) => Ok(()),
        other => Err(format!("VNC wrong-password result was {other:?}").into()),
    }
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
        Err(error) => Err(format!("VNC input was rejected: {error}").into()),
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

async fn successful_session(address: &str, password: String) -> Result<(), Box<dyn Error>> {
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
        EngineEvent::Clipboard(_) | EngineEvent::AudioState(_) => {}
    });
    let task = tokio::spawn(run(
        Box::new(stream),
        options(password),
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
                None => return Err("VNC event stream closed before ready/frame"),
            }
        }
        Ok::<_, &'static str>(frame.expect("checked above"))
    })
    .await??;
    let settled_frame = timeout(Duration::from_secs(8), async {
        let mut newest = first_frame;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
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
                Ok(None) => break,
                Err(_) => {}
            }
        }
        newest
    })
    .await?;
    if let Ok(path) = env::var("QA_VNC_FRAME") {
        write_ppm(Path::new(&path), &settled_frame)?;
        println!("VNC frame retained at {path}");
    }

    send_key(&commands, true).await?;
    send_key(&commands, false).await?;
    stop_tx.send(true)?;
    match timeout(WAIT, task).await?? {
        Err(EngineError::Cancelled) => Ok(()),
        other => Err(format!("VNC close returned {other:?}").into()),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let address = required("VNC_ADDR");
    let password = required("QA_PASSWORD");
    let failed_auth = if env::var_os("QA_SKIP_FAILED_AUTH").is_none() {
        // VNCAuth uses only the first eight password bytes. Alter the first
        // byte so this is a real rejected credential, not an accidental match.
        let wrong_password = format!("x{}", &password[1..]);
        authentication_rejects(&address, wrong_password).await?;
        "engine authenticationRejected"
    } else {
        "failed-auth observation skipped"
    };
    successful_session(&address, password).await?;
    println!("VNC QA smoke passed: {failed_auth}, frame, input, close");
    Ok(())
}
