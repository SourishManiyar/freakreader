# hyperref test papers

`onecol.pdf` (11 pt, one column) and `twocol.pdf` (10 pt, two columns) are one synthetic
mathematics paper, `body.tex`, set with two preambles. Every cross-reference in it is a hyperref
link to a named destination: theorem-like statements sharing one counter (`theorem.2.1` is
Lemma 2.1), a definition and a remark, numbered `equation` and `align` displays
(`equation.3.1`, `equation.3.2`), sections, a TikZ figure, a table and a three-entry bibliography
(`cite.alpha`). Links are hidden (`hidelinks`), as many papers have them.

- `*.tex` were written for this repository; nothing in them comes from a paper or a book.
- The PDFs embed Computer Modern Type 1 subsets from amsfonts (OFL-1.1:
  `../math/OFL-AMSFonts.txt`). Latin Modern is not used.
- `generate.sh` rebuilds them with pdfTeX (TeX Live 2023). The output is byte-reproducible
  (`SOURCE_DATE_EPOCH`, `FORCE_SOURCE_DATE`) and shell escape is off. The tests only read the
  committed PDFs.

The reference preview's tests read them: `crates/engine/tests/it/reference_blocks.rs` checks the
block each destination names, by the text it shows.
