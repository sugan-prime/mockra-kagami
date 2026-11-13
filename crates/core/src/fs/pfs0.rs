//! Partition FileSystem (PFS0 / HFS0 / NSP) Archive Parser and Extractor.
//!
//! PFS0 is the standard packaging container for Nintendo Switch executables (NSP)
//! and game card partitions (HFS0 / XCI).

use crate::fs::{DirectoryEntry, EntryType, FileMode, FsError, IDirectory, IFile, IFileSystem};
use std::collections::HashMap;
use std::sync::Arc;

pub const PFS0_MAGIC: [u8; 4] = *b"PFS0";
pub const HFS0_MAGIC: [u8; 4] = *b"HFS0";

/// Header descriptor of a PFS0 partition archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pfs0Header {
    pub magic: [u8; 4],
    pub file_count: u32,
    pub string_table_size: u32,
}

/// Metadata and location for a file contained within a PFS0 archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pfs0File {
    pub name: String,
    pub offset: u64,
    pub size: u64,
}

/// An open in-memory PFS0 archive implementing the `IFileSystem` interface.
pub struct Pfs0Archive {
    data: Arc<Vec<u8>>,
    header: Pfs0Header,
    files: Vec<Pfs0File>,
    file_map: HashMap<String, usize>,
    data_base_offset: usize,
}

impl Pfs0Archive {
    /// Parses an in-memory byte slice containing a valid PFS0 or HFS0 partition.
    pub fn parse(bytes: &[u8]) -> Result<Self, FsError> {
        if bytes.len() < 16 {
            return Err(FsError::Pfs0("Data smaller than PFS0 header".into()));
        }

        let magic = [bytes[0], bytes[1], bytes[2], bytes[3]];
        if magic != PFS0_MAGIC && magic != HFS0_MAGIC {
            return Err(FsError::Pfs0("Invalid magic, expected PFS0 or HFS0".into()));
        }

        let file_count = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let string_table_size = u32::from_le_bytes(bytes[8..12].try_into().unwrap());

        let header = Pfs0Header {
            magic,
            file_count,
            string_table_size,
        };

        let file_table_size = (file_count as usize) * 24;
        let string_table_start = 16 + file_table_size;
        let string_table_end = string_table_start + string_table_size as usize;
        let data_base_offset = string_table_end;

        if bytes.len() < string_table_end {
            return Err(FsError::Pfs0("Unexpected EOF reading PFS0 string table".into()));
        }

        let mut files = Vec::with_capacity(file_count as usize);
        let mut file_map = HashMap::with_capacity(file_count as usize);

        for i in 0..file_count as usize {
            let entry_offset = 16 + (i * 24);
            let offset = u64::from_le_bytes(bytes[entry_offset..entry_offset + 8].try_into().unwrap());
            let size = u64::from_le_bytes(bytes[entry_offset + 8..entry_offset + 16].try_into().unwrap());
            let name_offset = u32::from_le_bytes(bytes[entry_offset + 16..entry_offset + 20].try_into().unwrap()) as usize;

            let name_start = string_table_start + name_offset;
            let name_bytes: Vec<u8> = bytes[name_start..string_table_end]
                .iter()
                .take_while(|&&b| b != 0)
                .copied()
                .collect();

            let name = String::from_utf8(name_bytes)
                .map_err(|e| FsError::Pfs0(format!("Invalid UTF-8 file name: {}", e)))?;

            let file_entry = Pfs0File { name: name.clone(), offset, size };
            files.push(file_entry);
            file_map.insert(name, i);
        }

        Ok(Self {
            data: Arc::new(bytes.to_vec()),
            header,
            files,
            file_map,
            data_base_offset,
        })
    }

    pub fn files(&self) -> &[Pfs0File] {
        &self.files
    }

    pub fn header(&self) -> &Pfs0Header {
        &self.header
    }

    pub fn get_file_data(&self, name: &str) -> Option<&[u8]> {
        let clean = name.trim_start_matches('/');
        if let Some(&idx) = self.file_map.get(clean) {
            let entry = &self.files[idx];
            let start = self.data_base_offset + entry.offset as usize;
            let end = start + entry.size as usize;
            if end <= self.data.len() {
                return Some(&self.data[start..end]);
            }
        }
        None
    }
}

/// In-memory IFile view into a PFS0 file.
struct Pfs0FileHandle {
    data: Arc<Vec<u8>>,
    start: usize,
    size: usize,
}

impl IFile for Pfs0FileHandle {
    fn read(&self, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        if offset >= self.size as u64 {
            return Ok(0);
        }
        let file_start = self.start + offset as usize;
        let file_end = self.start + self.size;
        let available = &self.data[file_start..file_end];
        let to_copy = buf.len().min(available.len());
        buf[..to_copy].copy_from_slice(&available[..to_copy]);
        Ok(to_copy)
    }

    fn write(&mut self, _offset: u64, _buf: &[u8]) -> Result<usize, FsError> {
        Err(FsError::Io("PFS0 is read-only".into()))
    }

    fn get_size(&self) -> Result<u64, FsError> {
        Ok(self.size as u64)
    }

