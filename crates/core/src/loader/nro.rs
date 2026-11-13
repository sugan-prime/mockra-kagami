//! Nintendo Relocatable Object (NRO) parser and segment loader.

use crate::memory::bus::MemoryError;
use crate::memory::system_bus::SystemBus;
use std::fs;
use std::path::Path;
use thiserror::Error;

pub const NRO_MAGIC: [u8; 4] = *b"NRO0";
pub const MOD0_MAGIC: [u8; 4] = *b"MOD0";

#[derive(Error, Debug)]
pub enum NroError {
    #[error("I/O error reading NRO: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid NRO magic, expected 'NRO0'")]
    InvalidMagic,
    #[error("File size is smaller than NRO header: {0} bytes")]
    FileTooSmall(usize),
    #[error("Invalid MOD0 magic, expected 'MOD0'")]
    InvalidMod0Magic,
    #[error("Memory bus error during loading: {0}")]
    Memory(#[from] MemoryError),
}

#[derive(Debug, Clone, Copy)]
pub struct SegmentHeader {
    pub file_offset: u32,
    pub size: u32,
}

#[derive(Debug, Clone)]
pub struct NroHeader {
    pub magic: [u8; 4],
    pub version: u32,
    pub size: u32,
    pub flags: u32,
    pub text: SegmentHeader,
    pub rodata: SegmentHeader,
    pub data: SegmentHeader,
    pub bss_size: u32,
    pub build_id: [u8; 32],
}

impl NroHeader {
    pub fn parse(bytes: &[u8]) -> Result<Self, NroError> {
        if bytes.len() < 0x80 {
            return Err(NroError::FileTooSmall(bytes.len()));
        }

        // Magic is at offset 0x10
        let magic = [bytes[0x10], bytes[0x11], bytes[0x12], bytes[0x13]];
        if magic != NRO_MAGIC {
            return Err(NroError::InvalidMagic);
        }

        let version = u32::from_le_bytes(bytes[0x14..0x18].try_into().unwrap());
        let size = u32::from_le_bytes(bytes[0x18..0x1C].try_into().unwrap());
        let flags = u32::from_le_bytes(bytes[0x1C..0x20].try_into().unwrap());

        let text = SegmentHeader {
            file_offset: u32::from_le_bytes(bytes[0x20..0x24].try_into().unwrap()),
            size: u32::from_le_bytes(bytes[0x24..0x28].try_into().unwrap()),
        };
        let rodata = SegmentHeader {
            file_offset: u32::from_le_bytes(bytes[0x28..0x2C].try_into().unwrap()),
            size: u32::from_le_bytes(bytes[0x2C..0x30].try_into().unwrap()),
        };
        let data = SegmentHeader {
            file_offset: u32::from_le_bytes(bytes[0x30..0x34].try_into().unwrap()),
            size: u32::from_le_bytes(bytes[0x34..0x38].try_into().unwrap()),
        };
        let bss_size = u32::from_le_bytes(bytes[0x38..0x3C].try_into().unwrap());

        let mut build_id = [0u8; 32];
        build_id.copy_from_slice(&bytes[0x40..0x60]);

        Ok(Self {
            magic,
            version,
            size,
            flags,
            text,
            rodata,
            data,
            bss_size,
            build_id,
        })
    }
}

/// Loads an NRO executable into the system bus at `load_base` and applies dynamic relocations.
pub fn load_nro(
    bus: &mut SystemBus,
    path: impl AsRef<Path>,
    load_base: u64,
) -> Result<NroHeader, NroError> {
    let bytes = fs::read(path)?;
    load_nro_from_bytes(bus, &bytes, load_base)
}

/// Loads an NRO executable from an in-memory byte slice.
pub fn load_nro_from_bytes(
    bus: &mut SystemBus,
    bytes: &[u8],
    load_base: u64,
) -> Result<NroHeader, NroError> {
    let header = NroHeader::parse(bytes)?;

    // 1. Copy .text segment into bus
    let text_start = header.text.file_offset as usize;
    let text_end = text_start + header.text.size as usize;
    bus.load_bytes(load_base + header.text.file_offset as u64, &bytes[text_start..text_end])?;

    // 2. Copy .rodata segment into bus
    let ro_start = header.rodata.file_offset as usize;
    let ro_end = ro_start + header.rodata.size as usize;
    bus.load_bytes(load_base + header.rodata.file_offset as u64, &bytes[ro_start..ro_end])?;

    // 3. Copy .data segment into bus
    let data_start = header.data.file_offset as usize;
    let data_end = data_start + header.data.size as usize;
    bus.load_bytes(load_base + header.data.file_offset as u64, &bytes[data_start..data_end])?;

    // 4. Zero out .bss segment
    let bss_addr = load_base + header.data.file_offset as u64 + header.data.size as u64;
    let bss_zeros = vec![0u8; header.bss_size as usize];
    if !bss_zeros.is_empty() {
        bus.load_bytes(bss_addr, &bss_zeros)?;
    }

    // 5. Parse MOD0 and apply ELF dynamic relocations if present
    apply_mod0_relocations(bus, bytes, &header, load_base)?;

    Ok(header)
}

/// Reads the MOD0 header and performs R_AARCH64_RELATIVE relocations.
fn apply_mod0_relocations(
    bus: &mut SystemBus,
    bytes: &[u8],
    header: &NroHeader,
    load_base: u64,
) -> Result<(), NroError> {
    let text_offset = header.text.file_offset as usize;
    if bytes.len() < text_offset + 8 {
        return Ok(());
    }

    // Word 1 of .text holds the relative offset to MOD0
    let mod0_rel_offset = u32::from_le_bytes(bytes[text_offset + 4..text_offset + 8].try_into().unwrap()) as usize;
    let mod0_pos = text_offset + mod0_rel_offset;

    if bytes.len() < mod0_pos + 0x1C {
        return Ok(()); // No valid MOD0 present, skip
    }

    // Check MOD0 magic
    if &bytes[mod0_pos..mod0_pos + 4] != MOD0_MAGIC {
        return Ok(()); // Non-relocatable or custom binary
    }

    // Read dynamic offset (relative to MOD0 header position)
    let dynamic_rel_offset = i32::from_le_bytes(bytes[mod0_pos + 4..mod0_pos + 8].try_into().unwrap());
    let dynamic_pos = (mod0_pos as isize + dynamic_rel_offset as isize) as usize;

    if dynamic_pos >= bytes.len() {
        return Ok(());
    }

    // Scan ELF _DYNAMIC tags
    let mut rela_offset: Option<u64> = None;
    let mut rela_size: Option<u64> = None;
    let mut cur = dynamic_pos;

    while cur + 16 <= bytes.len() {
        let d_tag = i64::from_le_bytes(bytes[cur..cur + 8].try_into().unwrap());
        let d_val = u64::from_le_bytes(bytes[cur + 8..cur + 16].try_into().unwrap());
        cur += 16;

        match d_tag {
            0 => break, // DT_NULL
            7 => rela_offset = Some(d_val), // DT_RELA
            8 => rela_size = Some(d_val),   // DT_RELASZ
            _ => {}
        }
    }

    // Apply relocations
    if let (Some(offset), Some(size)) = (rela_offset, rela_size) {
        let mut entry_addr = load_base + offset;
        let entry_end = entry_addr + size;

        // Each Elf64_Rela is 24 bytes: r_offset (u64), r_info (u64), r_addend (i64)
        use crate::memory::bus::MemoryBus;
        while entry_addr + 24 <= entry_end {
            let r_offset = bus.read_u64(entry_addr)?;
            let r_info = bus.read_u64(entry_addr + 8)?;
            let r_addend = bus.read_u64(entry_addr + 16)? as i64;

            let r_type = (r_info & 0xFFFF_FFFF) as u32;
            const R_AARCH64_RELATIVE: u32 = 1027; // 0x403

            if r_type == R_AARCH64_RELATIVE {
                let target_addr = load_base + r_offset;
                let relocated_val = (load_base as i64 + r_addend) as u64;
                bus.write_u64(target_addr, relocated_val)?;
            }

            entry_addr += 24;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nro_header_parse_invalid() {
        let small = vec![0u8; 10];
        assert!(matches!(NroHeader::parse(&small), Err(NroError::FileTooSmall(_))));

        let mut invalid_magic = vec![0u8; 0x80];
        invalid_magic[0x10..0x14].copy_from_slice(b"NOPE");
        assert!(matches!(NroHeader::parse(&invalid_magic), Err(NroError::InvalidMagic)));
    }

    #[test]
    fn test_load_nro_with_relocation() {
        use crate::memory::bus::MemoryBus;

        let load_base = 0x710000_0000u64;
        let mut bus = SystemBus::new();
        bus.map_ram(load_base, 0x10_0000); // 1 MB RAM

        // Construct synthetic NRO
        let mut nro_data = vec![0u8; 0x4000];

        // 1. NRO Header at 0x00
        let total_size = nro_data.len() as u32;
        nro_data[0x10..0x14].copy_from_slice(b"NRO0");
        nro_data[0x14..0x18].copy_from_slice(&0u32.to_le_bytes()); // version
        nro_data[0x18..0x1C].copy_from_slice(&total_size.to_le_bytes()); // size
        nro_data[0x1C..0x20].copy_from_slice(&0u32.to_le_bytes()); // flags

        // text segment: offset 0, size 0x1000
        nro_data[0x20..0x24].copy_from_slice(&0u32.to_le_bytes());
        nro_data[0x24..0x28].copy_from_slice(&0x1000u32.to_le_bytes());

        // rodata segment: offset 0x1000, size 0x1000
        nro_data[0x28..0x2C].copy_from_slice(&0x1000u32.to_le_bytes());
        nro_data[0x2C..0x30].copy_from_slice(&0x1000u32.to_le_bytes());

        // data segment: offset 0x2000, size 0x1000
        nro_data[0x30..0x34].copy_from_slice(&0x2000u32.to_le_bytes());
        nro_data[0x34..0x38].copy_from_slice(&0x1000u32.to_le_bytes());

        // bss size: 0x800
        nro_data[0x38..0x3C].copy_from_slice(&0x800u32.to_le_bytes());

        // 2. MOD0 relative offset at text + 4
        // text is at 0, word 1 is at 4. Let MOD0 be at 0x80.
        let mod0_offset = 0x80u32;
        nro_data[4..8].copy_from_slice(&mod0_offset.to_le_bytes());

        // MOD0 structure at 0x80:
        // +0: magic "MOD0"
        // +4: dynamic offset relative to MOD0 header (i32). Let dynamic table be at 0x1000 (rodata start).
        // 0x1000 - 0x80 = 0xF80.
        nro_data[0x80..0x84].copy_from_slice(b"MOD0");
        let dyn_offset_rel = 0xF80i32;
        nro_data[0x84..0x88].copy_from_slice(&dyn_offset_rel.to_le_bytes());

        // 3. Dynamic table at 0x1000
        // DT_RELA = 7, val = 0x1100 (rela table at 0x1100)
        nro_data[0x1000..0x1008].copy_from_slice(&7i64.to_le_bytes());
        nro_data[0x1008..0x1010].copy_from_slice(&0x1100u64.to_le_bytes());

        // DT_RELASZ = 8, val = 24 (1 entry)
        nro_data[0x1010..0x1018].copy_from_slice(&8i64.to_le_bytes());
        nro_data[0x1018..0x1020].copy_from_slice(&24u64.to_le_bytes());

        // DT_NULL = 0, val = 0
        nro_data[0x1020..0x1028].copy_from_slice(&0i64.to_le_bytes());
        nro_data[0x1028..0x1030].copy_from_slice(&0u64.to_le_bytes());

        // 4. Elf64_Rela entry at 0x1100:
        // r_offset: 0x2050 (inside .data segment)
        // r_info: 1027 (R_AARCH64_RELATIVE)
        // r_addend: 0x1200 (points to some string/data in rodata)
        nro_data[0x1100..0x1108].copy_from_slice(&0x2050u64.to_le_bytes());
        nro_data[0x1108..0x1110].copy_from_slice(&1027u64.to_le_bytes());
        nro_data[0x1110..0x1118].copy_from_slice(&0x1200i64.to_le_bytes());

        let header = load_nro_from_bytes(&mut bus, &nro_data, load_base).unwrap();
        assert_eq!(&header.magic, b"NRO0");

        // Verify relocation was applied:
        // Target: load_base + 0x2050 should contain load_base + 0x1200
        let relocated_ptr = bus.read_u64(load_base + 0x2050).unwrap();
        assert_eq!(relocated_ptr, load_base + 0x1200);

        // Verify bss is zeroed:
        let bss_addr = load_base + 0x2000 + 0x1000;
        assert_eq!(bus.read_u64(bss_addr).unwrap(), 0);
    }
}
