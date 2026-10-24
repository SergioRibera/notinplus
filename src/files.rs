//! The system file pickers, as async calls that read or write the
//! whole file.

use std::io::{Read, Write};
use std::sync::Arc;

use istmo_file_picker::{FileFilter, FilePickerClient, FilePickerError, PickConfig, SaveConfig};

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
            Self::Picker(why) => write!(f, "selector de archivos: {why}"),
            Self::Io(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for FileError {}

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

/// Ask where to save and write `bytes` there. Returns the chosen file
/// name; `Ok(None)` when the user cancels.
///
/// # Errors
/// [`FileError`] when the picker or the write fails (no space).
pub async fn save(
    title: &str,
    suggested: &str,
    mime: &str,
    extension: &str,
    bytes: &[u8],
) -> Result<Option<String>, FileError> {
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
    Ok(Some(name))
}
