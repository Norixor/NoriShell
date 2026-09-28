//! Bounded frame and input contracts shared by desktop engines; owns no connections, credentials, or application state.
use std::sync::Arc;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{oneshot, watch},
};

pub const MAX_PIXELS: usize = 16_777_216;
pub const MAX_DIMENSION: u16 = 8_192;
pub const MAX_TEXT_BYTES: usize = 65_536;
/// RDP large pointers are at most 384x384; VNC rich cursors share the same bound.
pub const MAX_CURSOR_DIMENSION: u16 = 384;
/// Dirty regions keep a short rectangle list; beyond it the closest pair is merged.
pub const MAX_DIRTY_RECTS: usize = 16;
/// Engines publish decoded pixels at most this often; later updates coalesce into the pending region.
pub const PUBLISH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(16);

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

    pub fn area(self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    fn contains(self, other: Self) -> bool {
        self.x <= other.x
            && self.y <= other.y
            && u32::from(self.x) + u32::from(self.width)
                >= u32::from(other.x) + u32::from(other.width)
            && u32::from(self.y) + u32::from(self.height)
                >= u32::from(other.y) + u32::from(other.height)
    }

    fn touches(self, other: Self) -> bool {
        u32::from(self.x) <= u32::from(other.x) + u32::from(other.width)
            && u32::from(other.x) <= u32::from(self.x) + u32::from(self.width)
            && u32::from(self.y) <= u32::from(other.y) + u32::from(other.height)
            && u32::from(other.y) <= u32::from(self.y) + u32::from(self.height)
    }
}

/// A bounded list of changed rectangles. Overlapping or adjacent rectangles merge immediately; when the list
/// exceeds `MAX_DIRTY_RECTS`, the pair whose union adds the least area merges, so distant small changes do not
/// collapse into one full-screen bounding box. Rectangles may still over-cover, never under-cover.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirtyRegion {
    rects: Vec<DesktopRect>,
}

impl DirtyRegion {
    pub fn add(&mut self, rect: DesktopRect) {
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        let mut next = rect;
        loop {
            if self.rects.iter().any(|existing| existing.contains(next)) {
                return;
            }
            match self
                .rects
                .iter()
                .position(|existing| existing.touches(next))
            {
                Some(index) => next = next.union(self.rects.swap_remove(index)),
                None => break,
            }
        }
        self.rects.push(next);
        while self.rects.len() > MAX_DIRTY_RECTS {
            let mut best = (0, 1, u64::MAX);
            for i in 0..self.rects.len() {
                for j in i + 1..self.rects.len() {
                    let (a, b) = (self.rects[i], self.rects[j]);
                    let growth = a.union(b).area().saturating_sub(a.area() + b.area());
                    if growth < best.2 {
                        best = (i, j, growth);
                    }
                }
            }
            let second = self.rects.swap_remove(best.1);
            let first = self.rects.swap_remove(best.0);
            // Re-adding may cascade merges with rectangles the union now touches.
            self.add(first.union(second));
        }
    }

    pub fn extend(&mut self, other: &DirtyRegion) {
        for rect in &other.rects {
            self.add(*rect);
        }
    }

    pub fn rects(&self) -> &[DesktopRect] {
        &self.rects
    }

