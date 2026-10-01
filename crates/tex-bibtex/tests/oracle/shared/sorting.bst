ENTRY { title } { n } { lab }
INTEGERS { i }
STRINGS { g }
FUNCTION {setkey} { title "u" change.case$ #1 #1 substring$ 'sort.key$ := }
FUNCTION {rev} { title 'sort.key$ := }
FUNCTION {pr} { cite$ " " * sort.key$ * write$ newline$ }
FUNCTION {big} { "" 'g := #0 'i := { i #120 < } { g "abcde" * 'g := i #1 + 'i := } while$ g 'lab := lab text.length$ int.to.str$ write$ newline$ }
FUNCTION {errs} { #1 "a" + pop$ "x" #2 * pop$ 'pr 'big = pop$ title 'pr := "zz" chr.to.int$ pop$ #200 int.to.chr$ pop$ "q" "x" change.case$ pop$ "{" width$ pop$ "}}" purify$ pop$ "a } b" num.names$ pop$ "A and B" #3 "{ff}" format.name$ pop$ "A B" #1 "{ff}{z}" format.name$ pop$ #5 }
READ
ITERATE {rev}
SORT
ITERATE {setkey}
SORT
ITERATE {pr}
ITERATE {big}
ITERATE {errs}
EXECUTE {errs}
FUNCTION {tail} { "unterminated last line" write$ }
EXECUTE {tail}
