//! Nintendo Content Archive (NCA3) container parser and extractor.
//!
//! NCA is the primary file container format used for Nintendo Switch commercial software,
//! firmware titles, game updates, and downloadable content (DLC).
//! Encapsulates ExeFS (PFS0) executable code and RomFS game assets across up to 4 partition sections.

use std::sync::Arc;
use thiserror::Error;

pub const NCA_HEADER_SIZE: usize = 0x400; // 1024 bytes (Header + Magic at 0x200)
pub const NCA_MAGIC_OFFSET: usize = 0x200;
pub const NCA3_MAGIC: [u8; 4] = *b"NCA3";
pub const MEDIA_UNIT_SIZE: usize = 0x200; // 512 bytes

#[derive(Error, Debug, PartialEq, Eq)]
pub enum NcaError {
    #[error("NCA buffer too small: {0} bytes (minimum {1})")]
    BufferTooSmall(usize, usize),
    #[error("Invalid NCA magic, expected 'NCA3'")]
    InvalidMagic,
    #[error("Section index {0} out of bounds (max 4)")]
    SectionOutOfBounds(usize),
    #[error("Section {0} is disabled or empty")]
    SectionDisabled(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NcaContentType {
    Program = 0,
    Meta = 1,
    Control = 2,
    Manual = 3,
    Data = 4,
    PublicData = 5,
}

impl NcaContentType {
    pub fn from_u8(val: u8) -> Self {
        match val {
            0 => Self::Program,
            1 => Self::Meta,
            2 => Self::Control,
            3 => Self::Manual,
            4 => Self::Data,
            _ => Self::PublicData,
        }
    }
}

/// A section table entry describing physical offset within the NCA container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NcaSectionEntry {
    pub media_start_offset: u32,
    pub media_end_offset: u32,
    pub enabled: bool,
}

impl NcaSectionEntry {
    pub fn start_byte_offset(&self) -> usize {
        (self.media_start_offset as usize) * MEDIA_UNIT_SIZE
    }

    pub fn size_bytes(&self) -> usize {
        if self.media_end_offset > self.media_start_offset {
            (self.media_end_offset - self.media_start_offset) as usize * MEDIA_UNIT_SIZE
        } else {
            0
        }
    }
}

/// An opened Nintendo Content Archive (NCA3).
#[derive(Clone)]
pub struct NcaArchive {
    data: Arc<Vec<u8>>,
    pub content_type: NcaContentType,
    pub title_id: u64,
    pub content_size: u64,
    pub sections: [NcaSectionEntry; 4],
}

impl NcaArchive {
    /// Parses an NCA3 archive from a byte slice.
    pub fn parse(bytes: &[u8]) -> Result<Self, NcaError> {
        if bytes.len() < NCA_HEADER_SIZE {
            return Err(NcaError::BufferTooSmall(bytes.len(), NCA_HEADER_SIZE));
        }

        if &bytes[NCA_MAGIC_OFFSET..NCA_MAGIC_OFFSET + 4] != &NCA3_MAGIC {
            return Err(NcaError::InvalidMagic);
        }

        let content_type = NcaContentType::from_u8(bytes[0x205]);
        let content_size = u64::from_le_bytes(bytes[0x208..0x210].try_into().unwrap());
        let title_id = u64::from_le_bytes(bytes[0x210..0x218].try_into().unwrap());

        let mut sections = [NcaSectionEntry {
            media_start_offset: 0,
            media_end_offset: 0,
            enabled: false,
        }; 4];

        // Read 4 section entries from offset 0x240
        for (i, slot) in sections.iter_mut().enumerate() {
            let offset = 0x240 + i * 0x10;
            let start = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
            let end = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap());
            let enabled = start != 0 || end != 0;

            *slot = NcaSectionEntry {
                media_start_offset: start,
                media_end_offset: end,
                enabled,
            };
        }

        Ok(Self {
            data: Arc::new(bytes.to_vec()),
            content_type,
            title_id,
            content_size,
            sections,
        })
    }

    /// Reads raw partition section bytes (e.g. ExeFS PFS0 or RomFS).
    pub fn read_section(&self, section_idx: usize) -> Result<Vec<u8>, NcaError> {
        if section_idx >= 4 {
            return Err(NcaError::SectionOutOfBounds(section_idx));
        }

        let section = &self.sections[section_idx];
        if !section.enabled {
            return Err(NcaError::SectionDisabled(section_idx));
        }

        let start = section.start_byte_offset();
        let size = section.size_bytes();

        if start + size > self.data.len() {
            return Err(NcaError::BufferTooSmall(self.data.len(), start + size));
        }

        Ok(self.data[start..start + size].to_vec())
    }

