ENTRY { title } {} {}
FUNCTION {f} { title nosuchfn #12x "lit"x f 'f }
FUNCTION {f} { skip$ }
EXECUTE {f}
MACRO {jan} {"January"}
MACRO {jan} {"Again"}
INTEGERS { title }
READ
READ
FUNCTION {g} { "ok" write$ newline$ }

ITERATE {g}
REVERSE {nosuch}
ITERATE {title}
SORT
EXECUTE {g
