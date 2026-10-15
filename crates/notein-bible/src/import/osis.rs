//! OSIS XML import.
//!
//! Shape placeholder so the public import surface compiles alongside
//! the biblionlabs pipeline. Actual XML walk + verse / header / `<p>`
//! extraction lands next.

use crate::db::Db;

#[derive(Debug)]
pub enum OsisError {
    Io(std::io::Error),
    Db(crate::db::DbError),
    Format { reason: String },
}

impl std::fmt::Display for OsisError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "osis io: {e}"),
            Self::Db(e) => write!(f, "osis db: {e}"),
            Self::Format { reason } => write!(f, "osis format: {reason}"),
        }
    }
}

impl std::error::Error for OsisError {}

impl From<std::io::Error> for OsisError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<crate::db::DbError> for OsisError {
    fn from(e: crate::db::DbError) -> Self {
        Self::Db(e)
    }
}

pub fn import_osis_xml(_db: &Db, _bible_id: &str, _xml_bytes: &[u8]) -> Result<(), OsisError> {
    Err(OsisError::Format {
        reason: "OSIS import not implemented yet".into(),
    })
}
