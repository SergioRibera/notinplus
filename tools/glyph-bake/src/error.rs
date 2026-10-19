use std::fmt;
use std::path::PathBuf;

use notein_handwriting::HandwritingError;

#[derive(Debug)]
pub(crate) enum BakeError {
    Usage(String),
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Malformed {
        path: PathBuf,
        line: usize,
        reason: &'static str,
    },
    Font {
        path: PathBuf,
        source: ttf_parser::FaceParsingError,
    },
    Archive(HandwritingError),
}

impl BakeError {
    pub(crate) fn io(path: impl Into<PathBuf>) -> impl FnOnce(std::io::Error) -> Self {
        let path = path.into();
        move |source| Self::Io { path, source }
    }
}

impl fmt::Display for BakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) => write!(f, "{message}\n\n{}", crate::USAGE),
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Malformed { path, line, reason } => {
                write!(f, "{}:{line}: {reason}", path.display())
            }
            Self::Font { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Archive(source) => write!(f, "failed to archive: {source}"),
        }
    }
}

impl std::error::Error for BakeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Usage(_) | Self::Malformed { .. } => None,
            Self::Io { source, .. } => Some(source),
            Self::Font { source, .. } => Some(source),
            Self::Archive(source) => Some(source),
        }
    }
}

impl From<HandwritingError> for BakeError {
    fn from(source: HandwritingError) -> Self {
        Self::Archive(source)
    }
}
