ENTRY { title author } {} { s }
INTEGERS { i n }
STRINGS { t }
FUNCTION {w} { write$ newline$ }
FUNCTION {fmts}
{ 'n :=
  "  n=" n int.to.str$ * w
  author n "{ff~}{vv~}{ll}{, jj}" format.name$ "  a:" swap$ * w
  author n "{f.~}{vv~}{ll}{, jj}" format.name$ "  b:" swap$ * w
  author n "{vv~}{ll}{, f.}{, jj}" format.name$ "  c:" swap$ * w
  author n "{ll}" format.name$ "  d:" swap$ * w
  author n "{f{}}" format.name$ "  e:" swap$ * w
  author n "{l{-}}" format.name$ "  f:" swap$ * w
  author n "{vv{ } }{ll{ }}{  f{ }}{  jj{ }}" format.name$ "  g:" swap$ * w
  author n "{f{.}.}{ll~}x{{vv}}" format.name$ "  h:" swap$ * w
  author n "{F.}{L}{ ff{-}~}" format.name$ "  i:" swap$ * w
}
FUNCTION {probe}
{ "@" cite$ * w
  title 't :=
  t purify$ "  purify:" swap$ * w
  t text.length$ int.to.str$ "  tlen:" swap$ * w
  #0 'i :=
  { i #8 < }
  { t i text.prefix$ "  pre" i int.to.str$ * ":" * swap$ * w  i #1 + 'i := }
  while$
  t width$ int.to.str$ "  width:" swap$ * w
  t "t" change.case$ "  ct:" swap$ * w
  t "l" change.case$ "  cl:" swap$ * w
  t "u" change.case$ "  cu:" swap$ * w
  t add.period$ "  ap:" swap$ * w
  t #1 #3 substring$ "  s1:" swap$ * w
  t #-1 #3 substring$ "  s2:" swap$ * w
  t #2 #100 substring$ "  s3:" swap$ * w
  t #-3 #2 substring$ "  s4:" swap$ * w
  t #0 #1 substring$ "  s5:" swap$ * w
  t #5 #-1 substring$ "  s6:" swap$ * w
  t #-100 #3 substring$ "  s7:" swap$ * w
  t empty$ int.to.str$ "  empty:" swap$ * w
  author missing$ 
    { "  noauthor" w }
    { author num.names$ 'i :=
      "  names:" i int.to.str$ * w
      #1 'n :=
      { n i #1 + < } { n fmts n #1 + 'n := } while$ 
      author purify$ "  apur:" swap$ * w
      author "t" change.case$ "  act:" swap$ * w
    }
  if$
  title sort.key$ := 
}
READ
ITERATE {probe}
SORT
FUNCTION {so} { cite$ " " * sort.key$ * w }
ITERATE {so}
