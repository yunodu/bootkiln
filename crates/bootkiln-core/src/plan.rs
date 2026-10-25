//! Device write plans: which bytes go where on the target drive, and the write / verify
//! pipeline that executes them.

use std::io::{self, Read, Seek, SeekFrom};

use crate::buffer::AlignedBuffer;
use crate::device::BlockDevice;
use crate::error::Error;

/// Bytes moved per device I/O.
pub const CHUNK_SIZE: usize = 4 * 1024 * 1024;

/// Receives the bytes done so far; returning an error (e.g. [`Error::Cancelled`]) stops
/// the operation.
pub type Progress<'a> = dyn FnMut(u64) -> Result<(), Error> + 'a;

/// Where a run of device bytes comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// `len` bytes of the image file starting at `offset`.
    Image { offset: u64, len: u64 },
    Bytes(Vec<u8>),
    Zeros(u64),
}

impl Source {
    pub fn len(&self) -> u64 {
        match self {
            Source::Image { len, .. } | Source::Zeros(len) => *len,
            Source::Bytes(bytes) => bytes.len() as u64,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Device contents as non-overlapping runs in ascending offset order.
///
/// A run that does not directly continue the previous one must start on a sector
/// boundary; a run ending mid-sector is zero-padded to the sector end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    runs: Vec<(u64, Source)>,
}

impl Plan {
    /// Sorts `runs` by offset and drops empty ones.
    ///
    /// Panics if runs overlap: layouts are computed, so an overlap is a bug.
    pub fn new(mut runs: Vec<(u64, Source)>) -> Self {
        runs.retain(|(_, source)| !source.is_empty());
        runs.sort_by_key(|(offset, _)| *offset);
        for pair in runs.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            assert!(a.0 + a.1.len() <= b.0, "plan runs overlap at device byte {}", b.0);
        }
        Self { runs }
    }

    /// The whole image copied to the start of the device.
    pub fn raw_image(len: u64) -> Self {
        Self::new(vec![(0, Source::Image { offset: 0, len })])
    }

    pub fn runs(&self) -> &[(u64, Source)] {
        &self.runs
    }

    /// Bytes the plan writes; the progress callbacks count up to this.
    pub fn len(&self) -> u64 {
        self.runs.iter().map(|(_, source)| source.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// One past the highest device byte written.
    pub fn end(&self) -> u64 {
        self.runs.last().map_or(0, |(offset, source)| offset + source.len())
    }
}

/// Write `plan` to `device`, reading image runs from `image`.
///
/// `progress` receives the number of plan bytes written so far.
pub fn write_plan<R: Read + Seek>(
    image: &mut R,
    plan: &Plan,
    device: &mut dyn BlockDevice,
    progress: &mut Progress,
) -> Result<(), Error> {
    let sector = check_device(plan, device)?;
    stream(image, plan, sector, progress, &mut |start, data, _| Ok(device.write_at(start, data)?))?;
    device.sync()?;
    Ok(())
}

/// Compare `device` with what [`write_plan`] would have written.
pub fn verify_plan<R: Read + Seek>(
    image: &mut R,
    plan: &Plan,
    device: &mut dyn BlockDevice,
    progress: &mut Progress,
) -> Result<(), Error> {
    let sector = check_device(plan, device)?;
    let mut actual = AlignedBuffer::zeroed(CHUNK_SIZE);
    stream(image, plan, sector, progress, &mut |start, expected, valid| {
        let actual = &mut actual[..expected.len()];
        device.read_at(start, actual)?;
        match expected[..valid].iter().zip(&actual[..valid]).position(|(a, b)| a != b) {
            Some(i) => Err(Error::Mismatch { offset: start + i as u64 }),
            None => Ok(()),
        }
    })
}

fn check_device(plan: &Plan, device: &dyn BlockDevice) -> Result<usize, Error> {
    let sector = device.sector_size() as usize;
    if sector == 0 || !CHUNK_SIZE.is_multiple_of(sector) {
        return Err(io::Error::new(io::ErrorKind::Unsupported, format!("unsupported sector size {sector}")).into());
    }
    if plan.end() > device.size() {
        return Err(Error::ImageTooLarge { image: plan.end(), device: device.size() });
    }
    Ok(sector)
}

/// Receives `(device_offset, sector_padded_bytes, valid_len)`.
type Flush<'a> = dyn FnMut(u64, &[u8], usize) -> Result<(), Error> + 'a;

/// Assemble the plan's bytes into sector-aligned buffers of up to [`CHUNK_SIZE`] and hand
/// each to `flush`.
fn stream<R: Read + Seek>(
    image: &mut R,
    plan: &Plan,
    sector: usize,
    progress: &mut Progress,
    flush: &mut Flush,
) -> Result<(), Error> {
    let mut buf = AlignedBuffer::zeroed(CHUNK_SIZE);
    let mut start = 0u64;
    let mut filled = 0usize;
    let mut done = 0u64;
    let mut flush_padded = |buf: &mut AlignedBuffer, start: u64, filled: usize| {
        let padded = filled.next_multiple_of(sector);
        buf[filled..padded].fill(0);
        flush(start, &buf[..padded], filled)
    };

    for (offset, source) in plan.runs() {
        if filled > 0 && *offset != start + filled as u64 {
            flush_padded(&mut buf, start, filled)?;
            filled = 0;
        }
        if filled == 0 {
            if !offset.is_multiple_of(sector as u64) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("plan run at device byte {offset} is not sector-aligned"),
                )
                .into());
            }
            start = *offset;
        }
        if let Source::Image { offset, .. } = source {
            image.seek(SeekFrom::Start(*offset))?;
        }

        let mut consumed = 0u64;
        while consumed < source.len() {
            if filled == CHUNK_SIZE {
                flush_padded(&mut buf, start, filled)?;
                start += CHUNK_SIZE as u64;
                filled = 0;
            }
            let n = (source.len() - consumed).min((CHUNK_SIZE - filled) as u64) as usize;
            let dst = &mut buf[filled..filled + n];
            match source {
                Source::Image { offset, .. } => read_full(image, dst, offset + consumed)?,
                Source::Bytes(bytes) => dst.copy_from_slice(&bytes[consumed as usize..consumed as usize + n]),
                Source::Zeros(_) => dst.fill(0),
            }
            filled += n;
            consumed += n as u64;
            done += n as u64;
            progress(done)?;
        }
    }
    if filled > 0 {
        flush_padded(&mut buf, start, filled)?;
    }
    Ok(())
}

