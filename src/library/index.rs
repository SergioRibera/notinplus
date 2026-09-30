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
use super::model::LibraryIndex;

const CODEC: Configuration = config::standard();

/// data-store key the whole blob lives under. Suffix version bumps
/// track breaking schema changes so a stale key can be detected /
/// migrated instead of silently misread.
pub const INDEX_KEY: &str = "library.index.v1";

/// data-store namespace used for the library. Matches the app id
/// declared in `istmo.toml` so backends can partition per-app data
/// on shared surfaces (Android multi-user, iOS App Groups).
pub const NAMESPACE: &str = "rs.sergioribera.notinplus";

/// Fetch the persisted index or return a fresh default when the key
/// has never been written.
///
/// # Errors
/// Runtime / data-store / codec failures. A missing key is not an
/// error — the library starts empty on first launch.
pub async fn load(client: &DataStoreClient) -> Result<LibraryIndex> {
    let raw = client.get_bytes(INDEX_KEY.to_owned()).await?;
    let Some(bytes) = raw else {
        return Ok(LibraryIndex::default());
    };
    let (index, _) = bincode::decode_from_slice::<LibraryIndex, _>(&bytes, CODEC)
        .map_err(|e| LibraryError::Codec(e.to_string()))?;
    Ok(index)
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
