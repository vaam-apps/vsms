// Static Typst preamble shared by every generated chapter in the
// `cargo xtask docs-pdf` book.
//
// `.xtask/src/docs_pdf.rs` converts each source document with
// `pandoc -t typst` (no `--standalone`), i.e. body-only output, so that
// every chapter can be `#include`d into one shared page/heading/outline
// sequence instead of each carrying its own page setup. That has one
// consequence worth writing down: pandoc's *own* default Typst template
// (`pandoc -D typst`, read directly, not guessed) defines a handful of
// `#let`/`#show`/`#set` bindings that its body output assumes already
// exist — `horizontalrule` in particular is referenced, unqualified, by
// name, wherever a Markdown `---`/AsciiDoc thematic break appears, and
// Typst's `compile` genuinely fails on it (`error: unknown variable:
// horizontalrule`) the moment body-only output is compiled without them.
// `--standalone` would normally splice this preamble in automatically,
// once per file; body-only output needs it defined exactly once for the
// whole merged book instead, here.
//
// The five bindings below are copied verbatim from the *static* portion of
// `pandoc -D typst`'s own default template (the part with no `$if(...)$`
// templating left in it — the syntax-highlighting and `conf(...)` sections
// further down that template are `--standalone`-only concerns this book
// never uses, since Typst's own built-in raw-block highlighting covers
// every fenced code block pandoc emits without them). Verified live: the
// full 22-document merge compiles cleanly with exactly this preamble and
// no more; removing `horizontalrule` alone reproduces the failure above.
//
// Every generated fragment (`target/docs-pdf/fragments/*.typ`) starts with
// `#import "../helpers.typ": *` — see `docs_pdf.rs::write_fragment`.

#let horizontalrule = line(start: (25%, 0%), end: (75%, 0%))

#show terms.item: it => block(breakable: false)[
  #text(weight: "bold")[#it.term]
  #block(inset: (left: 1.5em, top: -0.4em))[#it.description]
]

#set table(
  inset: 6pt,
  stroke: none,
)

#show figure.where(kind: table): set figure.caption(position: top)
#show figure.where(kind: image): set figure.caption(position: bottom)
