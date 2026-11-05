## Search in the open translation (src/ui/face/search.rs).

search-title = Search the Bible
search-empty-title = No results
search-empty-body = Try other words or a reference like John 3:16.
search-placeholder = Words or a reference (John 3:16)
search-scope-this = This translation
search-scope-all = All
# Shown until something is typed; the quoted words and OR are query syntax.
search-help = Every word must appear, also as the start of another (love finds lovely). "exact phrase" · faith OR hope · -word to exclude.
# $count is the most matches listed.
search-closest = The { $count } closest
search-results = { $count ->
    [one] 1 result
   *[other] { $count } results
}
# $place is a book, chapter and maybe verse, e.g. “John 3:16”.
search-go-to = Go to { $place }
