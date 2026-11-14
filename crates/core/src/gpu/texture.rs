//! Tegra X1 / Maxwell Block-Linear Texture Swizzling & Deswizzling Engine.
//!
//! Tegra GPUs arrange render targets and sampled textures into Block-Linear (GOB)
//! tiles to optimize spatial 2D/3D cache locality during rasterization and texture sampling.
//!
//! A GOB (Graphics Operation Block) is 64 bytes wide by 8 lines high (512 bytes total),
//! laid out with a specialized Morton-order bit interleaving pattern:
//! - X coordinate bits: [bit 0, 1, 2, 3, 5, 8] (SWIZZLE_X_MASK: 0b100101111)
//! - Y coordinate bits: [bit 4, 6, 7]          (SWIZZLE_Y_MASK: 0b011010000)

use common::pdep_u32;

pub const GOB_WIDTH_BYTES: usize = 64;
pub const GOB_HEIGHT_LINES: usize = 8;
pub const GOB_SIZE_BYTES: usize = GOB_WIDTH_BYTES * GOB_HEIGHT_LINES; // 512 bytes

pub const SWIZZLE_X_MASK: u32 = 0b1_0010_1111; // 0x12F
pub const SWIZZLE_Y_MASK: u32 = 0b0_1101_0000; // 0x0D0

/// Supported texture color and compressed block formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureFormat {
    R8Unorm,
    Rgb565,
    Rgba8Unorm,
    Rgba16Float,
    Rgba32Float,
    Bc1,
    Bc2,
    Bc3,
    Bc4,
    Bc5,
    Bc7,
}

impl TextureFormat {
    /// Returns the bytes per texel for uncompressed formats,
    /// or bytes per 4x4 block for compressed formats.
    pub fn bytes_per_element(self) -> usize {
        match self {
            Self::R8Unorm => 1,
            Self::Rgb565 => 2,
            Self::Rgba8Unorm => 4,
            Self::Rgba16Float => 8,
            Self::Rgba32Float => 16,
            Self::Bc1 | Self::Bc4 => 8,
            Self::Bc2 | Self::Bc3 | Self::Bc5 | Self::Bc7 => 16,
        }
    }

    /// Whether this is a block-compressed (BC) format.
    pub fn is_compressed(self) -> bool {
        matches!(
            self,
            Self::Bc1 | Self::Bc2 | Self::Bc3 | Self::Bc4 | Self::Bc5 | Self::Bc7
        )
    }

    /// Returns the block dimensions in texels: (width, height).
    pub fn block_dimensions(self) -> (usize, usize) {
        if self.is_compressed() {
            (4, 4)
        } else {
            (1, 1)
        }
    }
}

/// Calculates the intra-GOB byte offset (0..512) for a given (x_in_bytes, y_line) inside a GOB.
#[inline(always)]
pub fn calculate_gob_offset(x_byte: usize, y_line: usize) -> usize {
    debug_assert!(x_byte < GOB_WIDTH_BYTES);
    debug_assert!(y_line < GOB_HEIGHT_LINES);
    let swizzled_x = pdep_u32(x_byte as u32, SWIZZLE_X_MASK);
    let swizzled_y = pdep_u32(y_line as u32, SWIZZLE_Y_MASK);
    (swizzled_x | swizzled_y) as usize
}

/// Computes the total required buffer size in bytes for a block-linear surface.
pub fn calculate_block_linear_size(
    width: usize,
    height: usize,
    bpp: usize,
    block_height_in_gobs: usize,
) -> usize {
    let block_height = block_height_in_gobs.max(1);
    let pitch_bytes = width.wrapping_mul(bpp);
    let stride_aligned = (pitch_bytes + GOB_WIDTH_BYTES - 1) & !(GOB_WIDTH_BYTES - 1);
    let gobs_in_x = stride_aligned / GOB_WIDTH_BYTES;

    let block_height_lines = block_height * GOB_HEIGHT_LINES;
    let blocks_in_y = (height + block_height_lines - 1) / block_height_lines;

    gobs_in_x * blocks_in_y * block_height * GOB_SIZE_BYTES
}

