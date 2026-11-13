//! Maxwell 3D Graphics Engine (GM20B) State Machine.
//!
//! Emulates the Tegra X1 3D graphics pipeline registers, render target binding,
//! surface clearing, and draw call execution.

use crate::memory::bus::{MemoryBus, MemoryError};

/// Maxwell 3D Method Register offsets (standardized on 32-bit word indices).
pub mod methods {
    pub const RT_ADDRESS_HIGH: u32 = 0x0080; // 0x0200 / 4
    pub const RT_ADDRESS_LOW: u32 = 0x0081;  // 0x0204 / 4
    pub const RT_WIDTH: u32 = 0x0082;        // 0x0208 / 4
    pub const RT_HEIGHT: u32 = 0x0083;       // 0x020C / 4
    pub const RT_FORMAT: u32 = 0x0084;       // 0x0210 / 4

    pub const CLEAR_COLOR_R: u32 = 0x00D8;   // 0x0360 / 4
    pub const CLEAR_COLOR_G: u32 = 0x00D9;   // 0x0364 / 4
    pub const CLEAR_COLOR_B: u32 = 0x00DA;   // 0x0368 / 4
    pub const CLEAR_COLOR_A: u32 = 0x00DB;   // 0x036C / 4
    pub const CLEAR_SURFACE: u32 = 0x00DD;   // 0x0374 / 4

    pub const DRAW_ARRAYS: u32 = 0x0238;     // 0x08E0 / 4
}

/// Maxwell 3D graphics processor state machine.
pub struct Maxwell3D {
    pub registers: Vec<u32>,
    pub render_target_address: u64,
    pub render_target_width: u32,
    pub render_target_height: u32,
    pub clear_color: [f32; 4],
    pub draw_call_count: u64,
}

impl Default for Maxwell3D {
    fn default() -> Self {
        Self::new()
    }
}

impl Maxwell3D {
    pub fn new() -> Self {
        Self {
            registers: vec![0; 0x1000],
            render_target_address: 0,
            render_target_width: 1280,
            render_target_height: 720,
            clear_color: [0.0, 0.0, 0.0, 1.0],
            draw_call_count: 0,
        }
    }

    /// Invokes a Maxwell 3D method with an associated argument.
    pub fn call_method(
        &mut self,
        method: u32,
        argument: u32,
        bus: &mut impl MemoryBus,
    ) -> Result<(), MemoryError> {
        let norm_method = if method >= 0x0200 && method % 4 == 0 {
            method / 4
        } else {
            method
        };

        let index = norm_method as usize;
        if index < self.registers.len() {
            self.registers[index] = argument;
        }

        match norm_method {
            methods::RT_ADDRESS_HIGH => {
                let high = (argument as u64) << 32;
                self.render_target_address = high | (self.render_target_address & 0xFFFF_FFFF);
            }
            methods::RT_ADDRESS_LOW => {
                let low = argument as u64;
                self.render_target_address = (self.render_target_address & 0xFFFF_FFFF_0000_0000) | low;
            }
            methods::RT_WIDTH => {
                self.render_target_width = argument;
            }
            methods::RT_HEIGHT => {
                self.render_target_height = argument;
            }
            methods::CLEAR_COLOR_R => {
                self.clear_color[0] = f32::from_bits(argument);
            }
            methods::CLEAR_COLOR_G => {
                self.clear_color[1] = f32::from_bits(argument);
            }
            methods::CLEAR_COLOR_B => {
                self.clear_color[2] = f32::from_bits(argument);
            }
            methods::CLEAR_COLOR_A => {
                self.clear_color[3] = f32::from_bits(argument);
            }
            methods::CLEAR_SURFACE => {
                self.execute_clear_surface(bus)?;
            }
            methods::DRAW_ARRAYS => {
                self.draw_call_count += 1;
            }
            _ => {}
        }

        Ok(())
    }

    /// Fills the current render target memory with the current RGBA clear color.
    fn execute_clear_surface(&self, bus: &mut impl MemoryBus) -> Result<(), MemoryError> {
        if self.render_target_address == 0 {
            return Ok(());
        }

        let r = (self.clear_color[0].clamp(0.0, 1.0) * 255.0) as u8;
        let g = (self.clear_color[1].clamp(0.0, 1.0) * 255.0) as u8;
        let b = (self.clear_color[2].clamp(0.0, 1.0) * 255.0) as u8;
        let a = (self.clear_color[3].clamp(0.0, 1.0) * 255.0) as u8;

        let total_pixels = (self.render_target_width as usize) * (self.render_target_height as usize);
        let pixel_bytes = [r, g, b, a];

        for i in 0..total_pixels {
            let offset = self.render_target_address.wrapping_add((i * 4) as u64);
            bus.write_u8(offset, pixel_bytes[0])?;
            bus.write_u8(offset + 1, pixel_bytes[1])?;
            bus.write_u8(offset + 2, pixel_bytes[2])?;
            bus.write_u8(offset + 3, pixel_bytes[3])?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::bus::FlatMemory;

    #[test]
    fn test_maxwell3d_clear_surface() {
        let mut maxwell = Maxwell3D::new();
        let mut bus = FlatMemory::new(0, 0x10000);

        let fb_addr = 0x2000;
        let width = 2;
        let height = 2;

        // Set Render Target address: 0x2000
        maxwell.call_method(methods::RT_ADDRESS_HIGH, 0, &mut bus).unwrap();
        maxwell.call_method(methods::RT_ADDRESS_LOW, fb_addr as u32, &mut bus).unwrap();
        maxwell.call_method(methods::RT_WIDTH, width, &mut bus).unwrap();
        maxwell.call_method(methods::RT_HEIGHT, height, &mut bus).unwrap();

        // Set Clear Color to Blue: (0.0, 0.0, 1.0, 1.0)
        maxwell.call_method(methods::CLEAR_COLOR_R, 0.0f32.to_bits(), &mut bus).unwrap();
        maxwell.call_method(methods::CLEAR_COLOR_G, 0.0f32.to_bits(), &mut bus).unwrap();
        maxwell.call_method(methods::CLEAR_COLOR_B, 1.0f32.to_bits(), &mut bus).unwrap();
        maxwell.call_method(methods::CLEAR_COLOR_A, 1.0f32.to_bits(), &mut bus).unwrap();

        // Trigger surface clear
        maxwell.call_method(methods::CLEAR_SURFACE, 1, &mut bus).unwrap();

        // Verify pixel 0 in memory is RGBA = (0, 0, 255, 255)
        assert_eq!(bus.read_u8(fb_addr).unwrap(), 0);
        assert_eq!(bus.read_u8(fb_addr + 1).unwrap(), 0);
        assert_eq!(bus.read_u8(fb_addr + 2).unwrap(), 255);
        assert_eq!(bus.read_u8(fb_addr + 3).unwrap(), 255);
    }
}
