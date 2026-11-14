//! Host FileSystem mount for Save Data (`save:/`) and SD Card (`sdmc:/`).
//!
//! Provides full read-write filesystem access sandboxed within a host folder.

use crate::fs::{DirectoryEntry, EntryType, FileMode, FsError, IDirectory, IFile, IFileSystem};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub struct HostFileSystem {
    root_path: PathBuf,
}

impl HostFileSystem {
    pub fn new(root_path: impl AsRef<Path>) -> Result<Self, FsError> {
        let root = root_path.as_ref().to_path_buf();
        fs::create_dir_all(&root).map_err(|e| FsError::Io(e.to_string()))?;
        Ok(Self { root_path: root })
    }

    /// Resolves and sandboxes a virtual path within the host directory root.
    fn resolve_path(&self, virtual_path: &str) -> Result<PathBuf, FsError> {
        let clean = virtual_path.trim_start_matches('/');
        // Prevent path traversal
        if clean.contains("..") {
            return Err(FsError::InvalidPath("Path traversal prohibited".into()));
        }
        Ok(self.root_path.join(clean))
    }
}

pub struct HostFile {
    file: Mutex<File>,
}

impl IFile for HostFile {
    fn read(&self, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        let mut file = self.file.lock().unwrap();
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| FsError::Io(e.to_string()))?;
        file.read(buf).map_err(|e| FsError::Io(e.to_string()))
    }

    fn write(&mut self, offset: u64, buf: &[u8]) -> Result<usize, FsError> {
        let mut file = self.file.lock().unwrap();
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| FsError::Io(e.to_string()))?;
        file.write_all(buf)
            .map_err(|e| FsError::Io(e.to_string()))?;
        Ok(buf.len())
    }

    fn get_size(&self) -> Result<u64, FsError> {
        let file = self.file.lock().unwrap();
        file.metadata()
            .map(|m| m.len())
            .map_err(|e| FsError::Io(e.to_string()))
    }

    fn set_size(&mut self, size: u64) -> Result<(), FsError> {
        let file = self.file.lock().unwrap();
        file.set_len(size)
            .map_err(|e| FsError::Io(e.to_string()))
    }
}

pub struct HostDirectory {
    entries: Vec<DirectoryEntry>,
}

impl IDirectory for HostDirectory {
    fn read_entries(&self) -> Result<Vec<DirectoryEntry>, FsError> {
        Ok(self.entries.clone())
    }

    fn entry_count(&self) -> Result<usize, FsError> {
        Ok(self.entries.len())
    }
}

impl IFileSystem for HostFileSystem {
    fn open_file(&self, path: &str, mode: FileMode) -> Result<Box<dyn IFile>, FsError> {
        let resolved = self.resolve_path(path)?;

        let mut options = OpenOptions::new();
        match mode {
            FileMode::Read => {
                options.read(true);
            }
            FileMode::Write => {
                options.write(true).create(true).truncate(false);
            }
            FileMode::ReadWrite => {
                options.read(true).write(true).create(true);
            }
        }

        // Ensure parent folder exists if writing
        if let Some(parent) = resolved.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let file = options.open(&resolved).map_err(|e| FsError::NotFound(e.to_string()))?;
        Ok(Box::new(HostFile {
            file: Mutex::new(file),
        }))
    }

    fn open_directory(&self, path: &str) -> Result<Box<dyn IDirectory>, FsError> {
        let resolved = self.resolve_path(path)?;
        if !resolved.is_dir() {
            return Err(FsError::NotADirectory(path.to_string()));
        }

        let read_dir = fs::read_dir(&resolved).map_err(|e| FsError::Io(e.to_string()))?;
        let mut entries = Vec::new();

        for item in read_dir.flatten() {
            let metadata = item.metadata().ok();
            let is_dir = metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);
            let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);

            entries.push(DirectoryEntry {
                name: item.file_name().to_string_lossy().to_string(),
                entry_type: if is_dir { EntryType::Directory } else { EntryType::File },
                file_size: size,
            });
        }

        Ok(Box::new(HostDirectory { entries }))
    }

    fn get_entry_type(&self, path: &str) -> Result<EntryType, FsError> {
        let resolved = self.resolve_path(path)?;
        let metadata = fs::metadata(resolved).map_err(|_| FsError::NotFound(path.to_string()))?;
        if metadata.is_dir() {
            Ok(EntryType::Directory)
        } else {
            Ok(EntryType::File)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_host_filesystem_read_write() {
        let temp_dir = env::temp_dir().join("mockra_kagami_save_test");
        let fs = HostFileSystem::new(&temp_dir).unwrap();

        let test_file = "game_save.dat";
        let test_data = b"ZELDA_BOTW_SAVE_STATE_DATA_SLOT_0";

        // 1. Write save data
        {
            let mut file = fs.open_file(test_file, FileMode::Write).unwrap();
            let written = file.write(0, test_data).unwrap();
            assert_eq!(written, test_data.len());
        }

        // 2. Read back
        {
            let file = fs.open_file(test_file, FileMode::Read).unwrap();
            assert_eq!(file.get_size().unwrap(), test_data.len() as u64);
            let mut read_buf = vec![0u8; test_data.len()];
            let bytes_read = file.read(0, &mut read_buf).unwrap();
            assert_eq!(bytes_read, test_data.len());
            assert_eq!(&read_buf, test_data);
        }

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }
}
