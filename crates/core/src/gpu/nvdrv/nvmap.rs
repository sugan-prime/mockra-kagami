//! nvmap: Tegra X1 memory allocator device (`/dev/nvmap`).
//!
//! Manages GPU virtual memory allocations, buffer sharing, and VRAM mapping
//! according to the Tegra NVDRV IOCTL protocol.

use std::collections::HashMap;
use thiserror::Error;

pub const NVMAP_IOC_CREATE: u32 = 0xC0080101;
pub const NVMAP_IOC_ALLOC: u32 = 0xC0180104;
pub const NVMAP_IOC_FREE: u32 = 0xC0080105;
pub const NVMAP_IOC_GET_ID: u32 = 0xC008010E;

#[derive(Error, Debug, PartialEq, Eq)]
pub enum NvMapError {
    #[error("Buffer too small for IOCTL: expected at least {expected} bytes, got {actual}")]
    BufferTooSmall { expected: usize, actual: usize },
    #[error("Invalid nvmap handle: 0x{0:08X}")]
    InvalidHandle(u32),
    #[error("Unknown or unhandled IOCTL command: 0x{0:08X}")]
    UnknownIoctl(u32),
}

/// An allocated nvmap memory object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NvMapHandle {
    pub id: u32,
    pub size: u32,
    pub address: u64,
    pub allocated: bool,
}

/// The `/dev/nvmap` device emulation.
#[derive(Debug, Default)]
pub struct NvMapDevice {
    next_handle: u32,
    handles: HashMap<u32, NvMapHandle>,
}

impl NvMapDevice {
    pub fn new() -> Self {
        Self {
            next_handle: 1,
            handles: HashMap::new(),
        }
    }

    /// Dispatches an IOCTL command to the nvmap driver.
    pub fn ioctl(&mut self, cmd: u32, in_buf: &[u8], out_buf: &mut [u8]) -> Result<u32, NvMapError> {
        match cmd {
            // NVMAP_IOC_CREATE: size (in), handle (out)
            NVMAP_IOC_CREATE => {
                if in_buf.len() < 4 || out_buf.len() < 8 {
                    return Err(NvMapError::BufferTooSmall {
                        expected: 8,
                        actual: out_buf.len(),
                    });
                }

                let size = u32::from_le_bytes(in_buf[0..4].try_into().unwrap());
                let handle = self.next_handle;
                self.next_handle = self.next_handle.wrapping_add(1);

                self.handles.insert(
                    handle,
                    NvMapHandle {
                        id: handle,
                        size,
                        address: 0,
                        allocated: false,
                    },
                );

                // Write handle to out_buf (size at 0..4, handle at 4..8 in struct nvmap_create)
                out_buf[0..4].copy_from_slice(&size.to_le_bytes());
                out_buf[4..8].copy_from_slice(&handle.to_le_bytes());
                Ok(0) // Success
            }

            // NVMAP_IOC_ALLOC: handle (in), heapmask, flags, align, kind, addr (in)
            NVMAP_IOC_ALLOC => {
                if in_buf.len() < 24 {
                    return Err(NvMapError::BufferTooSmall {
                        expected: 24,
                        actual: in_buf.len(),
                    });
                }

                let handle = u32::from_le_bytes(in_buf[0..4].try_into().unwrap());
                let addr = u64::from_le_bytes(in_buf[16..24].try_into().unwrap());

                if let Some(entry) = self.handles.get_mut(&handle) {
                    entry.address = addr;
                    entry.allocated = true;
                    Ok(0)
                } else {
                    Err(NvMapError::InvalidHandle(handle))
                }
            }

            // NVMAP_IOC_GET_ID: handle (in), id (out)
            NVMAP_IOC_GET_ID => {
                if in_buf.len() < 4 || out_buf.len() < 8 {
                    return Err(NvMapError::BufferTooSmall {
                        expected: 8,
                        actual: out_buf.len(),
                    });
                }

                let handle = u32::from_le_bytes(in_buf[0..4].try_into().unwrap());
                if let Some(entry) = self.handles.get(&handle) {
                    out_buf[0..4].copy_from_slice(&handle.to_le_bytes());
                    out_buf[4..8].copy_from_slice(&entry.id.to_le_bytes());
                    Ok(0)
                } else {
                    Err(NvMapError::InvalidHandle(handle))
                }
            }

            // NVMAP_IOC_FREE: handle (in)
            NVMAP_IOC_FREE => {
                if in_buf.len() < 4 {
                    return Err(NvMapError::BufferTooSmall {
                        expected: 4,
                        actual: in_buf.len(),
                    });
                }
                let handle = u32::from_le_bytes(in_buf[0..4].try_into().unwrap());
                self.handles.remove(&handle);
                Ok(0)
            }

            _ => Err(NvMapError::UnknownIoctl(cmd)),
        }
    }

    pub fn get_handle(&self, handle: u32) -> Option<&NvMapHandle> {
        self.handles.get(&handle)
    }

    pub fn len(&self) -> usize {
        self.handles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.handles.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nvmap_create_and_alloc() {
        let mut nvmap = NvMapDevice::new();

        // 1. Create handle for 64 KB buffer
        let mut in_buf = vec![0u8; 8];
        let mut out_buf = vec![0u8; 8];
        in_buf[0..4].copy_from_slice(&0x10000u32.to_le_bytes()); // 64 KB

        let res = nvmap.ioctl(NVMAP_IOC_CREATE, &in_buf, &mut out_buf).unwrap();
        assert_eq!(res, 0);

        let handle = u32::from_le_bytes(out_buf[4..8].try_into().unwrap());
        assert_eq!(handle, 1);

        // 2. Allocate buffer with guest address 0x9000_0000
        let mut alloc_buf = vec![0u8; 24];
        alloc_buf[0..4].copy_from_slice(&handle.to_le_bytes());
        alloc_buf[16..24].copy_from_slice(&0x9000_0000u64.to_le_bytes());

        let res_alloc = nvmap.ioctl(NVMAP_IOC_ALLOC, &alloc_buf, &mut []).unwrap();
        assert_eq!(res_alloc, 0);

        let entry = nvmap.get_handle(handle).unwrap();
        assert!(entry.allocated);
        assert_eq!(entry.address, 0x9000_0000);
        assert_eq!(entry.size, 0x10000);
    }
}
