//! Fastmem Software Paging Subsystem.
//!
//! Provides a high-performance 4KB-paged memory bus that bypasses linear region scans,
//! allowing constant-time O(1) page resolution for hot CPU instruction fetch,
//! data load/store, and JIT cache coherency dirty tracking.

use crate::memory::bus::{MemoryBus, MemoryError};
use crate::memory::mmio::MmioDevice;
use std::collections::HashMap;

pub const PAGE_SIZE: usize = 4096;
pub const PAGE_SHIFT: u32 = 12;
pub const PAGE_MASK: u64 = 0xFFF;

pub mod permissions {
    pub const READ: u8 = 1 << 0;
    pub const WRITE: u8 = 1 << 1;
    pub const EXECUTE: u8 = 1 << 2;
    pub const RW: u8 = READ | WRITE;
    pub const RWX: u8 = READ | WRITE | EXECUTE;
}

/// A single 4KB backing page in guest virtual memory.
pub struct Page {
    pub data: Box<[u8; PAGE_SIZE]>,
    pub flags: u8,
    pub is_dirty: bool,
}

impl Page {
    pub fn new(flags: u8) -> Self {
        Self {
            data: Box::new([0u8; PAGE_SIZE]),
            flags,
            is_dirty: false,
        }
    }
}

struct MmioEntry {
    base: u64,
    size: u64,
    device: Box<dyn MmioDevice>,
}

/// Fastmem software paged memory management unit.
pub struct Fastmem {
    pages: HashMap<u64, Page>,
    mmio: Vec<MmioEntry>,
    pub reads: u64,
    pub writes: u64,
    pub fast_hits: u64,
    pub page_faults: u64,
}

impl Default for Fastmem {
    fn default() -> Self {
        Self::new()
    }
}

impl Fastmem {
    pub fn new() -> Self {
        Self {
            pages: HashMap::new(),
            mmio: Vec::new(),
            reads: 0,
            writes: 0,
            fast_hits: 0,
            page_faults: 0,
        }
    }

    /// Maps a single 4KB page at the specified Virtual Page Number (VPN).
    pub fn map_page(&mut self, vpn: u64, flags: u8) {
        self.pages.insert(vpn, Page::new(flags));
    }

    /// Maps a contiguous virtual address range into 4KB pages.
    pub fn map_range(&mut self, base: u64, size: usize, flags: u8) {
        let start_vpn = base >> PAGE_SHIFT;
        let num_pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;

        for i in 0..num_pages {
            self.map_page(start_vpn + i as u64, flags);
        }
    }

    /// Maps an MMIO device at the given base address and size.
    pub fn map_mmio(&mut self, base: u64, size: u64, device: Box<dyn MmioDevice>) {
        self.mmio.push(MmioEntry { base, size, device });
    }

    /// Copies a host byte buffer into the mapped guest pages starting at `dest_addr`.
    pub fn load(&mut self, dest_addr: u64, bytes: &[u8]) -> Result<(), MemoryError> {
        let mut curr_addr = dest_addr;
        let mut remaining = bytes;

        while !remaining.is_empty() {
            let vpn = curr_addr >> PAGE_SHIFT;
            let offset = (curr_addr & PAGE_MASK) as usize;
            let chunk_len = (PAGE_SIZE - offset).min(remaining.len());

            let page = self.pages.get_mut(&vpn).ok_or(MemoryError::OutOfBounds(curr_addr))?;
            page.data[offset..offset + chunk_len].copy_from_slice(&remaining[..chunk_len]);
            page.is_dirty = true;

            curr_addr += chunk_len as u64;
            remaining = &remaining[chunk_len..];
        }

        Ok(())
    }

    /// Checks if a page has been modified since the last clear.
    pub fn is_page_dirty(&self, vpn: u64) -> bool {
        self.pages.get(&vpn).map_or(false, |p| p.is_dirty)
    }

    /// Clears dirty flags across all mapped pages.
    pub fn clear_dirty_flags(&mut self) {
        for page in self.pages.values_mut() {
            page.is_dirty = false;
        }
    }

