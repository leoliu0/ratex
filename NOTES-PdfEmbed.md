# NOTES-PdfEmbed.md — PDF shipout completion (PdfEmbed-4)

## Session result
PDF shipout pipeline completed end-to-end. Acceptance driver `/tmp/pdfhello`
(path-dep on tex-core) writes `/tmp/hello.pdf`: 2 pages, cmr10 fully
embedded (PFB -> /FontFile with Length1/2/3), /Encoding Differences from a
.enc vector, Widths/FirstChar/LastChar from TFM, URI link annotation, named
destination in /Names tree, /Outlines entry, /Info from \pdfinfo, MediaBox
from \pdfpagewidth/height, \pdfsavepos -> \pdflastxpos/\pdflastypos.
Validated: `pdfinfo` 0 errors, `pdffonts` shows `CMR10 Type 1 emb=yes`,
`mutool clean` parses clean, `pdftotext` extracts "Hello, PDF World! / red
link" cleanly.

## Files owned/changed by me
- `crates/tex-core/src/pdf_fonts.rs` (NEW): PFB container parse
  (`parse_pfb` -> Type1Program{data,length1,length2,length3}, handles
  ascii+binary+trailer 3-segment PFBs used by TeX Live), bare-PFA fallback
  (`parse_type1`), cleartext FontDescriptor metrics (`parse_metrics`:
  FontBBox/ItalicAngle/Ascent/Descent/CapHeight/StemV), `tfm_descriptor`
  (TFM fallback: ascent=max height, descent=max depth, capheight=H height,
  stemv=width(I)/4, all scaled x1000/at_size). Unit tests included.
- `crates/tex-core/src/pdfout.rs`: data model. `Annot{rect,uri,dest,attr}`;
  `PdfPage{content,width,height,annots,fonts,dests}` (dests = named dests
  anchored on that page, first definition wins); `PdfDoc{pages,info,
  catalog_extra,outlines,fonts}`; `EmbedFont` gained length1/2/3 +
  descriptor metrics + flags.
- `crates/tex-core/src/pdffile.rs`: full serializer rewrite. Dynamic
  object allocator (no numbering gaps; xref always consistent), /Pages
  tree, per-annot objects (/Subtype /Link + /A /URI or /Dest), sorted
  /Names /D name tree, flat /Outlines tree with Prev/Next chain (title as
  UTF-16BE hex for non-ASCII), /Info = Producer/Creator + raw \pdfinfo
  body, /Catalog + raw \pdfcatalog body, flate content streams, FontFile
  stream with real Length1/Length2/Length3 (uncompressed, per PDF spec).
  `make_embed_font` signature unchanged (tex-cli compatible).
- `crates/tex-core/src/pdfrender.rs`: `render_page(&mut self)` (was
  &self): records \pdfsavepos into `eng.pdf_last_x/pdf_last_y` (sp, from
  page edges; bp_to_sp helper added), copies pdf_outlines into the doc.
  Link stack with real bbox tracking (char extents approximate
  asc/desc 0.75/-0.25 em; rects from emit_rect corners); unclosed links
  closed at page end. \pdfdest recorded per page (no more junk text in
  the stream). Glue setting now applies in vlists too (was ignored),
  with correct fil/fill/filll order logic (hlist version fixed: it used
  to return INFINITY for lower-order glue). vbox/hbox shift semantics
  fixed (hbox shift vertical, vbox shift horizontal). nullfont/no-size
  chars advance without emitting. Zero-size rects skipped.