fn read_full(image: &mut dyn Read, buf: &mut [u8], offset: u64) -> Result<(), Error> {
    image.read_exact(buf).map_err(|e| match e.kind() {
        io::ErrorKind::UnexpectedEof => Error::ImageTruncated { offset },
        _ => Error::Io(e),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{MemDisk, pattern};
    use std::io::Cursor;

    #[test]
    fn writes_multi_chunk_image_with_padded_tail_and_verifies() {
        let len = CHUNK_SIZE * 2 + 1000; // last chunk ends mid-sector
        let img = pattern(len);
        let plan = Plan::raw_image(len as u64);
        let mut disk = MemDisk::new(CHUNK_SIZE * 3, 4096);
        let mut last = 0;
        write_plan(&mut Cursor::new(&img), &plan, &mut disk, &mut |p| { last = p; Ok(()) }).unwrap();

        assert_eq!(last, len as u64);
        assert!(disk.synced);
        assert_eq!(&disk.data[..len], &img[..]);
        let tail_end = len.next_multiple_of(4096);
        assert!(disk.data[len..tail_end].iter().all(|&b| b == 0), "tail padded with zeros");
        assert!(disk.data[tail_end..].iter().all(|&b| b == 0xEE), "nothing written past the image");

        verify_plan(&mut Cursor::new(&img), &plan, &mut disk, &mut |_| Ok(())).unwrap();
    }

    #[test]
    fn mixed_runs_land_at_their_offsets_and_gaps_stay_untouched() {
        let img = pattern(10_000);
        let plan = Plan::new(vec![
            (8192, Source::Image { offset: 100, len: 3000 }),
            (0, Source::Bytes(vec![7; 600])),
            // Directly continues the image run, so it shares its last sector.
            (8192 + 3000, Source::Bytes(vec![9; 100])),
            (16384, Source::Zeros(1024)),
        ]);
        let mut disk = MemDisk::new(32768, 512);
        write_plan(&mut Cursor::new(&img), &plan, &mut disk, &mut |_| Ok(())).unwrap();

        assert!(disk.data[..600].iter().all(|&b| b == 7));
        assert!(disk.data[600..1024].iter().all(|&b| b == 0), "partial sector padded");
        assert!(disk.data[1024..8192].iter().all(|&b| b == 0xEE), "gap untouched");
        assert_eq!(&disk.data[8192..11192], &img[100..3100]);
        assert!(disk.data[11192..11292].iter().all(|&b| b == 9));
        assert!(disk.data[16384..17408].iter().all(|&b| b == 0));
        assert!(disk.data[17408..].iter().all(|&b| b == 0xEE));

        verify_plan(&mut Cursor::new(&img), &plan, &mut disk, &mut |_| Ok(())).unwrap();
        disk.data[11250] = 0;
        let err = verify_plan(&mut Cursor::new(&img), &plan, &mut disk, &mut |_| Ok(())).unwrap_err();
        assert!(matches!(err, Error::Mismatch { offset: 11250 }), "{err:?}");
    }

    #[test]
    fn verify_reports_first_differing_byte() {
        let img = pattern(CHUNK_SIZE + 512);
        let plan = Plan::raw_image(img.len() as u64);
        let mut disk = MemDisk::new(CHUNK_SIZE * 2, 512);
        write_plan(&mut Cursor::new(&img), &plan, &mut disk, &mut |_| Ok(())).unwrap();
        let bad = CHUNK_SIZE + 17;
        disk.data[bad] ^= 0xFF;

        let err = verify_plan(&mut Cursor::new(&img), &plan, &mut disk, &mut |_| Ok(())).unwrap_err();
        assert!(matches!(err, Error::Mismatch { offset } if offset == bad as u64), "{err:?}");
    }

    #[test]
    fn rejects_plan_larger_than_device_before_writing() {
        let img = pattern(8192);
        let mut disk = MemDisk::new(4096, 512);
        let err = write_plan(&mut Cursor::new(&img), &Plan::raw_image(8192), &mut disk, &mut |_| Ok(())).unwrap_err();
        assert!(matches!(err, Error::ImageTooLarge { image: 8192, device: 4096 }), "{err:?}");
        assert!(disk.data.iter().all(|&b| b == 0xEE));
    }

    #[test]
    fn image_shorter_than_plan_is_reported() {
        let img = pattern(1000);
        let mut disk = MemDisk::new(CHUNK_SIZE, 512);
        let err = write_plan(&mut Cursor::new(&img), &Plan::raw_image(2000), &mut disk, &mut |_| Ok(())).unwrap_err();
        assert!(matches!(err, Error::ImageTruncated { offset: 0 }), "{err:?}");
    }

    #[test]
    fn unaligned_run_after_gap_is_rejected() {
        let plan = Plan::new(vec![(0, Source::Bytes(vec![1; 10])), (700, Source::Bytes(vec![2; 10]))]);
        let mut disk = MemDisk::new(4096, 512);
        let err = write_plan(&mut Cursor::new(Vec::new()), &plan, &mut disk, &mut |_| Ok(())).unwrap_err();
        assert!(matches!(&err, Error::Io(e) if e.kind() == io::ErrorKind::InvalidInput), "{err:?}");
    }

    #[test]
    fn progress_error_stops_the_write() {
        let img = pattern(CHUNK_SIZE * 3);
        let mut disk = MemDisk::new(CHUNK_SIZE * 3, 512);
        let err = write_plan(&mut Cursor::new(&img), &Plan::raw_image(img.len() as u64), &mut disk, &mut |done| {
            if done >= CHUNK_SIZE as u64 { Err(Error::Cancelled) } else { Ok(()) }
        })
        .unwrap_err();
        assert!(matches!(err, Error::Cancelled), "{err:?}");
        assert!(disk.data[CHUNK_SIZE..].iter().all(|&b| b == 0xEE), "nothing written after cancelling");
    }
}
