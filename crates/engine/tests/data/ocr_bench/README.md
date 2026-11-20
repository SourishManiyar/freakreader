# Scanned-paper OCR benchmark

Five pages of mathematical prose written for this repository, typeset by pdfTeX in Computer Modern
(the AMS Type 1 fonts, OFL-1.1): three development pages (`dev_*`) and two held-out pages
(`holdout_*`) that no parameter is chosen on. The text has accented names (Erdős, Hölder, Poincaré,
Lévy, Rényi, Gödel …), Greek letters and letter strings set in math italic (without tonos: the OFL
Computer Modern fonts have no accented Greek), and ordinary English prose in one and two columns.
Hyphenation is off, so every word of the truth is whole.

`crates/engine/tests/it/ocr_bench.rs` typesets each page twice. To DVI, where `dvitype` lists every
glyph and the end of every word (`\ocrpar`, from `pdfcraft_oracle::tex::ocr::MACROS`, ships a
`\special` after each word), which spells the truth; and to PDF, which PdfCraft renders at 300 dpi
and degrades like a scanner: a 0.5° turn, a 3×3 blur, seeded noise, a JPEG round trip at quality 40,
and all four together. The truth is never read from the PDF. The recogniser's reading is scored by
word error rate (WER) and character error rate (CER), both in reading order, so a two-column page
read across the columns scores badly, and by how many accented or Greek words come back intact.

The benchmark needs TeX Live (`pdflatex`, `dvitype`) and the OCR models (`cargo xtask models`), so it
is ignored in the plain run:

```bash
RUST_TEST_THREADS=1 cargo test -j 2 -p pdfcraft-engine --test it ocr_bench -- --ignored --nocapture
```

A missing TeX is a declared skip, or a failure when `PDFCRAFT_ORACLES_REQUIRED` is set; missing
models are a skip, or a failure when `PDFCRAFT_OCR_BENCH_REQUIRED` is set.
`PDFCRAFT_OCR_BENCH_ENGINE` picks the engine, and `PDFCRAFT_OCR_BENCH_VERBOSE` prints each clean
page's truth and reading.