    /// Returns the number of mapped 4KB pages.
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }
}

impl MemoryBus for Fastmem {
    fn read_u8(&self, addr: u64) -> Result<u8, MemoryError> {
        let vpn = addr >> PAGE_SHIFT;
        let offset = (addr & PAGE_MASK) as usize;

        if let Some(page) = self.pages.get(&vpn) {
            if page.flags & permissions::READ == 0 {
                return Err(MemoryError::PermissionDenied(addr));
            }
            Ok(page.data[offset])
        } else {
            // Check MMIO fallback
            for entry in &self.mmio {
                if addr >= entry.base && addr < entry.base + entry.size {
                    return entry.device.read(addr - entry.base, 1).map(|v| v as u8);
                }
            }
            Err(MemoryError::OutOfBounds(addr))
        }
    }

    fn read_u16(&self, addr: u64) -> Result<u16, MemoryError> {
        if addr % 2 != 0 {
            return Err(MemoryError::Misaligned(addr, 2));
        }

        let vpn = addr >> PAGE_SHIFT;
        let offset = (addr & PAGE_MASK) as usize;

        if offset + 2 <= PAGE_SIZE {
            if let Some(page) = self.pages.get(&vpn) {
                if page.flags & permissions::READ == 0 {
                    return Err(MemoryError::PermissionDenied(addr));
                }
                return Ok(u16::from_le_bytes([page.data[offset], page.data[offset + 1]]));
            }
        }

        // Cross-page or MMIO fallback
        let b0 = self.read_u8(addr)?;
        let b1 = self.read_u8(addr + 1)?;
        Ok(u16::from_le_bytes([b0, b1]))
    }

