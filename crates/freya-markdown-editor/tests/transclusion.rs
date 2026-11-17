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

#[test]
fn a_chosen_colour_travels_on_the_marker() {
    let colored = LIVE.replacen("[!app:live]", "[!app:live color=#3b82f6]", 1);
    let document = Document::parse(&colored);
    let BlockKind::Transclusion(transclusion) = only(&document) else {
        panic!("{:?}", document.blocks());
    };
    assert_eq!(transclusion.kind, "app:live");
    assert_eq!(transclusion.color(), Some(Rgba::rgb(0x3b, 0x82, 0xf6)));
    assert_eq!(
        transclusion.fallback,
        "³ Y dijo Dios: Sea la luz.\n\notra línea"
    );
    assert_eq!(document.to_markdown(Dialect::Rich).trim(), colored);
}

#[test]
fn settings_nobody_knows_are_kept_as_written() {
    let marked = LIVE.replacen(
        "[!app:live]",
        "[!app:live hide=pronunciation,grammar x-plugin=7]",
        1,
    );
    let document = Document::parse(&marked);
    let BlockKind::Transclusion(transclusion) = only(&document) else {
        panic!("{:?}", document.blocks());
    };
    assert_eq!(transclusion.attr("hide"), Some("pronunciation,grammar"));
    assert_eq!(transclusion.attr("x-plugin"), Some("7"));
    assert_eq!(transclusion.color(), None);
    assert_eq!(document.to_markdown(Dialect::Rich).trim(), marked);
}

#[test]
fn stray_marker_words_and_bad_colours_are_ignored() {
    let odd = LIVE.replacen("[!app:live]", "[!app:live loose color=nope]", 1);
    let document = Document::parse(&odd);
    let BlockKind::Transclusion(transclusion) = only(&document) else {
        panic!("{:?}", document.blocks());
    };
    assert_eq!(transclusion.attr("loose"), None);
    assert_eq!(transclusion.attr("color"), Some("nope"));
    assert_eq!(transclusion.color(), None);
    assert_eq!(
        transclusion.fallback,
        "³ Y dijo Dios: Sea la luz.\n\notra línea"
    );
}

#[test]
fn settings_are_set_and_dropped_in_place() {
    let BlockKind::Transclusion(mut transclusion) = only(&Document::parse(LIVE)).clone() else {
        panic!("not a transclusion");
    };
    assert!(transclusion.set_attr("color", Some("#c80000")));
    assert!(transclusion.set_attr("hide", Some("a,b")));
    assert!(transclusion.set_attr("color", Some("#00c800")));
    assert_eq!(
        transclusion.attrs,
        [
            ("color".to_owned(), "#00c800".to_owned()),
            ("hide".to_owned(), "a,b".to_owned())
        ]
    );
    assert!(!transclusion.set_attr("bad key", Some("x")));
    assert!(!transclusion.set_attr("hide", Some("a b")));
    assert!(transclusion.set_attr("color", None));
    assert_eq!(transclusion.attr("color"), None);
}
