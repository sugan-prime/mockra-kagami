//! nvhost-as-gpu: Tegra X1 Address Space management device (`/dev/nvhost-as-gpu`).
//!
//! Maps nvmap memory allocations into the GPU virtual address space (GVA).

use std::collections::HashMap;
use thiserror::Error;

pub const NVGPU_AS_IOCTL_BIND_CHANNEL: u32 = 0x40044101;
pub const NVGPU_AS_IOCTL_MAP_BUFFER_EX: u32 = 0xC0284106;
pub const NVGPU_AS_IOCTL_UNMAP_BUFFER: u32 = 0x40084105;

#[derive(Error, Debug, PartialEq, Eq)]
pub enum NvHostAsGpuError {
    #[error("Buffer too small for IOCTL: expected at least {expected} bytes, got {actual}")]
    BufferTooSmall { expected: usize, actual: usize },
    #[error("Unknown or unhandled IOCTL command: 0x{0:08X}")]
    UnknownIoctl(u32),
}

#[derive(Debug, Clone)]
pub struct GpuMapping {
    pub nvmap_handle: u32,
    pub gpu_address: u64,
    pub size: u64,
}

/// The `/dev/nvhost-as-gpu` device emulation.
#[derive(Debug, Default)]
pub struct NvHostAsGpuDevice {
    next_gpu_address: u64,
    mappings: HashMap<u64, GpuMapping>,
}

impl NvHostAsGpuDevice {
    pub fn new() -> Self {
        Self {
            next_gpu_address: 0x1_0000_0000, // 4 GB base for GPU Virtual Address space
            mappings: HashMap::new(),
        }
    }

    /// Dispatches an IOCTL command to the nvhost-as-gpu driver.
    pub fn ioctl(&mut self, cmd: u32, in_buf: &[u8], out_buf: &mut [u8]) -> Result<u32, NvHostAsGpuError> {
        match cmd {
            NVGPU_AS_IOCTL_BIND_CHANNEL => {
                Ok(0) // Bind accepted
            }

            // NVGPU_AS_IOCTL_MAP_BUFFER_EX:
            // Input struct: flags(4), kind(4), nvmap_handle(4), page_size(4), buffer_offset(8), mapping_size(8), offset(8)
            // Output: offset(8) written at offset 24..32
            NVGPU_AS_IOCTL_MAP_BUFFER_EX => {
                if in_buf.len() < 32 || out_buf.len() < 32 {
                    return Err(NvHostAsGpuError::BufferTooSmall {
                        expected: 32,
                        actual: in_buf.len(),
                    });
                }

                let nvmap_handle = u32::from_le_bytes(in_buf[8..12].try_into().unwrap());
                let mapping_size = u64::from_le_bytes(in_buf[20..28].try_into().unwrap());
                let mut requested_offset = u64::from_le_bytes(in_buf[28..32].try_into().unwrap());

                if requested_offset == 0 {
                    requested_offset = self.next_gpu_address;
                    self.next_gpu_address = self.next_gpu_address.wrapping_add(mapping_size).max(0x1000);
                }

                self.mappings.insert(
                    requested_offset,
                    GpuMapping {
                        nvmap_handle,
                        gpu_address: requested_offset,
                        size: mapping_size,
                    },
                );

                out_buf[..in_buf.len()].copy_from_slice(in_buf);
                out_buf[28..36].copy_from_slice(&requested_offset.to_le_bytes());

                Ok(0)
            }

            NVGPU_AS_IOCTL_UNMAP_BUFFER => {
                if in_buf.len() >= 8 {
                    let offset = u64::from_le_bytes(in_buf[0..8].try_into().unwrap());
                    self.mappings.remove(&offset);
                }
                Ok(0)
            }

            _ => Err(NvHostAsGpuError::UnknownIoctl(cmd)),
        }
    }
}
