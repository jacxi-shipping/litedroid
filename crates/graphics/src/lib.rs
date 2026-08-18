use std::fmt;

use litedroid_core::Result;
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Framebuffer
// ---------------------------------------------------------------------------

/// Simple RGBA framebuffer stored in host memory.
pub struct Framebuffer {
    width: u32,
    height: u32,
    pixels: Vec<u8>, // width * height * 4 (RGBA)
}

impl Framebuffer {
    /// Create a new framebuffer with the given dimensions (pixels initialised to 0).
    pub fn new(width: u32, height: u32) -> Self {
        let pixels = vec![0u8; (width as usize) * (height as usize) * 4];
        Self {
            width,
            height,
            pixels,
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Number of bytes per row (always `width * 4` for RGBA).
    pub fn stride(&self) -> u32 {
        self.width * 4
    }

    /// Read-only access to the pixel data.
    pub fn data(&self) -> &[u8] {
        &self.pixels
    }

    /// Mutable access to the pixel data.
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.pixels
    }

    /// Set a single pixel (bounds-checked — silently ignored if out of range).
    pub fn set_pixel(&mut self, x: u32, y: u32, r: u8, g: u8, b: u8, a: u8) {
        if x >= self.width || y >= self.height {
            return;
        }
        let idx = ((y as usize) * (self.width as usize) + (x as usize)) * 4;
        self.pixels[idx] = r;
        self.pixels[idx + 1] = g;
        self.pixels[idx + 2] = b;
        self.pixels[idx + 3] = a;
    }

    /// Get a single pixel (returns (0,0,0,0) if out of bounds).
    pub fn get_pixel(&self, x: u32, y: u32) -> (u8, u8, u8, u8) {
        if x >= self.width || y >= self.height {
            return (0, 0, 0, 0);
        }
        let idx = ((y as usize) * (self.width as usize) + (x as usize)) * 4;
        (
            self.pixels[idx],
            self.pixels[idx + 1],
            self.pixels[idx + 2],
            self.pixels[idx + 3],
        )
    }

    /// Fill the entire framebuffer with a single colour.
    pub fn fill(&mut self, color: [u8; 4]) {
        for chunk in self.pixels.chunks_exact_mut(4) {
            chunk.copy_from_slice(&color);
        }
    }

    /// Copy a rectangle from `src` into the framebuffer at `(x, y)`.
    ///
    /// `src_stride` is the number of bytes per row in `src`.
    /// The copy is clamped to the framebuffer bounds.
    pub fn blit_from(
        &mut self,
        src: &[u8],
        src_stride: u32,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
    ) {
        let dst_stride = self.stride();
        for row in 0..h {
            let sy = y as usize + row as usize;
            if sy >= self.height as usize {
                break;
            }
            let sx_start = x as usize;
            let copy_w = (w as usize).min(self.width as usize - sx_start);
            let copy_w_bytes = copy_w * 4;
            let src_row_start = row as usize * src_stride as usize;
            let src_end = (src_row_start + copy_w_bytes).min(src.len());
            let actual = src_end - src_row_start;
            if actual == 0 {
                continue;
            }
            let dst_start = sy * dst_stride as usize + sx_start * 4;
            let dst_end = (dst_start + actual).min(self.pixels.len());
            let actual = dst_end - dst_start;
            if actual == 0 {
                continue;
            }
            self.pixels[dst_start..dst_start + actual]
                .copy_from_slice(&src[src_row_start..src_row_start + actual]);
        }
    }

    /// Copy a rectangle from the framebuffer into `dst` at `(x, y)`.
    ///
    /// `dst_stride` is the number of bytes per row in `dst`.
    pub fn blit_to(
        &self,
        dst: &mut [u8],
        dst_stride: u32,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
    ) {
        let src_stride = self.stride();
        for row in 0..h {
            let sy = y as usize + row as usize;
            if sy >= self.height as usize {
                break;
            }
            let sx_start = x as usize;
            let copy_w = (w as usize).min(self.width as usize - sx_start);
            let copy_w_bytes = copy_w * 4;
            let src_start = sy * src_stride as usize + sx_start * 4;
            let src_end = (src_start + copy_w_bytes).min(self.pixels.len());
            let actual = src_end - src_start;
            if actual == 0 {
                continue;
            }
            let dst_start = row as usize * dst_stride as usize;
            let dst_end = (dst_start + actual).min(dst.len());
            let actual = dst_end - dst_start;
            if actual == 0 {
                continue;
            }
            dst[dst_start..dst_start + actual]
                .copy_from_slice(&self.pixels[src_start..src_start + actual]);
        }
    }
}

impl fmt::Debug for Framebuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Framebuffer")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("stride", &self.stride())
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// DisplayEvent
// ---------------------------------------------------------------------------

/// Events emitted by a display backend.
#[derive(Debug, Clone)]
pub enum DisplayEvent {
    /// The window needs to be repainted.
    Expose,
    /// The window was closed by the user.
    Close,
    /// The window was resized.
    Resize { w: u32, h: u32 },
    /// No event is available.
    Empty,
}

// ---------------------------------------------------------------------------
// DisplayBackend
// ---------------------------------------------------------------------------

/// Trait for display backends that present a [`Framebuffer`] to the user.
pub trait DisplayBackend {
    /// Present the latest framebuffer contents.
    fn update(&mut self, fb: &Framebuffer) -> Result<()>;

    /// Set the window title.
    fn set_title(&self, title: &str);

    /// Close the display window.
    fn close(&mut self);

    /// Poll for display events (non-blocking).
    fn process_events(&mut self) -> Vec<DisplayEvent>;
}

// ---------------------------------------------------------------------------
// NullDisplay
// ---------------------------------------------------------------------------

/// Headless display backend that discards all output. Useful for CI and
/// testing environments where no windowing system is available.
pub struct NullDisplay {
    updated_once: bool,
}

impl NullDisplay {
    pub fn new() -> Self {
        Self {
            updated_once: false,
        }
    }
}

impl Default for NullDisplay {
    fn default() -> Self {
        Self::new()
    }
}

impl DisplayBackend for NullDisplay {
    fn update(&mut self, _fb: &Framebuffer) -> Result<()> {
        if !self.updated_once {
            info!("NullDisplay: first update (headless mode)");
            self.updated_once = true;
        }
        Ok(())
    }

    fn set_title(&self, _title: &str) {
        // No-op in headless mode.
    }

    fn close(&mut self) {
        warn!("NullDisplay: close called (no-op in headless mode)");
    }

    fn process_events(&mut self) -> Vec<DisplayEvent> {
        Vec::new()
    }
}
