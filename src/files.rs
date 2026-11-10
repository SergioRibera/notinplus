//! The system file pickers, as async calls that read or write the
//! whole file.

use std::io::{Read, Write};
use std::sync::Arc;

use istmo_file_picker::{FileFilter, FilePickerClient, FilePickerError, PickConfig, SaveConfig};
use notein_i18n::tr;
use notein_plugin::UserMessage;

/// A file the user chose, read into memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picked {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// Why a file could not be read or written.
#[derive(Debug)]
pub enum FileError {
    Picker(String),
    Io(std::io::Error),
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Picker(why) => write!(f, "file picker: {why}"),
            Self::Io(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for FileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Picker(_) => None,
        }
    }
}

impl UserMessage for FileError {
    fn user_message(&self) -> String {
        match self {
            Self::Picker(_) => tr!("errors-file-picker"),
            Self::Io(err) => err.user_message(),
        }
    }
}

impl From<std::io::Error> for FileError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<FilePickerError> for FileError {
    fn from(err: FilePickerError) -> Self {
        Self::Picker(format!("{err:?}"))
    }
}

fn client() -> Result<FilePickerClient, FileError> {
    FilePickerClient::acquire().map_err(|err| FileError::Picker(err.to_string()))
}

fn filter(extensions: &[&str], mime_types: &[&str]) -> FileFilter {
    FileFilter {
        mime_types: mime_types.iter().map(|m| (*m).to_owned()).collect(),
        extensions: extensions.iter().map(|e| (*e).to_owned()).collect(),
        uti_types: Vec::new(),
    }
}

/// Ask for one file and read it. `Ok(None)` when the user cancels.
///
/// # Errors
/// [`FileError`] when the picker or the read fails.
#[sismo::function]
pub async fn pick(
    title: &str,
    extensions: &[&str],
    mime_types: &[&str],
) -> Result<Option<Picked>, FileError> {
    let picker = client()?;
    let config = PickConfig {
        dialog_title: Some(title.to_owned()),
        filter: filter(extensions, mime_types),
        start_directory_hint: None,
        allow_multiple: false,
    };
    let Some(file) = picker
        .pick_file_owned(config)
        .await
        .map_err(|e| FileError::Picker(e.to_string()))?
    else {
        return Ok(None);
    };
    let name = file.display_name.clone();
    let file = Arc::new(file);
    let mut reader = file.open_reader(&picker).await?;
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    Ok(Some(Picked { name, bytes }))
}

/// A file [`save`] wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    /// Name the system shows for it.
    pub name: String,
    /// Where it lives, when the system lets the app know (Linux): what
    /// "Mostrar en la carpeta" opens.
    pub path: Option<std::path::PathBuf>,
}

/// Ask where to save and write `bytes` there. `Ok(None)` when the user
/// cancels.
///
/// # Errors
/// [`FileError`] when the picker or the write fails (no space).
#[sismo::function]
pub async fn save(
    title: &str,
    suggested: &str,
    mime: &str,
    extension: &str,
    bytes: &[u8],
) -> Result<Option<Saved>, FileError> {
    let picker = client()?;
    let config = SaveConfig {
        dialog_title: Some(title.to_owned()),
        suggested_name: Some(suggested.to_owned()),
        mime_type: Some(mime.to_owned()),
        filter: filter(&[extension], &[mime]),
        start_directory_hint: None,
        overwrite_prompt: true,
    };
    let Some(file) = picker
        .save_file_owned(config)
        .await
        .map_err(|e| FileError::Picker(e.to_string()))?
    else {
        return Ok(None);
    };
    let name = file.display_name.clone();
    let file = Arc::new(file);
    let mut writer = file.open_writer(&picker).await?;
    writer.write_all(bytes)?;
    writer.flush()?;
    Ok(Some(Saved {
        name,
        path: written_path(&writer),
    }))
}

/// The path behind the descriptor the picker handed out. Only Linux
/// names it (`/proc/self/fd`); elsewhere the descriptor may point into
/// a document provider with no folder to show.
#[cfg(target_os = "linux")]
fn written_path(file: &impl std::os::fd::AsRawFd) -> Option<std::path::PathBuf> {
    std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))
        .ok()
        .filter(|path| path.is_absolute() && path.exists())
}

#[cfg(not(target_os = "linux"))]
const fn written_path<F>(_file: &F) -> Option<std::path::PathBuf> {
    None
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    #[test]
    fn an_open_file_names_its_path() {
        let path = std::env::temp_dir().join(format!("notein-saved-{}.notein", std::process::id()));
        let file = std::fs::File::create(&path).expect("create");
        let named = super::written_path(&file);
        let _ = std::fs::remove_file(&path);
        assert_eq!(named, Some(path));
    }
}

/// Largest file the system may hand to the app to open.
pub const OPENED_MAX_BYTES: u64 = 512 * 1024 * 1024;

/// Read a file the system asked the app to open, at most `cap` bytes.
///
/// Its path comes from a double-click, a forwarded launch or a share, not
/// from a picker, so only regular files are read: a FIFO would block and
/// a device or `/proc` entry may never end.
///
/// # Errors
/// The file cannot be opened, is not a regular file or is larger than
/// `cap`.
pub fn read_opened(path: &std::path::Path, cap: u64) -> std::io::Result<Vec<u8>> {
    use std::io::{Error, ErrorKind};

    #[cfg(windows)]
    if let Some(std::path::Component::Prefix(prefix)) = path.components().next() {
        use std::path::Prefix;
        // `\\.\` device paths, `\\?\` paths that are not plain disk paths
        // and `\\host\pipe\…` name devices and pipes, not documents.
        let pipe_share =
            matches!(prefix.kind(), Prefix::UNC(_, share) if share.eq_ignore_ascii_case("pipe"));
        if pipe_share
            || matches!(
                prefix.kind(),
                Prefix::DeviceNS(_) | Prefix::Verbatim(_) | Prefix::VerbatimUNC(..)
            )
        {
            return Err(Error::new(ErrorKind::InvalidInput, "not a document path"));
        }
    }
    // Checked before opening: opening a FIFO for reading blocks until a
    // writer shows up.
    if !std::fs::metadata(path)?.is_file() {
        return Err(Error::new(ErrorKind::InvalidInput, "not a regular file"));
    }
    let file = std::fs::File::open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(Error::new(ErrorKind::InvalidInput, "not a regular file"));
    }
    if meta.len() > cap {
        return Err(Error::new(ErrorKind::InvalidData, "file too large"));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
    file.take(cap + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > cap {
        return Err(Error::new(ErrorKind::InvalidData, "file too large"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod opened_tests {
    use super::*;

    #[test]
    fn opened_files_must_be_small_regular_files() {
        let dir = std::env::temp_dir().join(format!("notinplus-opened-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.md");
        std::fs::write(&file, b"hello").unwrap();
        assert_eq!(read_opened(&file, 5).unwrap(), b"hello");
        assert!(read_opened(&file, 4).is_err(), "over the cap");
        assert!(read_opened(&dir, 1024).is_err(), "a directory");
        #[cfg(unix)]
        assert!(
            read_opened(std::path::Path::new("/dev/zero"), 1024).is_err(),
            "a device"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
