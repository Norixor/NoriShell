//! Bounded frame and input contracts shared by desktop engines; owns no connections, credentials, or application state.
use std::sync::Arc;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{oneshot, watch},
};

pub const MAX_PIXELS: usize = 16_777_216;
pub const MAX_DIMENSION: u16 = 8_192;
pub const MAX_TEXT_BYTES: usize = 65_536;

pub trait DesktopIo: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> DesktopIo for T {}
pub type BoxedDesktopIo = Box<dyn DesktopIo>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    #[error("invalidConfiguration")]
    InvalidConfiguration,
    #[error("authenticationRejected")]
    AuthenticationRejected,
    #[error("certificateRejected")]
    CertificateRejected,
    #[error("unsupportedAuthentication")]
    UnsupportedAuthentication,
    #[error("unsupportedOperation")]
    UnsupportedOperation,
    #[error("protocolError")]
    Protocol,
    #[error("rdpUdpUnavailable")]
    RdpUdpUnavailable,
    #[error("rdpGraphicsUnavailable")]
    RdpGraphicsUnavailable,
    #[error("rdpResolutionUnavailable")]
    RdpResolutionUnavailable,
    #[error("rdpResolutionModeDisabled")]
    RdpResolutionModeDisabled,
    #[error("rdpResolutionNotApplied")]
    RdpResolutionNotApplied,
    #[error("vncResolutionUnavailable")]
    VncResolutionUnavailable,
    #[error("vncResolutionRejected")]
    VncResolutionRejected,
    #[error("vncResolutionNotApplied")]
    VncResolutionNotApplied,
    #[error("vncResolutionModeDisabled")]
    VncResolutionModeDisabled,
    #[error("resourceLimit")]
    ResourceLimit,
    #[error("connectionLost")]
    ConnectionLost,
    #[error("timeout")]
    Timeout,
    #[error("cancelled")]
    Cancelled,
    #[error("staleInput")]
    StaleInput,
}

pub type Result<T> = std::result::Result<T, EngineError>;

#[derive(Clone)]
pub struct DesktopFrame {
    pub width: u16,
    pub height: u16,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DesktopRect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl DesktopRect {
    pub fn fits(self, width: u16, height: u16) -> bool {
        self.width != 0
            && self.height != 0
            && u32::from(self.x) + u32::from(self.width) <= u32::from(width)
            && u32::from(self.y) + u32::from(self.height) <= u32::from(height)
    }

    pub fn union(self, other: Self) -> Self {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = (u32::from(self.x) + u32::from(self.width))
            .max(u32::from(other.x) + u32::from(other.width));
        let bottom = (u32::from(self.y) + u32::from(self.height))
            .max(u32::from(other.y) + u32::from(other.height));
        Self {
            x,
            y,
            width: (right - u32::from(x)) as u16,
            height: (bottom - u32::from(y)) as u16,
        }
    }
}

impl DesktopFrame {
    pub fn new(width: u16, height: u16) -> Result<Self> {
        let len = frame_len(width, height)?;
        Ok(Self {
            width,
            height,
            rgba: vec![0; len],
        })
    }

    pub fn write_rect(
        &mut self,
        x: u16,
        y: u16,
        width: u16,
        height: u16,
        rgba: &[u8],
    ) -> Result<()> {
        self.check_rect(x, y, width, height)?;
        let row_bytes = usize::from(width) * 4;
        if rgba.len() != row_bytes * usize::from(height) {
            return Err(EngineError::Protocol);
        }
        for row in 0..usize::from(height) {
            let target = ((usize::from(y) + row) * usize::from(self.width) + usize::from(x)) * 4;
            self.rgba[target..target + row_bytes]
                .copy_from_slice(&rgba[row * row_bytes..(row + 1) * row_bytes]);
        }
        Ok(())
    }

    pub fn copy_rect(
        &mut self,
        source_x: u16,
        source_y: u16,
        x: u16,
        y: u16,
        width: u16,
        height: u16,
    ) -> Result<()> {
        self.check_rect(source_x, source_y, width, height)?;
        self.check_rect(x, y, width, height)?;
        let rows: Box<dyn Iterator<Item = usize>> = if y > source_y {
            Box::new((0..usize::from(height)).rev())
        } else {
            Box::new(0..usize::from(height))
        };
        for row in rows {
            let source = ((usize::from(source_y) + row) * usize::from(self.width)
                + usize::from(source_x))
                * 4;
            let target = ((usize::from(y) + row) * usize::from(self.width) + usize::from(x)) * 4;
            self.rgba
                .copy_within(source..source + usize::from(width) * 4, target);
        }
        Ok(())
    }

