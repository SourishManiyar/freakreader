#!/bin/sh
# Rebuilds the PDFs and their ground-truth glyph lists from the .tex sources (TeX Live 2023: pdfTeX
# 1.40.25 with AMS Computer Modern Type 1 fonts). Run it from this directory; the tests only read
# the results. The truth is not read from the PDF: pdfTeX is run twice, once to DVI, where
# `dvitype` lists each glyph (font, code, position) and rule exactly as TeX placed it, and once to
# PDF, which sets the same boxes.
set -eu
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
for name in scripts_fractions delimiters_radicals symbols_array; do
    cp "$name.tex" "$work/"
    (cd "$work" && pdflatex -interaction=batchmode -halt-on-error -output-format=dvi "$name.tex" >/dev/null &&
        dvitype -output-level=4 -dpi=72.27 -magnification=0 "$name.dvi" > "$name.dvitype" &&
        pdflatex -interaction=batchmode -halt-on-error "$name.tex" >/dev/null)
    awk -f dvi_truth.awk "$work/$name.dvitype" > "$name.truth.tsv"
    cp "$work/$name.pdf" "$name.pdf"
done
