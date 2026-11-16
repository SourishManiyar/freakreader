# LaTeX math test pages

`scripts_fractions`, `delimiters_radicals` and `symbols_array` are small LaTeX pages (pdfTeX, Computer
Modern and AMS Type 1 fonts). Each has:

- `NAME.tex`, the source, written for this repository;
- `NAME.pdf`, its output, with the font subsets pdfTeX embeds (AMS fonts, OFL-1.1: `OFL-AMSFonts.txt`);
- `NAME.truth.tsv`, the ground truth: every glyph (`G font code h v`) and rule (`R h v width height`)
  TeX typeset, in scaled points from the DVI origin, as `dvitype` lists them for the same source run to
  DVI. It is not read from the PDF, so a reader that mislabels a glyph cannot agree with it by accident.

`crates/render/tests/it/math_glyphs.rs` checks `PageRenderer::glyph_details` against the truth: every
glyph at its place with its font and code, every rule, and nothing else.

`generate.sh` rebuilds all of it (needs TeX Live with `pdflatex` and `dvitype`; the tests don't).
The pages' size is set with `geometry` so every glyph falls inside the page: a glyph outside the crop
box is rightly absent from the page's list.
