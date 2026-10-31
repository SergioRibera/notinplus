//! Error surface for [`super::Library`]. Every variant is actionable
//! by the caller, so the enum stays small and each arm carries just
//! enough context for a user-facing toast.

use std::fmt;
use std::io;

use istmo::IstmoError;
use istmo_data_store::DataStoreError;
use notein_i18n::tr;
use notein_plugin::UserMessage;

use super::model::{FolderId, ItemId, LinkId, TagId, WorkspaceId};

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
    /// A link whose two ends are the same item.
    SelfLink,
}

/// Which id-space produced a [`LibraryError::NotFound`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotFoundKind {
    Folder(FolderId),
    Item(ItemId),
    Tag(TagId),
    Workspace(WorkspaceId),
    Link(LinkId),
}

impl fmt::Display for LibraryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Runtime(err) => write!(f, "library runtime error: {err}"),
            Self::Store(err) => write!(f, "library data-store error: {err}"),
            Self::Io(err) => write!(f, "library io error: {err}"),
            Self::Codec(msg) => write!(f, "library codec error: {msg}"),
            Self::NotFound(kind) => match kind {
                NotFoundKind::Folder(id) => write!(f, "folder {id} not found"),
                NotFoundKind::Item(id) => write!(f, "item {id} not found"),
                NotFoundKind::Tag(id) => write!(f, "tag {id} not found"),
                NotFoundKind::Workspace(id) => write!(f, "workspace {id} not found"),
                NotFoundKind::Link(id) => write!(f, "link {id} not found"),
            },
            Self::Cycle => f.write_str("folder move would form a cycle"),
            Self::RootImmutable => f.write_str("root folder is read-only"),
            Self::SelfLink => f.write_str("a link needs two different items"),
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

impl UserMessage for LibraryError {
    fn user_message(&self) -> String {
        match self {
            Self::Runtime(_) => tr!("errors-library-runtime"),
            Self::Store(DataStoreError::Backend(_)) => tr!("errors-library-store"),
            Self::Store(DataStoreError::Corrupted(_)) | Self::Codec(_) => {
                tr!("errors-library-damaged")
            }
            Self::Io(err) => err.user_message(),
            Self::NotFound(kind) => match kind {
                NotFoundKind::Folder(_) => tr!("errors-library-missing-folder"),
                NotFoundKind::Item(_) => tr!("errors-library-missing-item"),
                NotFoundKind::Tag(_) => tr!("errors-library-missing-tag"),
                NotFoundKind::Workspace(_) => tr!("errors-library-missing-desk"),
                NotFoundKind::Link(_) => tr!("errors-library-missing-link"),
            },
            Self::Cycle => tr!("errors-library-cycle"),
            Self::RootImmutable => tr!("errors-library-root"),
            Self::SelfLink => tr!("errors-library-self-link"),
        }
    }
}

#[cfg(test)]
mod tests {
    use notein_i18n::Lang;
    use notein_plugin::UserMessage;

    use super::{LibraryError, NotFoundKind};
    use crate::library::ItemId;

    #[test]
    fn library_errors_explain_themselves_in_spanish() {
        Lang::Es.make_current();
        let damaged = LibraryError::Codec("UnexpectedVariant".to_owned()).user_message();
        assert!(damaged.contains("no se pueden leer"), "{damaged}");
        assert!(!damaged.contains("UnexpectedVariant"), "{damaged}");
        let full = LibraryError::Io(std::io::ErrorKind::StorageFull.into()).user_message();
        assert!(full.contains("espacio"), "{full}");
        let gone = LibraryError::NotFound(NotFoundKind::Item(ItemId([1; 16]))).user_message();
        assert!(gone.contains("ya no existe"), "{gone}");
    }
}
