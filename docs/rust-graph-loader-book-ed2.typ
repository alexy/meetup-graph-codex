#set document(
  title: "Learning Rust Through a Portable Graph Loader, Edition 2",
  author: "Codex",
)

#set page(
  paper: "us-letter",
  margin: (x: 0.82in, y: 0.78in),
  numbering: "1",
  header: context {
    if counter(page).get().first() > 1 [
      #set text(size: 8.5pt, fill: rgb("#64748b"))
      #grid(
        columns: (1fr, auto),
        align: (left, right),
        [Learning Rust Through a Portable Graph Loader, Edition 2],
        [#counter(page).display()]
      )
    ]
  },
)

#set text(
  font: "New Computer Modern",
  size: 10.6pt,
  lang: "en",
)

#set par(
  justify: true,
  leading: 0.58em,
)

#show heading.where(level: 1): it => {
  pagebreak(weak: true)
  v(0.4em)
  text(size: 21pt, weight: "bold", fill: rgb("#0f172a"), it.body)
  v(0.38em)
  line(length: 100%, stroke: 0.7pt + rgb("#38bdf8"))
  v(0.75em)
}

#show heading.where(level: 2): it => {
  v(0.6em)
  text(size: 14pt, weight: "bold", fill: rgb("#1e293b"), it.body)
  v(0.25em)
}

#show raw.where(block: true): it => {
  block(
    fill: rgb("#f8fafc"),
    stroke: 0.55pt + rgb("#cbd5e1"),
    radius: 4pt,
    inset: 8pt,
    width: 100%,
    breakable: true,
  )[
    #set text(font: "DejaVu Sans Mono", size: 8.15pt, fill: rgb("#0f172a"))
    #it
  ]
}

#show raw.where(block: false): it => {
  box(
    fill: rgb("#f1f5f9"),
    radius: 2pt,
    inset: (x: 2.2pt, y: 0.8pt),
  )[
    #set text(font: "DejaVu Sans Mono", size: 8.6pt, fill: rgb("#0f172a"))
    #it
  ]
}

#show list: it => {
  set par(leading: 0.48em)
  it
}

#align(center)[
  #v(1.15in)
  #text(size: 28pt, weight: "bold", fill: rgb("#0f172a"))[
    Learning Rust Through a Portable Graph Loader, Edition 2
  ]

  #v(0.22in)
  #text(size: 15pt, fill: rgb("#334155"))[
    Architecture, traits, SDKs, functional Rust, and the By the Bay graph pipeline
  ]

  #v(0.32in)
  #line(length: 62%, stroke: 1.2pt + rgb("#38bdf8"))

  #v(0.42in)
  #text(size: 11pt, fill: rgb("#475569"))[
    Ownership, borrowing, data modeling, backend abstraction, SDK transports, and practical Rust design
  ]

  #v(0.62in)
  #text(size: 10pt, fill: rgb("#64748b"))[
    Generated from the implementation in #raw("src/main.rs"), #raw("src/bin/load_graph.rs"), and #raw("src/graph_loader.rs")
  ]

  #v(1fr)
  #text(size: 10pt, fill: rgb("#64748b"))[
    May 2026
  ]
]

#pagebreak()

#outline(title: "Contents", depth: 2)

#pagebreak()

#include "rust-graph-loader-book-ed2.body.typ"
