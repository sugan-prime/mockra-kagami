//! GPU Buffer Cache and Memory Coherency Engine.
//!
//! Synchronizes shared physical DRAM between ARM64 CPU and Maxwell GPU
//! through fine-grained 4KB dirty page tracking, avoiding redundant memory copies.

use std::collections::{HashMap, HashSet};

pub const PAGE_SIZE: u64 = 4096;

/// Represents a contiguous GPU buffer mapped in guest memory.
#[derive(Debug, Clone)]
pub struct BufferBinding {
    pub guest_address: u64,
    pub size: usize,
    pub is_render_target: bool,
}

/// Buffer cache tracking memory ranges and page-level dirtiness.
#[derive(Debug, Default)]
pub struct BufferCache {
    /// Tracked buffers keyed by their guest base address.
    buffers: HashMap<u64, BufferBinding>,
    /// Set of dirty 4KB page indices.
    dirty_pages: HashSet<u64>,
}

impl BufferCache {
    pub fn new() -> Self {
        Self {
            buffers: HashMap::new(),
            dirty_pages: HashSet::new(),
        }
    }

    /// Registers a guest memory range as an active GPU buffer.
    pub fn register_buffer(&mut self, guest_address: u64, size: usize, is_render_target: bool) {
        self.buffers.insert(
            guest_address,
            BufferBinding {
                guest_address,
                size,
                is_render_target,
            },
        );
        self.mark_dirty(guest_address, size);
    }

    /// Marks the 4KB pages covering `[address, address + size)` as dirty.
    pub fn mark_dirty(&mut self, address: u64, size: usize) {
        let start_page = address / PAGE_SIZE;
        let end_page = (address.wrapping_add(size as u64).saturating_sub(1)) / PAGE_SIZE;

        for page in start_page..=end_page {
            self.dirty_pages.insert(page);
        }
    }

    /// Checks if any 4KB page within `[address, address + size)` is marked dirty.
    pub fn is_dirty(&self, address: u64, size: usize) -> bool {
        let start_page = address / PAGE_SIZE;
        let end_page = (address.wrapping_add(size as u64).saturating_sub(1)) / PAGE_SIZE;

        for page in start_page..=end_page {
            if self.dirty_pages.contains(&page) {
                return true;
            }
        }
        false
    }

    /// Clears the dirty state for the pages covering `[address, address + size)`.
    pub fn clear_dirty(&mut self, address: u64, size: usize) {
        let start_page = address / PAGE_SIZE;
        let end_page = (address.wrapping_add(size as u64).saturating_sub(1)) / PAGE_SIZE;

        for page in start_page..=end_page {
            self.dirty_pages.remove(&page);
        }
    }

    /// Number of active dirty pages currently flagged.
    pub fn dirty_page_count(&self) -> usize {
        self.dirty_pages.len()
    }

    pub fn get_buffer(&self, guest_address: u64) -> Option<&BufferBinding> {
        self.buffers.get(&guest_address)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_buffer_cache_dirty_pages() {
        let mut cache = BufferCache::new();

        let base_addr = 0x8800_1000; // Page 0x88001
        let size = 0x3000;          // Spans 3 pages: 0x88001, 0x88002, 0x88003

        cache.register_buffer(base_addr, size, false);
        assert_eq!(cache.dirty_page_count(), 3);
        assert!(cache.is_dirty(base_addr, size));

        // Clearing dirty range
        cache.clear_dirty(base_addr, size);
        assert_eq!(cache.dirty_page_count(), 0);
        assert!(!cache.is_dirty(base_addr, size));

        // Re-marking a single page in middle
        cache.mark_dirty(base_addr + 0x1000, 100);
        assert_eq!(cache.dirty_page_count(), 1);
        assert!(cache.is_dirty(base_addr, size));
    }
}
