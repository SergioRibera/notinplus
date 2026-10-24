# freya-markdown-editor

Block based rich Markdown editor and viewer for [Freya](https://freyaui.dev).

Markdown is the source of truth: the editor parses a string, lets people
edit it like a word processor (Notion style blocks, toolbar, shortcuts) and
writes markdown back after every change.

```rust
use freya::prelude::*;
use freya_markdown_editor::prelude::*;

fn app() -> impl IntoElement {
    let notes = use_state(|| String::from("# Hello\n\nStart **typing**"));
    let controller = use_editor_controller();

    rect()
        .expanded()
        .child(MarkdownToolbar::new(controller))
        .child(
            MarkdownEditor::new(notes)
                .controller(controller)
                .slash_menu()
                .placeholder("Write something…")
                .padding(16.)
                .expanded(),
        )
}
```

Run the playground: `cargo run -p freya-markdown-editor --example playground`.

## Pieces

| Item | What it is |
|------|------------|
| `MarkdownEditor` | The component. Editable or read only. Implements `LayoutExt`, `ContainerExt` (size, position, padding, constraints) and `TextStyleExt` (font family, size, color...) like any Freya element. |
| `EditorController` | `Copy` handle to the shared state. Every toolbar button is a call on it, so custom toolbars, menus and shortcuts use the same API. |
| `MarkdownToolbar` | Ready made Word style bar (feature `toolbar`, on by default). |
| `Decorator` | Hooks that map markdown constructs to your own visuals. |
| `SuggestionSource` | Sections of the `/` menu (blocks, references, mentions...). |
| `document` | The editable model: flat blocks, inline marks and atoms. |
| `markdown` | Parsing (comrak) and writing in two dialects. |

## Edit and read only

```rust,ignore
MarkdownEditor::new(value).read_only(true);   // forced by the prop
controller.set_read_only(true);               // or switched at runtime
controller.toggle_read_only();
```

In read only mode links (`on_link`) open on click, spoilers are hidden,
and decorators may replace code blocks (e.g. render `mermaid`).

## Controller API

```rust,ignore
let controller = use_editor_controller();

// inline formatting (selection, or the next typed text with a bare caret)
controller.toggle_mark(MarkKind::Strong);
controller.set_color(Some(Rgba::rgb(239, 68, 68)));
controller.set_background(Some(Rgba::rgb(250, 204, 21)));
controller.set_font_family(Some("Fira Code".into()));
controller.set_font_size(Some(20));
controller.set_link(Some("https://freyaui.dev".into()));
controller.clear_formatting();

// blocks
controller.set_block_type(BlockType::Heading(HeadingLevel::H2));
controller.set_align(Align::Center);
controller.indent();
controller.insert_table(3, 3);
controller.insert_block(BlockKind::Divider);

// content and history
controller.insert_text("hello");
controller.insert_embed("Roadmap", "app://doc/roadmap");
controller.insert_markdown("**pasted** markdown");
controller.undo();

// state for your own UI (reading subscribes the component)
let format: ActiveFormat = controller.active_format();
format.has(MarkFamily::Strong);
format.block_type;
format.color();
let markdown = controller.markdown(Dialect::Portable);
```

## Decorators

```rust,ignore
MarkdownEditor::new(value)
    // `app://...` links become atomic chips rendered by your closure
    .decorator(SchemeEmbed::new("app", |embed| {
        rect().height(Size::px(20.)).child(embed.label.to_owned()).into_element()
    }))
    // style `#tags`
    .decorator(PrefixTokens::new('#', SpanStyle::default().bold()))
```

Implement `Decorator` for anything else: `claims_link` + `render_embed`
(atomic widgets for URLs), `link_style`, `match_text` (style any text
pattern), `render_code_block` (e.g. diagrams) and `render_image`.

Embedded widgets are inline paragraph children: give them an explicit
height.

## The `/` menu

Everything lives behind one `/` menu (typed at the start of a word). It is
built from sources, each one a titled section:

```rust,ignore
MarkdownEditor::new(value)
    // your own section, `/at ada` scopes the menu to it
    .suggestions(
        SuggestionGroup::new("People", |query| {
            people_matching(query)
                .map(|name| Suggestion::new(name, SuggestionAction::InsertText(format!("@{name}"))))
                .collect()
        })
        .keyword("at"),
    )
    // the built in `BlockSuggestions` (headings, lists, code, tables...),
    // captioned with the theme labels
    .slash_menu()
```

Implement `SuggestionSource` for richer sources. Actions: insert text,
rich inline, link, embed, change block type, insert a block, or
`SuggestionAction::Run` to do anything through the controller (e.g. update
what the link points to before inserting it). Arrow keys
navigate, Enter/Tab pick, Escape closes. The list is virtualized and the
query results are cached, so large sources stay fast; on touch screens it
scrolls by dragging and only a tap picks.

## Formatting boundaries and markdown source

Typing right after bold, italic, a color... continues it, like in a word
processor. To leave it:

- press the shortcut again (Ctrl+B) or use the toolbar,
- press Right at the end of the block,
- or delete back to its end: what you type next is not formatted.

With `.syntax_reveal(true)` the formatting touching the caret is shown as
its markdown source (`**bold**`, `[link](url)`, `` `code` ``...) and edited
as text, like Obsidian or Typora: delete a `*` and it is plain text again,
type after the closing `**` and it is not bold. Styling that only exists
as HTML (underline, colors, fonts, sizes) keeps being edited visually.

In this mode the formatting shortcuts (Ctrl+B, I, E, Shift+X, Shift+H, `.`,
`,`) with no selection type the delimiter pair around the caret, like
Obsidian; pressing it again on an empty pair removes it. With a selection
they format it as usual.

```rust,ignore
MarkdownEditor::new(value).syntax_reveal(settings.show_markdown);
```

## Reordering blocks

```rust,ignore
MarkdownEditor::new(value).reorderable(true); // drag handle on every block

controller.move_block(id, -1);          // by offset
controller.move_block_to(id, 0);        // to an index
controller.move_block_onto(id, other);  // where `other` is
controller.move_current_block(1);       // the block holding the caret
```

Ctrl+Shift+Up/Down moves the current block too.

## Markdown support

Parsing uses [comrak]: `CommonMark`, GFM (tables, task lists,
strikethrough, autolinks, footnotes, alerts) and extensions: math (`$`,
`$$`, ```` ```math ````), `==highlight==`, `^sup^`, `~sub~`, `||spoiler||`,
definition lists, front matter, heading ids `{#id}`, emoji shortcodes
(feature `emoji`), abbreviations `*[HTML]: ...`, and `[TOC]`.

