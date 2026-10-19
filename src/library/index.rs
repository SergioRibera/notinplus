//! Load / save the [`LibraryIndex`] blob to `data-store`.
//!
//! The whole index is one bincoded blob under a single key — small
//! enough (folders + items + tags without body bytes) that a full
//! rewrite on every mutation stays cheap, and atomic by virtue of the
//! native backend's replace-in-place semantics
//! (`SharedPreferences.commit`, `UserDefaults.setObject`).

use bincode::config::{self, Configuration};
use istmo_data_store::DataStoreClient;

use super::error::{LibraryError, Result};
use super::model::{INDEX_VERSION, LibraryIndex, WorkspaceIndex};

const CODEC: Configuration = config::standard();

/// data-store key the whole blob lives under. The suffix tracks the
/// on-disk schema: a blob written under an older key is never read, so
/// a schema break simply starts the library over.
pub const INDEX_KEY: &str = "library.index.v3";

/// data-store key for the [`WorkspaceIndex`] blob. Kept on its own key
/// so introducing workspaces does not force a bump of [`INDEX_KEY`].
pub const WORKSPACES_KEY: &str = "library.workspaces.v4";

/// data-store namespace used for the library. Matches the app id
/// declared in `istmo.toml` so backends can partition per-app data
/// on shared surfaces (Android multi-user, iOS App Groups).
pub const NAMESPACE: &str = "rs.sergioribera.notinplus";

/// Fetch the persisted index. `None` when the key has never been
/// written or the stored blob carries a schema version this build does
/// not read — the caller starts from a fresh index in both cases.
///
/// # Errors
/// Runtime / data-store / codec failures.
pub async fn load(client: &DataStoreClient) -> Result<Option<LibraryIndex>> {
    let raw = client.get_bytes(INDEX_KEY.to_owned()).await?;
    let Some(bytes) = raw else {
        return Ok(None);
    };
    let (index, _) = bincode::decode_from_slice::<LibraryIndex, _>(&bytes, CODEC)
        .map_err(|e| LibraryError::Codec(e.to_string()))?;
    if index.version != INDEX_VERSION {
        log::warn!(
            "library index version {} does not match {INDEX_VERSION}; starting over",
            index.version
        );
        return Ok(None);
    }
    Ok(Some(index))
}

/// Overwrite the persisted index with `snapshot`.
///
/// # Errors
/// Runtime / data-store / codec failures.
pub async fn save(client: &DataStoreClient, snapshot: &LibraryIndex) -> Result<()> {
    let bytes =
        bincode::encode_to_vec(snapshot, CODEC).map_err(|e| LibraryError::Codec(e.to_string()))?;
    client.set_bytes(INDEX_KEY.to_owned(), bytes).await?;
    Ok(())
}

/// Fetch the persisted workspace index or return a fresh default.
///
/// # Errors
/// Runtime / data-store / codec failures.
pub async fn load_workspaces(client: &DataStoreClient) -> Result<WorkspaceIndex> {
    let raw = client.get_bytes(WORKSPACES_KEY.to_owned()).await?;
    let Some(bytes) = raw else {
        return Ok(WorkspaceIndex::default());
    };
    let (index, _) = bincode::decode_from_slice::<WorkspaceIndex, _>(&bytes, CODEC)
        .map_err(|e| LibraryError::Codec(e.to_string()))?;
    Ok(index)
}

/// Overwrite the persisted workspace index.
///
/// # Errors
/// Runtime / data-store / codec failures.
pub async fn save_workspaces(client: &DataStoreClient, snapshot: &WorkspaceIndex) -> Result<()> {
    let bytes =
        bincode::encode_to_vec(snapshot, CODEC).map_err(|e| LibraryError::Codec(e.to_string()))?;
    client.set_bytes(WORKSPACES_KEY.to_owned(), bytes).await?;
    Ok(())
}
