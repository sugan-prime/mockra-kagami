pub mod flat;
pub mod nca;
pub mod nro;

pub use flat::{load_flat_binary, LoaderError};
pub use nca::{NcaArchive, NcaBuilder, NcaContentType, NcaError, NcaSectionEntry, NCA3_MAGIC};
pub use nro::{load_nro, load_nro_from_bytes, NroError, NroHeader, MOD0_MAGIC, NRO_MAGIC};
