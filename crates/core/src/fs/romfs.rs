//! Read-Only Memory FileSystem (RomFS) reader and builder.
//!
//! RomFS packages application read-only assets into a hierarchical directory tree,
//! mounted by titles as `romfs:/` to load textures, audio, models, and scripts.

use crate::fs::{DirectoryEntry, EntryType, FileMode, FsError, IDirectory, IFile, IFileSystem};
use std::collections::HashMap;
use std::sync::Arc;

/// An entry in the RomFS virtual tree.
#[derive(Debug, Clone)]
enum RomfsNode {
    Directory {
        children: Vec<String>,
    },
    File {
        data: Arc<Vec<u8>>,
    },
}

/// A mounted RomFS filesystem.
pub struct RomfsFileSystem {
    nodes: HashMap<String, RomfsNode>,
}

impl RomfsFileSystem {
    pub fn new() -> Self {
        let mut nodes = HashMap::new();
        // Insert root directory
        nodes.insert(
            "/".to_string(),
            RomfsNode::Directory {
                children: Vec::new(),
            },
        );
        Self { nodes }
    }

    /// Normalizes path by guaranteeing leading slash and stripping trailing slashes.
    fn normalize_path(path: &str) -> String {
        let trimmed = path.trim().trim_end_matches('/');
        if trimmed.is_empty() || trimmed == "/" {
            "/".to_string()
        } else if !trimmed.starts_with('/') {
            format!("/{}", trimmed)
        } else {
            trimmed.to_string()
        }
    }

    /// Adds a file at the specified virtual path.
    pub fn insert_file(&mut self, path: &str, data: Vec<u8>) {
        let norm_path = Self::normalize_path(path);
        let parts: Vec<&str> = norm_path.split('/').filter(|p| !p.is_empty()).collect();

        // Ensure parent directories exist
        let mut cur_dir = String::new();
        for (i, part) in parts.iter().enumerate() {
            if i == parts.len() - 1 {
                // Leaf file
                let file_name = part.to_string();
                let parent_key = if cur_dir.is_empty() {
                    "/".to_string()
                } else {
                    cur_dir.clone()
                };

                if let Some(RomfsNode::Directory { children }) = self.nodes.get_mut(&parent_key) {
                    if !children.contains(&file_name) {
                        children.push(file_name);
                    }
                }

                self.nodes.insert(norm_path.clone(), RomfsNode::File { data: Arc::new(data) });
                break;
            }

            cur_dir = format!("{}/{}", cur_dir, part);
            let parent_key = if i == 0 {
                "/".to_string()
            } else {
                let p_parts = &parts[..i];
                format!("/{}", p_parts.join("/"))
            };

            if let Some(RomfsNode::Directory { children }) = self.nodes.get_mut(&parent_key) {
                let dir_name = part.to_string();
                if !children.contains(&dir_name) {
                    children.push(dir_name);
                }
            }

            self.nodes
                .entry(cur_dir.clone())
                .or_insert_with(|| RomfsNode::Directory {
                    children: Vec::new(),
                });
        }
    }
}

impl Default for RomfsFileSystem {
    fn default() -> Self {
        Self::new()
    }
}

/// In-memory file handle for RomFS files.
struct RomfsFileHandle {
    data: Arc<Vec<u8>>,
}

impl IFile for RomfsFileHandle {
    fn read(&self, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        if offset >= self.data.len() as u64 {
            return Ok(0);
        }
        let available = &self.data[offset as usize..];
        let to_copy = buf.len().min(available.len());
        buf[..to_copy].copy_from_slice(&available[..to_copy]);
        Ok(to_copy)
    }

    fn write(&mut self, _offset: u64, _buf: &[u8]) -> Result<usize, FsError> {
        Err(FsError::Io("RomFS is read-only".into()))
    }

    fn get_size(&self) -> Result<u64, FsError> {
        Ok(self.data.len() as u64)
    }

    fn set_size(&mut self, _size: u64) -> Result<(), FsError> {
        Err(FsError::Io("RomFS is read-only".into()))
    }
}

/// Directory reader for RomFS directories.
struct RomfsDirectory {
    entries: Vec<DirectoryEntry>,
}

