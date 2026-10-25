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
    #[error("not an ISO 9660 or UDF image")]
    NotIso9660,
    #[error("malformed disc image: {0}")]
    InvalidIso(String),
    #[error("the ISO has no UEFI boot loader (\\EFI\\BOOT\\BOOT*.EFI)")]
    NoUefiBootloader,
    #[error("{path} is {size} bytes; FAT32 cannot store files of 4 GiB or more")]
    FileTooLarge { path: String, size: u64 },
    #[error("{path}: names differ only in case and their contents differ; the drive cannot hold both")]
    NameConflict { path: String },
    #[error("{path}: not a valid FAT file name")]
    InvalidName { path: String },
    #[error("the files need {needed} bytes but the drive's volume holds {available}")]
    DriveTooSmall { needed: u64, available: u64 },
    #[error("verification failed: {path} on the drive differs from the image")]
    FileMismatch { path: String },
    #[error("{0}")]
    WindowsOption(String),
    #[error("cancelled; the drive is incomplete and will not boot. Write it again or format it")]
    Cancelled,
    #[error("no drive {number}")]
    NoSuchDrive { number: u32 },
    #[error("drive {number} holds the running Windows installation; refusing to write it")]
    SystemDrive { number: u32 },
    #[error("drive {number} ({model}) is not a removable USB / SD drive; refusing to write it")]
    NotRemovable { number: u32, model: String },
    #[error("the image is stored on drive {number}; copy it elsewhere first")]
    ImageOnDrive { number: u32 },
    #[error(
        "this is an ISO 9660/UDF image without a boot record or UEFI boot loader; a raw copy will not boot from USB"
    )]
    OpticalOnlyImage,
    #[error("the image has neither a boot record nor an ISO 9660 descriptor")]
    UnknownImage,
}
