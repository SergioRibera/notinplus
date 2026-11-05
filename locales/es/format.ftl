## Fechas, duraciones y tamaños (src/format.rs).

format-today = hoy
format-yesterday = ayer
format-weekday = { $day ->
    [1] lunes
    [2] martes
    [3] miércoles
    [4] jueves
    [5] viernes
    [6] sábado
   *[7] domingo
}
format-date = { $day }/{ $month }
format-date-year = { $day }/{ $month }/{ $year }
format-just-now = Hace un momento
format-minutes-ago = Hace { $minutes } min
format-hours-ago = Hace { $hours } h
format-decimal-separator = ,
