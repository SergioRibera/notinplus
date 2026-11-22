//! `notin-pack <plugin folder> [<out.ntp>]`: build a `.ntp` package.
//!
//! Runs on the author's machine (or CI), where loading the plugin is
//! loading their own code. It:
//!
//! 1. loads the plugin library built for this machine and records what
//!    it registers (document types, Create entries, settings);
//! 2. writes that, plus the SHA-256 of every library file, into
//!    `plugin.ntp.kdl` (lines it wrote before are replaced);
//! 3. zips the folder into `<id>-<version>.ntp`.
//!
//! The app shows those lines before installing, checks the checksum of
//! a library before loading it, and refuses a plugin that registers
//! something else than it declares.

use std::fmt;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use notin_plugin::loader::{self, LoaderError, sha256_hex};
use notin_plugin::manifest::{Checksum, ManifestError, PluginManifest};

const MANIFEST: &str = "plugin.ntp.kdl";
/// Marks the generated block, so a later run replaces it.
const GENERATED: &str = "// Written by notin-pack: do not edit below.";
/// Nodes the tool owns.
const OWNED_NODES: [&str; 4] = ["kind", "create", "settings", "checksum"];

#[derive(Debug)]
enum PackError {
    Usage,
    Read {
        path: PathBuf,
        error: std::io::Error,
    },
    Write {
        path: PathBuf,
        error: std::io::Error,
    },
    Manifest(ManifestError),
    Load(LoaderError),
    MissingLibrary(PathBuf),
    Zip(zip::result::ZipError),
}

impl fmt::Display for PackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage => f.write_str("usage: notin-pack <plugin folder> [<out.ntp>]"),
            Self::Read { path, error } => write!(f, "read {}: {error}", path.display()),
            Self::Write { path, error } => write!(f, "write {}: {error}", path.display()),
            Self::Manifest(err) => write!(f, "manifest: {err}"),
            Self::Load(err) => write!(f, "loading the plugin: {err}"),
            Self::MissingLibrary(path) => {
                write!(
                    f,
                    "{} is listed in the manifest but missing",
                    path.display()
                )
            }
            Self::Zip(err) => write!(f, "zip: {err}"),
        }
    }
}

impl std::error::Error for PackError {}

impl From<zip::result::ZipError> for PackError {
    fn from(err: zip::result::ZipError) -> Self {
        Self::Zip(err)
    }
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(out) => {
            println!("packed {}", out.display());
            std::process::ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("notin-pack: {err}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<PathBuf, PackError> {
    let mut args = std::env::args_os().skip(1);
    let folder = PathBuf::from(args.next().ok_or(PackError::Usage)?);
    let out = args.next().map(PathBuf::from);
    let manifest_path = folder.join(MANIFEST);

    // Drop what an earlier run wrote first: stale checksums would stop
    // the plugin from loading below.
    let text = std::fs::read_to_string(&manifest_path).map_err(|error| PackError::Read {
        path: manifest_path.clone(),
        error,
    })?;
    let authored = authored_part(&text);
    write(&manifest_path, &authored)?;

    let loaded = loader::load(&manifest_path).map_err(PackError::Load)?;
    let declared = loaded.declared();
    let loaded_library = loaded.library_path.clone();
    drop(loaded);

    let manifest = PluginManifest::from_path(&manifest_path).map_err(PackError::Manifest)?;
    let libraries: Vec<String> = if manifest.libraries.is_empty() {
        // No `library` lines: the loader found the default file name.
        vec![relative(&folder, &loaded_library)]
    } else {
        manifest.libraries.iter().map(|l| l.path.clone()).collect()
    };
    let mut generated = format!("\n\n{GENERATED}\n{}", declared.to_kdl());
    for path in libraries {
        let file = folder.join(&path);
        if !file.is_file() {
            return Err(PackError::MissingLibrary(file));
        }
        let sha256 = sha256_hex(&file).map_err(|error| PackError::Read {
            path: file.clone(),
            error,
        })?;
        generated.push_str(&Checksum { path, sha256 }.to_kdl());
    }
    write(
        &manifest_path,
        &format!("{}{generated}", authored.trim_end()),
    )?;

    let out = out.unwrap_or_else(|| {
        folder
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(format!("{}-{}.ntp", manifest.id, manifest.version))
    });
    zip_folder(&folder, &out)?;
    Ok(out)
}

/// `text` without the lines this tool writes.
fn authored_part(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let trimmed = line.trim_start();
        let node = trimmed.split_whitespace().next().unwrap_or_default();
        if trimmed == GENERATED || OWNED_NODES.contains(&node) {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn relative(folder: &Path, file: &Path) -> String {
    file.strip_prefix(folder)
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/")
}

fn write(path: &Path, text: &str) -> Result<(), PackError> {
    std::fs::write(path, text).map_err(|error| PackError::Write {
        path: path.to_path_buf(),
        error,
    })
}

/// Zip every regular file under `folder` (hidden ones and packages
/// aside) into `out`, paths relative to `folder`.
fn zip_folder(folder: &Path, out: &Path) -> Result<(), PackError> {
    let file = std::fs::File::create(out).map_err(|error| PackError::Write {
        path: out.to_path_buf(),
        error,
    })?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut pending = vec![folder.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|error| PackError::Read {
            path: dir.clone(),
            error,
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| PackError::Read {
                path: dir.clone(),
                error,
            })?;
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name.ends_with(".ntp") {
                continue;
            }
            let kind = entry.file_type().map_err(|error| PackError::Read {
                path: path.clone(),
                error,
            })?;
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file() {
                zip.start_file(relative(folder, &path), options)?;
                let bytes = std::fs::read(&path).map_err(|error| PackError::Read {
                    path: path.clone(),
                    error,
                })?;
                zip.write_all(&bytes).map_err(|error| PackError::Write {
                    path: out.to_path_buf(),
                    error,
                })?;
            }
        }
    }
    zip.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn earlier_output_is_replaced() {
        let text = "id \"q\"\nname \"Q\"\n\n// Written by notin-pack: do not edit below.\nkind \"a\" label=\"A\"\nsettings 1\nchecksum \"l.so\" sha256=\"00\"\n";
        assert_eq!(authored_part(text), "id \"q\"\nname \"Q\"\n\n");
    }
}
