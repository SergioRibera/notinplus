use freya_markdown_editor::prelude::*;

const LIVE: &str = "> [!app:live]\n>\n> ³ Y dijo Dios: Sea la luz.\n>\n> otra línea\n>\n> — [Génesis 1:3 (RVR1960)](app://bible/gen-1-3)";

fn only(document: &Document) -> &BlockKind {
    let blocks: Vec<_> = document
        .blocks()
        .iter()
        .filter(|block| !matches!(&block.kind, BlockKind::Paragraph(content) if content.is_empty()))
        .collect();
    assert_eq!(blocks.len(), 1, "{blocks:?}");
    &blocks[0].kind
}

#[test]
fn a_marked_quote_reads_as_a_transclusion() {
    let document = Document::parse(LIVE);
    let BlockKind::Transclusion(transclusion) = only(&document) else {
        panic!("{:?}", document.blocks());
    };
    assert_eq!(transclusion.kind, "app:live");
    assert_eq!(transclusion.url, "app://bible/gen-1-3");
    assert_eq!(transclusion.label, "Génesis 1:3 (RVR1960)");
    assert_eq!(
        transclusion.fallback,
        "³ Y dijo Dios: Sea la luz.\n\notra línea"
    );
}

#[test]
fn a_transclusion_writes_back_the_same_markdown() {
    let document = Document::parse(LIVE);
    assert_eq!(document.to_markdown(Dialect::Rich).trim(), LIVE);
}

#[test]
fn content_on_the_marker_line_is_kept() {
    let document = Document::parse("> [!app:live]\n> pegado\n>\n> — [Fuente](app://x)");
    let BlockKind::Transclusion(transclusion) = only(&document) else {
        panic!("{:?}", document.blocks());
    };
    assert_eq!(transclusion.fallback, "pegado");
}

#[test]
fn portable_markdown_keeps_a_plain_quote() {
    let portable = Document::parse(LIVE).to_markdown(Dialect::Portable);
    assert!(!portable.contains("[!app:live]"), "{portable}");
    assert!(portable.trim().starts_with("> ³ Y dijo Dios"), "{portable}");
}

#[test]
fn alerts_and_quotes_without_a_source_stay_what_they_were() {
    let alert = Document::parse("> [!NOTE]\n> cuidado");
    assert!(matches!(only(&alert), BlockKind::Callout { .. }));
    let unsourced = Document::parse("> [!app:live]\n>\n> sin fuente");
    assert!(
        unsourced
            .blocks()
            .iter()
            .all(|block| !matches!(block.kind, BlockKind::Transclusion(_)))
    );
}
