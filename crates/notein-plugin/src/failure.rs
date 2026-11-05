//! Turning failures into sentences the user can read.
//!
//! Error types keep an English `Display` for logs; the text shown on
//! screen comes from [`UserMessage::user_message`], formatted in the
//! interface language when it is asked for. Store the error itself
//! (or an `Arc` of it) rather than its message, so a language switch
//! re-renders it in the new language.
//!
//! System errors carry text from the OS that cannot be translated, so
//! [`std::io::Error`] maps by its [`ErrorKind`] and network failures
//! by [`NetFailure`].

use std::io::ErrorKind;

use notein_i18n::tr;

/// A failure that can explain itself to the user.
pub trait UserMessage: std::fmt::Debug {
    /// One calm, translated sentence saying what failed and, when it is
    /// obvious, what the user can do about it.
    fn user_message(&self) -> String;
}

impl UserMessage for ErrorKind {
    fn user_message(&self) -> String {
        match self {
            Self::NotFound => tr!("io-not-found"),
            Self::PermissionDenied => tr!("io-permission-denied"),
            Self::StorageFull | Self::QuotaExceeded => tr!("io-storage-full"),
            Self::FileTooLarge => tr!("io-file-too-large"),
            Self::ReadOnlyFilesystem => tr!("io-read-only"),
            Self::AlreadyExists => tr!("io-already-exists"),
            Self::TimedOut => tr!("io-timed-out"),
            Self::ConnectionRefused
            | Self::ConnectionReset
            | Self::ConnectionAborted
            | Self::NotConnected
            | Self::BrokenPipe
            | Self::AddrNotAvailable
            | Self::HostUnreachable
            | Self::NetworkUnreachable
            | Self::NetworkDown => tr!("io-connection"),
            Self::Interrupted | Self::WouldBlock => tr!("io-interrupted"),
            Self::InvalidData | Self::UnexpectedEof => tr!("io-invalid-data"),
            Self::Unsupported => tr!("io-unsupported"),
            Self::ResourceBusy | Self::ExecutableFileBusy => tr!("io-busy"),
            Self::OutOfMemory => tr!("io-out-of-memory"),
            // `ErrorKind` is non-exhaustive; anything else reads as a
            // generic file problem.
            _ => tr!("io-other"),
        }
    }
}

impl UserMessage for std::io::Error {
    fn user_message(&self) -> String {
        self.kind().user_message()
    }
}

/// Why a request over the network failed, in the terms the user cares
/// about. Small and `Copy` so screens can keep it for display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetFailure {
    /// No connection reached the server (offline, DNS, refused).
    Offline,
    /// The server did not answer in time.
    TimedOut,
    /// The server answered with this HTTP status.
    Status(u16),
    /// Anything else (a broken response body, TLS).
    Other,
}

impl NetFailure {
    /// Failure from what a client knows about it; `status` wins over
    /// the connection flags when the server did answer.
    #[must_use]
    pub const fn classify(connect: bool, timeout: bool, status: Option<u16>) -> Self {
        match (status, timeout, connect) {
            (Some(code), _, _) => Self::Status(code),
            (None, true, _) => Self::TimedOut,
            (None, false, true) => Self::Offline,
            (None, false, false) => Self::Other,
        }
    }
}

impl std::fmt::Display for NetFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Offline => f.write_str("could not connect"),
            Self::TimedOut => f.write_str("timed out"),
            Self::Status(code) => write!(f, "server answered {code}"),
            Self::Other => f.write_str("network error"),
        }
    }
}

impl std::error::Error for NetFailure {}

impl UserMessage for NetFailure {
    fn user_message(&self) -> String {
        match self {
            Self::Offline => tr!("net-offline"),
            Self::TimedOut => tr!("net-timed-out"),
            Self::Status(code) => tr!("net-server-error", "status" => *code),
            Self::Other => tr!("net-other"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;

    use notein_i18n::Lang;

    use super::{NetFailure, UserMessage};

    #[test]
    fn io_errors_read_by_kind_in_spanish() {
        Lang::Es.make_current();
        let full = std::io::Error::new(ErrorKind::StorageFull, "No space left on device");
        assert!(
            full.user_message().contains("espacio"),
            "{}",
            full.user_message()
        );
        let missing = std::io::Error::from(ErrorKind::NotFound);
        assert!(
            missing.user_message().contains("encontr"),
            "{}",
            missing.user_message()
        );
        let denied = ErrorKind::PermissionDenied.user_message();
        assert!(denied.contains("permiso"), "{denied}");
    }

    #[test]
    fn unknown_io_kinds_fall_back_to_a_generic_sentence() {
        Lang::Es.make_current();
        assert_eq!(
            ErrorKind::Other.user_message(),
            ErrorKind::IsADirectory.user_message()
        );
        assert!(!ErrorKind::Other.user_message().contains("io-other"));
    }

    #[test]
    fn network_failures_classify_status_first() {
        assert_eq!(
            NetFailure::classify(true, true, Some(503)),
            NetFailure::Status(503)
        );
        assert_eq!(NetFailure::classify(true, true, None), NetFailure::TimedOut);
        assert_eq!(NetFailure::classify(true, false, None), NetFailure::Offline);
        assert_eq!(NetFailure::classify(false, false, None), NetFailure::Other);
    }

    #[test]
    fn server_errors_name_the_status() {
        Lang::Es.make_current();
        let message = NetFailure::Status(503).user_message();
        assert!(message.contains("503"), "{message}");
        assert!(NetFailure::Offline.user_message().contains("conexión"));
    }
}
