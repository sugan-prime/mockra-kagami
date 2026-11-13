pub mod nvhost_as_gpu;
pub mod nvhost_ctrl;
pub mod nvmap;

pub use nvhost_as_gpu::NvHostAsGpuDevice;
pub use nvhost_ctrl::NvHostCtrlDevice;
pub use nvmap::NvMapDevice;

use crate::kernel::ipc::{IpcContext, IpcError, ServiceHandler};
use std::collections::HashMap;
use std::sync::Mutex;
use thiserror::Error;

#[derive(Error, Debug, PartialEq, Eq)]
pub enum NvDrvError {
    #[error("Device not found: {0}")]
    DeviceNotFound(String),
    #[error("Invalid file descriptor: {0}")]
    InvalidFd(u32),
}

pub enum NvDevice {
    NvMap(NvMapDevice),
    NvHostCtrl(NvHostCtrlDevice),
    NvHostAsGpu(NvHostAsGpuDevice),
}

/// The Horizon OS NVDRV (`nvdrv:`, `nvdrv:a`) driver service.
pub struct NvDrvService {
    next_fd: Mutex<u32>,
    devices: Mutex<HashMap<u32, NvDevice>>,
}

impl Default for NvDrvService {
    fn default() -> Self {
        Self::new()
    }
}

impl NvDrvService {
    pub fn new() -> Self {
        Self {
            next_fd: Mutex::new(1),
            devices: Mutex::new(HashMap::new()),
        }
    }

    /// Opens a virtual device path and returns its allocated file descriptor.
    pub fn open_device(&self, path: &str) -> Result<u32, NvDrvError> {
        let clean_path = path.trim_matches(char::from(0));
        let device = match clean_path {
            "/dev/nvmap" => NvDevice::NvMap(NvMapDevice::new()),
            "/dev/nvhost-ctrl" => NvDevice::NvHostCtrl(NvHostCtrlDevice::new()),
            "/dev/nvhost-as-gpu" => NvDevice::NvHostAsGpu(NvHostAsGpuDevice::new()),
            _ => return Err(NvDrvError::DeviceNotFound(path.to_string())),
        };

        let mut next_fd = self.next_fd.lock().unwrap();
        let fd = *next_fd;
        *next_fd = next_fd.wrapping_add(1);

        self.devices.lock().unwrap().insert(fd, device);
        Ok(fd)
    }

    /// Dispatches an IOCTL command to an open device descriptor.
    pub fn ioctl(&self, fd: u32, cmd: u32, in_buf: &[u8], out_buf: &mut [u8]) -> Result<u32, NvDrvError> {
        let mut devices = self.devices.lock().unwrap();
        if let Some(device) = devices.get_mut(&fd) {
            let res = match device {
                NvDevice::NvMap(d) => d.ioctl(cmd, in_buf, out_buf).unwrap_or(1),
                NvDevice::NvHostCtrl(d) => d.ioctl(cmd, in_buf, out_buf).unwrap_or(1),
                NvDevice::NvHostAsGpu(d) => d.ioctl(cmd, in_buf, out_buf).unwrap_or(1),
            };
            Ok(res)
        } else {
            Err(NvDrvError::InvalidFd(fd))
        }
    }

    /// Closes an open device file descriptor.
    pub fn close(&self, fd: u32) -> bool {
        self.devices.lock().unwrap().remove(&fd).is_some()
    }
}

impl ServiceHandler for NvDrvService {
    fn service_name(&self) -> &str {
        "nvdrv"
    }

    fn dispatch(&self, context: &mut IpcContext) -> Result<(), IpcError> {
        match context.request.command_id {
            // Command 0: Open(path: string) -> fd: u32, result: u32
            0 => {
                let path_str = context
                    .request
                    .read_cstring(0)
                    .unwrap_or_else(|| "/dev/nvmap".to_string());

                match self.open_device(&path_str) {
                    Ok(fd) => {
                        context.response.push_u32(fd);
                        context.response.push_u32(0); // Error code: 0
                        context.response.result_code = 0;
                        Ok(())
                    }
                    Err(_) => {
                        context.response.result_code = 0xE401; // Not found
                        Ok(())
                    }
                }
            }

            // Command 1: Ioctl(fd: u32, cmd: u32)
            1 => {
                let fd = context.request.read_u32(0).unwrap_or(0);
                let cmd = context.request.read_u32(4).unwrap_or(0);
                let mut out_buf = vec![0u8; 64];

                let in_payload = if context.request.raw_data.len() > 8 {
                    &context.request.raw_data[8..]
                } else {
                    &[]
                };

                let _ = self.ioctl(fd, cmd, in_payload, &mut out_buf);
                context.response.push_bytes(&out_buf);
                context.response.result_code = 0;
                Ok(())
            }

            // Command 2: Close(fd: u32)
            2 => {
                let fd = context.request.read_u32(0).unwrap_or(0);
                self.close(fd);
                context.response.result_code = 0;
                Ok(())
            }

            // Command 3: Initialize
            3 => {
                context.response.result_code = 0;
                Ok(())
            }

            cmd => Err(IpcError::UnknownCommand(cmd)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nvdrv_open_and_close() {
        let service = NvDrvService::new();

        let fd = service.open_device("/dev/nvmap").unwrap();
        assert_eq!(fd, 1);

        let fd2 = service.open_device("/dev/nvhost-ctrl").unwrap();
        assert_eq!(fd2, 2);

        assert!(service.close(fd));
        assert!(!service.close(fd)); // already closed
    }
}
