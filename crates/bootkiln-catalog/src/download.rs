//! Resumable download with SHA-256 verification. A finished file is reused when its
//! checksum still matches; a partial download (`<file>.part`) continues where it stopped.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{Error, Release, http};

const CHUNK: usize = 1024 * 1024;
/// A download that receives nothing for this long fails (and can be resumed).
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// How often a quiet connection still checks for pause / cancel.
const POLL: Duration = Duration::from_millis(500);
/// Chunks buffered between the network reader thread and the writer.
const CHANNEL_CHUNKS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    /// Hashing a file already on disk (a finished download or the part to resume).
    Checking,
    Downloading,
}

/// Download `release` into `dir` and verify it. `progress(stage, done, total)` is called as
/// work proceeds (`total` is 0 when unknown); returning [`Error::Paused`] stops and keeps
/// the partial file, [`Error::Cancelled`] stops and deletes it.
pub fn download(
    release: &Release,
    dir: &Path,
    progress: &mut dyn FnMut(Stage, u64, u64) -> Result<(), Error>,
) -> Result<PathBuf, Error> {
    download_with_idle(release, dir, IDLE_TIMEOUT, progress)
}

fn download_with_idle(
    release: &Release,
    dir: &Path,
    idle: Duration,
    progress: &mut dyn FnMut(Stage, u64, u64) -> Result<(), Error>,
) -> Result<PathBuf, Error> {
    let name = Path::new(&release.file_name)
        .file_name()
        .filter(|n| !n.is_empty())
        .ok_or_else(|| Error::Format(format!("unusable file name {:?}", release.file_name)))?;
    fs::create_dir_all(dir)?;
    let path = dir.join(name);
    let part = dir.join(format!("{}.part", name.to_string_lossy()));

    if path.exists() {
        let mut hasher = Sha256::new();
        hash_file(&path, &mut hasher, progress)?;
        if hex(hasher) == release.sha256 {
            return Ok(path);
        }
        fs::remove_file(&path)?;
    }

    let result = fetch(release, &part, idle, progress);
    match result {
        Ok(digest) if digest == release.sha256 => {
            fs::rename(&part, &path)?;
            Ok(path)
        }
        Ok(_) => {
            fs::remove_file(&part)?;
            Err(Error::ChecksumMismatch { file: release.file_name.clone() })
        }
        Err(Error::Paused) => Err(Error::Paused),
        Err(e) => {
            // Keep the part for transient network errors; drop it when the user cancels.
            if matches!(e, Error::Cancelled) {
                let _ = fs::remove_file(&part);
            }
            Err(e)
        }
    }
}

