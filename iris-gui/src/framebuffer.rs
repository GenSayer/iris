//! Headless renderer that captures REX3's framebuffer into a shared buffer
//! for egui to upload as a texture each frame.

use iris::rex3::Renderer;
use parking_lot::Mutex;
use std::sync::Arc;

/// One captured frame: tightly-packed RGBA bytes plus pixel dimensions.
#[derive(Clone, Default)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    /// Length is width * height * 4. Pixel order: R, G, B, A.
    pub rgba: Vec<u8>,
    /// Bumped every time `Renderer::render` lands a new buffer; egui uses
    /// this to skip the texture upload when nothing has changed.
    pub seq: u64,
}

/// Shared latest-frame slot. The renderer writes; the GUI reads.
#[derive(Default, Clone)]
pub struct FrameSink(pub Arc<Mutex<Frame>>);

impl FrameSink {
    pub fn new() -> Self { Self::default() }
    pub fn snapshot(&self) -> Frame { self.0.lock().clone() }
}

/// `iris::rex3::Renderer` implementation that copies each `render` call
/// into a `FrameSink`. The REX3 refresh thread invokes us at video rate;
/// we keep the work minimal (one stride-aware copy) so we don't stall it.
pub struct CaptureRenderer {
    sink: FrameSink,
    seq:  u64,
}

impl CaptureRenderer {
    pub fn new(sink: FrameSink) -> Self {
        Self { sink, seq: 0 }
    }
}

impl Renderer for CaptureRenderer {
    fn render(&mut self, buffer: &[u32], width: usize, height: usize) {
        // The REX3 framebuffer has row stride 2048 in u32 words regardless
        // of `width` — typical mode is 1280×1024 displayable.
        const STRIDE: usize = 2048;
        if width == 0 || height == 0 { return; }
        let needed = width.checked_mul(height).and_then(|n| n.checked_mul(4)).unwrap_or(0);
        if needed == 0 { return; }

        let mut frame = self.sink.0.lock();
        if frame.rgba.len() != needed {
            frame.rgba = vec![0u8; needed];
        }
        frame.width = width;
        frame.height = height;

        // REX3 pixel layout is RGBA in u32 little-endian: R is the low
        // byte, matching what glow uploads as `glow::RGBA` /
        // `UNSIGNED_BYTE`. egui's `ColorImage::from_rgba_unmultiplied`
        // expects the same byte order, so the copy is a straight
        // reinterpret per row.
        for y in 0..height {
            let src_row_start = y * STRIDE;
            let src_row_end   = src_row_start + width;
            if src_row_end > buffer.len() { break; }
            let src_row = &buffer[src_row_start..src_row_end];
            let dst_row_start = y * width * 4;
            let dst_row_end   = dst_row_start + width * 4;
            let dst_row       = &mut frame.rgba[dst_row_start..dst_row_end];

            // Safety: u32 → 4×u8 reinterpret. We rely on little-endian
            // host byte order to put `R, G, B, A` at consecutive
            // addresses — iris already assumes this in `ui.rs`'s glow
            // upload path (line 226–227), so we inherit that assumption.
            let src_bytes = unsafe {
                std::slice::from_raw_parts(src_row.as_ptr() as *const u8, src_row.len() * 4)
            };
            dst_row.copy_from_slice(src_bytes);
        }

        self.seq = self.seq.wrapping_add(1);
        frame.seq = self.seq;
    }

    fn resize(&mut self, _width: usize, _height: usize) {
        // No-op: we resize the destination buffer in `render` based on the
        // actual `width × height` of the next frame.
    }
}
