//! Full featured editor: toolbar, slash menu, mentions, a custom embed
//! scheme and a live view of the markdown it writes.
//!
//! `cargo run -p freya-markdown-editor --example playground`

use freya::prelude::*;
use freya_markdown_editor::prelude::*;

const WELCOME: &str = r"# Welcome to the playground

Type `/` for blocks, `@` to mention someone, or select text and use the toolbar.
Markdown shortcuts work too: `# `, `- `, `1. `, `[ ] `, `> `, `**bold**`, `==mark==`.

> [!TIP]
> Links to `app://` are rendered as chips: [Roadmap](app://doc/roadmap).

- [x] CommonMark and GFM
- [ ] Your next idea #todo
";

fn main() {
    launch(
        LaunchConfig::new().with_window(
            WindowConfig::new(app)
                .with_size(1100., 780.)
                .with_title("freya-markdown-editor"),
        ),
    );
}

fn app() -> impl IntoElement {
    let markdown = use_state(|| WELCOME.to_owned());
    let controller = use_editor_controller();
    let theme = EditorTheme::dark();
    let surface = theme.colors.surface;
    let muted = theme.colors.muted;

    rect()
        .expanded()
        .background(surface)
        .horizontal()
        .content(Content::flex())
        .child(
            rect()
                .width(Size::flex(2.))
                .height(Size::fill())
                .padding(16.)
                .spacing(12.)
                .child(MarkdownToolbar::new(controller))
                .child(
                    ScrollView::new().expanded().child(
                        MarkdownEditor::new(markdown)
                            .controller(controller)
                            .theme(theme)
                            .placeholder("Start writing…")
                            .slash_commands()
                            .trigger(mentions())
                            .decorator(PrefixTokens::new(
                                '#',
                                SpanStyle::default().color((168, 85, 247)).bold(),
                            ))
                            .decorator(SchemeEmbed::new("app", |embed| {
                                rect()
                                    .height(Size::px(22.))
                                    .padding((2., 10.))
                                    .corner_radius(11.)
                                    .background((52, 64, 104))
                                    .center()
                                    .child(
                                        label()
                                            .font_size(12.)
                                            .color((210, 222, 255))
                                            .text(format!("◆ {}", embed.label)),
                                    )
                                    .into_element()
                            }))
                            .on_link(|url| println!("open {url}"))
                            .width(Size::fill())
                            .padding((0., 0., 80., 0.)),
                    ),
                ),
        )
        .child(
            rect()
                .width(Size::flex(1.))
                .height(Size::fill())
                .padding(16.)
                .background((24, 26, 31))
                .child(label().color(muted).font_size(12.).text("MARKDOWN"))
                .child(
                    ScrollView::new().expanded().child(
                        label()
                            .font_family("monospace")
                            .font_size(12.)
                            .color((190, 196, 206))
                            .text(markdown.read().clone()),
                    ),
                ),
        )
}

fn mentions() -> Trigger {
    const PEOPLE: [&str; 4] = [
        "Ada Lovelace",
        "Alan Turing",
        "Grace Hopper",
        "Linus Torvalds",
    ];
    Trigger::new('@', |query| {
        let query = query.to_lowercase();
        PEOPLE
            .iter()
            .filter(|name| name.to_lowercase().contains(&query))
            .map(|name| {
                Suggestion::new(
                    *name,
                    SuggestionAction::InsertLink {
                        label: format!("@{name}"),
                        url: format!("https://people.example/{}", name.replace(' ', "-")),
                    },
                )
                .icon("@")
            })
            .collect()
    })
}
