use freya_markdown_editor::document::{
    Align, Atom, BlockKind, CalloutKind, ColumnAlign, Document, HeadingLevel, ListKind, MarkKind,
    Rgba,
};
use freya_markdown_editor::markdown::{Dialect, Extensions, ParseOptions};

const SAMPLE: &str = r#"---
title: Sample
---

# Title {#top}

[TOC]

Some **bold**, *italic*, ~~strike~~, ==mark==, `code`, H~2~O, x^2^ and ||secret||.
A [link](https://example.com "Example") and an ![image](pic.png).
Line one\
line two with <u>underline</u> and <span style="color: #ff0000; font-family: Fira Code">red code font</span>.

<div align="center">

Centered **text**

</div>

- one
- two
  - nested
    1. deep
- [x] done
- [ ] todo

3. three
4. four

> quoted *words*

> [!WARNING] Careful
> This is a callout.

```rust
fn main() {}
```

$$
E = mc^2
$$

Inline math $a + b$ and a footnote[^note].

| Name | Value |
|:-----|------:|
| a    | 1     |
| b \| c | **2** |

Term
: Definition text

---

<details>raw html</details>

[^note]: The footnote body.

*[HTML]: HyperText Markup Language
"#;

fn shape(document: &Document) -> Vec<(BlockKind, Align)> {
    document
        .blocks()
        .iter()
        .map(|block| (block.kind.clone(), block.align))
        .collect()
}

#[test]
fn rich_dialect_round_trips() {
    let first = Document::parse(SAMPLE);
    let written = first.to_markdown(Dialect::Rich);
    let second = Document::parse(&written);
    assert_eq!(shape(&first), shape(&second), "rewritten:\n{written}");
    assert_eq!(first.abbreviations, second.abbreviations);
    assert_eq!(
        written,
        second.to_markdown(Dialect::Rich),
        "output is stable"
    );
}

#[test]
fn sample_lowers_to_expected_blocks() {
    let document = Document::parse(SAMPLE);
    let kinds: Vec<&BlockKind> = document.blocks().iter().map(|block| &block.kind).collect();

    assert!(matches!(kinds[0], BlockKind::FrontMatter(raw) if raw.text() == "title: Sample"));
    assert!(matches!(
        kinds[1],
        BlockKind::Heading { level: HeadingLevel::H1, anchor: Some(anchor), .. } if anchor == "top"
    ));
    assert_eq!(kinds[2], &BlockKind::TableOfContents);

    let BlockKind::Paragraph(rich) = kinds[3] else {
        panic!("expected paragraph, got {:?}", kinds[3]);
    };
    let families: Vec<_> = rich.marks().iter().map(|mark| mark.kind.family()).collect();
    for expected in [
        MarkKind::Strong,
        MarkKind::Emphasis,
        MarkKind::Strikethrough,
        MarkKind::Highlight,
        MarkKind::Code,
        MarkKind::Subscript,
        MarkKind::Superscript,
        MarkKind::Spoiler,
        MarkKind::Underline,
        MarkKind::Color(Rgba::rgb(255, 0, 0)),
        MarkKind::FontFamily("Fira Code".into()),
    ] {
        assert!(
            families.contains(&expected.family()),
            "missing {expected:?} in {families:?}"
        );
    }
    assert!(rich.text().contains("one\nline two"));
    assert!(matches!(rich.atoms(), [Atom::Image { url, .. }] if url == "pic.png"));

    assert_eq!(document.blocks()[4].align, Align::Center);

    let lists: Vec<(ListKind, u8)> = kinds
        .iter()
        .filter_map(|kind| match kind {
            BlockKind::ListItem { kind, depth, .. } => Some((*kind, *depth)),
            _ => None,
        })
        .collect();
    assert_eq!(
        lists,
        vec![
            (ListKind::Bullet, 0),
            (ListKind::Bullet, 0),
            (ListKind::Bullet, 1),
            (ListKind::Ordered { start: 1 }, 2),
            (ListKind::Task { checked: true }, 0),
            (ListKind::Task { checked: false }, 0),
            (ListKind::Ordered { start: 3 }, 0),
            (ListKind::Ordered { start: 3 }, 0),
        ]
    );

    assert!(kinds.iter().any(|kind| matches!(kind, BlockKind::Quote(_))));
    assert!(kinds.iter().any(|kind| matches!(
        kind,
        BlockKind::Callout { kind: CalloutKind::Warning, title: Some(title), .. } if title == "Careful"
    )));
    assert!(
        kinds
            .iter()
            .any(|kind| matches!(kind, BlockKind::Code { language, .. } if language == "rust"))
    );
    assert!(
        kinds
            .iter()
            .any(|kind| matches!(kind, BlockKind::Math(tex) if tex.text() == "E = mc^2"))
    );
    let table = kinds
        .iter()
        .find_map(|kind| match kind {
            BlockKind::Table(table) => Some(table),
            _ => None,
        })
        .expect("table");
    assert_eq!(table.columns(), &[ColumnAlign::Left, ColumnAlign::Right]);
    assert_eq!(table.rows()[2][0].text(), "b | c");
    assert!(kinds.iter().any(|kind| matches!(kind, BlockKind::Term(_))));
    assert!(
        kinds
            .iter()
            .any(|kind| matches!(kind, BlockKind::Definition(_)))
    );
    assert!(kinds.iter().any(|kind| matches!(kind, BlockKind::Divider)));
    assert!(kinds.iter().any(|kind| matches!(kind, BlockKind::Html(_))));
    assert!(
        kinds
            .iter()
            .any(|kind| matches!(kind, BlockKind::Footnote { label, .. } if label == "note"))
    );
    assert_eq!(document.abbreviations.len(), 1);
}

#[test]
fn portable_dialect_is_plain_gfm() {
    let document = Document::parse(SAMPLE);
    let portable = document.to_markdown(Dialect::Portable);
    for forbidden in [
        "<span",
        "==mark==",
        "||secret||",
        "[TOC]",
        "{#top}",
        "*[HTML]",
        "<div",
    ] {
        assert!(
            !portable.contains(forbidden),
            "portable output still has `{forbidden}`:\n{portable}"
        );
    }
    assert!(portable.contains("- [Title](#top)"));
    let reparsed = Document::parse_with(&portable, &ParseOptions::from(Extensions::gfm()));
    assert!(reparsed.blocks().len() > 10);
}

#[test]
fn embeds_are_claimed_by_filter() {
    let options = ParseOptions {
        embed: Some(std::rc::Rc::new(|url: &str| url.starts_with("app://"))),
        ..ParseOptions::default()
    };
    let document = Document::parse_with(
        "see [Chapter](app://doc/1) and [web](https://x.y)",
        &options,
    );
    let BlockKind::Paragraph(content) = &document.blocks()[0].kind else {
        panic!("paragraph");
    };
    assert!(matches!(
        content.atoms(),
        [Atom::Embed { url, label, .. }] if url == "app://doc/1" && label == "Chapter"
    ));
    assert!(
        content
            .marks()
            .iter()
            .any(|mark| matches!(mark.kind, MarkKind::Link(_)))
    );
    assert_eq!(
        document.to_markdown(Dialect::Rich),
        "see [Chapter](app://doc/1) and [web](https://x.y)\n"
    );
}

#[test]
fn empty_input_still_has_an_editable_block() {
    let document = Document::parse("");
    assert_eq!(document.blocks().len(), 1);
    assert_eq!(document.to_markdown(Dialect::Rich), "");
}

#[test]
fn plain_text_strips_markup() {
    let document =
        Document::parse("# Hi\n\nsome **bold** [link](x)\n\n| a | b |\n|---|---|\n| 1 | 2 |");
    assert_eq!(document.plain_text(), "Hi\nsome bold link\na | b\n1 | 2");
}
