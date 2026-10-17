//! Print a markdown file in both dialects.
//!
//! `cargo run -p freya-markdown-editor --example convert -- notes.md`

use freya_markdown_editor::document::Document;
use freya_markdown_editor::markdown::Dialect;

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: convert <file.md>");
        return;
    };
    let source = match std::fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("cannot read {path}: {error}");
            return;
        }
    };
    let document = Document::parse(&source);
    println!("{}", document.to_markdown(Dialect::Rich));
    println!("---------- portable ----------");
    println!("{}", document.to_markdown(Dialect::Portable));
}
