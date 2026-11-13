//! Unified system memory bus with support for DRAM and MMIO peripherals.

use crate::memory::bus::{MemoryBus, MemoryError};
use crate::memory::mmio::MmioDevice;

struct RamRegion {
    base: u64,
    size: usize,
    data: Vec<u8>,
}

struct MmioMapping {
    base: u64,
    size: u64,
    device: Box<dyn MmioDevice>,
}

/// The system interconnect routing memory reads and writes between CPU, RAM, and Peripherals.
pub struct SystemBus {
    ram: Vec<RamRegion>,
    mmio: Vec<MmioMapping>,
}

impl Default for SystemBus {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemBus {
    pub fn new() -> Self {
        Self {
            ram: Vec::new(),
            mmio: Vec::new(),
        }
    }

    /// Maps a region of RAM at `base` of `size` bytes.
    pub fn map_ram(&mut self, base: u64, size: usize) {
        self.ram.push(RamRegion {
            base,
            size,
            data: vec![0; size],
        });
    }

    /// Maps an MMIO device at `base` spanning `size` bytes.
    pub fn map_mmio(&mut self, base: u64, size: u64, device: Box<dyn MmioDevice>) {
        self.mmio.push(MmioMapping { base, size, device });
    }

    /// Loads binary data into mapped RAM starting at `dest_addr`.
    pub fn load_bytes(&mut self, dest_addr: u64, bytes: &[u8]) -> Result<(), MemoryError> {
        for region in &mut self.ram {
            if dest_addr >= region.base && (dest_addr + bytes.len() as u64) <= (region.base + region.size as u64) {
                let offset = (dest_addr - region.base) as usize;
                region.data[offset..offset + bytes.len()].copy_from_slice(bytes);
                return Ok(());
            }
        }
        Err(MemoryError::OutOfBounds(dest_addr))
    }

    #[inline(always)]
    fn find_ram(&self, addr: u64, len: usize) -> Option<(&RamRegion, usize)> {
        for region in &self.ram {
            if addr >= region.base && (addr + len as u64) <= (region.base + region.size as u64) {
                let offset = (addr - region.base) as usize;
                return Some((region, offset));
            }
        }
        None
    }

    #[inline(always)]
    fn find_ram_mut(&mut self, addr: u64, len: usize) -> Option<(&mut RamRegion, usize)> {
        for region in &mut self.ram {
            if addr >= region.base && (addr + len as u64) <= (region.base + region.size as u64) {
                let offset = (addr - region.base) as usize;
                return Some((region, offset));
            }
        }
        None
    }

    #[inline(always)]
    fn find_mmio(&self, addr: u64, len: usize) -> Option<(&MmioMapping, u64)> {
        for mapping in &self.mmio {
            if addr >= mapping.base && (addr + len as u64) <= (mapping.base + mapping.size) {
                let offset = addr - mapping.base;
                return Some((mapping, offset));
            }
        }
        None
    }

    #[inline(always)]
    fn find_mmio_mut(&mut self, addr: u64, len: usize) -> Option<(&mut MmioMapping, u64)> {
        for mapping in &mut self.mmio {
            if addr >= mapping.base && (addr + len as u64) <= (mapping.base + mapping.size) {
                let offset = addr - mapping.base;
                return Some((mapping, offset));
            }
        }
        None
    }
}

impl MemoryBus for SystemBus {
    fn read_u8(&self, addr: u64) -> Result<u8, MemoryError> {
        if let Some((region, off)) = self.find_ram(addr, 1) {
            return Ok(region.data[off]);
        }
        if let Some((mapping, off)) = self.find_mmio(addr, 1) {
            return Ok(mapping.device.read(off, 1)? as u8);
        }
        Err(MemoryError::OutOfBounds(addr))
    }

    fn read_u16(&self, addr: u64) -> Result<u16, MemoryError> {
        if let Some((region, off)) = self.find_ram(addr, 2) {
            return Ok(u16::from_le_bytes(region.data[off..off + 2].try_into().unwrap()));
        }
        if let Some((mapping, off)) = self.find_mmio(addr, 2) {
            return Ok(mapping.device.read(off, 2)? as u16);
        }
        Err(MemoryError::OutOfBounds(addr))
    }

    fn read_u32(&self, addr: u64) -> Result<u32, MemoryError> {
        if let Some((region, off)) = self.find_ram(addr, 4) {
            return Ok(u32::from_le_bytes(region.data[off..off + 4].try_into().unwrap()));
        }
        if let Some((mapping, off)) = self.find_mmio(addr, 4) {
            return Ok(mapping.device.read(off, 4)? as u32);
        }
        Err(MemoryError::OutOfBounds(addr))
    }

    fn read_u64(&self, addr: u64) -> Result<u64, MemoryError> {
        if let Some((region, off)) = self.find_ram(addr, 8) {
            return Ok(u64::from_le_bytes(region.data[off..off + 8].try_into().unwrap()));
        }
        if let Some((mapping, off)) = self.find_mmio(addr, 8) {
            return mapping.device.read(off, 8);
        }
        Err(MemoryError::OutOfBounds(addr))
    }

    fn write_u8(&mut self, addr: u64, val: u8) -> Result<(), MemoryError> {
        if let Some((region, off)) = self.find_ram_mut(addr, 1) {
            region.data[off] = val;
            return Ok(());
        }
        if let Some((mapping, off)) = self.find_mmio_mut(addr, 1) {
            return mapping.device.write(off, val as u64, 1);
        }
        Err(MemoryError::OutOfBounds(addr))
    }

    fn write_u16(&mut self, addr: u64, val: u16) -> Result<(), MemoryError> {
        if let Some((region, off)) = self.find_ram_mut(addr, 2) {
            region.data[off..off + 2].copy_from_slice(&val.to_le_bytes());
            return Ok(());
        }
        if let Some((mapping, off)) = self.find_mmio_mut(addr, 2) {
            return mapping.device.write(off, val as u64, 2);
        }
        Err(MemoryError::OutOfBounds(addr))
    }

    fn write_u32(&mut self, addr: u64, val: u32) -> Result<(), MemoryError> {
        if let Some((region, off)) = self.find_ram_mut(addr, 4) {
            region.data[off..off + 4].copy_from_slice(&val.to_le_bytes());
            return Ok(());
        }
        if let Some((mapping, off)) = self.find_mmio_mut(addr, 4) {
            return mapping.device.write(off, val as u64, 4);
        }
        Err(MemoryError::OutOfBounds(addr))
    }

    fn write_u64(&mut self, addr: u64, val: u64) -> Result<(), MemoryError> {
        if let Some((region, off)) = self.find_ram_mut(addr, 8) {
            region.data[off..off + 8].copy_from_slice(&val.to_le_bytes());
            return Ok(());
        }
        if let Some((mapping, off)) = self.find_mmio_mut(addr, 8) {
            return mapping.device.write(off, val, 8);
        }
        Err(MemoryError::OutOfBounds(addr))
    }
}