/// Download into `part` (continuing it when possible); returns the lowercase hex SHA-256.
fn fetch(
    release: &Release,
    part: &Path,
    idle: Duration,
    progress: &mut dyn FnMut(Stage, u64, u64) -> Result<(), Error>,
) -> Result<String, Error> {
    let mut hasher = Sha256::new();
    let mut offset = if part.exists() { hash_file(part, &mut hasher, progress)? } else { 0 };

    let mut request = http::agent().get(&release.url);
    if offset > 0 {
        request = request.header("Range", format!("bytes={offset}-"));
    }
    let response = request.call().map_err(|e| Error::Http(format!("{}: {e}", release.file_name)))?;
    if offset > 0 && response.status().as_u16() != 206 {
        // The server ignored the range: start over.
        offset = 0;
        hasher = Sha256::new();
    }
    let remaining: Option<u64> =
        response.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok());
    let total = remaining.map(|r| r + offset).or(release.size).unwrap_or(0);

    let mut file = if offset > 0 {
        OpenOptions::new().append(true).open(part)?
    } else {
        File::create(part)?
    };
    // A reader thread hands over chunks, so a silent connection cannot block this loop:
    // pause and cancel stay responsive, and a stall becomes an error after `idle`.
    let (chunks, received) = mpsc::sync_channel::<std::io::Result<Vec<u8>>>(CHANNEL_CHUNKS);
    let mut body = response.into_body().into_reader();
    thread::spawn(move || {
        loop {
            let mut buf = vec![0u8; CHUNK];
            let read = body.read(&mut buf).map(|n| {
                buf.truncate(n);
                buf
            });
            let end = !matches!(&read, Ok(b) if !b.is_empty());
            if chunks.send(read).is_err() || end {
                return;
            }
        }
    });

    let mut done = offset;
    let mut quiet = Duration::ZERO;
    progress(Stage::Downloading, done, total)?;
    loop {
        let chunk = match received.recv_timeout(POLL) {
            Ok(Ok(chunk)) if chunk.is_empty() => break,
            Ok(Ok(chunk)) => chunk,
            Ok(Err(e)) => return Err(Error::Http(format!("{}: {e}", release.file_name))),
            Err(RecvTimeoutError::Timeout) => {
                quiet += POLL;
                if quiet >= idle {
                    return Err(Error::Http(format!(
                        "{}: no data for {} s; try again to resume",
                        release.file_name,
                        idle.as_secs()
                    )));
                }
                progress(Stage::Downloading, done, total)?;
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        quiet = Duration::ZERO;
        file.write_all(&chunk)?;
        hasher.update(&chunk);
        done += chunk.len() as u64;
        progress(Stage::Downloading, done, total)?;
    }
    file.sync_all()?;
    if remaining.is_some_and(|r| done != offset + r) {
        return Err(Error::Http(format!("{}: the connection closed early; try again to resume", release.file_name)));
    }
    Ok(hex(hasher))
}

/// Feed `path` into `hasher`; returns its length.
fn hash_file(
    path: &Path,
    hasher: &mut Sha256,
    progress: &mut dyn FnMut(Stage, u64, u64) -> Result<(), Error>,
) -> Result<u64, Error> {
    let mut file = File::open(path)?;
    let total = file.metadata()?.len();
    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            return Ok(done);
        }
        hasher.update(&buf[..n]);
        done += n as u64;
        progress(Stage::Checking, done, total)?;
    }
}