    fn check_rect(&self, x: u16, y: u16, width: u16, height: u16) -> Result<()> {
        if width == 0
            || height == 0
            || u32::from(x) + u32::from(width) > u32::from(self.width)
            || u32::from(y) + u32::from(height) > u32::from(self.height)
        {
            return Err(EngineError::Protocol);
        }
        if self.rgba.len() != frame_len(self.width, self.height)? {
            return Err(EngineError::Protocol);
        }
        Ok(())
    }
}

pub fn frame_len(width: u16, height: u16) -> Result<usize> {
    let pixels = usize::from(width) * usize::from(height);
    if width == 0
        || height == 0
        || width > MAX_DIMENSION
        || height > MAX_DIMENSION
        || pixels > MAX_PIXELS
    {
        return Err(EngineError::ResourceLimit);
    }
    Ok(pixels * 4)
}

#[derive(Debug, Clone)]
pub enum DesktopInput {
    Key {
        scan_code: u16,
        keysym: u32,
        down: bool,
    },
    Pointer {
        x: u16,
        y: u16,
        buttons: u8,
    },
    Wheel {
        x: u16,
        y: u16,
        delta_x: i16,
        delta_y: i16,
    },
    Text(String),
    Clipboard(String),
    Resize {
        width: u16,
        height: u16,
    },
    ReleaseAll,
}

impl DesktopInput {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Text(text) | Self::Clipboard(text)
                if text.len() > MAX_TEXT_BYTES || text.contains('\0') =>
            {
                Err(EngineError::ResourceLimit)
            }
            Self::Key {
                scan_code, keysym, ..
            } if *scan_code > 0x1ff || *keysym == 0 => Err(EngineError::InvalidConfiguration),
            Self::Pointer { buttons, .. } if *buttons > 7 => Err(EngineError::InvalidConfiguration),
            Self::Resize { width, height } => frame_len(*width, *height).map(|_| ()),
            _ => Ok(()),
        }
    }
}

pub struct EngineCommand {
    pub input: DesktopInput,
    /// None is reserved for a session-fenced adaptive resize, which does not own keyboard focus.
    pub focus_epoch: Option<u64>,
    /// Acknowledge only after the protocol writer completes; callers must not replay non-idempotent input automatically.
    pub completion: oneshot::Sender<Result<()>>,
}

pub struct EngineControl {
    pub stop: watch::Receiver<bool>,
    /// Incremented on every focus change. Engines release held inputs first and reject queued input from the old epoch.
    pub focus_epoch: watch::Receiver<u64>,
}

impl EngineControl {
    pub fn accepts(&self, command: &EngineCommand) -> bool {
        !*self.stop.borrow()
            && match command.focus_epoch {
                Some(epoch) => epoch == *self.focus_epoch.borrow(),
                None => matches!(&command.input, DesktopInput::Resize { .. }),
            }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioPlaybackState {
    Waiting,
    Ready,
    Unavailable,
    Unsupported,
}

/// Core maintains the mute projection for each desktop session. Every actual toggle increments `revision`;
/// audio workers use it as a fence to discard old PCM even if the session is unmuted again between polls.
#[derive(Debug, Clone, Copy, Default)]
pub struct AudioMuteState {
    pub muted: bool,
    pub revision: u64,
}

pub enum EngineEvent {
    Ready,
    RdpTransport(RdpTransportActual),
    RdpGraphics(RdpGraphicsActual),
    Frame(Arc<DesktopFrame>),
    FrameDirty(Arc<DesktopFrame>, DesktopRect),
    Clipboard(String),
    AudioState(AudioPlaybackState),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RdpTransportActual {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RdpGraphicsActual {
    Bitmap,
    RemoteFx,
    RemoteFxProgressive,
    Avc420,
}

/// Callbacks replace only the latest projection; they must not queue every frame or block the protocol worker.
pub type EventSink = Arc<dyn Fn(EngineEvent) + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dirty_rect_union_covers_missed_updates() {
        let first = DesktopRect {
            x: 1,
            y: 2,
            width: 2,
            height: 2,
        };
        let later = DesktopRect {
            x: 6,
            y: 5,
            width: 1,
            height: 1,
        };
        assert_eq!(
            first.union(later),
            DesktopRect {
                x: 1,
                y: 2,
                width: 6,
                height: 4
            }
        );
        assert!(first.union(later).fits(8, 8));
        assert!(!first.union(later).fits(6, 8));
    }
    #[test]
    fn rejects_server_dimensions_and_rectangles_before_writing() {
        assert!(DesktopFrame::new(u16::MAX, u16::MAX).is_err());
        let mut image = DesktopFrame::new(2, 2).unwrap();
        assert!(image.write_rect(u16::MAX, 0, 2, 1, &[0; 8]).is_err());
        assert!(image.write_rect(0, 0, 2, 2, &[0; 15]).is_err());
        assert!(image.rgba.iter().all(|byte| *byte == 0));
    }
    #[test]
    fn overlapping_copy_keeps_original_rows() {
        let mut image = DesktopFrame::new(1, 3).unwrap();
        image.rgba = vec![1, 1, 1, 255, 2, 2, 2, 255, 3, 3, 3, 255];
        image.copy_rect(0, 0, 0, 1, 1, 2).unwrap();
        assert_eq!(&image.rgba[4..], &[1, 1, 1, 255, 2, 2, 2, 255]);
    }
}