/// Swizzles a linear pitch buffer into Tegra block-linear layout.
pub fn swizzle_pitch_to_block_linear(
    src: &[u8],
    width: usize,
    height: usize,
    bpp: usize,
    block_height_in_gobs: usize,
) -> Vec<u8> {
    let block_height = block_height_in_gobs.max(1);
    let total_size = calculate_block_linear_size(width, height, bpp, block_height);
    let mut dst = vec![0u8; total_size];

    let pitch_bytes = width * bpp;
    let stride_aligned = (pitch_bytes + GOB_WIDTH_BYTES - 1) & !(GOB_WIDTH_BYTES - 1);
    let gobs_in_x = stride_aligned / GOB_WIDTH_BYTES;
    let block_size = gobs_in_x * block_height * GOB_SIZE_BYTES;

    for y in 0..height {
        let block_y = y / (block_height * GOB_HEIGHT_LINES);
        let gob_y = (y % (block_height * GOB_HEIGHT_LINES)) / GOB_HEIGHT_LINES;
        let line_in_gob = y % GOB_HEIGHT_LINES;
        let swizzled_y = pdep_u32(line_in_gob as u32, SWIZZLE_Y_MASK) as usize;

        for x in 0..width {
            let x_byte = x * bpp;
            let gob_x = x_byte / GOB_WIDTH_BYTES;
            let byte_in_gob = x_byte % GOB_WIDTH_BYTES;

            let block_offset = block_y * block_size;
            let gob_offset = (gob_x * block_height + gob_y) * GOB_SIZE_BYTES;
            let intra_gob_offset = (pdep_u32(byte_in_gob as u32, SWIZZLE_X_MASK) as usize) | swizzled_y;

            let dst_offset = block_offset + gob_offset + intra_gob_offset;
            let src_offset = y * pitch_bytes + x_byte;

            if src_offset + bpp <= src.len() && dst_offset + bpp <= dst.len() {
                dst[dst_offset..dst_offset + bpp].copy_from_slice(&src[src_offset..src_offset + bpp]);
            }
        }
    }

    dst
}

/// Deswizzles a Tegra block-linear surface into standard CPU pitch-linear scanlines.
pub fn deswizzle_block_linear_to_pitch(
    src: &[u8],
    width: usize,
    height: usize,
    bpp: usize,
    block_height_in_gobs: usize,
) -> Vec<u8> {
    let block_height = block_height_in_gobs.max(1);
    let pitch_bytes = width * bpp;
    let mut dst = vec![0u8; pitch_bytes * height];

    let stride_aligned = (pitch_bytes + GOB_WIDTH_BYTES - 1) & !(GOB_WIDTH_BYTES - 1);
    let gobs_in_x = stride_aligned / GOB_WIDTH_BYTES;
    let block_size = gobs_in_x * block_height * GOB_SIZE_BYTES;

    for y in 0..height {
        let block_y = y / (block_height * GOB_HEIGHT_LINES);
        let gob_y = (y % (block_height * GOB_HEIGHT_LINES)) / GOB_HEIGHT_LINES;
        let line_in_gob = y % GOB_HEIGHT_LINES;
        let swizzled_y = pdep_u32(line_in_gob as u32, SWIZZLE_Y_MASK) as usize;

        for x in 0..width {
            let x_byte = x * bpp;
            let gob_x = x_byte / GOB_WIDTH_BYTES;
            let byte_in_gob = x_byte % GOB_WIDTH_BYTES;

            let block_offset = block_y * block_size;
            let gob_offset = (gob_x * block_height + gob_y) * GOB_SIZE_BYTES;
            let intra_gob_offset = (pdep_u32(byte_in_gob as u32, SWIZZLE_X_MASK) as usize) | swizzled_y;

            let src_offset = block_offset + gob_offset + intra_gob_offset;
            let dst_offset = y * pitch_bytes + x_byte;

            if src_offset + bpp <= src.len() && dst_offset + bpp <= dst.len() {
                dst[dst_offset..dst_offset + bpp].copy_from_slice(&src[src_offset..src_offset + bpp]);
            }
        }
    }

    dst
}

