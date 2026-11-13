//! In-memory Codecave / IPSwitch dynamic patching engine.
//!
//! Inspired by IPSwitch / Atmosphere / Cemu patches, this allows applying
//! dynamic memory overrides (such as 60fps patches, debug hooks, or bug fixes)
//! to guest code without modifying original executable files on disk.

use crate::memory::bus::{MemoryBus, MemoryError};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum PatchError {
    #[error("Invalid hex byte sequence: {0}")]
    InvalidHex(String),
    #[error("Memory bus write error during patch application: {0}")]
    Memory(#[from] MemoryError),
}

/// A discrete memory patch targeting an offset relative to load base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryPatch {
    pub name: String,
    pub offset: u64,
    pub original_bytes: Option<Vec<u8>>,
    pub patch_bytes: Vec<u8>,
    pub enabled: bool,
}

impl MemoryPatch {
    pub fn new(name: impl Into<String>, offset: u64, patch_bytes: Vec<u8>) -> Self {
        Self {
            name: name.into(),
            offset,
            original_bytes: None,
            patch_bytes,
            enabled: true,
        }
    }

    pub fn with_expected(
        name: impl Into<String>,
        offset: u64,
        original_bytes: Vec<u8>,
        patch_bytes: Vec<u8>,
    ) -> Self {
        Self {
            name: name.into(),
            offset,
            original_bytes: Some(original_bytes),
            patch_bytes,
            enabled: true,
        }
    }
}

/// Manages collections of binary and instruction patches.
#[derive(Debug, Default, Clone)]
pub struct PatchManager {
    patches: Vec<MemoryPatch>,
}

impl PatchManager {
    pub fn new() -> Self {
        Self { patches: Vec::new() }
    }

    pub fn add_patch(&mut self, patch: MemoryPatch) {
        self.patches.push(patch);
    }

    /// Registers a patch using a hexadecimal string (e.g. "1F 20 03 D5" for NOP).
    pub fn add_hex_patch(
        &mut self,
        name: impl Into<String>,
        offset: u64,
        hex_str: &str,
    ) -> Result<(), PatchError> {
        let clean: String = hex_str.chars().filter(|c| !c.is_whitespace()).collect();
        if clean.len() % 2 != 0 {
            return Err(PatchError::InvalidHex(format!("Odd hex string length: {}", hex_str)));
        }

        let mut bytes = Vec::with_capacity(clean.len() / 2);
        for i in (0..clean.len()).step_by(2) {
            let byte = u8::from_str_radix(&clean[i..i + 2], 16)
                .map_err(|_| PatchError::InvalidHex(hex_str.to_string()))?;
            bytes.push(byte);
        }

        self.add_patch(MemoryPatch::new(name, offset, bytes));
        Ok(())
    }

    /// Applies all active patches relative to `base_address`.
    pub fn apply_all(&self, bus: &mut impl MemoryBus, base_address: u64) -> Result<usize, PatchError> {
        let mut count = 0;
        for patch in &self.patches {
            if !patch.enabled {
                continue;
            }

            let target = base_address.wrapping_add(patch.offset);

            // If original bytes are specified, verify them before applying
            if let Some(expected) = &patch.original_bytes {
                let mut current = vec![0u8; expected.len()];
                for (i, byte_slot) in current.iter_mut().enumerate() {
                    *byte_slot = bus.read_u8(target.wrapping_add(i as u64))?;
                }
                if &current != expected {
                    // Mismatch, skip patch to prevent corrupting mismatched binaries
                    continue;
                }
            }

            for (i, &byte) in patch.patch_bytes.iter().enumerate() {
                bus.write_u8(target.wrapping_add(i as u64), byte)?;
            }
            count += 1;
        }

        Ok(count)
    }

    pub fn patches(&self) -> &[MemoryPatch] {
        &self.patches
    }

    pub fn len(&self) -> usize {
        self.patches.len()
    }

    pub fn is_empty(&self) -> bool {
        self.patches.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::bus::FlatMemory;

    #[test]
    fn test_patch_manager_hex_application() {
        let base = 0x100;
        let mut mem = FlatMemory::new(base, 0x1000);

        let mut pm = PatchManager::new();
        // AArch64 NOP instruction: 0x1F, 0x20, 0x03, 0xD5 (little-endian: D5 03 20 1F)
        pm.add_hex_patch("NOP out call", 0x10, "1F 20 03 D5").unwrap();

        let applied = pm.apply_all(&mut mem, base).unwrap();
        assert_eq!(applied, 1);

        assert_eq!(mem.read_u8(base + 0x10).unwrap(), 0x1F);
        assert_eq!(mem.read_u8(base + 0x11).unwrap(), 0x20);
        assert_eq!(mem.read_u8(base + 0x12).unwrap(), 0x03);
        assert_eq!(mem.read_u8(base + 0x13).unwrap(), 0xD5);
    }

    #[test]
    fn test_patch_manager_expected_guard() {
        let base = 0x100;
        let mut mem = FlatMemory::new(base, 0x1000);

        // Populate base with original bytes [0xAA, 0xBB]
        mem.write_u8(base + 0x20, 0xAA).unwrap();
        mem.write_u8(base + 0x21, 0xBB).unwrap();

        let mut pm = PatchManager::new();
        // Matching patch
        pm.add_patch(MemoryPatch::with_expected("Valid Patch", 0x20, vec![0xAA, 0xBB], vec![0x11, 0x22]));
        // Mismatching patch
        pm.add_patch(MemoryPatch::with_expected("Mismatched Patch", 0x20, vec![0xCC, 0xDD], vec![0x33, 0x44]));

        let applied = pm.apply_all(&mut mem, base).unwrap();
        assert_eq!(applied, 1);
        assert_eq!(mem.read_u8(base + 0x20).unwrap(), 0x11);
        assert_eq!(mem.read_u8(base + 0x21).unwrap(), 0x22);
    }
}
