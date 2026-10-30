//! What went wrong in the sync service, kept typed in the snapshot so
//! Settings words it in the language showing when it is read.

use std::sync::Arc;

use notein_i18n::tr;
use notein_plugin::UserMessage;
use notein_sync::{IdentityError, NetError, SyncError};

use super::replica::ReplicaError;

/// A failed exchange step against the local library.
pub type ExchangeError = SyncError<ReplicaError>;

/// The last problem the sync service ran into.
#[derive(Clone, Debug)]
pub enum SyncProblem {
    /// The service thread could not be spawned.
    Thread(Arc<std::io::Error>),
    ProfileLoad(Arc<IdentityError>),
    ProfileSave(Arc<IdentityError>),
    SharesSave(Arc<std::io::Error>),
    /// The configured server address does not parse.
    BadRelay,
    /// The transport could not start.
    Network(Arc<NetError>),
    Exchange(Arc<ExchangeError>),
    /// A peer runs a build with another sync protocol.
    Outdated {
        name: String,
        version: u16,
    },
}

impl std::fmt::Display for SyncProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Thread(err) => write!(f, "sync thread: {err}"),
            Self::ProfileLoad(err) => write!(f, "load {err}"),
            Self::ProfileSave(err) => write!(f, "save {err}"),
            Self::SharesSave(err) => write!(f, "save share book: {err}"),
            Self::BadRelay => f.write_str("bad relay address"),
            Self::Network(err) => write!(f, "network: {err}"),
            Self::Exchange(err) => write!(f, "{err}"),
            Self::Outdated { name, version } => write!(f, "{name} speaks protocol {version}"),
        }
    }
}

impl UserMessage for SyncProblem {
    fn user_message(&self) -> String {
        match self {
            Self::Thread(err) => tr!("errors-sync-start", "reason" => err.user_message()),
            Self::ProfileLoad(err) => {
                tr!("errors-sync-profile-load", "reason" => identity_message(err))
            }
            Self::ProfileSave(err) => {
                tr!("errors-sync-profile-save", "reason" => identity_message(err))
            }
            Self::SharesSave(err) => tr!("errors-sync-shares-save", "reason" => err.user_message()),
            Self::BadRelay => tr!("errors-sync-bad-relay"),
            Self::Network(err) => {
                let reason = match &**err {
                    NetError::Bind(_) => tr!("errors-sync-network-bind"),
                    NetError::Thread(err) => err.user_message(),
                    NetError::Stopped => tr!("errors-sync-network-stopped"),
                };
                tr!("errors-sync-network", "reason" => reason)
            }
            Self::Exchange(err) => match &**err {
                SyncError::Replica(err) => {
                    tr!("errors-sync-exchange", "reason" => err.user_message())
                }
                SyncError::Codec(_) => tr!("errors-sync-garbled"),
                SyncError::Version(version) => {
                    tr!("errors-sync-version", "version" => *version)
                }
            },
            Self::Outdated { name, version } => tr!(
                "errors-sync-outdated",
                "name" => name.as_str(),
                "version" => *version,
            ),
        }
    }
}

fn identity_message(err: &IdentityError) -> String {
    match err {
        IdentityError::Io(err) => err.user_message(),
        IdentityError::Codec(_) | IdentityError::Version(_) => tr!("errors-sync-profile-damaged"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use notein_i18n::Lang;
    use notein_plugin::UserMessage;

    use super::SyncProblem;

    #[test]
    fn sync_problems_read_in_spanish_without_system_text() {
        Lang::Es.make_current();
        let err = std::io::Error::new(std::io::ErrorKind::StorageFull, "No space left on device");
        let message = SyncProblem::SharesSave(Arc::new(err)).user_message();
        assert!(message.contains("espacio"), "{message}");
        assert!(!message.contains("No space"), "{message}");
        let outdated = SyncProblem::Outdated {
            name: "Tablet".to_owned(),
            version: 3,
        }
        .user_message();
        assert!(outdated.contains("Tablet"), "{outdated}");
    }
}
