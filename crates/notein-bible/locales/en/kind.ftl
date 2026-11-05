## The Bible document kind: create entries, names and labels the shell shows (src/kind.rs).

kind-default-name = Bible
# $abbrev is the translation's short name, e.g. KJV.
kind-default-name-of = Bible { $abbrev }
kind-label = Bible
kind-plural = Bible
kind-search-group = In the Bible
# $name is the translation's name.
kind-meta-highlights = { $count ->
    [one] { $name } · 1 highlight
   *[other] { $name } · { $count } highlights
}
kind-action-change-translation = Change translation
kind-entry-label = Bible
kind-entry-subtitle = Read and highlight
kind-sheet-title = New Bible
kind-sheet-name-hint = E.g. Study Bible
kind-choose-title = Open Bible
kind-choose-add-translation = Add translation
kind-choice-label = Translation
