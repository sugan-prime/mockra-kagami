//! Flat binary ROM file loader.

use crate::memory::bus::MemoryError;
use crate::memory::system_bus::SystemBus;
use std::fs;
use std::path::Path;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum LoaderError {
    #[error("I/O error reading binary file: {0}")]
    Io(#[from] std::io::Error),
    #[error("Memory mapping error: {0}")]
    Memory(#[from] MemoryError),
    #[error("Empty binary file")]
    EmptyFile,
}

/// Loads a raw binary into memory at `load_addr`.
pub fn load_flat_binary(
    bus: &mut SystemBus,
    path: impl AsRef<Path>,
    load_addr: u64,
) -> Result<u64, LoaderError> {
    let bytes = fs::read(path)?;
    if bytes.is_empty() {
        return Err(LoaderError::EmptyFile);
    }
    bus.load_bytes(load_addr, &bytes)?;
    Ok(load_addr)
}