Inline HTML the editor understands: `<u>`, `<ins>`, `<mark>`, `<sup>`,
`<sub>`, `<kbd>`, `<s>`, `<del>`, `<b>`, `<i>`, `<br>`, `<font>` and
`<span style>` with `color`, `background-color`, `font-family`,
`font-size`. `<div align>` around blocks sets alignment.

### Dialects

- `Dialect::Rich` (default): lossless for everything the editor can do.
  Colors, fonts and sizes travel as `<span style>`, underline as `<u>`,
  alignment as `<div align>`, extensions keep their syntax.
- `Dialect::Portable`: plain `CommonMark` + GFM that renders anywhere.
  Editor only styling is dropped and `[TOC]` expands into a list of
  heading links.

```rust,ignore
MarkdownEditor::new(value).dialect(Dialect::Portable);
let export = controller.markdown(Dialect::Portable);
```

### Model normalization

The editor is block based, so nested markdown is flattened: list items
carry a depth, a multi paragraph quote becomes consecutive quote blocks,
and blocks nested inside list items are lifted out. Empty paragraphs exist
while editing but are not written.

## Keyboard

| Keys | Action |
|------|--------|
| Ctrl+B / I / U / E | Bold, italic, underline, inline code |
| Ctrl+Shift+X, Ctrl+Shift+H | Strikethrough, highlight |
| Ctrl+. / Ctrl+, | Superscript, subscript |
| Ctrl+K / Ctrl+Shift+K | Link selection to the URL in the clipboard / unlink |
| Ctrl+\\ | Clear formatting |
| Ctrl+Alt+0..6 | Paragraph, heading 1..6 |
| Ctrl+Alt+7 / 8 / 9 | Numbered, bulleted, to-do list |
| Ctrl+Z, Ctrl+Y / Ctrl+Shift+Z | Undo, redo |
| Enter / Shift+Enter | New block / line break |
| Ctrl+Enter | Paragraph after the block (leave any block) |
| Enter on an empty last line | Leave a code, math or raw block |
| Ctrl+Shift+Up / Down | Move the current block |
| Right at the end of a block | Stop the formatting that ends there |
| Tab / Shift+Tab | Indent list items, move between table cells |
| Backspace at start | Turn block into text, then merge with the previous |
| Arrows at edges | Move between blocks |
| Ctrl+click | Open link while editing |

Typing shortcuts: `# `..`###### `, `- `, `* `, `1. `, `[ ] `, `[x] `, `> `,
```` ``` ````, `---`, `$$`, and inline `**bold**`, `*italic*`, `` `code` ``,
`~~strike~~`, `==mark==`, `||spoiler||`, `^sup^`, `~sub~`.

## Theming

`EditorTheme` (colors, sizes, fonts, toolbar palette, font list) can be
passed with `.theme(...)` or provided as context so every editor and
toolbar below picks it up. `EditorTheme::dark()` and `EditorTheme::light()`
are included.

The text the editor and toolbar show on their own (tooltips, menus, block
captions, the `/` menu) comes from `EditorTheme::labels`, English by
default. Translate it with `Labels::from_fn`, matching on every `Label`
so a new label fails the build until it is translated:

```rust,ignore
theme.labels = Labels::from_fn(|label| match label {
    Label::Undo => "Deshacer (Ctrl+Z)",
    // ...
});
```

## Limitations

- The document always ends with a paragraph when its last block is not one
  (code, list, table...), so there is always a place to keep writing.
  Empty paragraphs are not written to markdown.

- Selections never span more than one block.
- Images render as a labelled pill unless a decorator renders them.
- Diagrams and math are shown as source unless a decorator renders them.

[comrak]: https://github.com/kivikakk/comrak
