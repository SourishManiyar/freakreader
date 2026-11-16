# Copy as LaTeX

Edit ▸ Copy as LaTeX (also in the command palette) turns the pointer into a crosshair. Drag a
rectangle around a typeset formula: when you let go, PdfCraft reads the formula from the page's
glyphs and rules and puts its LaTeX on the clipboard as plain text. It is not an Acrobat feature.

- **What is copied.** The body of the formula, such as `\frac{1}{2}x^{2}`, with nothing around it,
  so it can be pasted inside whatever you are writing. Preferences ▸ Documents and view ▸ "Copy as
  LaTeX wraps the formula in" can add `$…$` (inline) or `\[…\]` (display) instead.
- **What it says.** The notice at the bottom of the window says what happened:
  - "Copied LaTeX: …" with the start of what was copied, when the reader is sure of every glyph and
    of the layout;
  - "Copied LaTeX with low confidence, check it: … (reason)" when it had to choose between readings
    the page cannot settle (for example `\frac` or `\dfrac`, which look the same in a display), or
    when it met something it only partly explains. Check such an answer before you use it;
  - "Nothing recognised: … The clipboard is unchanged." when it cannot read the area: nothing is
    copied, and the clipboard keeps what it held.
- **Busy.** While a formula is read the area stays outlined with a spinner beside it, and the window
  keeps working. Dragging another rectangle replaces it; closing the document stops it. Copying
  something else meanwhile wins over it.
- **Permissions.** A document whose security does not allow copying text is not read, and the notice
  says so, as for every other copy.

## What it reads

The reader and its scores against TeX-typeset equations are described in
`crates/render/tests/data/math/oracle/README.md`. In short, it reads equations typeset by pdfTeX in
Computer Modern, AMS Euler and the AMS symbol fonts: rows, scripts, fractions, radicals, large
operators with limits, integrals, delimiters, accents, alphabets, arrows, relations, function names,
text and spacing. It does not read matrices, `cases`, aligned rows, braces over or under a formula,
stacked symbols, `\binom`, or fonts other than those (OpenType math fonts, Times, Latin Modern): it
says so instead of guessing.

The rectangle must hold the formula alone: a glyph counts when its origin (the left end of its
baseline) is inside. An equation number beside a display is read as part of it if it is inside.
There is no automatic detection of formulas yet.

## Headless

The same reader is the `text_recover_latex` tool (`pdfcraft-cli run`, MCP); its `wrap` argument
returns the text Copy as LaTeX would put on the clipboard. See `crates/automation/README.md`.
