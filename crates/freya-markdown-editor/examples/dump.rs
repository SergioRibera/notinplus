use freya_markdown_editor::document::Document;
use freya_markdown_editor::markdown::Dialect;

fn main() {
    let source = std::fs::read_to_string(std::env::args().nth(1).expect("path")).expect("read");
    let document = Document::parse(&source);
    println!("{}", document.to_markdown(Dialect::Rich));
    println!("=================== portable");
    println!("{}", document.to_markdown(Dialect::Portable));
}
