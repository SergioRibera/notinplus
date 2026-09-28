//! Error surface for [`super::Library`]. Every variant is actionable
//! by the caller, so the enum stays small and each arm carries just
//! enough context for a user-facing toast.

use std::fmt;
use std::io;

use istmo::IstmoError;
use istmo_data_store::DataStoreError;

use super::model::{FolderId, ItemId, TagId};

/// One-off result alias so call sites don't have to spell the error
/// type on every method.
pub type Result<T> = std::result::Result<T, LibraryError>;

/// Error returned by every mutating method on [`super::Library`].
#[derive(Debug)]
pub enum LibraryError {
    /// Runtime rejected the data-store call (plugin not declared,
    /// runtime shutting down, …).
    Runtime(IstmoError),
    /// Native data-store backend rejected the operation (disk /
    /// permission failure, corrupted value, …).
    Store(DataStoreError),
    /// Filesystem I/O against the on-disk body / PDF blob failed.
    Io(io::Error),
    /// bincode encode / decode failure. Almost always means an older
    /// version of the app wrote a shape we no longer understand.
    Codec(String),
    /// Referenced id does not exist in the index. Carries the id so
    /// the caller can decide whether to refresh its cached list or
    /// surface an error.
    NotFound(NotFoundKind),
    /// A folder move that would place a folder inside its own
    /// subtree.
    Cycle,
    /// Root folder is implicit; user cannot rename / delete / colour
    /// it.
    RootImmutable,
}

/// Which id-space produced a [`LibraryError::NotFound`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotFoundKind {
    Folder(FolderId),
    Item(ItemId),
    Tag(TagId),
}

impl fmt::Display for LibraryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Runtime(err) => write!(f, "library runtime error: {err}"),
            Self::Store(err) => write!(f, "library data-store error: {err}"),
            Self::Io(err) => write!(f, "library io error: {err}"),
            Self::Codec(msg) => write!(f, "library codec error: {msg}"),
            Self::NotFound(kind) => match kind {
                NotFoundKind::Folder(id) => write!(f, "folder {} not found", id.0),
                NotFoundKind::Item(id) => write!(f, "item {} not found", id.0),
                NotFoundKind::Tag(id) => write!(f, "tag {} not found", id.0),
            },
            Self::Cycle => f.write_str("folder move would form a cycle"),
            Self::RootImmutable => f.write_str("root folder is read-only"),
        }
    }
}

impl std::error::Error for LibraryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Runtime(err) => Some(err),
            Self::Store(err) => Some(err),
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<IstmoError> for LibraryError {
    fn from(err: IstmoError) -> Self {
        Self::Runtime(err)
    }
}

impl From<DataStoreError> for LibraryError {
    fn from(err: DataStoreError) -> Self {
        Self::Store(err)
    }
}

impl From<io::Error> for LibraryError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}