    pub fn is_program(&self) -> bool {
        self.content_type == NcaContentType::Program
    }

    pub fn is_control(&self) -> bool {
        self.content_type == NcaContentType::Control
    }
}

/// Builder for constructing valid NCA3 archives (useful for testing and package generation).
pub struct NcaBuilder {
    title_id: u64,
    content_type: NcaContentType,
    sections: Vec<Vec<u8>>,
}

impl Default for NcaBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl NcaBuilder {
    pub fn new() -> Self {
        Self {
            title_id: 0x01007EF00011E000, // Default to BotW Title ID
            content_type: NcaContentType::Program,
            sections: Vec::new(),
        }
    }

    pub fn set_title_id(&mut self, title_id: u64) -> &mut Self {
        self.title_id = title_id;
        self
    }

    pub fn set_content_type(&mut self, content_type: NcaContentType) -> &mut Self {
        self.content_type = content_type;
        self
    }

    pub fn add_section(&mut self, section_data: &[u8]) -> &mut Self {
        // Pad to MEDIA_UNIT_SIZE (512 bytes)
        let pad_len = (MEDIA_UNIT_SIZE - (section_data.len() % MEDIA_UNIT_SIZE)) % MEDIA_UNIT_SIZE;
        let mut padded = section_data.to_vec();
        padded.extend(std::iter::repeat(0).take(pad_len));

        self.sections.push(padded);
        self
    }

    pub fn build(&self) -> Vec<u8> {
        let mut nca = vec![0u8; 0xC00]; // 0xC00 header block (0x400 header + 4 * 0x200 section headers)

        // Write "NCA3" magic at offset 0x200
        nca[0x200..0x204].copy_from_slice(&NCA3_MAGIC);
        nca[0x204] = 0; // Distribution type: System
        nca[0x205] = self.content_type as u8;
        nca[0x210..0x218].copy_from_slice(&self.title_id.to_le_bytes());

        let mut current_media_offset = (nca.len() / MEDIA_UNIT_SIZE) as u32;

        for (i, sec_data) in self.sections.iter().take(4).enumerate() {
            let start_offset = current_media_offset;
            let sec_units = (sec_data.len() / MEDIA_UNIT_SIZE) as u32;
            let end_offset = start_offset + sec_units;

            let entry_offset = 0x240 + i * 0x10;
            nca[entry_offset..entry_offset + 4].copy_from_slice(&start_offset.to_le_bytes());
            nca[entry_offset + 4..entry_offset + 8].copy_from_slice(&end_offset.to_le_bytes());

            current_media_offset = end_offset;
        }

        // Append section data bodies
        for sec_data in &self.sections {
            nca.extend_from_slice(sec_data);
        }

        let total_size = nca.len() as u64;
        nca[0x208..0x210].copy_from_slice(&total_size.to_le_bytes());

        nca
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nca_builder_and_parse() {
        let mut builder = NcaBuilder::new();
        builder.set_title_id(0x01007EF00011E000);
        builder.set_content_type(NcaContentType::Program);

        // Section 0: ExeFS payload
        let sec0_data = b"ExeFS Main Binary Content Mock";
        builder.add_section(sec0_data);

        // Section 1: RomFS payload
        let sec1_data = b"RomFS Assets Archive Mock";
        builder.add_section(sec1_data);

        let nca_bytes = builder.build();
        assert!(nca_bytes.len() >= 0xC00);

        let nca = NcaArchive::parse(&nca_bytes).unwrap();
        assert_eq!(nca.title_id, 0x01007EF00011E000);
        assert_eq!(nca.content_type, NcaContentType::Program);
        assert!(nca.is_program());
        assert!(!nca.is_control());

        // Extract Section 0
        let extracted_sec0 = nca.read_section(0).unwrap();
        assert!(extracted_sec0.starts_with(sec0_data));

        // Extract Section 1
        let extracted_sec1 = nca.read_section(1).unwrap();
        assert!(extracted_sec1.starts_with(sec1_data));

        // Section 2 is disabled
        assert_eq!(nca.read_section(2), Err(NcaError::SectionDisabled(2)));
    }
}
