//! Persistent Maxwell Shader Bytecode Cache & Hashing Engine.
//!
//! Computes unique 64-bit cryptographic/distribution hashes of Maxwell GPU
//! shader bytecode (Vertex, Fragment, Compute) to prevent compilation stutter
//! by maintaining an in-memory and disk-friendly cache.

use std::collections::HashMap;

/// Maxwell GPU pipeline programmable shader stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShaderStage {
    Vertex,
    TessellationControl,
    TessellationEvaluation,
    Geometry,
    Fragment,
    Compute,
}

/// Metadata and hash identity for a compiled Maxwell shader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShaderInfo {
    pub hash: u64,
    pub stage: ShaderStage,
    pub guest_address: u64,
    pub code_size: usize,
}

/// Computes a 64-bit FNV-1a hash over binary bytecode.
pub fn hash_fnv1a_64(data: &[u8]) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET_BASIS;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Computes a 64-bit MurmurHash3-style hash over binary bytecode.
pub fn hash_murmur3_64(data: &[u8], seed: u64) -> u64 {
    let mut hash = seed ^ (data.len() as u64).wrapping_mul(0xc6a4a7935bd1e995);

    let chunks = data.chunks_exact(8);
    let remainder = chunks.remainder();

    for chunk in chunks {
        let mut k = u64::from_le_bytes(chunk.try_into().unwrap());
        k = k.wrapping_mul(0xc6a4a7935bd1e995);
        k ^= k >> 47;
        k = k.wrapping_mul(0xc6a4a7935bd1e995);

        hash ^= k;
        hash = hash.wrapping_mul(0xc6a4a7935bd1e995);
    }

    if !remainder.is_empty() {
        let mut rem_k = 0u64;
        for (i, &b) in remainder.iter().enumerate() {
            rem_k |= (b as u64) << (i * 8);
        }
        hash ^= rem_k;
        hash = hash.wrapping_mul(0xc6a4a7935bd1e995);
    }

    hash ^= hash >> 47;
    hash = hash.wrapping_mul(0xc6a4a7935bd1e995);
    hash ^= hash >> 47;

    hash
}

/// In-memory cache tracking loaded and analyzed Maxwell shaders.
#[derive(Debug, Default)]
pub struct ShaderCache {
    shaders: HashMap<u64, ShaderInfo>,
}

impl ShaderCache {
    pub fn new() -> Self {
        Self {
            shaders: HashMap::new(),
        }
    }

    /// Hashes the given shader bytecode, registers it, and returns its ShaderInfo.
    pub fn register(
        &mut self,
        stage: ShaderStage,
        guest_address: u64,
        bytecode: &[u8],
    ) -> ShaderInfo {
        let hash = hash_fnv1a_64(bytecode);
        let info = ShaderInfo {
            hash,
            stage,
            guest_address,
            code_size: bytecode.len(),
        };
        self.shaders.insert(hash, info.clone());
        info
    }

    pub fn get(&self, hash: u64) -> Option<&ShaderInfo> {
        self.shaders.get(&hash)
    }

    pub fn len(&self) -> usize {
        self.shaders.len()
    }

    pub fn is_empty(&self) -> bool {
        self.shaders.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fnv1a_determinism() {
        let code1 = b"MOV R0, R1; FADD R2, R0, R3; EXIT;";
        let code2 = b"MOV R0, R1; FADD R2, R0, R3; EXIT;";
        let diff = b"MOV R0, R2; FADD R2, R0, R3; EXIT;";

        assert_eq!(hash_fnv1a_64(code1), hash_fnv1a_64(code2));
        assert_ne!(hash_fnv1a_64(code1), hash_fnv1a_64(diff));
    }

    #[test]
    fn test_murmur3_hashing() {
        let data = b"Sample Maxwell Shader Bytecode Payload";
        let h1 = hash_murmur3_64(data, 0);
        let h2 = hash_murmur3_64(data, 0);
        assert_ne!(h1, 0);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_shader_cache_registration() {
        let mut cache = ShaderCache::new();
        let vs_code = vec![0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0];
        let info = cache.register(ShaderStage::Vertex, 0x71001000, &vs_code);

        assert_eq!(info.stage, ShaderStage::Vertex);
        assert_eq!(info.guest_address, 0x71001000);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get(info.hash).unwrap().code_size, 8);
    }
}