impl IDirectory for RomfsDirectory {
    fn read_entries(&self) -> Result<Vec<DirectoryEntry>, FsError> {
        Ok(self.entries.clone())
    }

    fn entry_count(&self) -> Result<usize, FsError> {
        Ok(self.entries.len())
    }
}

impl IFileSystem for RomfsFileSystem {
    fn open_file(&self, path: &str, _mode: FileMode) -> Result<Box<dyn IFile>, FsError> {
        let norm_path = Self::normalize_path(path);
        match self.nodes.get(&norm_path) {
            Some(RomfsNode::File { data }) => Ok(Box::new(RomfsFileHandle { data: data.clone() })),
            Some(RomfsNode::Directory { .. }) => Err(FsError::NotAFile(path.to_string())),
            None => Err(FsError::NotFound(path.to_string())),
        }
    }

    fn open_directory(&self, path: &str) -> Result<Box<dyn IDirectory>, FsError> {
        let norm_path = Self::normalize_path(path);
        match self.nodes.get(&norm_path) {
            Some(RomfsNode::Directory { children }) => {
                let mut entries = Vec::new();
                for child in children {
                    let child_path = if norm_path == "/" {
                        format!("/{}", child)
                    } else {
                        format!("{}/{}", norm_path, child)
                    };

                    let (entry_type, file_size) = match self.nodes.get(&child_path) {
                        Some(RomfsNode::File { data }) => (EntryType::File, data.len() as u64),
                        Some(RomfsNode::Directory { .. }) => (EntryType::Directory, 0),
                        None => continue,
                    };

                    entries.push(DirectoryEntry {
                        name: child.clone(),
                        entry_type,
                        file_size,
                    });
                }
                Ok(Box::new(RomfsDirectory { entries }))
            }
            Some(RomfsNode::File { .. }) => Err(FsError::NotADirectory(path.to_string())),
            None => Err(FsError::NotFound(path.to_string())),
        }
    }

    fn get_entry_type(&self, path: &str) -> Result<EntryType, FsError> {
        let norm_path = Self::normalize_path(path);
        match self.nodes.get(&norm_path) {
            Some(RomfsNode::File { .. }) => Ok(EntryType::File),
            Some(RomfsNode::Directory { .. }) => Ok(EntryType::Directory),
            None => Err(FsError::NotFound(path.to_string())),
        }
    }
}

/// Builder helper to assemble a RomFS image.
pub struct RomfsBuilder {
    fs: RomfsFileSystem,
}

impl Default for RomfsBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl RomfsBuilder {
    pub fn new() -> Self {
        Self {
            fs: RomfsFileSystem::new(),
        }
    }

    pub fn add_file(&mut self, path: &str, data: Vec<u8>) -> &mut Self {
        self.fs.insert_file(path, data);
        self
    }

    pub fn build(self) -> RomfsFileSystem {
        self.fs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_romfs_tree_and_file_reading() {
        let mut builder = RomfsBuilder::new();
        builder
            .add_file("/audio/bgm.wav", b"AUDIO_WAV_BYTES".to_vec())
            .add_file("/shaders/main.glsl", b"SHADER_SOURCE".to_vec())
            .add_file("/config.json", b"{\"fps\": 60}".to_vec());

        let romfs = builder.build();

        // Check file exists
        assert!(romfs.file_exists("/audio/bgm.wav"));
        assert!(romfs.file_exists("/shaders/main.glsl"));
        assert!(romfs.file_exists("/config.json"));
        assert!(romfs.directory_exists("/audio"));
        assert!(romfs.directory_exists("/shaders"));
        assert!(!romfs.file_exists("/missing.txt"));

        // Read file contents
        let file = romfs.open_file("/audio/bgm.wav", FileMode::Read).unwrap();
        assert_eq!(file.get_size().unwrap(), 15);
        let mut buf = [0u8; 15];
        assert_eq!(file.read(0, &mut buf).unwrap(), 15);
        assert_eq!(&buf, b"AUDIO_WAV_BYTES");

        // Enumerate directory
        let dir = romfs.open_directory("/audio").unwrap();
        let entries = dir.read_entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "bgm.wav");
        assert_eq!(entries[0].entry_type, EntryType::File);
        assert_eq!(entries[0].file_size, 15);
    }
}
