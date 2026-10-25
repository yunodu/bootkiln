//! Platform-independent logic for scribe: drive model, image inspection, ISO 9660
//! reading, FAT32 layout, and the write / verify pipeline.

pub mod buffer;
pub mod device;
pub mod error;
pub mod fat32;
pub mod image;
pub mod iso9660;
pub mod iso_mode;
pub mod plan;
#[cfg(test)]
mod testutil;

pub use buffer::{AlignedBuffer, BUFFER_ALIGN};
pub use device::{BlockDevice, BusKind, DiskBackend, Drive};
pub use error::Error;
pub use image::{ImageInfo, ImageKind};
pub use plan::{CHUNK_SIZE, Plan, Source, verify_plan, write_plan};
