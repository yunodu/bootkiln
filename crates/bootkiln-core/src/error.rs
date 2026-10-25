use std::io;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("image ({image} bytes) is larger than the drive ({device} bytes)")]
    ImageTooLarge { image: u64, device: u64 },
    #[error("image ended early at byte {offset}; was the file modified during the write?")]
    ImageTruncated { offset: u64 },
    #[error("verification failed: drive contents differ from the image at byte {offset}")]
    Mismatch { offset: u64 },
    #[error("not an ISO 9660 image")]
    NotIso9660,
    #[error("malformed ISO 9660 image: {0}")]
    InvalidIso(String),
    #[error("the ISO has no UEFI boot loader (\\EFI\\BOOT\\BOOT*.EFI)")]
    NoUefiBootloader,
    #[error("{path} is {size} bytes; FAT32 cannot store files of 4 GiB or more")]
    FileTooLarge { path: String, size: u64 },
    #[error("{path}: names differ only in case and their contents differ; FAT32 cannot hold both")]
    NameConflict { path: String },
    #[error("{path}: not a valid FAT file name")]
    InvalidName { path: String },
    #[error("the files need {needed} bytes but the drive's FAT32 volume holds {available}")]
    DriveTooSmall { needed: u64, available: u64 },
}
