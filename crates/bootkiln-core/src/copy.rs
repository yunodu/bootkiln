//! Copy a file tree onto a mounted file system, and verify the copy by reading it back.

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::error::Error;
use crate::fat32::{Node, NodeKind};
use crate::plan::Source;

const CHUNK: usize = 4 * 1024 * 1024;

/// Total file bytes in `nodes`; the progress callbacks count up to this.
pub fn total_len(nodes: &[Node]) -> u64 {
    nodes
        .iter()
        .map(|n| match &n.kind {
            NodeKind::Dir(children) => total_len(children),
            NodeKind::File(sources) => sources.iter().map(Source::len).sum(),
        })
        .sum()
}

/// Create `nodes` below `root`. `progress` receives the file bytes written so far.
pub fn copy_tree<R: Read + Seek>(
    image: &mut R,
    nodes: &[Node],
    root: &Path,
    progress: &mut dyn FnMut(u64),
) -> Result<(), Error> {
    let mut walker = Walker { image, buf: vec![0u8; CHUNK], other: Vec::new(), done: 0, progress };
    walker.copy(nodes, root, "")
}

/// Compare the files below `root` with `nodes`. `progress` receives the bytes compared.
pub fn verify_tree<R: Read + Seek>(
    image: &mut R,
    nodes: &[Node],
    root: &Path,
    progress: &mut dyn FnMut(u64),
) -> Result<(), Error> {
    let mut walker = Walker { image, buf: vec![0u8; CHUNK], other: vec![0u8; CHUNK], done: 0, progress };
    walker.verify(nodes, root, "")
}

struct Walker<'a, R> {
    image: &'a mut R,
    buf: Vec<u8>,
    /// Second buffer for verification; empty when copying.
    other: Vec<u8>,
    done: u64,
    progress: &'a mut dyn FnMut(u64),
}

impl<R: Read + Seek> Walker<'_, R> {
    fn copy(&mut self, nodes: &[Node], dir: &Path, path: &str) -> Result<(), Error> {
        for node in nodes {
            let target = dir.join(&node.name);
            let path = format!("{path}/{}", node.name);
            match &node.kind {
                NodeKind::Dir(children) => {
                    fs::create_dir(&target).map_err(|e| at(&path, e))?;
                    self.copy(children, &target, &path)?;
                }
                NodeKind::File(sources) => {
                    let mut file = File::create(&target).map_err(|e| at(&path, e))?;
                    for source in sources {
                        self.stream(source, &mut |chunk| file.write_all(chunk).map_err(|e| at(&path, e)))?;
                    }
                }
            }
        }
        Ok(())
    }

    fn verify(&mut self, nodes: &[Node], dir: &Path, path: &str) -> Result<(), Error> {
        for node in nodes {
            let target = dir.join(&node.name);
            let path = format!("{path}/{}", node.name);
            match &node.kind {
                NodeKind::Dir(children) => self.verify(children, &target, &path)?,
                NodeKind::File(sources) => {
                    let expected: u64 = sources.iter().map(Source::len).sum();
                    let mut file = File::open(&target).map_err(|e| at(&path, e))?;
                    if file.metadata().map_err(|e| at(&path, e))?.len() != expected {
                        return Err(Error::FileMismatch { path });
                    }
                    let mut other = std::mem::take(&mut self.other);
                    let result = sources.iter().try_for_each(|source| {
                        self.stream(source, &mut |chunk| {
                            let actual = &mut other[..chunk.len()];
                            file.read_exact(actual).map_err(|e| at(&path, e))?;
                            if actual != chunk { Err(Error::FileMismatch { path: path.clone() }) } else { Ok(()) }
                        })
                    });
                    self.other = other;
                    result?;
                }
            }
        }
        Ok(())
    }

    /// Feed `source` to `sink` in chunks of at most [`CHUNK`] bytes.
    fn stream(&mut self, source: &Source, sink: &mut dyn FnMut(&[u8]) -> Result<(), Error>) -> Result<(), Error> {
        if let Source::Image { offset, .. } = source {
            self.image.seek(SeekFrom::Start(*offset))?;
        }
        let mut consumed = 0u64;
        while consumed < source.len() {
            let n = (source.len() - consumed).min(CHUNK as u64) as usize;
            let chunk = &mut self.buf[..n];
            match source {
                Source::Image { offset, .. } => self.image.read_exact(chunk).map_err(|e| match e.kind() {
                    io::ErrorKind::UnexpectedEof => Error::ImageTruncated { offset: offset + consumed },
                    _ => Error::Io(e),
                })?,
                Source::Bytes(bytes) => chunk.copy_from_slice(&bytes[consumed as usize..consumed as usize + n]),
                Source::Zeros(_) => chunk.fill(0),
            }
            sink(chunk)?;
            consumed += n as u64;
            self.done += n as u64;
            (self.progress)(self.done);
        }
        Ok(())
    }
}

fn at(path: &str, e: io::Error) -> Error {
    Error::Io(io::Error::new(e.kind(), format!("{path}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fat32::DosTime;
    use crate::testutil::pattern;
    use std::io::Cursor;

    const T: DosTime = DosTime { date: 0, time: 0 };

    fn tree() -> Vec<Node> {
        vec![
            Node {
                name: "sources".into(),
                modified: T,
                kind: NodeKind::Dir(vec![Node {
                    name: "install.wim".into(),
                    modified: T,
                    kind: NodeKind::File(vec![
                        Source::Image { offset: 100, len: 5_000_000 },
                        Source::Zeros(10),
                        Source::Bytes(b"tail".to_vec()),
                    ]),
                }]),
            },
            Node { name: "empty".into(), modified: T, kind: NodeKind::File(Vec::new()) },
        ]
    }

    #[test]
    fn copies_and_verifies_then_detects_tampering() {
        let image = pattern(6_000_000);
        let dir = std::env::temp_dir().join(format!("bootkiln-copy-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let nodes = tree();

        let mut last = 0;
        copy_tree(&mut Cursor::new(&image), &nodes, &dir, &mut |n| last = n).unwrap();
        assert_eq!(last, total_len(&nodes));
        let wim = fs::read(dir.join("sources/install.wim")).unwrap();
        assert_eq!(&wim[..5_000_000], &image[100..5_000_100]);
        assert_eq!(&wim[5_000_000..], b"\0\0\0\0\0\0\0\0\0\0tail");
        assert_eq!(fs::read(dir.join("empty")).unwrap(), b"");
        verify_tree(&mut Cursor::new(&image), &nodes, &dir, &mut |_| {}).unwrap();

        let mut tampered = wim.clone();
        tampered[4_500_000] ^= 1;
        fs::write(dir.join("sources/install.wim"), &tampered).unwrap();
        let err = verify_tree(&mut Cursor::new(&image), &nodes, &dir, &mut |_| {}).unwrap_err();
        assert!(matches!(&err, Error::FileMismatch { path } if path == "/sources/install.wim"), "{err:?}");

        fs::write(dir.join("empty"), b"x").unwrap();
        fs::write(dir.join("sources/install.wim"), &wim).unwrap();
        let err = verify_tree(&mut Cursor::new(&image), &nodes, &dir, &mut |_| {}).unwrap_err();
        assert!(matches!(&err, Error::FileMismatch { path } if path == "/empty"), "{err:?}");
        fs::remove_dir_all(&dir).unwrap();
    }
}
