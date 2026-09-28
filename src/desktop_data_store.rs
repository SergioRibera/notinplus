//! Desktop-only [`DataStore`] backend.
//!
//! `istmo-data-store` ships native backends for Android
//! (`SharedPreferences`) and iOS (`UserDefaults`); on desktop the crate
//! only exposes the client. This module fills the gap with a tiny
//! filesystem-backed implementation so [`crate::library::Library`] can
//! open the same way on every platform.
//!
//! Layout: one bincoded file per namespace under
//! [`istmo::path::data_dir`]`/data_store/<namespace>.bin`. Each file
//! carries a `HashMap<String, Value>` — the `Value` tag disambiguates
//! the typed getters (`get_string`, `get_i64`, …). A wrong-type read
//! surfaces as [`DataStoreError::Corrupted`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use bincode::config::standard;
use bincode::{Decode, Encode};
use istmo_data_store::{DataStore, DataStoreConfig, DataStoreError, DataStoreFactory, DataStoreHost};

/// Register the desktop backend on `runtime`. Returns the host so
/// the caller can hand it to `runtime.register_host(...)`.
#[must_use]
pub fn host() -> DataStoreHost<DesktopFactory> {
    DataStoreHost::new(DesktopFactory)
}

/// Zero-state factory — the namespace + on-disk path are derived per
/// instance from the [`DataStoreConfig`] the runtime hands over.
#[derive(Debug, Default)]
pub struct DesktopFactory;

impl DataStoreFactory for DesktopFactory {
    type Instance = DesktopStore;

    fn create(&self, config: DataStoreConfig) -> DesktopStore {
        DesktopStore::open(config.namespace)
    }
}

#[derive(Debug, Encode, Decode, Clone)]
enum Value {
    Str(String),
    I64(i64),
    F64(f64),
    Bool(bool),
    Bytes(Vec<u8>),
}

/// Namespaced key-value store. All mutations flush the whole map back
/// to disk — fine for UI-side config volumes, wrong choice for
/// large-value or high-frequency workloads.
#[derive(Debug)]
pub struct DesktopStore {
    path: PathBuf,
    map: Mutex<HashMap<String, Value>>,
}

impl DesktopStore {
    fn open(namespace: String) -> Self {
        let dir = istmo::path::data_dir().join("data_store");
        let path = dir.join(format!("{namespace}.bin"));
        let map = load(&path).unwrap_or_default();
        Self {
            path,
            map: Mutex::new(map),
        }
    }

    fn with_map<R>(&self, f: impl FnOnce(&mut HashMap<String, Value>) -> R) -> R {
        let mut guard = self.map.lock().unwrap_or_else(|p| p.into_inner());
        f(&mut guard)
    }

    fn flush(&self) -> Result<(), DataStoreError> {
        let bytes = self.with_map(|m| {
            bincode::encode_to_vec(&*m, standard())
                .map_err(|e| DataStoreError::Backend(format!("encode: {e}")))
        })?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| DataStoreError::Backend(format!("mkdir {}: {e}", parent.display())))?;
        }
        std::fs::write(&self.path, bytes)
            .map_err(|e| DataStoreError::Backend(format!("write {}: {e}", self.path.display())))
    }
}

fn load(path: &std::path::Path) -> Option<HashMap<String, Value>> {
    let bytes = std::fs::read(path).ok()?;
    let (map, _) = bincode::decode_from_slice::<HashMap<String, Value>, _>(&bytes, standard()).ok()?;
    Some(map)
}

fn wrong_type(kind: &str, key: &str) -> DataStoreError {
    DataStoreError::Corrupted(format!("key `{key}` is not a {kind}"))
}

impl DataStore for DesktopStore {
    async fn get_string(&self, key: String) -> Result<Option<String>, DataStoreError> {
        self.with_map(|m| match m.get(&key) {
            None => Ok(None),
            Some(Value::Str(s)) => Ok(Some(s.clone())),
            Some(_) => Err(wrong_type("string", &key)),
        })
    }

    async fn set_string(&self, key: String, value: String) -> Result<(), DataStoreError> {
        self.with_map(|m| m.insert(key, Value::Str(value)));
        self.flush()
    }

    async fn get_i64(&self, key: String) -> Result<Option<i64>, DataStoreError> {
        self.with_map(|m| match m.get(&key) {
            None => Ok(None),
            Some(Value::I64(v)) => Ok(Some(*v)),
            Some(_) => Err(wrong_type("i64", &key)),
        })
    }

    async fn set_i64(&self, key: String, value: i64) -> Result<(), DataStoreError> {
        self.with_map(|m| m.insert(key, Value::I64(value)));
        self.flush()
    }

    async fn get_f64(&self, key: String) -> Result<Option<f64>, DataStoreError> {
        self.with_map(|m| match m.get(&key) {
            None => Ok(None),
            Some(Value::F64(v)) => Ok(Some(*v)),
            Some(_) => Err(wrong_type("f64", &key)),
        })
    }

    async fn set_f64(&self, key: String, value: f64) -> Result<(), DataStoreError> {
        self.with_map(|m| m.insert(key, Value::F64(value)));
        self.flush()
    }

    async fn get_bool(&self, key: String) -> Result<Option<bool>, DataStoreError> {
        self.with_map(|m| match m.get(&key) {
            None => Ok(None),
            Some(Value::Bool(v)) => Ok(Some(*v)),
            Some(_) => Err(wrong_type("bool", &key)),
        })
    }

    async fn set_bool(&self, key: String, value: bool) -> Result<(), DataStoreError> {
        self.with_map(|m| m.insert(key, Value::Bool(value)));
        self.flush()
    }

    async fn get_bytes(&self, key: String) -> Result<Option<Vec<u8>>, DataStoreError> {
        self.with_map(|m| match m.get(&key) {
            None => Ok(None),
            Some(Value::Bytes(b)) => Ok(Some(b.clone())),
            Some(_) => Err(wrong_type("bytes", &key)),
        })
    }

    async fn set_bytes(&self, key: String, value: Vec<u8>) -> Result<(), DataStoreError> {
        self.with_map(|m| m.insert(key, Value::Bytes(value)));
        self.flush()
    }

    async fn remove(&self, key: String) -> Result<bool, DataStoreError> {
        let existed = self.with_map(|m| m.remove(&key).is_some());
        if existed {
            self.flush()?;
        }
        Ok(existed)
    }

    async fn contains(&self, key: String) -> Result<bool, DataStoreError> {
        Ok(self.with_map(|m| m.contains_key(&key)))
    }

    async fn keys(&self) -> Result<Vec<String>, DataStoreError> {
        Ok(self.with_map(|m| m.keys().cloned().collect()))
    }

    async fn clear(&self) -> Result<(), DataStoreError> {
        self.with_map(HashMap::clear);
        self.flush()
    }
}