    fn read_u32(&self, addr: u64) -> Result<u32, MemoryError> {
        if addr % 4 != 0 {
            return Err(MemoryError::Misaligned(addr, 4));
        }

        let vpn = addr >> PAGE_SHIFT;
        let offset = (addr & PAGE_MASK) as usize;

        if offset + 4 <= PAGE_SIZE {
            if let Some(page) = self.pages.get(&vpn) {
                if page.flags & permissions::READ == 0 {
                    return Err(MemoryError::PermissionDenied(addr));
                }
                let bytes: [u8; 4] = page.data[offset..offset + 4].try_into().unwrap();
                return Ok(u32::from_le_bytes(bytes));
            }
        }

        // Cross-page or MMIO fallback
        let mut bytes = [0u8; 4];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = self.read_u8(addr + i as u64)?;
        }
        Ok(u32::from_le_bytes(bytes))
    }

    fn read_u64(&self, addr: u64) -> Result<u64, MemoryError> {
        if addr % 8 != 0 {
            return Err(MemoryError::Misaligned(addr, 8));
        }

        let vpn = addr >> PAGE_SHIFT;
        let offset = (addr & PAGE_MASK) as usize;

        if offset + 8 <= PAGE_SIZE {
            if let Some(page) = self.pages.get(&vpn) {
                if page.flags & permissions::READ == 0 {
                    return Err(MemoryError::PermissionDenied(addr));
                }
                let bytes: [u8; 8] = page.data[offset..offset + 8].try_into().unwrap();
                return Ok(u64::from_le_bytes(bytes));
            }
        }

        // Cross-page or MMIO fallback
        let mut bytes = [0u8; 8];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = self.read_u8(addr + i as u64)?;
        }
        Ok(u64::from_le_bytes(bytes))
    }

    fn write_u8(&mut self, addr: u64, val: u8) -> Result<(), MemoryError> {
        let vpn = addr >> PAGE_SHIFT;
        let offset = (addr & PAGE_MASK) as usize;

        if let Some(page) = self.pages.get_mut(&vpn) {
            if page.flags & permissions::WRITE == 0 {
                return Err(MemoryError::PermissionDenied(addr));
            }
            page.data[offset] = val;
            page.is_dirty = true;
            Ok(())
        } else {
            for entry in &mut self.mmio {
                if addr >= entry.base && addr < entry.base + entry.size {
                    return entry.device.write(addr - entry.base, val as u64, 1);
                }
            }
            Err(MemoryError::OutOfBounds(addr))
        }
    }

    fn write_u16(&mut self, addr: u64, val: u16) -> Result<(), MemoryError> {
        if addr % 2 != 0 {
            return Err(MemoryError::Misaligned(addr, 2));
        }

        let vpn = addr >> PAGE_SHIFT;
        let offset = (addr & PAGE_MASK) as usize;

        if offset + 2 <= PAGE_SIZE {
            if let Some(page) = self.pages.get_mut(&vpn) {
                if page.flags & permissions::WRITE == 0 {
                    return Err(MemoryError::PermissionDenied(addr));
                }
                let bytes = val.to_le_bytes();
                page.data[offset..offset + 2].copy_from_slice(&bytes);
                page.is_dirty = true;
                return Ok(());
            }
        }

        let bytes = val.to_le_bytes();
        self.write_u8(addr, bytes[0])?;
        self.write_u8(addr + 1, bytes[1])?;
        Ok(())
    }

    fn write_u32(&mut self, addr: u64, val: u32) -> Result<(), MemoryError> {
        if addr % 4 != 0 {
            return Err(MemoryError::Misaligned(addr, 4));
        }

        let vpn = addr >> PAGE_SHIFT;
        let offset = (addr & PAGE_MASK) as usize;

        if offset + 4 <= PAGE_SIZE {
            if let Some(page) = self.pages.get_mut(&vpn) {
                if page.flags & permissions::WRITE == 0 {
                    return Err(MemoryError::PermissionDenied(addr));
                }
                let bytes = val.to_le_bytes();
                page.data[offset..offset + 4].copy_from_slice(&bytes);
                page.is_dirty = true;
                return Ok(());
            }
        }

        let bytes = val.to_le_bytes();
        for (i, &b) in bytes.iter().enumerate() {
            self.write_u8(addr + i as u64, b)?;
        }
        Ok(())
    }

    fn write_u64(&mut self, addr: u64, val: u64) -> Result<(), MemoryError> {
        if addr % 8 != 0 {
            return Err(MemoryError::Misaligned(addr, 8));
        }

        let vpn = addr >> PAGE_SHIFT;
        let offset = (addr & PAGE_MASK) as usize;

        if offset + 8 <= PAGE_SIZE {
            if let Some(page) = self.pages.get_mut(&vpn) {
                if page.flags & permissions::WRITE == 0 {
                    return Err(MemoryError::PermissionDenied(addr));
                }
                let bytes = val.to_le_bytes();
                page.data[offset..offset + 8].copy_from_slice(&bytes);
                page.is_dirty = true;
                return Ok(());
            }
        }

        let bytes = val.to_le_bytes();
        for (i, &b) in bytes.iter().enumerate() {
            self.write_u8(addr + i as u64, b)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fastmem_page_mapping_and_read_write() {
        let mut mem = Fastmem::new();
        // Map 8KB at 0x8000_0000 (VPN 0x80000 and 0x80001)
        mem.map_range(0x8000_0000, 0x2000, permissions::RW);

        assert_eq!(mem.page_count(), 2);

        mem.write_u32(0x8000_0000, 0x12345678).unwrap();
        mem.write_u64(0x8000_0FF8, 0xDEADBEEFCAFEBABE).unwrap();

        assert_eq!(mem.read_u32(0x8000_0000).unwrap(), 0x12345678);
        assert_eq!(mem.read_u64(0x8000_0FF8).unwrap(), 0xDEADBEEFCAFEBABE);

        assert!(mem.is_page_dirty(0x8000_0000 >> PAGE_SHIFT));
    }

    #[test]
    fn test_fastmem_unmapped_access_fails() {
        let mem = Fastmem::new();
        assert!(mem.read_u32(0x7100_0000).is_err());
    }
}
