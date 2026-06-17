//! Headless renderer that captures the composited REX3 framebuffer into a
//! shared buffer for egui to upload as a texture each frame.
//!
//! This renderer has no GL context.  It runs `SwCompositor::compose_pixels()`
//! directly (CPU-only, no GL upload) and reads from the resulting pixel buffer.

use iris::rex3::Renderer;
use iris::disp::{Rex3Screen, StatusBar, StatusBarTexture, BarStats};
use iris::debug_overlay::DebugOverlay;
use iris::compositor::SwCompositor;
use parking_lot::{Mutex, MutexGuard};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// One captured frame: tightly-packed RGBA bytes plus pixel dimensions.
#[derive(Clone, Default)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    /// Length is `width * height * 4`. Pixel order: R, G, B, A.
    pub rgba: Vec<u8>,
    /// Bumped every time a new frame is captured; egui uses this to skip the
    /// texture upload when nothing has changed.
    pub seq: u64,
}

/// Shared latest-frame slot. The renderer writes; the GUI reads.
///
/// `seq` is mirrored into a lock-free atomic so the GUI can check for new
/// frames without taking the mutex or cloning the multi-MB buffer.
#[derive(Default, Clone)]
pub struct FrameSink {
    frame: Arc<Mutex<Frame>>,
    seq:   Arc<AtomicU64>,
}

impl FrameSink {
    pub fn new() -> Self { Self::default() }

    /// Lock-free latest sequence number (0 = no frame produced yet).
    pub fn seq(&self) -> u64 { self.seq.load(Ordering::Acquire) }

    /// Clone the latest frame. Gate this on `seq()` having changed to avoid
    /// copying the whole buffer on every repaint when nothing is new.
    pub fn snapshot(&self) -> Frame { self.frame.lock().clone() }

    /// Reset to the "no frame yet" state (seq 0, blank frame). Call before a
    /// fresh run starts rendering so a restart shows the "waiting for first
    /// frame" placeholder instead of the previous run's last frame.
    pub fn reset(&self) {
        *self.frame.lock() = Frame::default();
        self.seq.store(0, Ordering::Release);
    }

    fn lock(&self) -> MutexGuard<'_, Frame> { self.frame.lock() }
}

/// Headless `Renderer` that captures the composited frame into a `FrameSink`.
///
/// Runs `SwCompositor::compose_pixels()` (CPU-only, no GL context needed) and
/// packs the result into egui-friendly RGBA bytes.
pub struct CaptureRenderer {
    sink:       FrameSink,
    seq:        u64,
    compositor: SwCompositor,
}

impl CaptureRenderer {
    pub fn new(sink: FrameSink) -> Self {
        Self { sink, seq: 0, compositor: SwCompositor::new() }
    }
}

impl Renderer for CaptureRenderer {
    fn present(
        &mut self,
        screen:        &mut Rex3Screen,
        _overlay:       &mut DebugOverlay,
        _status:        &mut StatusBar,
        _sbtex:         &mut StatusBarTexture,
        _stats:         &BarStats,
        _need_readback: bool,
    ) {
        let width  = screen.width;
        let height = screen.height;
        if width == 0 || height == 0 { return; }

        // Run SW compositor pixel loop (no GL).
        let src = screen.compositor_source();
        self.compositor.compose_pixels(&src);
        drop(src);

        let needed = width * height * 4;
        let mut frame = self.sink.lock();
        if frame.rgba.len() != needed {
            frame.rgba = vec![0u8; needed];
        }
        frame.width  = width;
        frame.height = height;

        // compositor buf is stride-2048, 0xFFBBGGRR.
        // egui wants tightly-packed [R, G, B, A] — same byte order on LE.
        let buf = self.compositor.pixels();
        for y in 0..height {
            let src_row = &buf[y * 2048..y * 2048 + width];
            let dst_row = &mut frame.rgba[y * width * 4..(y + 1) * width * 4];
            for (dst_px, &word) in dst_row.chunks_exact_mut(4).zip(src_row) {
                dst_px.copy_from_slice(&(word | 0xFF00_0000).to_le_bytes());
            }
        }

        self.seq = self.seq.wrapping_add(1);
        frame.seq = self.seq;
        drop(frame);
        self.sink.seq.store(self.seq, Ordering::Release);
    }

    fn resize(&mut self, _width: usize, _height: usize) {}
}
