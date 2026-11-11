//! Downloading the packs: the manifest names the files and their
//! hashes; each lands decompressed in [`super::dir`].

use std::io::Read as _;
use std::path::Path;

use reqwest::blocking::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::{META_BUILT, Testament};
use crate::db::Db;
use crate::install_background::InstallFailure;

/// Where `originals-pack` publishes its output.
pub const MANIFEST_URL: &str = "https://github.com/biblionlabs/extra_data_source/releases/download/originals-latest/originals.json";

/// Environment variable that points the download at another manifest
/// (a local build served from `dist/`, for testing).
pub const MANIFEST_URL_ENV: &str = "NOTEIN_ORIGINALS_URL";

/// The manifest to download, [`MANIFEST_URL`] unless
/// [`MANIFEST_URL_ENV`] names another.
#[must_use]
pub fn manifest_url() -> String {
    std::env::var(MANIFEST_URL_ENV)
        .ok()
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| MANIFEST_URL.to_owned())
}

#[derive(Debug, Deserialize)]
struct Manifest {
    format_version: u16,
    built_unix: u64,
    packs: Vec<PackEntry>,
    senses: Vec<SensesEntry>,
}

#[derive(Debug, Deserialize)]
struct PackEntry {
    testament: String,
    #[serde(flatten)]
    file: FileEntry,
}

#[derive(Debug, Deserialize)]
struct SensesEntry {
    language: String,
    #[serde(flatten)]
    file: FileEntry,
}

#[derive(Debug, Deserialize)]
struct FileEntry {
    file: String,
    sha256: String,
    bytes: u64,
}

#[derive(Debug)]
pub enum DownloadError {
    Http(reqwest::Error),
    Io(std::io::Error),
    /// The manifest is not what this reader expects.
    Manifest(String),
    /// A file's hash did not match the manifest.
    Hash(String),
    Db(crate::db::DbError),
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(err) => write!(f, "originals download: {err}"),
            Self::Io(err) => write!(f, "originals download: {err}"),
            Self::Manifest(reason) => write!(f, "originals manifest: {reason}"),
            Self::Hash(file) => write!(f, "originals download: {file} does not match its hash"),
            Self::Db(err) => write!(f, "originals download: {err}"),
        }
    }
}

impl std::error::Error for DownloadError {}

impl From<reqwest::Error> for DownloadError {
    fn from(err: reqwest::Error) -> Self {
        Self::Http(err)
    }
}

impl From<std::io::Error> for DownloadError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<crate::db::DbError> for DownloadError {
    fn from(err: crate::db::DbError) -> Self {
        Self::Db(err)
    }
}

impl From<DownloadError> for InstallFailure {
    fn from(err: DownloadError) -> Self {
        match err {
            DownloadError::Http(err) => Self::Net(notin_sdk::NetFailure::classify(
                err.is_connect(),
                err.is_timeout(),
                err.status().map(|status| status.as_u16()),
            )),
            DownloadError::Io(err) => Self::Disk(err.kind()),
            DownloadError::Manifest(_) | DownloadError::Hash(_) => Self::BadData,
            DownloadError::Db(err) => Self::Database(std::sync::Arc::new(err)),
        }
    }
}

/// Bring every pack and senses file the manifest lists to `dir`,
/// skipping the ones already there with the right hash. `on_progress`
/// gets `(files done, files total)`.
#[cfg_attr(feature = "sismo", sismo::function("bible::originals_download"))]
pub fn ensure(
    db: &Db,
    client: &Client,
    dir: &Path,
    mut on_progress: impl FnMut(usize, usize),
) -> Result<(), DownloadError> {
    let manifest_url = manifest_url();
    let manifest: Manifest = client
        .get(&manifest_url)
        .send()?
        .error_for_status()?
        .json()?;
    if manifest.format_version != originals_format::FORMAT_VERSION {
        return Err(DownloadError::Manifest(format!(
            "format {} published, this reader wants {}",
            manifest.format_version,
            originals_format::FORMAT_VERSION
        )));
    }
    let base = manifest_url.rsplit_once('/').map_or("", |(base, _)| base);
    let mut wanted: Vec<(FileEntry, String)> = Vec::new();
    for pack in manifest.packs {
        let testament = match pack.testament.as_str() {
            "ot" => Testament::Old,
            "nt" => Testament::New,
            other => {
                return Err(DownloadError::Manifest(format!(
                    "unknown testament {other}"
                )));
            }
        };
        wanted.push((pack.file, super::pack_file(testament).to_owned()));
    }
    for senses in manifest.senses {
        if notin_i18n::Lang::ALL
            .iter()
            .any(|lang| lang.tag() == senses.language)
        {
            let local = super::senses_file(&senses.language);
            wanted.push((senses.file, local));
        }
    }
    std::fs::create_dir_all(dir)?;
    let total = wanted.len();
    on_progress(0, total);
    for (done, (entry, local)) in wanted.into_iter().enumerate() {
        let key = format!("originals_sha:{local}");
        let have = db.meta(&key)?;
        let path = dir.join(&local);
        if path.exists() && have.as_deref() == Some(entry.sha256.as_str()) {
            on_progress(done + 1, total);
            continue;
        }
        let url = format!("{base}/{}", entry.file);
        log::info!("originals: downloading {url} ({} bytes)", entry.bytes);
        let compressed = client.get(&url).send()?.error_for_status()?.bytes()?;
        let digest = Sha256::digest(&compressed);
        let hex = digest.iter().fold(String::new(), |mut hex, byte| {
            use std::fmt::Write as _;
            let _ = write!(hex, "{byte:02x}");
            hex
        });
        if !hex.eq_ignore_ascii_case(&entry.sha256) {
            return Err(DownloadError::Hash(entry.file));
        }
        let mut bytes = Vec::new();
        flate2::read::GzDecoder::new(compressed.as_ref()).read_to_end(&mut bytes)?;
        let part = path.with_extension("part");
        std::fs::write(&part, &bytes)?;
        std::fs::rename(&part, &path)?;
        db.set_meta(&key, &entry.sha256)?;
        on_progress(done + 1, total);
    }
    db.set_meta(META_BUILT, &manifest.built_unix.to_string())?;
    Ok(())
}
