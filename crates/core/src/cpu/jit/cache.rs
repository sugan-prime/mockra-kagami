//! Basic Block Cache and Chaining Subsystem.
//!
//! Stores compiled AArch64 BasicBlocks indexed by guest entry PC, manages block chaining,
//! and handles cache invalidation on code writes or dynamic relocations.

use super::block::{BasicBlock, BlockTerminal};
use super::compiler::{BlockCompiler, CompileError};
use crate::memory::bus::MemoryBus;
use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct CacheStats {
    pub lookups: u64,
    pub hits: u64,
    pub misses: u64,
    pub invalidations: u64,
    pub chained_jumps: u64,
}

pub struct BlockCache {
    blocks: HashMap<u64, BasicBlock>,
    links: HashMap<u64, u64>,
    pub stats: CacheStats,
}

impl Default for BlockCache {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockCache {
    pub fn new() -> Self {
        Self {
            blocks: HashMap::new(),
            links: HashMap::new(),
            stats: CacheStats::default(),
        }
    }

    /// Fetches a compiled basic block for `pc`, or compiles and inserts it if missing.
    pub fn get_or_compile(&mut self, pc: u64, bus: &impl MemoryBus) -> Result<&mut BasicBlock, CompileError> {
        self.stats.lookups += 1;

        if !self.blocks.contains_key(&pc) {
            self.stats.misses += 1;
            let block = BlockCompiler::compile(pc, bus)?;

            // If block ends in a static unconditional branch, record a chain link
            if let BlockTerminal::BranchDirect { target_pc } = block.terminal {
                self.links.insert(pc, target_pc);
            }

            self.blocks.insert(pc, block);
        } else {
            self.stats.hits += 1;
        }

        Ok(self.blocks.get_mut(&pc).unwrap())
    }

    /// Looks up an already compiled block without compiling.
    pub fn get_block(&self, pc: u64) -> Option<&BasicBlock> {
        self.blocks.get(&pc)
    }

    /// Checks if a direct chain link exists from `from_pc`.
    pub fn get_chain_target(&self, from_pc: u64) -> Option<u64> {
        self.links.get(&from_pc).copied()
    }

    /// Invalidates all cached blocks intersecting the address range `[start, start + size)`.
    pub fn invalidate_range(&mut self, start: u64, size: u64) {
        let end = start.wrapping_add(size);
        let before_count = self.blocks.len();

        self.blocks.retain(|&pc, b| {
            let block_end = b.guest_pc_end.wrapping_add(4);
            // Retain block if it does not overlap [start, end)
            block_end <= start || pc >= end
        });

        // Clear affected links
        self.links.retain(|&from, to| {
            self.blocks.contains_key(&from) && self.blocks.contains_key(to)
        });

        let removed = before_count - self.blocks.len();
        self.stats.invalidations += removed as u64;
    }

    /// Flushes all cached blocks.
    pub fn clear(&mut self) {
        self.blocks.clear();
        self.links.clear();
    }

    /// Returns the number of cached basic blocks.
    pub fn count(&self) -> usize {
        self.blocks.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::bus::FlatMemory;

    #[test]
    fn test_block_cache_get_hit_and_invalidate() {
        let mut mem = FlatMemory::new(0x8000_0000, 0x1000);
        let mut cache = BlockCache::new();

        // 1. MOVZ X0, #1
        // 2. SVC #1
        mem.write_u32(0x8000_0000, 0xD2800020).unwrap();
        mem.write_u32(0x8000_0004, 0xD4000021).unwrap();

        // First lookup: Miss + Compile
        let b1 = cache.get_or_compile(0x8000_0000, &mem).unwrap();
        assert_eq!(b1.instruction_count, 2);
        assert_eq!(cache.stats.misses, 1);
        assert_eq!(cache.stats.hits, 0);

        // Second lookup: Hit
        let _ = cache.get_or_compile(0x8000_0000, &mem).unwrap();
        assert_eq!(cache.stats.hits, 1);

        // Invalidate range covering 0x8000_0000
        cache.invalidate_range(0x8000_0000, 0x10);
        assert_eq!(cache.count(), 0);
        assert_eq!(cache.stats.invalidations, 1);
    }
}
