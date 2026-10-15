//! biblionlabs downloader.
//!
//! Fetches the catalogue manifest + per-Bible desc + per-book verse
//! bundles and populates the shared `Db`. Entry point:
//! [`install_bible`] drives the whole flow for a single translation.

use crate::db::Db;

#[derive(Debug)]
pub enum ImportError {
    Http(reqwest::Error),
    Json(serde_json::Error),
    Db(crate::db::DbError),
    Format { reason: String },
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(e) => write!(f, "biblionlabs http: {e}"),
            Self::Json(e) => write!(f, "biblionlabs json: {e}"),
            Self::Db(e) => write!(f, "biblionlabs db: {e}"),
            Self::Format { reason } => write!(f, "biblionlabs format: {reason}"),
        }
    }
}

impl std::error::Error for ImportError {}

impl From<reqwest::Error> for ImportError {
    fn from(e: reqwest::Error) -> Self {
        Self::Http(e)
    }
}

impl From<serde_json::Error> for ImportError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

impl From<crate::db::DbError> for ImportError {
    fn from(e: crate::db::DbError) -> Self {
        Self::Db(e)
    }
}

/// Progress event emitted while [`install_bible`] runs. Hooked by the
/// translation-picker UI to drive its progress bar.
#[derive(Clone, Debug)]
pub enum Progress {
    Started { bible_id: String },
    ManifestFetched { total_books: usize },
    BookDone { book_id: String, verses: usize },
    Finished { bible_id: String },
}

/// Install one Bible from the biblionlabs catalogue. `on_progress` is
/// invoked synchronously as each stage completes — callers that need
/// async drive this on a background thread + forward via a channel.
pub fn install_bible(
    _db: &Db,
    _bible_id: &str,
    _manifest_url: &str,
    _desc_url: &str,
    _books_url_template: &str,
    on_progress: impl FnMut(Progress),
) -> Result<(), ImportError> {
    let _ = on_progress;
    Err(ImportError::Format {
        reason: "biblionlabs importer not implemented yet".into(),
    })
}
