//! Downloading a release file and handing it to the installer.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::{Asset, UpdateError};

/// Bytes read from the network per write to disk.
const CHUNK: usize = 64 * 1024;

/// Counters a download updates for the UI to read.
#[derive(Debug, Default)]
pub struct Progress {
    received: AtomicU64,
    cancelled: AtomicBool,
}

impl Progress {
    #[must_use]
    pub fn received(&self) -> u64 {
        self.received.load(Ordering::Relaxed)
    }

    /// Stop at the next chunk; the partial file is removed.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

/// Folder a downloaded release lands in. On Android it must be the
/// cache's `updates/`, the only place the installer's provider exposes;
/// on desktop, the person's Downloads folder so they can find it.
#[must_use]
pub fn folder() -> PathBuf {
    if cfg!(target_os = "android") {
        return istmo::path::cache_dir().join("updates");
    }
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" });
    home.map(|home| PathBuf::from(home).join("Downloads"))
        .filter(|dir| dir.is_dir())
        .unwrap_or_else(|| istmo::path::cache_dir().join("updates"))
}

/// Download `asset` into [`folder`], checking its SHA-256. A file
/// already there with the right hash is reused. Blocking.
///
/// # Errors
/// Network or disk failures, a hash mismatch, or a cancel.
pub fn fetch(asset: &Asset, progress: &Arc<Progress>) -> Result<PathBuf, UpdateError> {
    fetch_into(&folder(), asset, progress)
}

fn fetch_into(dir: &Path, asset: &Asset, progress: &Progress) -> Result<PathBuf, UpdateError> {
    std::fs::create_dir_all(dir)?;
    let dest = dir.join(file_name(&asset.name));
    if matches_hash(&dest, &asset.sha256) {
        progress.received.store(asset.size, Ordering::Relaxed);
        return Ok(dest);
    }
    let partial = dest.with_extension("part");
    let result = write(asset, &partial, progress);
    match result {
        Ok(()) => {
            std::fs::rename(&partial, &dest)?;
            mark_executable(&dest);
            Ok(dest)
        }
        Err(err) => {
            let _ = std::fs::remove_file(&partial);
            Err(err)
        }
    }
}

fn write(asset: &Asset, partial: &Path, progress: &Progress) -> Result<(), UpdateError> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .user_agent(concat!("notinplus/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let mut response = client.get(&asset.url).send()?.error_for_status()?;
    let mut file = std::io::BufWriter::new(std::fs::File::create(partial)?);
    let mut hasher = Sha256::new();
    let mut buf = vec![0; CHUNK];
    let mut received = 0u64;
    loop {
        if progress.is_cancelled() {
            return Err(UpdateError::Cancelled);
        }
        let read = response.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
        file.write_all(&buf[..read])?;
        received += read as u64;
        progress.received.store(received, Ordering::Relaxed);
    }
    file.flush()?;
    let actual = hex(&hasher.finalize());
    if actual.eq_ignore_ascii_case(&asset.sha256) {
        Ok(())
    } else {
        Err(UpdateError::Checksum)
    }
}

/// The asset's name without any folder part a broken manifest might
/// carry.
fn file_name(name: &str) -> &str {
    name.rsplit(['/', '\\']).next().unwrap_or(name)
}

fn matches_hash(path: &Path, expected: &str) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).is_ok()
        && hex(&hasher.finalize()).eq_ignore_ascii_case(expected)
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

/// An `AppImage` only runs once it is executable.
fn mark_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("AppImage"))
        {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// What the app can do with a downloaded file on this system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finish {
    /// Hand it to the system installer (Android, Windows).
    Install,
    /// Show it in its folder; the person installs it.
    Reveal,
}

impl Finish {
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(any(target_os = "android", target_os = "windows")) {
            Self::Install
        } else {
            Self::Reveal
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_read_as_lower_case_hex() {
        assert_eq!(
            hex(&Sha256::digest(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// Serve `body` once over HTTP on a local port; the URL of the file.
    fn serve_once(body: &'static [u8]) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut request = [0; 1024];
                let _ = stream.read(&mut request);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body);
            }
        });
        format!("http://127.0.0.1:{port}/notinplus.apk")
    }

    fn asset(url: String, body: &[u8], sha256: String) -> Asset {
        Asset {
            platform: "android".into(),
            arch: "aarch64".into(),
            kind: "apk".into(),
            name: "notinplus.apk".into(),
            url,
            size: body.len() as u64,
            sha256,
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("notein-update-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_download_counts_its_bytes_and_is_reused_once_verified() {
        const BODY: &[u8] = &[7; 200_000];
        let dir = scratch("ok");
        let wanted = asset(serve_once(BODY), BODY, hex(&Sha256::digest(BODY)));
        let progress = Progress::default();
        let path = fetch_into(&dir, &wanted, &progress).expect("download");
        assert_eq!(std::fs::read(&path).expect("file"), BODY);
        assert_eq!(progress.received(), BODY.len() as u64);
        // The server is gone: a second fetch must use the file on disk.
        let again = fetch_into(&dir, &wanted, &Progress::default()).expect("reuse");
        assert_eq!(again, path);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_download_is_thrown_away() {
        const BODY: &[u8] = b"not the release";
        let dir = scratch("bad");
        let wanted = asset(serve_once(BODY), BODY, "00".repeat(32));
        let result = fetch_into(&dir, &wanted, &Progress::default());
        assert!(matches!(result, Err(UpdateError::Checksum)));
        assert_eq!(std::fs::read_dir(&dir).expect("dir").count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cancelled_download_leaves_nothing_behind() {
        const BODY: &[u8] = &[1; 1024];
        let dir = scratch("cancel");
        let wanted = asset(serve_once(BODY), BODY, hex(&Sha256::digest(BODY)));
        let progress = Progress::default();
        progress.cancel();
        let result = fetch_into(&dir, &wanted, &progress);
        assert!(matches!(result, Err(UpdateError::Cancelled)));
        assert_eq!(std::fs::read_dir(&dir).expect("dir").count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_name_never_leaves_the_download_folder() {
        assert_eq!(file_name("../../etc/passwd"), "passwd");
        assert_eq!(file_name("notinplus.apk"), "notinplus.apk");
        assert_eq!(file_name(r"..\x.exe"), "x.exe");
    }
}