    /// Sum of rectangle areas; an upper bound on changed pixels when rectangles do not overlap.
    pub fn area(&self) -> u64 {
        self.rects.iter().map(|rect| rect.area()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    pub fn clear(&mut self) {
        self.rects.clear();
    }

    pub fn fits(&self, width: u16, height: u16) -> bool {
        self.rects.iter().all(|rect| rect.fits(width, height))
    }
}

/// Pixel rows for changed rectangles of a frame whose size equals the most recent full `Frame` event.
#[derive(Clone)]
pub struct FramePatch {
    pub width: u16,
    pub height: u16,
    /// Each entry holds tightly packed RGBA rows for exactly that rectangle.
    pub rects: Vec<(DesktopRect, Vec<u8>)>,
}

/// Remote pointer presentation. Bitmaps carry straight (non-premultiplied) RGBA.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DesktopCursor {
    /// The platform default arrow.
    Default,
    Hidden,
    Bitmap(Arc<CursorBitmap>),
}

#[derive(Debug, PartialEq, Eq)]
pub struct CursorBitmap {
    pub width: u16,
    pub height: u16,
    pub hotspot_x: u16,
    pub hotspot_y: u16,
    pub rgba: Vec<u8>,
}

impl CursorBitmap {
    pub fn new(
        width: u16,
        height: u16,
        hotspot_x: u16,
        hotspot_y: u16,
        rgba: Vec<u8>,
    ) -> Result<Self> {
        if width == 0
            || height == 0
            || width > MAX_CURSOR_DIMENSION
            || height > MAX_CURSOR_DIMENSION
            || rgba.len() != usize::from(width) * usize::from(height) * 4
        {
            return Err(EngineError::Protocol);
        }
        // Servers occasionally report a hotspot on the far edge; clamp instead of rejecting the cursor.
        Ok(Self {
            width,
            height,
            hotspot_x: hotspot_x.min(width - 1),
            hotspot_y: hotspot_y.min(height - 1),
            rgba,
        })
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

    /// Copies the listed rectangles out of this frame for a `FramePatch` event.
    pub fn extract_patch(&self, region: &DirtyRegion) -> Result<FramePatch> {
        let mut rects = Vec::with_capacity(region.rects().len());
        for rect in region.rects() {
            self.check_rect(rect.x, rect.y, rect.width, rect.height)?;
            rects.push((*rect, self.read_rect(*rect)));
        }
        Ok(FramePatch {
            width: self.width,
            height: self.height,
            rects,
        })
    }

    /// Copies one validated rectangle into tightly packed RGBA rows.
    pub fn read_rect(&self, rect: DesktopRect) -> Vec<u8> {
        read_rect(&self.rgba, self.width, rect)
    }

    pub fn apply_patch(&mut self, patch: &FramePatch) -> Result<()> {
        if patch.width != self.width || patch.height != self.height {
            return Err(EngineError::Protocol);
        }
        for (rect, rgba) in &patch.rects {
            self.write_rect(rect.x, rect.y, rect.width, rect.height, rgba)?;
        }
        Ok(())
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

/// Copies `rect` out of a packed RGBA buffer `stride_width` pixels wide. Callers validate bounds first.
pub fn read_rect(rgba: &[u8], stride_width: u16, rect: DesktopRect) -> Vec<u8> {
    let row_bytes = usize::from(rect.width) * 4;
    let mut out = Vec::with_capacity(row_bytes * usize::from(rect.height));
    for row in 0..usize::from(rect.height) {
        let start =
            ((usize::from(rect.y) + row) * usize::from(stride_width) + usize::from(rect.x)) * 4;
        out.extend_from_slice(&rgba[start..start + row_bytes]);
    }
    out
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
        /// Remote UI scale in percent (100 = standard). RDP forwards it through Display Control; VNC ignores it.
        scale_percent: u16,
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
            Self::Resize {
                width,
                height,
                scale_percent,
            } => {
                if !(100..=500).contains(scale_percent) {
                    return Err(EngineError::InvalidConfiguration);
                }
                frame_len(*width, *height).map(|_| ())
            }
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
    /// Complete replacement. Sent for the first frame and whenever the size changes; later patches must match its size.
    Frame(Arc<DesktopFrame>),
    /// Changed rectangles relative to the previously published pixels. Engines coalesce updates and publish at most
    /// once per `PUBLISH_INTERVAL`; a size mismatch with the last `Frame` is a protocol error.
    FramePatch(FramePatch),
    /// Pointer shape changes; the WebView draws the pointer locally instead of compositing it into frames.
    Cursor(DesktopCursor),
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
    fn rect(x: u16, y: u16, width: u16, height: u16) -> DesktopRect {
        DesktopRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn dirty_region_keeps_distant_rectangles_separate_and_merges_touching_ones() {
        let mut region = DirtyRegion::default();
        region.add(rect(0, 0, 2, 2));
        region.add(rect(100, 100, 2, 2));
        assert_eq!(region.rects().len(), 2);
        region.add(rect(2, 0, 2, 2));
        assert_eq!(region.rects().len(), 2);
        assert!(region.rects().contains(&rect(0, 0, 4, 2)));
        region.add(rect(1, 1, 1, 1));
        assert_eq!(region.rects().len(), 2);
    }

    #[test]
    fn dirty_region_bounds_rectangle_count_without_losing_coverage() {
        let mut region = DirtyRegion::default();
        let mut added = Vec::new();
        for i in 0..40u16 {
            let next = rect((i % 8) * 50, (i / 8) * 50, 3, 3);
            added.push(next);
            region.add(next);
        }
        assert!(region.rects().len() <= MAX_DIRTY_RECTS);
        for next in added {
            assert!(region.rects().iter().any(|covered| covered.contains(next)));
        }
    }

    #[test]
    fn patch_round_trips_changed_rectangles() {
        let mut source = DesktopFrame::new(4, 4).unwrap();
        source.write_rect(1, 1, 2, 1, &[9; 8]).unwrap();
        let mut region = DirtyRegion::default();
        region.add(rect(1, 1, 2, 1));
        let patch = source.extract_patch(&region).unwrap();
        let mut target = DesktopFrame::new(4, 4).unwrap();
        target.apply_patch(&patch).unwrap();
        assert!(target.rgba == source.rgba);
        let mut other = DesktopFrame::new(4, 5).unwrap();
        assert!(other.apply_patch(&patch).is_err());
    }

    #[test]
    fn cursor_bitmap_is_bounded_and_clamps_hotspot() {
        assert!(CursorBitmap::new(0, 1, 0, 0, vec![]).is_err());
        assert!(CursorBitmap::new(385, 1, 0, 0, vec![0; 385 * 4]).is_err());
        assert!(CursorBitmap::new(2, 2, 0, 0, vec![0; 15]).is_err());
        let cursor = CursorBitmap::new(2, 2, 9, 9, vec![0; 16]).unwrap();
        assert_eq!((cursor.hotspot_x, cursor.hotspot_y), (1, 1));
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
