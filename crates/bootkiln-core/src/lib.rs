//! Platform-independent logic for bootkiln: drive model, image inspection, ISO 9660 and
//! UDF reading, FAT32 / NTFS drive layouts, Windows answer files, and the write / copy /
//! verify pipelines.

pub mod buffer;
pub mod copy;
pub mod device;
pub mod disc;
pub mod error;
pub mod fat32;
pub mod image;
pub mod iso9660;
pub mod iso_mode;
pub mod mbr;
pub mod plan;
#[cfg(test)]
mod testutil;
pub mod udf;
pub mod uefi_ntfs;
pub mod unattend;
pub mod windows_iso;

pub use buffer::{AlignedBuffer, BUFFER_ALIGN};
pub use device::{BlockDevice, BusKind, DiskBackend, Drive};
pub use error::Error;
pub use image::{ImageInfo, ImageKind};
pub use plan::{CHUNK_SIZE, Plan, Source, verify_plan, write_plan};
