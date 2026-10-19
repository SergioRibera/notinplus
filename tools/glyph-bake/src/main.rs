//! Offline baker for `notein-handwriting`.

mod error;
mod eval;
mod fonts;
mod select;
mod uji;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use notein_handwriting::TemplateSet;

use crate::error::BakeError;
use crate::select::{Extraction, medoids};
use crate::uji::Split;

pub(crate) const USAGE: &str = "\
usage:
  glyph-bake templates <ujipenchars2.txt> [--per-class N] [--out FILE]
  glyph-bake eval      <ujipenchars2.txt> [--per-class N[,N...]]
  glyph-bake fonts     <font.ttf>... [--out-dir DIR]

--per-class 0 keeps every sample. Outputs default to the app's
assets/handwriting/.";

const DEFAULT_PER_CLASS: &str = "16";

/// Parsed command line.
#[derive(Debug)]
enum Command {
    Templates {
        dataset: PathBuf,
        per_class: usize,
        out: PathBuf,
    },
    Eval {
        dataset: PathBuf,
        per_class: Vec<usize>,
    },
    Fonts {
        fonts: Vec<PathBuf>,
        out_dir: PathBuf,
    },
}

impl Command {
    fn parse(mut args: impl Iterator<Item = String>) -> Result<Self, BakeError> {
        let usage = |message: &str| BakeError::Usage(message.to_owned());
        let name = args.next().ok_or_else(|| usage("missing command"))?;
        let mut positional = Vec::new();
        let mut per_class = None;
        let mut out = None;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--per-class" => {
                    per_class = Some(
                        args.next()
                            .ok_or_else(|| usage("--per-class needs a value"))?,
                    );
                }
                "--out" | "--out-dir" => {
                    out = Some(PathBuf::from(
                        args.next().ok_or_else(|| usage("--out needs a value"))?,
                    ));
                }
                _ => positional.push(PathBuf::from(arg)),
            }
        }
        let per_class: Vec<usize> = per_class
            .as_deref()
            .unwrap_or(DEFAULT_PER_CLASS)
            .split(',')
            .map(|count| {
                count
                    .trim()
                    .parse()
                    .map_err(|_| usage("--per-class takes non-negative integers"))
            })
            .collect::<Result<_, _>>()?;
        let dataset = || match positional.as_slice() {
            [path] => Ok(path.clone()),
            _ => Err(usage("expected exactly one dataset path")),
        };
        match name.as_str() {
            "templates" => match per_class.as_slice() {
                [per_class] => Ok(Self::Templates {
                    dataset: dataset()?,
                    per_class: *per_class,
                    out: out.unwrap_or_else(|| assets().join("templates.rkyv")),
                }),
                _ => Err(usage("templates takes a single --per-class value")),
            },
            "eval" => Ok(Self::Eval {
                dataset: dataset()?,
                per_class,
            }),
            "fonts" if !positional.is_empty() => Ok(Self::Fonts {
                fonts: positional,
                out_dir: out.unwrap_or_else(|| assets().join("fonts")),
            }),
            "fonts" => Err(usage("expected at least one font")),
            other => Err(usage(&format!("unknown command `{other}`"))),
        }
    }

    fn run(self) -> Result<(), BakeError> {
        match self {
            Self::Templates {
                dataset,
                per_class,
                out,
            } => {
                let samples = uji::load(&dataset)?;
                let set = TemplateSet::new(medoids(
                    Extraction::Decomposed.templates(&samples),
                    per_class,
                ));
                let written = write(&out, &set.to_bytes()?)?;
                println!(
                    "{} templates from {} samples → {} ({written} bytes)",
                    set.templates.len(),
                    samples.len(),
                    out.display()
                );
            }
            Self::Eval { dataset, per_class } => {
                let (train, test): (Vec<_>, Vec<_>) = uji::load(&dataset)?
                    .into_iter()
                    .partition(|sample| sample.split == Split::Train);
                eval::run(&train, &test, &per_class);
            }
            Self::Fonts { fonts, out_dir } => {
                for font in fonts {
                    let (ink, missing) = fonts::bake(&font, uji::GLYPHS.chars())?;
                    let stem = font.file_stem().unwrap_or_default().to_string_lossy();
                    let out = out_dir.join(format!("{}.rkyv", fonts::kebab(&stem)));
                    let written = write(&out, &ink.to_bytes()?)?;
                    let missing: String = missing.into_iter().collect();
                    println!(
                        "{}: {} glyphs → {} ({written} bytes); missing: {missing}",
                        ink.name,
                        ink.glyphs.len(),
                        out.display()
                    );
                }
            }
        }
        Ok(())
    }
}

/// The app's `assets/handwriting/`, two levels up from this crate.
fn assets() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/handwriting")
}

fn write(path: &Path, bytes: &[u8]) -> Result<usize, BakeError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(BakeError::io(parent))?;
    }
    std::fs::write(path, bytes).map_err(BakeError::io(path))?;
    Ok(bytes.len())
}

fn main() -> ExitCode {
    match Command::parse(std::env::args().skip(1)).and_then(Command::run) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("glyph-bake: {error}");
            ExitCode::FAILURE
        }
    }
}