- `crates/tex-core/src/pdftex.rs` (NEW): `do_pdfmapfile` (+name/-name/=name),
  `do_pdfmapline` (+entry / -tfm remove), `pdflast_xpos()/pdflast_ypos()`,
  `embed_used_fonts()` (collect fonts used by pages -> full PFB embed ->
  remap page font refs to doc indices). tex-cli's inline copy of that
  logic can be replaced by one call (TexMk's call, optional).

## Hooks wired by Main
- lib.rs: `pub mod pdf_fonts; pub mod pdftex;`

## Hooks still needed (Main)
- maincontrol.rs: `PdfMapFile => self.do_pdfmapfile()`, `PdfMapLine =>
  self.do_pdfmapline()` (currently both just scan-and-drop; NOTE: default
  pdftex.map already loads via FontLoader::new so most docs work anyway).
- \pdflastxpos/\pdflastypos as \the-able quantities (scan.rs territory):
  values are `eng.pdf_last_x/pdf_last_y` (sp; x from left edge, y from
  bottom edge), or accessor `eng.pdflast_xpos()/pdflast_ypos()`.
  Currently maincontrol errors "position primitive needs \the".

## Gotchas learned
- TeX Live cm PFBs (BlueSky) have THREE segments: ascii, binary, ascii
  trailer (512 zeros + cleartomark) -> /Length3 = 545, not 0. Stream
  dict must close `>>` *before* `stream` (regression caught by poppler).
- cm PFB cleartext has NO /Ascent /Descent /CapHeight /StemV (and
  FontBBox is {-40 -250 1009 750}); TFM fallback fills those.
- Extraction works off glyph names: no /ToUnicode needed for CM fonts;
  pdftotext output clean with builtin encoding or Differences.
- Engine default `pdf_horigin/pdf_vorigin = 65536` (1pt) in engine.rs —
  pdfTeX default is 1in (4736287). Driver sets it explicitly; consider
  changing the Engine default (Main's file).
- Driver (tex-cli) embeds fonts with widths truncated (`*1000/at_size`);
  kept same rounding in embed_used_fonts.

## Deferred (not in scope)
- Font subsetting (full fonts embedded per assignment).
- Map slant/extend -> synthetic font matrices; Type3/Type42/TTF;
  \pdfxform/\pdfximage; colorstack implementation beyond push/pop;
  outline nesting from negative counts (flat tree now).

## Late additions (post-first-green)
- `pdf_fonts::builtin_encoding(cleartext)` parses the font's own
  `/Encoding 256 array` (`dup <code> /<Name> put` sequence);
  `make_embed_font` now adopts it when no external .enc is given
  (pdfTeX-style accurate /Encoding Differences; pdffonts shows
  `Custom`). Do NOT graft a mismatched .enc (e.g. texnansx on cmr10):
  extraction turns to garbage — glyph names must match the actual slots.
- Final validation (2-page hello.pdf): pdfinfo 0 errors; pdffonts
  `CMR10 Type1 Custom emb=yes`; mutool clean; pdftotext AND mutool
  txt both extract "Hello, PDF World! / red link" per page; link annot
  rect is glyph-tight [242.83 64.53 259.16 74.49]; name tree
  `(sec:intro) [page /XYZ 242.83 72 null]`; FontDescriptor
  bbox from PFB, CapHeight 683.33/StemV 90.28 from TFM fallback.
- Workspace `cargo check -p tex-core` clean except linebreak.rs
  (LineBreak-5 mid-rewrite); acceptance built against /tmp/snapcore
  (fresh copy + linebreak stub, workspace untouched).

## Follow-up: leaders + null-rule sentinels (Main's request, done)
- pdfrender.rs renders Node::Leaders in both ship_hlist and ship_vlist:
  rule body = single rect over the whole set glue advance (tex.web:
  rule_wd/rule_ht = glue width), with RULE_FILL dims resolved against the
  containing box; box body = boxes::leader_layout(kind, lh+ld or lw,
  set-advance, left_edge, cur) per copy, shipped via ship_leader_copy
  (hlist copies baseline-aligned, vlist copies top-aligned).
- RULE_FILL (i32::MIN) sentinels resolved at shipout: hrule width ->
  containing vbox width (ship_vlist Rule + Leaders rule body); vrule
  height/depth -> containing hbox height/depth (ship_hlist Rule +
  Leaders rule body).
- Containing-box context threaded via RenderCtx fields
  {left_edge_sp, box_w_sp, box_h_sp, box_d_sp} (save/set/restore around
  every nested box recursion, incl. leader copies).
- Verified in driver: c-leaders rule = one 30pt x 1pt rect; x-leaders
  box copies (\"..\") render at spread positions and text-extract;
  pdfinfo/mutool clean on the updated /tmp/hello.pdf.

## Fonts/ToUnicode round (PdfFont-5, done)
- pdftex.map parser (`fontload::parse_map_line`) rewritten: quote-aware
  tokenizer (sections may fuse `".167 SlantFont"`), `<[foo.enc` = encoding
  FILE (was mis-parsed as a PFB name — broke all newtx embeds), `<<` =
  font without re-encoding, basename stripping, MapEntry gains `enc_name`
  for the `" T1Encoding ReEncodeFont "` form (resolved as <name>.enc via
  kpse when no explicit vector file). Slant/Extend accept separated and
  fused value tokens. 8 parser unit tests.
- /ToUnicode: `pdf_fonts::glyph_to_unicode` — merged AGL + pdfglyphlist +
  texglyphlist static table (4558 entries, tex overrides; texglyphlist
  multi-scalar semantics: first comma group, space-separated scalars
  concatenated, e.g. Germandbls -> "SS" per glyphtounicode.tex), then
  uniXXXX/uXXXXXX conventions, variant suffix stripping (a.sc -> a).
  `EmbedFont.to_unicode` built in make_embed_font from the encoding diff
  (non-identity slots only); `pdffile::to_unicode_cmap` emits a
  flate-compressed bfchar CMap (/ToUnicode ref in the font dict).
- build.rs fixes blocking real shipouts: (1) end_box checks
  shipout_pending BEFORE setbox_target (\shipout<vbox> was storing into
  \box255 and shipping nothing); (2) end_gracefully: closing a vbox group
  mid-paragraph forces \par (chars were vpack-discarded -> empty pages);
  (3) end_paragraph restores into the vbox list when saved_mode is
  InternalVertical (was hijacking paragraphs to the page list).
- pdftex.rs: \pdfmapfile/\pdfmapline participate in the lazy default-map
  load (FormatCache contract): +/-/remove ensure the default db first,
  `=`-replace clears + mark_map_loaded so it is never re-added.
- Smoke (plain-mode shipout of an inline-hyperref doc): cmr10 +
  ntx-Regular-tlf-t1 fully embedded (FontFile segments sum exactly to
  /Length), /ToUnicode x2, URI + named-dest link annots with glyph-tight
  rects, /Names tree with /XYZ and /FitBH; pdfinfo 0 errors, mutool
  clean, pdftotext extracts cleanly. Unit tests: fontload 8, pdf_fonts 3.
