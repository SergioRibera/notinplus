## The Bible reader: header actions, chapter bar, book picker, selection bar and cross-references (src/ui/face*, src/ui/bottom_nav.rs).

reader-action-translation = Translation
reader-action-search = Search the Bible
reader-loading-chapter = Loading chapter…
reader-nav-previous-chapter = Previous chapter
reader-nav-next-chapter = Next chapter
reader-nav-previous-passage = Previous passage
reader-nav-next-passage = Next passage
reader-books-title = Books
reader-selection-verses = { $count ->
    [one] 1 verse
   *[other] { $count } verses
}
reader-selection-verse = Verse { $verse }
# $first and $last are verse numbers of one chapter.
reader-selection-verse-range = Verses { $first }–{ $last }
reader-selection-share = Share
reader-refs-count = { $count ->
    [one] 1 reference
   *[other] { $count } references
}
# $place is the verse the references start from, e.g. “John 3:16”.
reader-refs-title = References of { $place }
reader-refs-close = Close