    fn set_size(&mut self, _size: u64) -> Result<(), FsError> {
        Err(FsError::Io("PFS0 is read-only".into()))
    }
}

/// Directory reader for PFS0 root.
struct Pfs0Directory {
    entries: Vec<DirectoryEntry>,
}

impl IDirectory for Pfs0Directory {
    fn read_entries(&self) -> Result<Vec<DirectoryEntry>, FsError> {
        Ok(self.entries.clone())
    }

    fn entry_count(&self) -> Result<usize, FsError> {
        Ok(self.entries.len())
    }
}

impl IFileSystem for Pfs0Archive {
    fn open_file(&self, path: &str, _mode: FileMode) -> Result<Box<dyn IFile>, FsError> {
        let clean = path.trim_start_matches('/');
        if let Some(&idx) = self.file_map.get(clean) {
            let entry = &self.files[idx];
            let start = self.data_base_offset + entry.offset as usize;
            Ok(Box::new(Pfs0FileHandle {
                data: self.data.clone(),
                start,
                size: entry.size as usize,
            }))
        } else {
            Err(FsError::NotFound(path.to_string()))
        }
    }

    fn open_directory(&self, path: &str) -> Result<Box<dyn IDirectory>, FsError> {
        let clean = path.trim_matches('/');
        if clean.is_empty() {
            let entries = self
                .files
                .iter()
                .map(|f| DirectoryEntry {
                    name: f.name.clone(),
                    entry_type: EntryType::File,
                    file_size: f.size,
                })
                .collect();
            Ok(Box::new(Pfs0Directory { entries }))
        } else {
            Err(FsError::NotFound(path.to_string()))
        }
    }

    fn get_entry_type(&self, path: &str) -> Result<EntryType, FsError> {
        let clean = path.trim_matches('/');
        if clean.is_empty() {
            Ok(EntryType::Directory)
        } else if self.file_map.contains_key(clean) {
            Ok(EntryType::File)
        } else {
            Err(FsError::NotFound(path.to_string()))
        }
    }
}

/// Helper builder for synthesizing PFS0 / NSP archives for automated testing.
pub struct Pfs0Builder {
    files: Vec<(String, Vec<u8>)>,
}

impl Default for Pfs0Builder {
    fn default() -> Self {
        Self::new()
    }
}

impl Pfs0Builder {
    pub fn new() -> Self {
        Self { files: Vec::new() }
    }

    pub fn add_file(&mut self, name: impl Into<String>, data: Vec<u8>) {
        self.files.push((name.into(), data));
    }

    pub fn build(&self) -> Vec<u8> {
        let file_count = self.files.len() as u32;

        // Build string table
        let mut string_table = Vec::new();
        let mut name_offsets = Vec::new();

        for (name, _) in &self.files {
            name_offsets.push(string_table.len() as u32);
            string_table.extend_from_slice(name.as_bytes());
            string_table.push(0); // null terminator
        }

        let string_table_size = string_table.len() as u32;

        let mut output = Vec::new();
        // Header
        output.extend_from_slice(&PFS0_MAGIC);
        output.extend_from_slice(&file_count.to_le_bytes());
        output.extend_from_slice(&string_table_size.to_le_bytes());
        output.extend_from_slice(&0u32.to_le_bytes()); // reserved

        // File headers
        let mut current_data_offset = 0u64;
        for (i, (_, data)) in self.files.iter().enumerate() {
            let data_size = data.len() as u64;
            output.extend_from_slice(&current_data_offset.to_le_bytes());
            output.extend_from_slice(&data_size.to_le_bytes());
            output.extend_from_slice(&name_offsets[i].to_le_bytes());
            output.extend_from_slice(&0u32.to_le_bytes()); // reserved
            current_data_offset += data_size;
        }

        // String table
        output.extend_from_slice(&string_table);

        // File data payloads
        for (_, data) in &self.files {
            output.extend_from_slice(data);
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pfs0_builder_and_parse() {
        let mut builder = Pfs0Builder::new();
        builder.add_file("main", b"ARM64_CODE_PAYLOAD".to_vec());
        builder.add_file("main.npdm", b"NPDM_METADATA".to_vec());
        builder.add_file("control.nacp", b"NACP_TITLE_INFO".to_vec());

        let archive_bytes = builder.build();
        let archive = Pfs0Archive::parse(&archive_bytes).unwrap();

        assert_eq!(archive.files().len(), 3);
        assert_eq!(archive.get_file_data("main").unwrap(), b"ARM64_CODE_PAYLOAD");
        assert_eq!(archive.get_file_data("main.npdm").unwrap(), b"NPDM_METADATA");
        assert_eq!(archive.get_file_data("control.nacp").unwrap(), b"NACP_TITLE_INFO");
        assert!(archive.get_file_data("non_existent").is_none());

        // Test VFS IFileSystem interface
        let mut buf = [0u8; 18];
        let file = archive.open_file("main", FileMode::Read).unwrap();
        assert_eq!(file.get_size().unwrap(), 18);
        assert_eq!(file.read(0, &mut buf).unwrap(), 18);
        assert_eq!(&buf, b"ARM64_CODE_PAYLOAD");
    }
}