/// Delete a paused download's partial file, if any.
pub fn discard_partial(release: &Release, dir: &Path) -> Result<(), Error> {
    let Some(name) = Path::new(&release.file_name).file_name() else { return Ok(()) };
    match fs::remove_file(dir.join(format!("{}.part", name.to_string_lossy()))) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

fn hex(hasher: Sha256) -> String {
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;

    /// Minimal HTTP/1.1 file server honouring `Range: bytes=N-`. Records each request's
    /// range start (`None` for a plain GET).
    struct Server {
        url: String,
        requests: Arc<Mutex<Vec<Option<u64>>>>,
    }

    fn serve(data: Vec<u8>, honour_ranges: bool) -> Server {
        serve_with(data, honour_ranges, None)
    }

    /// `stall_after`: send that many body bytes, then keep the connection open silently.
    fn serve_with(data: Vec<u8>, honour_ranges: bool, stall_after: Option<usize>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/file.iso", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&requests);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut range = None;
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        range = v.trim().trim_end_matches('-').parse::<u64>().ok();
                    }
                }
                log.lock().unwrap().push(range);
                let start = if honour_ranges { range.unwrap_or(0) as usize } else { 0 };
                let status = if honour_ranges && range.is_some() { "206 Partial Content" } else { "200 OK" };
                let body = &data[start..];
                let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                stream.write_all(head.as_bytes()).unwrap();
                match stall_after {
                    Some(n) => {
                        stream.write_all(&body[..n]).unwrap();
                        thread::sleep(Duration::from_secs(10));
                    }
                    None => stream.write_all(body).unwrap(),
                }
            }
        });
        Server { url, requests }
    }

    fn data() -> Vec<u8> {
        (0..3 * CHUNK + 1234).map(|i| (i * 31 % 251) as u8).collect()
    }

    fn release(url: &str, data: &[u8]) -> Release {
        Release {
            entry: "test".into(),
            version: "1".into(),
            file_name: "file.iso".into(),
            url: url.into(),
            sha256: Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect(),
            size: Some(data.len() as u64),
            language: None,
            languages: Vec::new(),
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bootkiln-dl-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn quiet(_: Stage, _: u64, _: u64) -> Result<(), Error> {
        Ok(())
    }

    #[test]
    fn downloads_verifies_and_reuses_the_finished_file() {
        let data = data();
        let server = serve(data.clone(), true);
        let dir = temp_dir("fresh");
        let r = release(&server.url, &data);

        let path = download(&r, &dir, &mut quiet).unwrap();
        assert_eq!(fs::read(&path).unwrap(), data);
        assert!(!dir.join("file.iso.part").exists());

        let again = download(&r, &dir, &mut quiet).unwrap();
        assert_eq!(again, path);
        assert_eq!(server.requests.lock().unwrap().len(), 1, "a verified file is not downloaded again");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pause_keeps_the_part_and_the_next_run_resumes_with_a_range() {
        let data = data();
        let server = serve(data.clone(), true);
        let dir = temp_dir("resume");
        let r = release(&server.url, &data);

        let err = download(&r, &dir, &mut |stage, done, _| {
            if stage == Stage::Downloading && done >= CHUNK as u64 { Err(Error::Paused) } else { Ok(()) }
        })
        .unwrap_err();
        assert!(matches!(err, Error::Paused), "{err:?}");
        let kept = fs::metadata(dir.join("file.iso.part")).unwrap().len();
        assert!(kept >= CHUNK as u64);

        let path = download(&r, &dir, &mut quiet).unwrap();
        assert_eq!(fs::read(&path).unwrap(), data);
        assert_eq!(*server.requests.lock().unwrap(), vec![None, Some(kept)]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn server_without_ranges_restarts_from_zero() {
        let data = data();
        let server = serve(data.clone(), false);
        let dir = temp_dir("norange");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("file.iso.part"), &data[..1000]).unwrap();

        let path = download(&release(&server.url, &data), &dir, &mut quiet).unwrap();
        assert_eq!(fs::read(&path).unwrap(), data);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn checksum_mismatch_and_cancel_delete_the_download() {
        let data = data();
        let server = serve(data.clone(), true);
        let dir = temp_dir("bad");
        let mut r = release(&server.url, &data);
        r.sha256 = "0".repeat(64);
        let err = download(&r, &dir, &mut quiet).unwrap_err();
        assert!(matches!(err, Error::ChecksumMismatch { .. }), "{err:?}");
        assert!(!dir.join("file.iso.part").exists() && !dir.join("file.iso").exists());

        let calls = AtomicUsize::new(0);
        let err = download(&release(&server.url, &data), &dir, &mut |_, _, _| {
            if calls.fetch_add(1, Ordering::SeqCst) == 2 { Err(Error::Cancelled) } else { Ok(()) }
        })
        .unwrap_err();
        assert!(matches!(err, Error::Cancelled), "{err:?}");
        assert!(!dir.join("file.iso.part").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn silent_connection_fails_after_the_idle_time_and_keeps_the_part() {
        let data = data();
        let server = serve_with(data.clone(), true, Some(CHUNK));
        let dir = temp_dir("stall");
        let started = std::time::Instant::now();
        let err = download_with_idle(&release(&server.url, &data), &dir, Duration::from_secs(2), &mut quiet).unwrap_err();
        assert!(matches!(&err, Error::Http(m) if m.contains("no data")), "{err:?}");
        assert!(started.elapsed() < Duration::from_secs(8), "did not wait for the server");
        assert_eq!(fs::metadata(dir.join("file.iso.part")).unwrap().len(), CHUNK as u64);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancel_works_while_the_connection_is_silent() {
        let data = data();
        let server = serve_with(data.clone(), true, Some(1000));
        let dir = temp_dir("stallcancel");
        let started = std::time::Instant::now();
        let err = download(&release(&server.url, &data), &dir, &mut |_, _, _| {
            if started.elapsed() > Duration::from_secs(1) { Err(Error::Cancelled) } else { Ok(()) }
        })
        .unwrap_err();
        assert!(matches!(err, Error::Cancelled), "{err:?}");
        assert!(started.elapsed() < Duration::from_secs(4));
        let _ = fs::remove_dir_all(&dir);
    }
}