/// Swizzles a texture surface according to its format, taking into account block compression.
pub fn swizzle_surface(
    src: &[u8],
    pixel_width: usize,
    pixel_height: usize,
    format: TextureFormat,
    block_height_in_gobs: usize,
) -> Vec<u8> {
    let (block_w, block_h) = format.block_dimensions();
    let elem_width = (pixel_width + block_w - 1) / block_w;
    let elem_height = (pixel_height + block_h - 1) / block_h;
    let elem_bpp = format.bytes_per_element();

    swizzle_pitch_to_block_linear(src, elem_width, elem_height, elem_bpp, block_height_in_gobs)
}

/// Deswizzles a texture surface according to its format, taking into account block compression.
pub fn deswizzle_surface(
    src: &[u8],
    pixel_width: usize,
    pixel_height: usize,
    format: TextureFormat,
    block_height_in_gobs: usize,
) -> Vec<u8> {
    let (block_w, block_h) = format.block_dimensions();
    let elem_width = (pixel_width + block_w - 1) / block_w;
    let elem_height = (pixel_height + block_h - 1) / block_h;
    let elem_bpp = format.bytes_per_element();

    deswizzle_block_linear_to_pitch(src, elem_width, elem_height, elem_bpp, block_height_in_gobs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gob_intra_offset_bijection() {
        // Every (x_byte, y_line) pair in 0..64 x 0..8 must map to a unique offset in 0..512.
        let mut seen = [false; GOB_SIZE_BYTES];
        for y in 0..GOB_HEIGHT_LINES {
            for x in 0..GOB_WIDTH_BYTES {
                let offset = calculate_gob_offset(x, y);
                assert!(offset < GOB_SIZE_BYTES, "Offset out of bounds: {}", offset);
                assert!(!seen[offset], "Collision at offset: {} for ({}, {})", offset, x, y);
                seen[offset] = true;
            }
        }
        assert!(seen.iter().all(|&s| s), "Not all GOB offsets were covered");
    }

    #[test]
    fn test_swizzle_deswizzle_roundtrip_rgba8() {
        let width = 64;
        let height = 32;
        let bpp = 4; // RGBA8
        let block_height = 2; // 2 GOBs = 16 lines per block

        // Generate distinctive pattern
        let mut linear = vec![0u8; width * height * bpp];
        for y in 0..height {
            for x in 0..width {
                let idx = (y * width + x) * bpp;
                linear[idx + 0] = (x & 0xFF) as u8;
                linear[idx + 1] = (y & 0xFF) as u8;
                linear[idx + 2] = ((x + y) & 0xFF) as u8;
                linear[idx + 3] = 255;
            }
        }

        let swizzled = swizzle_pitch_to_block_linear(&linear, width, height, bpp, block_height);
        assert_eq!(swizzled.len(), calculate_block_linear_size(width, height, bpp, block_height));

        let deswizzled = deswizzle_block_linear_to_pitch(&swizzled, width, height, bpp, block_height);
        assert_eq!(linear, deswizzled, "Roundtrip swizzle -> deswizzle did not preserve data!");
    }

    #[test]
    fn test_swizzle_deswizzle_compressed_bc3() {
        // 128x128 pixels with BC3 compression (4x4 blocks -> 32x32 blocks, 16 bytes each)
        let width = 128;
        let height = 128;
        let format = TextureFormat::Bc3;
        let block_height = 4;

        let num_blocks = (width / 4) * (height / 4);
        let linear_bytes = num_blocks * format.bytes_per_element();
        let mut linear = vec![0u8; linear_bytes];
        for i in 0..linear.len() {
            linear[i] = (i * 17 % 251) as u8;
        }

        let swizzled = swizzle_surface(&linear, width, height, format, block_height);
        let deswizzled = deswizzle_surface(&swizzled, width, height, format, block_height);

        assert_eq!(linear, deswizzled);
    }
}
