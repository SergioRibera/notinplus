## Library names made by the app (src/library.rs).

# Stored as the name of a duplicated document.
library-item-copy-name = { $name } (copy)
# Stored as the name of a duplicated desk.
library-desk-copy-name = Copy of { $name }
library-empty-desk = Empty desk
# Title of an unnamed desk: its first document's name and how many more it holds.
library-desk-title-more = { $count ->
    [one] { $first } and 1 more
   *[other] { $first } and { $count } more
}
