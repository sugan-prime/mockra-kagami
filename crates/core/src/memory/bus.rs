//! Memory bus interface and test memory backing.

use thiserror::Error;

#[derive(Error, Debug, PartialEq, Eq)]
pub enum MemoryError {
    #[error("Address out of bounds: 0x{0:016X}")]
    OutOfBounds(u64),
    #[error("Misaligned access: 0x{0:016X} (required alignment: {1})")]
    Misaligned(u64, usize),
    #[error("Permission denied at 0x{0:016X}")]
    PermissionDenied(u64),
}

/// Abstract memory bus interface decoupled from CPU execution.
pub trait MemoryBus {
    fn read_u8(&self, addr: u64) -> Result<u8, MemoryError>;
    fn read_u16(&self, addr: u64) -> Result<u16, MemoryError>;
    fn read_u32(&self, addr: u64) -> Result<u32, MemoryError>;
    fn read_u64(&self, addr: u64) -> Result<u64, MemoryError>;

    fn write_u8(&mut self, addr: u64, val: u8) -> Result<(), MemoryError>;
    fn write_u16(&mut self, addr: u64, val: u16) -> Result<(), MemoryError>;
    fn write_u32(&mut self, addr: u64, val: u32) -> Result<(), MemoryError>;
    fn write_u64(&mut self, addr: u64, val: u64) -> Result<(), MemoryError>;
}

/// A contiguous flat memory block (useful for unit tests and early flat binary runners).
pub struct FlatMemory {
    base_addr: u64,
    data: Vec<u8>,
}

impl FlatMemory {
    pub fn new(base_addr: u64, size: usize) -> Self {
        Self {
            base_addr,
            data: vec![0; size],
        }
    }

    /// Loads a byte slice into memory starting at `dest_addr`.
    pub fn load(&mut self, dest_addr: u64, bytes: &[u8]) -> Result<(), MemoryError> {
        let offset = self.check_range(dest_addr, bytes.len())?;
        self.data[offset..offset + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }

    #[inline(always)]
    fn check_range(&self, addr: u64, len: usize) -> Result<usize, MemoryError> {
        if addr < self.base_addr {
            return Err(MemoryError::OutOfBounds(addr));
        }
        let offset = (addr - self.base_addr) as usize;
        if offset + len > self.data.len() {
            return Err(MemoryError::OutOfBounds(addr));
        }
        Ok(offset)
    }
}

impl MemoryBus for FlatMemory {
    fn read_u8(&self, addr: u64) -> Result<u8, MemoryError> {
        let off = self.check_range(addr, 1)?;
        Ok(self.data[off])
    }

    fn read_u16(&self, addr: u64) -> Result<u16, MemoryError> {
        let off = self.check_range(addr, 2)?;
        Ok(u16::from_le_bytes(self.data[off..off + 2].try_into().unwrap()))
    }

    fn read_u32(&self, addr: u64) -> Result<u32, MemoryError> {
        let off = self.check_range(addr, 4)?;
        Ok(u32::from_le_bytes(self.data[off..off + 4].try_into().unwrap()))
    }

    fn read_u64(&self, addr: u64) -> Result<u64, MemoryError> {
        let off = self.check_range(addr, 8)?;
        Ok(u64::from_le_bytes(self.data[off..off + 8].try_into().unwrap()))
    }

    fn write_u8(&mut self, addr: u64, val: u8) -> Result<(), MemoryError> {
        let off = self.check_range(addr, 1)?;
        self.data[off] = val;
        Ok(())
    }

    fn write_u16(&mut self, addr: u64, val: u16) -> Result<(), MemoryError> {
        let off = self.check_range(addr, 2)?;
        self.data[off..off + 2].copy_from_slice(&val.to_le_bytes());
        Ok(())
    }

    fn write_u32(&mut self, addr: u64, val: u32) -> Result<(), MemoryError> {
        let off = self.check_range(addr, 4)?;
        self.data[off..off + 4].copy_from_slice(&val.to_le_bytes());
        Ok(())
    }

    fn write_u64(&mut self, addr: u64, val: u64) -> Result<(), MemoryError> {
        let off = self.check_range(addr, 8)?;
        self.data[off..off + 8].copy_from_slice(&val.to_le_bytes());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flat_memory_read_write() {
        let mut mem = FlatMemory::new(0x8000_0000, 0x10000);
        
        // Write 32-bit and 64-bit values
        mem.write_u32(0x8000_0000, 0x1234_5678).unwrap();
        mem.write_u64(0x8000_0008, 0xDEAD_BEEF_CAFE_BABE).unwrap();

        assert_eq!(mem.read_u32(0x8000_0000).unwrap(), 0x1234_5678);
        assert_eq!(mem.read_u64(0x8000_0008).unwrap(), 0xDEAD_BEEF_CAFE_BABE);

        // Byte order check (little endian)
        assert_eq!(mem.read_u8(0x8000_0000).unwrap(), 0x78);
        assert_eq!(mem.read_u8(0x8000_0001).unwrap(), 0x56);
    }
}
