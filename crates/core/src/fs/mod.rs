//! Virtual File System (VFS) and Horizon OS Storage Abstractions.

pub mod fsp_srv;
pub mod host;
pub mod pfs0;
pub mod romfs;

pub use fsp_srv::FileSystemProxyService;
pub use host::HostFileSystem;
pub use pfs0::{Pfs0Archive, Pfs0Builder, Pfs0Header};
pub use romfs::{RomfsBuilder, RomfsFileSystem};

use thiserror::Error;

#[derive(Error, Debug, PartialEq, Eq)]
pub enum FsError {
    #[error("File or directory not found: {0}")]
    NotFound(String),
    #[error("Path already exists: {0}")]
    AlreadyExists(String),
    #[error("Not a directory: {0}")]
    NotADirectory(String),
    #[error("Not a file: {0}")]
    NotAFile(String),
    #[error("Invalid path: {0}")]
    InvalidPath(String),
    #[error("I/O error: {0}")]
    Io(String),
    #[error("PFS0 format error: {0}")]
    Pfs0(String),
    #[error("RomFS format error: {0}")]
    Romfs(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryType {
    Directory,
    File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileMode {
    Read,
    Write,
    ReadWrite,
}

/// Abstract file handle interface.
pub trait IFile: Send + Sync {
    fn read(&self, offset: u64, buf: &mut [u8]) -> Result<usize, FsError>;
    fn write(&mut self, offset: u64, buf: &[u8]) -> Result<usize, FsError>;
    fn get_size(&self) -> Result<u64, FsError>;
    fn set_size(&mut self, size: u64) -> Result<(), FsError>;
}

/// Directory entry description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub name: String,
    pub entry_type: EntryType,
    pub file_size: u64,
}

/// Abstract directory enumerator interface.
pub trait IDirectory: Send + Sync {
    fn read_entries(&self) -> Result<Vec<DirectoryEntry>, FsError>;
    fn entry_count(&self) -> Result<usize, FsError>;
}

/// Virtual File System interface mounted by Horizon OS services.
pub trait IFileSystem: Send + Sync {
    fn open_file(&self, path: &str, mode: FileMode) -> Result<Box<dyn IFile>, FsError>;
    fn open_directory(&self, path: &str) -> Result<Box<dyn IDirectory>, FsError>;
    fn get_entry_type(&self, path: &str) -> Result<EntryType, FsError>;

    fn file_exists(&self, path: &str) -> bool {
        self.get_entry_type(path).map(|t| t == EntryType::File).unwrap_or(false)
    }

    fn directory_exists(&self, path: &str) -> bool {
        self.get_entry_type(path).map(|t| t == EntryType::Directory).unwrap_or(false)
    }
}
