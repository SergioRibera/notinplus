//! App-side helpers for wording failures; the shared vocabulary
//! ([`UserMessage`], [`NetFailure`]) lives in `notin-plugin`.

use notin_plugin::NetFailure;
#[cfg(doc)]
use notin_plugin::UserMessage;

/// What a `reqwest` failure means to the user.
pub(crate) trait NetFailureExt {
    fn net_failure(&self) -> NetFailure;
}

impl NetFailureExt for reqwest::Error {
    fn net_failure(&self) -> NetFailure {
        NetFailure::classify(
            self.is_connect(),
            self.is_timeout(),
            self.status().map(|status| status.as_u16()),
        )
    }
}
