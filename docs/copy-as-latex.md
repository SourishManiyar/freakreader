# Copy as LaTeX

Edit ▸ Copy as LaTeX (also in the command palette) turns the pointer into a crosshair. Drag a
rectangle around a typeset formula: when you let go, PdfCraft reads the formula from the page's
glyphs and rules and puts its LaTeX on the clipboard as plain text. The tool stays on for the next
formula; Escape or a right-click on the document leaves it (back to selecting, and a formula still
being read is dropped). It is not an Acrobat feature.

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
`crates/render/tests/data/math/oracle/README.md`. In short, it reads rows, scripts, fractions,
radicals, large operators with limits, integrals, delimiters, accents, alphabets, arrows, relations,
function names, text and spacing. It does not read matrices, `cases`, aligned rows, braces over or
under a formula, stacked symbols or `\binom`: it says so instead of guessing.

Fonts:

- **Computer Modern, AMS Euler and the AMS symbol fonts** as pdfTeX embeds them are read by font and
  character code, with their metrics: the most answers, and the most of them confident.
- **Other fonts are read by the Unicode text the PDF gives each glyph**: OpenType math fonts with
  `unicode-math` under LuaLaTeX or XeLaTeX (STIX Two, TeX Gyre Termes, Pagella and Bonum, Latin
  Modern Math, Fira Math, Libertinus Math) and Times-style packages under pdfLaTeX (newtx,
  mathptmx, txfonts), including a formula that mixes them with Computer Modern. Other OpenType math
  fonts (Cambria Math, for one) go the same way but have not been measured. Symbols,
  Greek, the math alphabets (`\mathbb`, `\mathcal`, `\mathfrak`, `unicode-math`'s `\symbf`) and
  structure are read as for Computer Modern. Plain letters take their alphabet from the font's name
  (`…-Italic`, `…-Regular`, `…-Bold`) and from what the page shows; a letter of a font whose name says
  nothing (a symbol font's `R` may be a blackboard R) is not guessed.
- **What such fonts leave open is said, not guessed.** Their metrics are not in the PDF, so the
  answer is marked low confidence when it depends on them: whether a grown delimiter is
  `\left…\right` or `\big…`, a gap that is either a letter's italic correction or a space, a word
  space at the edge of `\text`, `\mathrm{tr}` against `\text{tr}`, a character Unicode gives to several
  commands (`\emptyset`, `\varnothing`). Expect fewer confident answers than with Computer Modern,
  fewest for Times. A glyph with no Unicode text (some Type 3 fonts) is not read.

The rectangle must hold the formula alone: a glyph counts when its origin (the left end of its
baseline) is inside. An equation number beside a display is read as part of it if it is inside.
There is no automatic detection of formulas yet.

## Headless

The same reader is the `text_recover_latex` tool (`pdfcraft-cli run`, MCP); its `wrap` argument
returns the text Copy as LaTeX would put on the clipboard. See `crates/automation/README.md`.
