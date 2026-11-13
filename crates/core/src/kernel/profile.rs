//! Game Profile Registry and Per-Title Configuration.
//!
//! Stores hardware quirks, target framerates, memory boundaries, and default
//! in-memory Codecave patches keyed by 64-bit Nintendo Title ID (e.g. BotW `01007EF00011E000`).

use crate::kernel::patch::MemoryPatch;
use std::collections::HashMap;

/// Configuration profile tailored for a specific game executable.
#[derive(Debug, Clone)]
pub struct GameProfile {
    pub title_id: u64,
    pub title_name: String,
    pub target_fps: u32,
    pub default_heap_size: u64,
    pub default_patches: Vec<MemoryPatch>,
    pub properties: HashMap<String, String>,
}

impl GameProfile {
    pub fn new(title_id: u64, title_name: impl Into<String>) -> Self {
        Self {
            title_id,
            title_name: title_name.into(),
            target_fps: 60,
            default_heap_size: 0x2000_0000, // 512 MB default heap
            default_patches: Vec::new(),
            properties: HashMap::new(),
        }
    }

    pub fn with_fps(mut self, fps: u32) -> Self {
        self.target_fps = fps;
        self
    }

    pub fn with_heap_size(mut self, size: u64) -> Self {
        self.default_heap_size = size;
        self
    }

    pub fn with_patch(mut self, patch: MemoryPatch) -> Self {
        self.default_patches.push(patch);
        self
    }

    pub fn with_property(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.properties.insert(key.into(), value.into());
        self
    }

    /// Formats the Title ID as a standardized 16-character uppercase hexadecimal string.
    pub fn title_id_hex(&self) -> String {
        format!("{:016X}", self.title_id)
    }
}

/// Registry mapping Nintendo Title IDs to their optimized execution profiles.
#[derive(Debug, Default, Clone)]
pub struct ProfileRegistry {
    profiles: HashMap<u64, GameProfile>,
}

impl ProfileRegistry {
    pub fn new() -> Self {
        Self {
            profiles: HashMap::new(),
        }
    }

    /// Pre-populates the registry with hardware configurations for flagship titles.
    pub fn with_known_titles() -> Self {
        let mut registry = Self::new();

        // The Legend of Zelda: Breath of the Wild (Title ID: 01007EF00011E000)
        let botw = GameProfile::new(0x01007EF00011E000, "The Legend of Zelda: Breath of the Wild")
            .with_fps(30)
            .with_heap_size(0x4000_0000) // 1 GB heap
            .with_property("GPUBufferCacheAccuracy", "High")
            .with_property("SyncInterval", "2");
        registry.register(botw);

        // Super Mario Odyssey (Title ID: 0100000000010000)
        let odyssey = GameProfile::new(0x0100000000010000, "Super Mario Odyssey")
            .with_fps(60)
            .with_heap_size(0x3000_0000)
            .with_property("GPUBufferCacheAccuracy", "Normal");
        registry.register(odyssey);

        // Super Smash Bros. Ultimate (Title ID: 01006A800016E000)
        let smash = GameProfile::new(0x01006A800016E000, "Super Smash Bros. Ultimate")
            .with_fps(60)
            .with_heap_size(0x4000_0000);
        registry.register(smash);

        // Animal Crossing: New Horizons (Title ID: 01006F8002326000)
        let acnh = GameProfile::new(0x01006F8002326000, "Animal Crossing: New Horizons")
            .with_fps(30)
            .with_heap_size(0x3000_0000);
        registry.register(acnh);

        // Mario Kart 8 Deluxe (Title ID: 0100152000022000)
        let mk8d = GameProfile::new(0x0100152000022000, "Mario Kart 8 Deluxe")
            .with_fps(60)
            .with_heap_size(0x2800_0000);
        registry.register(mk8d);

        registry
    }

    pub fn register(&mut self, profile: GameProfile) {
        self.profiles.insert(profile.title_id, profile);
    }

    pub fn get(&self, title_id: u64) -> Option<&GameProfile> {
        self.profiles.get(&title_id)
    }

    /// Retrieves the profile for `title_id` if known, or creates a standard default profile.
    pub fn get_or_default(&self, title_id: u64) -> GameProfile {
        self.profiles
            .get(&title_id)
            .cloned()
            .unwrap_or_else(|| GameProfile::new(title_id, format!("Unknown Title 0x{:016X}", title_id)))
    }

    pub fn len(&self) -> usize {
        self.profiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_profile_registry_known_titles() {
        let registry = ProfileRegistry::with_known_titles();
        let botw = registry.get(0x01007EF00011E000).expect("BotW profile missing");

        assert_eq!(botw.title_name, "The Legend of Zelda: Breath of the Wild");
        assert_eq!(botw.title_id_hex(), "01007EF00011E000");
        assert_eq!(botw.target_fps, 30);
        assert_eq!(botw.default_heap_size, 0x4000_0000);
        assert_eq!(botw.properties.get("GPUBufferCacheAccuracy").unwrap(), "High");
    }

    #[test]
    fn test_profile_registry_fallback() {
        let registry = ProfileRegistry::with_known_titles();
        let custom = registry.get_or_default(0x0123456789ABCDEF);

        assert_eq!(custom.title_id, 0x0123456789ABCDEF);
        assert_eq!(custom.target_fps, 60);
        assert_eq!(custom.default_heap_size, 0x2000_0000);
    }
}
