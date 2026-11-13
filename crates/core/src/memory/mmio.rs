//! Memory-Mapped I/O (MMIO) traits and peripheral devices.

use crate::memory::bus::MemoryError;
use std::sync::{Arc, Mutex};

/// Trait for peripherals that interface with the system memory bus.
pub trait MmioDevice: Send + Sync {
    /// Reads `size` bytes (1, 2, 4, or 8) from `offset` within this device's address window.
    fn read(&self, offset: u64, size: usize) -> Result<u64, MemoryError>;

    /// Writes `size` bytes (1, 2, 4, or 8) to `offset` within this device's address window.
    fn write(&mut self, offset: u64, val: u64, size: usize) -> Result<(), MemoryError>;
}

/// Simple Debug UART device for logging output from guest assembly code.
///
/// Register Map:
/// - Offset 0x00: TX Data (Write a byte to transmit; reading returns status 0 = ready)
/// - Offset 0x04: Status (bit 0 = TX ready, always 1)
#[derive(Debug, Default)]
pub struct DebugUart {
    /// Internal log buffer of all output received from guest.
    output_log: Arc<Mutex<Vec<u8>>>,
    /// Whether to mirror output directly to host stdout in real time.
    mirror_stdout: bool,
}

impl DebugUart {
    pub fn new(mirror_stdout: bool) -> Self {
        Self {
            output_log: Arc::new(Mutex::new(Vec::new())),
            mirror_stdout,
        }
    }

    /// Returns a clone of the captured output log as a UTF-8 String (lossy).
    pub fn get_output_string(&self) -> String {
        let lock = self.output_log.lock().unwrap();
        String::from_utf8_lossy(&lock).to_string()
    }

    /// Clears the captured log.
    pub fn clear(&mut self) {
        let mut lock = self.output_log.lock().unwrap();
        lock.clear();
    }
}

impl MmioDevice for DebugUart {
    fn read(&self, offset: u64, _size: usize) -> Result<u64, MemoryError> {
        match offset {
            // Data register: reading returns 0
            0x00 => Ok(0),
            // Status register: bit 0 = TX ready
            0x04 => Ok(1),
            _ => Ok(0),
        }
    }

    fn write(&mut self, offset: u64, val: u64, _size: usize) -> Result<(), MemoryError> {
        match offset {
            // TX Data register
            0x00 => {
                let byte = val as u8;
                let mut log = self.output_log.lock().unwrap();
                log.push(byte);

                if self.mirror_stdout {
                    use std::io::Write;
                    let _ = std::io::stdout().write_all(&[byte]);
                    let _ = std::io::stdout().flush();
                }
                Ok(())
            }
            _ => Ok(()), // Ignore writes to other offsets
        }
    }
}
