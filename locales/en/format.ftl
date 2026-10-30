## Dates, durations and sizes (src/format.rs).

format-today = today
format-yesterday = yesterday
# $day: ISO weekday, Monday = 1.
format-weekday = { $day ->
    [1] Monday
    [2] Tuesday
    [3] Wednesday
    [4] Thursday
    [5] Friday
    [6] Saturday
   *[7] Sunday
}
# $day and $month are zero-padded; $year has two digits.
format-date = { $month }/{ $day }
format-date-year = { $month }/{ $day }/{ $year }
format-just-now = Just now
format-minutes-ago = { $minutes } min ago
format-hours-ago = { $hours } h ago
format-decimal-separator = .
