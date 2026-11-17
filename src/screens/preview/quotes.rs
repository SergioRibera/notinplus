//! Quotes of the original text as a note shows them: a word card and
//! verses word by word, Greek left to right and Hebrew right to left.

use freya::prelude::*;
use freya_markdown_editor::prelude::use_editor_controller;
use notin_plugin::kind::{GlossedWord, Interlinear};

use super::render;
use crate::editor::interlinear::InterlinearView;

const WORD_CARD: &str = "### <font face=\"Noto Serif\">Μακάριοι</font> *makárioi*\n\n\
de <font face=\"Noto Serif\">μακάριος</font>, se pronuncia ma-KÁ-ri-os. Adjetivo, plural, masculino.\n\n\
**Aquí:** bienaventurados (los pobres en espíritu)\n\n\
**También:** dichoso (12), feliz (3)";

fn words(words: &[(&str, &str, &str)]) -> Vec<GlossedWord> {
    words
        .iter()
        .map(|(text, reading, gloss)| {
            GlossedWord::new(*text)
                .reading(*reading)
                .gloss(*gloss)
                .link("notein://x")
        })
        .collect()
}

fn greek() -> Interlinear {
    Interlinear::new(words(&[
        ("Μακάριοι", "makárioi", "bienaventurados"),
        ("οἱ", "hoi", "los"),
        ("πτωχοὶ", "ptōchoì", "pobres"),
        ("τῷ", "tō̂", "en el"),
        ("πνεύματι,", "pneúmati", "espíritu"),
        ("ὅτι", "hóti", "porque"),
        ("αὐτῶν", "autō̂n", "de ellos"),
        ("ἐστιν", "estin", "es"),
        ("ἡ", "hē", "el"),
        ("βασιλεία", "basileía", "reino"),
        ("τῶν", "tō̂n", "de los"),
        ("οὐρανῶν.", "ouranō̂n", "cielos"),
    ]))
    .font_family(notin_bible::fonts::GREEK_FAMILY)
}

fn hebrew() -> Interlinear {
    Interlinear::new(words(&[
        ("בְּרֵאשִׁית", "bərēʾšît", "en el principio"),
        ("בָּרָא", "bārāʾ", "creó"),
        ("אֱלֹהִים", "ʾĕlōhîm", "Dios"),
        ("אֵת", "ʾēt", "(objeto)"),
        ("הַשָּׁמַיִם", "haššāmayim", "los cielos"),
        ("וְאֵת", "wəʾēt", "y"),
        ("הָאָרֶץ׃", "hāʾāreṣ", "la tierra"),
    ]))
    .right_to_left(true)
    .font_family(notin_bible::fonts::HEBREW_FAMILY)
}

fn quotes() -> Element {
    let theme = crate::editor::theme(&notin_ui_kit::palette());
    rect()
        .width(Size::fill())
        .padding(24.)
        .spacing(24.)
        .child(WordCard.into_element())
        .child(InterlinearView {
            interlinear: greek(),
            theme: theme.clone(),
        })
        .child(InterlinearView {
            interlinear: hebrew(),
            theme,
        })
        .into_element()
}

#[derive(PartialEq)]
struct WordCard;

impl Component for WordCard {
    fn render(&self) -> impl IntoElement {
        let controller = use_editor_controller();
        let value = use_state(|| WORD_CARD.to_owned());
        crate::editor::editor(value, controller)
            .read_only(true)
            .width(Size::fill())
    }
}

#[test]
fn original_quotes_tablet() {
    render("quotes-original-tablet", (720.0, 640.0), quotes);
}

#[test]
fn original_quotes_phone() {
    render("quotes-original-phone", (390.0, 900.0), quotes);
}
