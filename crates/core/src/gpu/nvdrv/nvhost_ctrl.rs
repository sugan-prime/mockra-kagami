//! nvhost-ctrl: Tegra X1 host1x control & synchronization device (`/dev/nvhost-ctrl`).
//!
//! Manages GPU syncpoint increments, syncpoint wait queries, and device events.

use thiserror::Error;

pub const NVHOST_IOCTL_CTRL_SYNCPT_READ: u32 = 0xC0080014;
pub const NVHOST_IOCTL_CTRL_SYNCPT_INCR: u32 = 0x40040015;
pub const NVHOST_IOCTL_CTRL_SYNCPT_WAIT: u32 = 0xC0100016;

#[derive(Error, Debug, PartialEq, Eq)]
pub enum NvHostCtrlError {
    #[error("Buffer too small for IOCTL: expected at least {expected} bytes, got {actual}")]
    BufferTooSmall { expected: usize, actual: usize },
    #[error("Invalid syncpoint index: {0}")]
    InvalidSyncpoint(u32),
    #[error("Unknown or unhandled IOCTL command: 0x{0:08X}")]
    UnknownIoctl(u32),
}

/// The `/dev/nvhost-ctrl` device emulation.
#[derive(Debug)]
pub struct NvHostCtrlDevice {
    pub syncpoints: [u32; 64],
}

impl Default for NvHostCtrlDevice {
    fn default() -> Self {
        Self::new()
    }
}

impl NvHostCtrlDevice {
    pub fn new() -> Self {
        Self {
            syncpoints: [0; 64],
        }
    }

    /// Dispatches an IOCTL command to the nvhost-ctrl driver.
    pub fn ioctl(&mut self, cmd: u32, in_buf: &[u8], out_buf: &mut [u8]) -> Result<u32, NvHostCtrlError> {
        match cmd {
            // NVHOST_IOCTL_CTRL_SYNCPT_READ: syncpt_id (in), value (out)
            NVHOST_IOCTL_CTRL_SYNCPT_READ => {
                if in_buf.len() < 4 || out_buf.len() < 8 {
                    return Err(NvHostCtrlError::BufferTooSmall {
                        expected: 8,
                        actual: out_buf.len(),
                    });
                }

                let id = u32::from_le_bytes(in_buf[0..4].try_into().unwrap());
                if (id as usize) < self.syncpoints.len() {
                    let val = self.syncpoints[id as usize];
                    out_buf[0..4].copy_from_slice(&id.to_le_bytes());
                    out_buf[4..8].copy_from_slice(&val.to_le_bytes());
                    Ok(0)
                } else {
                    Err(NvHostCtrlError::InvalidSyncpoint(id))
                }
            }

            // NVHOST_IOCTL_CTRL_SYNCPT_INCR: syncpt_id (in)
            NVHOST_IOCTL_CTRL_SYNCPT_INCR => {
                if in_buf.len() < 4 {
                    return Err(NvHostCtrlError::BufferTooSmall {
                        expected: 4,
                        actual: in_buf.len(),
                    });
                }

                let id = u32::from_le_bytes(in_buf[0..4].try_into().unwrap());
                if (id as usize) < self.syncpoints.len() {
                    self.syncpoints[id as usize] = self.syncpoints[id as usize].wrapping_add(1);
                    Ok(0)
                } else {
                    Err(NvHostCtrlError::InvalidSyncpoint(id))
                }
            }

            // NVHOST_IOCTL_CTRL_SYNCPT_WAIT: syncpt_id (in), threshold (in), timeout (in)
            NVHOST_IOCTL_CTRL_SYNCPT_WAIT => {
                if in_buf.len() < 8 {
                    return Err(NvHostCtrlError::BufferTooSmall {
                        expected: 8,
                        actual: in_buf.len(),
                    });
                }

                let id = u32::from_le_bytes(in_buf[0..4].try_into().unwrap());
                let threshold = u32::from_le_bytes(in_buf[4..8].try_into().unwrap());

                if (id as usize) < self.syncpoints.len() {
                    let cur = self.syncpoints[id as usize];
                    if cur >= threshold {
                        Ok(0) // Immediately satisfied
                    } else {
                        // In single-threaded emulation, advance syncpoint to threshold
                        self.syncpoints[id as usize] = threshold;
                        Ok(0)
                    }
                } else {
                    Err(NvHostCtrlError::InvalidSyncpoint(id))
                }
            }

            _ => Err(NvHostCtrlError::UnknownIoctl(cmd)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_syncpt_read_and_incr() {
        let mut ctrl = NvHostCtrlDevice::new();
        let id = 5u32;

        // Increment syncpoint 5
        let in_buf = id.to_le_bytes();
        ctrl.ioctl(NVHOST_IOCTL_CTRL_SYNCPT_INCR, &in_buf, &mut []).unwrap();
        ctrl.ioctl(NVHOST_IOCTL_CTRL_SYNCPT_INCR, &in_buf, &mut []).unwrap();

        // Read syncpoint 5
        let mut out_buf = [0u8; 8];
        ctrl.ioctl(NVHOST_IOCTL_CTRL_SYNCPT_READ, &in_buf, &mut out_buf).unwrap();

        let val = u32::from_le_bytes(out_buf[4..8].try_into().unwrap());
        assert_eq!(val, 2);
    }
}
