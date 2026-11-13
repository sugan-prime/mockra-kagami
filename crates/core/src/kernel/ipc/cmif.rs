//! Horizon OS Common Message Interface Format (CMIF) parser & builder.
//!
//! CMIF is the standard protocol utilized for IPC service communications
//! across Horizon OS microkernel sessions.

use thiserror::Error;

pub const CMIF_IN_HEADER_MAGIC: [u8; 4] = *b"SFCI";
pub const CMIF_OUT_HEADER_MAGIC: [u8; 4] = *b"SFCO";

#[derive(Error, Debug, PartialEq, Eq)]
pub enum CmifError {
    #[error("Buffer too small for CMIF header: {0} bytes")]
    BufferTooSmall(usize),
    #[error("Invalid CMIF request magic, expected 'SFCI'")]
    InvalidInMagic,
    #[error("Invalid CMIF response magic, expected 'SFCO'")]
    InvalidOutMagic,
}

/// A parsed CMIF incoming service request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmifRequest {
    pub command_id: u32,
    pub raw_data: Vec<u8>,
    pub in_handles: Vec<u32>,
}

impl CmifRequest {
    /// Parses a CMIF request from an IPC buffer.
    pub fn parse(buffer: &[u8]) -> Result<Self, CmifError> {
        // Find "SFCI" magic within the first 64 bytes (typically at offset 0x00 or 0x10)
        let magic_offset = buffer
            .windows(4)
            .take(64)
            .position(|w| w == CMIF_IN_HEADER_MAGIC)
            .ok_or(CmifError::InvalidInMagic)?;

        if buffer.len() < magic_offset + 16 {
            return Err(CmifError::BufferTooSmall(buffer.len()));
        }

        // Header:
        // +0x00: Magic "SFCI"
        // +0x04: Version (u32)
        // +0x08: Command ID (u32)
        // +0x0C: Reserved/Padding (u32)
        let cmd_id_bytes: [u8; 4] = buffer[magic_offset + 8..magic_offset + 12].try_into().unwrap();
        let command_id = u32::from_le_bytes(cmd_id_bytes);

        let data_start = magic_offset + 16;
        let raw_data = buffer[data_start..].to_vec();

        Ok(Self {
            command_id,
            raw_data,
            in_handles: Vec::new(),
        })
    }

    /// Reads an aligned `u32` argument from the payload at `offset`.
    pub fn read_u32(&self, offset: usize) -> Option<u32> {
        if offset + 4 <= self.raw_data.len() {
            Some(u32::from_le_bytes(self.raw_data[offset..offset + 4].try_into().unwrap()))
        } else {
            None
        }
    }

    /// Reads an aligned `u64` argument from the payload at `offset`.
    pub fn read_u64(&self, offset: usize) -> Option<u64> {
        if offset + 8 <= self.raw_data.len() {
            Some(u64::from_le_bytes(self.raw_data[offset..offset + 8].try_into().unwrap()))
        } else {
            None
        }
    }

    /// Reads an 8-byte ASCII string (such as service names in `sm:GetService`).
    pub fn read_string_8(&self, offset: usize) -> Option<String> {
        if offset + 8 <= self.raw_data.len() {
            let slice = &self.raw_data[offset..offset + 8];
            let clean: Vec<u8> = slice.iter().take_while(|&&b| b != 0).copied().collect();
            String::from_utf8(clean).ok()
        } else {
            None
        }
    }
}

/// A CMIF outgoing service response builder.
#[derive(Debug, Clone, Default)]
pub struct CmifResponse {
    pub result_code: u32,
    pub raw_data: Vec<u8>,
    pub out_handles: Vec<u32>,
}

impl CmifResponse {
    pub fn new(result_code: u32) -> Self {
        Self {
            result_code,
            raw_data: Vec::new(),
            out_handles: Vec::new(),
        }
    }

    pub fn success() -> Self {
        Self::new(0)
    }

    pub fn push_u32(&mut self, val: u32) {
        self.raw_data.extend_from_slice(&val.to_le_bytes());
    }

    pub fn push_u64(&mut self, val: u64) {
        self.raw_data.extend_from_slice(&val.to_le_bytes());
    }

    pub fn push_bytes(&mut self, bytes: &[u8]) {
        self.raw_data.extend_from_slice(bytes);
    }

    pub fn push_handle(&mut self, handle: u32) {
        self.out_handles.push(handle);
    }

    /// Serializes the response into a destination byte buffer.
    pub fn serialize(&self, dest: &mut [u8]) -> Result<usize, CmifError> {
        let total_size = 16 + self.raw_data.len();
        if dest.len() < total_size {
            return Err(CmifError::BufferTooSmall(dest.len()));
        }

        // Header:
        // +0x00: Magic "SFCO"
        // +0x04: Version (0)
        // +0x08: Result Code (u32)
        // +0x0C: Reserved/Padding (0)
        dest[0..4].copy_from_slice(&CMIF_OUT_HEADER_MAGIC);
        dest[4..8].copy_from_slice(&0u32.to_le_bytes());
        dest[8..12].copy_from_slice(&self.result_code.to_le_bytes());
        dest[12..16].copy_from_slice(&0u32.to_le_bytes());

        // Payload
        dest[16..16 + self.raw_data.len()].copy_from_slice(&self.raw_data);

        Ok(total_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cmif_request_parse() {
        let mut buffer = vec![0u8; 64];
        // Header at offset 0x10
        buffer[0x10..0x14].copy_from_slice(b"SFCI");
        buffer[0x14..0x18].copy_from_slice(&0u32.to_le_bytes()); // version
        buffer[0x18..0x1C].copy_from_slice(&42u32.to_le_bytes()); // cmd_id 42
        buffer[0x1C..0x20].copy_from_slice(&0u32.to_le_bytes());

        // Payload: u64 value 0x1122334455667788
        buffer[0x20..0x28].copy_from_slice(&0x1122334455667788u64.to_le_bytes());

        let req = CmifRequest::parse(&buffer).unwrap();
        assert_eq!(req.command_id, 42);
        assert_eq!(req.read_u64(0), Some(0x1122334455667788));
    }

    #[test]
    fn test_cmif_response_serialize() {
        let mut resp = CmifResponse::success();
        resp.push_u32(0xDEADBEEF);
        resp.push_u64(0x123456789ABCDEF0);

        let mut dest = vec![0u8; 64];
        let size = resp.serialize(&mut dest).unwrap();
        assert_eq!(size, 16 + 4 + 8);

        assert_eq!(&dest[0..4], b"SFCO");
        assert_eq!(u32::from_le_bytes(dest[8..12].try_into().unwrap()), 0); // success
        assert_eq!(u32::from_le_bytes(dest[16..20].try_into().unwrap()), 0xDEADBEEF);
        assert_eq!(u64::from_le_bytes(dest[20..28].try_into().unwrap()), 0x123456789ABCDEF0);
    }
}
