#!/bin/sh
# Rebuilds onecol.pdf and twocol.pdf from their .tex sources (TeX Live 2023: pdfTeX 1.40.25 with
# the Computer Modern and AMS Type 1 fonts of the amsfonts package; OFL-1.1, ../math/OFL-AMSFonts.txt).
# Run it from this directory; the tests only read the results. Both papers \input body.tex, a
# synthetic mathematics paper set with hyperref, so every cross-reference is a link to a named
# destination (equation.2.1, cite.alpha, theorem.2.3, ...). Three runs settle the references.
# The output is byte-reproducible: the dates come from SOURCE_DATE_EPOCH, and shell escape is off.
set -eu
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cp body.tex "$work/"
for name in onecol twocol; do
    cp "$name.tex" "$work/"
    for _ in 1 2 3; do
        (cd "$work" && pdflatex -no-shell-escape -interaction=batchmode -halt-on-error "$name.tex" >/dev/null)
    done
    cp "$work/$name.pdf" "$name.pdf"
done
