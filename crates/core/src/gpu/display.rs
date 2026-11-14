//! Framebuffer and Display Presentation Engine.
//!
//! Models the Horizon OS VI (Visual Interface) presentation surface,
//! converting guest GPU render targets into 720p / 1080p displayable frames.

use crate::gpu::texture::{calculate_block_linear_size, deswizzle_block_linear_to_pitch};
use crate::memory::bus::{MemoryBus, MemoryError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorFormat {
    Rgba8Unorm,
    Bgra8Unorm,
    Rgb565,
}

impl ColorFormat {
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            ColorFormat::Rgba8Unorm | ColorFormat::Bgra8Unorm => 4,
            ColorFormat::Rgb565 => 2,
        }
    }
}

/// Represents an active presentation surface.
#[derive(Debug, Clone)]
pub struct DisplaySurface {
    pub width: u32,
    pub height: u32,
    pub format: ColorFormat,
    pub framebuffer_address: u64,
    pub is_block_linear: bool,
    pub block_height_gobs: usize,
}

impl Default for DisplaySurface {
    fn default() -> Self {
        Self::new(1280, 720, ColorFormat::Rgba8Unorm, 0)
    }
}

impl DisplaySurface {
    pub fn new(width: u32, height: u32, format: ColorFormat, framebuffer_address: u64) -> Self {
        Self {
            width,
            height,
            format,
            framebuffer_address,
            is_block_linear: false,
            block_height_gobs: 16,
        }
    }

    /// Sets the surface memory layout to Tegra block-linear.
    pub fn with_block_linear(mut self, block_height_gobs: usize) -> Self {
        self.is_block_linear = true;
        self.block_height_gobs = block_height_gobs.max(1);
        self
    }

    /// Creates a 720p (1280x720) handheld surface.
    pub fn handheld_720p(framebuffer_address: u64) -> Self {
        Self::new(1280, 720, ColorFormat::Rgba8Unorm, framebuffer_address)
    }

    /// Creates a 1080p (1920x1080) docked surface.
    pub fn docked_1080p(framebuffer_address: u64) -> Self {
        Self::new(1920, 1080, ColorFormat::Rgba8Unorm, framebuffer_address)
    }

    pub fn total_bytes(&self) -> usize {
        (self.width as usize) * (self.height as usize) * self.format.bytes_per_pixel()
    }

    /// Reads raw pixel buffer from the guest memory bus.
    pub fn extract_pixels(&self, bus: &impl MemoryBus) -> Result<Vec<u8>, MemoryError> {
        let size = self.total_bytes();
        let mut pixels = Vec::with_capacity(size);

        for i in 0..size {
            let byte = bus.read_u8(self.framebuffer_address.wrapping_add(i as u64))?;
            pixels.push(byte);
        }

        Ok(pixels)
    }

    /// Reads pixel buffer and automatically deswizzles if the surface is stored in block-linear format.
    pub fn extract_presentation_frame(&self, bus: &impl MemoryBus) -> Result<Vec<u8>, MemoryError> {
        if !self.is_block_linear {
            return self.extract_pixels(bus);
        }

        let w = self.width as usize;
        let h = self.height as usize;
        let bpp = self.format.bytes_per_pixel();
        let tiled_size = calculate_block_linear_size(w, h, bpp, self.block_height_gobs);

        let mut tiled_bytes = Vec::with_capacity(tiled_size);
        for i in 0..tiled_size {
            let b = bus.read_u8(self.framebuffer_address.wrapping_add(i as u64))?;
            tiled_bytes.push(b);
        }

        Ok(deswizzle_block_linear_to_pitch(&tiled_bytes, w, h, bpp, self.block_height_gobs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::bus::FlatMemory;

    #[test]
    fn test_display_surface_extract_pixels() {
        let width = 4;
        let height = 2;
        let surface = DisplaySurface::new(width, height, ColorFormat::Rgba8Unorm, 0x1000);
        assert_eq!(surface.total_bytes(), 32);

        let mut mem = FlatMemory::new(0, 0x10000);
        // Fill first pixel with red: RGBA = (255, 0, 0, 255)
        mem.write_u8(0x1000, 255).unwrap();
        mem.write_u8(0x1001, 0).unwrap();
        mem.write_u8(0x1002, 0).unwrap();
        mem.write_u8(0x1003, 255).unwrap();

        let pixels = surface.extract_pixels(&mem).unwrap();
        assert_eq!(pixels.len(), 32);
        assert_eq!(pixels[0..4], [255, 0, 0, 255]);
    }

    #[test]
    fn test_display_surface_block_linear_presentation() {
        use crate::gpu::texture::swizzle_pitch_to_block_linear;

        let width = 64;
        let height = 16;
        let bpp = 4;
        let block_height = 2;

        let mut linear = vec![0u8; width * height * bpp];
        linear[0] = 0xAA;
        linear[1] = 0xBB;
        linear[2] = 0xCC;
        linear[3] = 0xFF;

        let swizzled = swizzle_pitch_to_block_linear(&linear, width, height, bpp, block_height);

        let fb_addr = 0x2000;
        let mut mem = FlatMemory::new(0, 0x40000);
        for (i, &b) in swizzled.iter().enumerate() {
            mem.write_u8(fb_addr + i as u64, b).unwrap();
        }

        let surface = DisplaySurface::new(width as u32, height as u32, ColorFormat::Rgba8Unorm, fb_addr)
            .with_block_linear(block_height);

        let presented = surface.extract_presentation_frame(&mem).unwrap();
        assert_eq!(presented.len(), linear.len());
        assert_eq!(presented, linear);
    }
}
