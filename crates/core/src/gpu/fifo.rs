//! Maxwell GPFIFO Command Processor.
//!
//! Decodes Tegra X1 pushbuffer command streams and dispatches methods
//! to the Maxwell 3D graphics pipeline.

use crate::gpu::maxwell3d::Maxwell3D;
use crate::memory::bus::{MemoryBus, MemoryError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketMode {
    IncMethod = 0,
    NonIncMethod = 1,
    OneIncMethod = 2,
}

impl PacketMode {
    pub fn from_u32(val: u32) -> Self {
        match val {
            1 => PacketMode::NonIncMethod,
            2 => PacketMode::OneIncMethod,
            _ => PacketMode::IncMethod,
        }
    }
}

/// Decodes and executes GPFIFO pushbuffer command streams.
#[derive(Debug, Default)]
pub struct GpfifoProcessor {
    pub processed_packets: u64,
}

impl GpfifoProcessor {
    pub fn new() -> Self {
        Self {
            processed_packets: 0,
        }
    }

    /// Processes a slice of 32-bit pushbuffer command words.
    pub fn process_pushbuffer(
        &mut self,
        maxwell: &mut Maxwell3D,
        words: &[u32],
        bus: &mut impl MemoryBus,
    ) -> Result<usize, MemoryError> {
        let mut idx = 0;

        while idx < words.len() {
            let header = words[idx];
            idx += 1;

            let mode_val = (header >> 29) & 0b111;
            let count = ((header >> 16) & 0x1FFF) as usize;
            let mut method = header & 0xFFFF;
            let mode = PacketMode::from_u32(mode_val);

            for i in 0..count {
                if idx >= words.len() {
                    break;
                }
                let arg = words[idx];
                idx += 1;

                maxwell.call_method(method, arg, bus)?;

                match mode {
                    PacketMode::IncMethod => method += 1,
                    PacketMode::NonIncMethod => {}
                    PacketMode::OneIncMethod => {
                        if i == 0 {
                            method += 1;
                        }
                    }
                }
            }

            self.processed_packets += 1;
        }

        Ok(idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::maxwell3d::methods;
    use crate::memory::bus::FlatMemory;

    #[test]
    fn test_gpfifo_inc_method_execution() {
        let mut fifo = GpfifoProcessor::new();
        let mut maxwell = Maxwell3D::new();
        let mut bus = FlatMemory::new(0, 0x10000);

        // Packet: IncMethod (0b000 << 29), count = 4, starting at CLEAR_COLOR_R (0x0360)
        // Header = (0 << 29) | (4 << 16) | 0x0360
        let header = (4 << 16) | methods::CLEAR_COLOR_R;
        let words = vec![
            header,
            1.0f32.to_bits(), // Red
            0.5f32.to_bits(), // Green
            0.0f32.to_bits(), // Blue
            1.0f32.to_bits(), // Alpha
        ];

        fifo.process_pushbuffer(&mut maxwell, &words, &mut bus).unwrap();

        assert_eq!(fifo.processed_packets, 1);
        assert_eq!(maxwell.clear_color[0], 1.0);
        assert_eq!(maxwell.clear_color[1], 0.5);
        assert_eq!(maxwell.clear_color[2], 0.0);
        assert_eq!(maxwell.clear_color[3], 1.0);
    }
}
