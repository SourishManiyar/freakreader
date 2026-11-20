# pdfcraft-ocr

Layer L4 (may use `fonts` for the text layers' metrics): Scan & OCR — recognising the words in
a page raster and turning them back into page content (execution plan M10).

```rust
// The caller renders a page to pixels and hands them over:
let image = OcrImage::new(w, h, rgba)?;                  // validated by construction
let lines = recognizer.recognize(&image, &options)?;     // Vec<Line>: words in reading order
let placed: Vec<PlacedWord> = …;                         // PlacedWord::place maps boxes to user space
// The editable output draws real glyphs: plan the words, register the display faces into the
// plan (`pdfcraft_edit::add_type3_fonts`), then generate the layer. An unregistered plan falls
// back to WinAnsi, and characters no registered face has become '?'.
let plan = TextPlan::for_words(placed.iter().map(|w| w.text.as_str()));
add_type3_fonts(&mut doc, page, &mut plan)?;             // names the plan's fonts on the page
let layer = visible_text_layer(&placed, &plan);          // visible text: the editable output
// The searchable layer needs no display fonts: the engine's own layer (engine::ocr::layer)
// keeps every Unicode character with empty Type3 programs and a ToUnicode map beyond WinAnsi.
```

- **Engines behind one trait.** A [`Recognizer`] reports its `id`, whether it is `available`,
  the languages it reads, and reads `Vec<Line>` from an image. Two exist: the built-in
  `PpOcrRecognizer` (id `ppocr`: PaddleOCR's PP-OCRv5 mobile models, Apache-2.0, run on the
  pure-Rust `rten` runtime; fetched by `cargo xtask models`; found via `$PDFCRAFT_MODELS` or
  `models/` beside the executable — see ATTRIBUTION.toml and "The PP-OCR engine" below), and —
  off the web build —
  `tesseract::TesseractCli`, which drives an installed `tesseract` binary as an external
  process: never linked, bundled or downloaded. Implementations are shared across threads
  (`Send + Sync`); nothing spawns on wasm32.
- **The tesseract process is fenced in.** The program comes from a user-configured location or
  the PATH, never from document data (the engine validates a configured path as an absolute
  path to an existing executable file before it is used). The language code is allow-listed
  (`tesseract::language_code`, 12 codes) before anything reaches a process; the page image
  goes to a private temp file (a fresh 0700 directory holding a 0600 file on Unix) that is
  deleted afterwards; the run has a 120 s timeout, a 64 MiB output cap and a 50 000-word
  reading cap (an incomplete reading is rejected rather than applied); every failure comes back through `OcrError` with its cause named. A configured
  location that is invalid fails closed (it is never replaced by the PATH search). Availability
  is probed with `--list-langs` (its list is read from stdout, or from stderr where older
  versions print it); a failed probe is remembered for 30 s, so an install that appears later
  is found without a restart. Known limit: at the timeout or the output cap only the direct
  child is killed; a wrapper script's own children may outlive it.
- **Confidence drives review.** One classifier (`band_for`) puts a word in the High
  (≥ 90), Medium (70–89) or Low (< 70) band — the review overlay's colours, the legend and the
  counts; the batch flag rule (`low_confidence`, default threshold 60) is deliberately a
  separate threshold. PP-OCR gives every word a confidence (the mean probability of its
  characters); a reading whose confidence is unknown (some Tesseract words) is treated as
  unknown, never as good.
- **Two engines can be merged (ROVER).** Strategies: `PrimaryOnly` (the default), and two
  ensembles — `ConfidenceWeighted` (the secondary engine joins when the primary's mean
  confidence is under 70 or unknown; an empty primary keeps the fast path) and `RoverVote`
  (both engines always run). The merge lets every primary word adopt the best-overlapping
  unused secondary word at IoU ≥ 0.5; the higher confidence wins text, box and confidence (a
  tie keeps the primary; against a primary word of unknown confidence the secondary
  wins only at 70 or more, `UNKNOWN_CONFIDENCE_BAR`), the survivor is named `"ROVER"`, and
  unclaimed secondary words are
  appended under their engine's name. A merge is capped at 100 M word-pair comparisons (over
  it, the primary reading stands alone), and a failing partner degrades to the primary with a
  recorded note — a page is never skipped because a partner could not read it.
- **Preprocessing between the raster and recognition**, a pure function in fixed order: DPI
  normalization to 300 (only when one of the steps below is on, since they are calibrated for
  it; with all of them off the raster is read exactly as rendered, and a scan at or above 300
  is left alone), then the opt-in steps —
  orientation (90°/180°/270° from projection-profile banding), deskew (± 5°, shear-scored,
  below 0.1° is a no-op), denoise (3×3 median) and binarization (Sauvola, window 8, k 0.34).
  Every geometric step composes one exact affine `InverseTransform`, and callers map every
  word box back to the original raster before placing it. The heuristics are conservative and
  panic-free: too small to judge, blank, or no clear winner is a no-op, never a guess.
- **Searchable output preserves Unicode without display fonts.** The engine uses empty
  Type3 character programs and `/ToUnicode` maps for non-WinAnsi words, leaving the scan
  unchanged. WinAnsi words use `/PCHelv`; the `/OCR` layer is fitted to each word's box.
- **Editable output uses display glyphs.** `visible_text_layer` writes visible text whose
  size comes from the box height and whose horizontal scaling uses real font advances.
  Beyond WinAnsi, attributed craft-fonts faces supply outline Type3 glyphs with `/ToUnicode`.
  Uncovered characters become `?` and are reported in editable output notes; searchable
  output retains them. The engine refuses nonfinite geometry or oversized input before
  planning fonts: 4 MiB of text and 65,536 words, across all pages for editable export.
  Empty geometry is skipped; rotated words carry `across`/`up` vectors and retain direction.
- **Hostile input is rejected, not panicked at.** `OcrImage` holds private, validated state:
  non-zero sides, a 10 000-pixel side cap, a 64-megapixel area cap (checked arithmetic — the
  counts come from documents, so they are untrusted), and an exact buffer length.

## Where it is wired

The engine's `ocr` module renders pages into `OcrImage`s, runs jobs (`OcrJob::run`) and applies
the words: searchable-image mode draws the invisible layer over the page's own image and leaves
that image untouched; editable mode writes a NEW `<name>_ocr.pdf` of visible text (the scan
image is not carried over, the source is never overwritten). Skip probes keep pages — and
whole files — that already contain text out of the work unless Force OCR overrides them. A
searchable result applies as one undoable step; the Editable text output opens the OCR Verify
screen first, and Scan & OCR ▸ Correct recognized text reviews any recognition there before it
is applied (confidence-coloured boxes, corrections, a per-language user dictionary — a
plain-text file in the preferences folder, so words added from a document persist on disk
outside it until removed — Re-OCR of a region or a page, Accept/Reject), a
multiple-file run writes searchable copies as it goes, and the automation tools `ocr_recognize`,
`ocr_words`, `ocr_status` and `ocr_recognize_files` expose recognition headlessly.
`ocr_words` with `retain_review` returns a bounded session-local handle; `ocr_review_apply`
applies text corrections at the stored original boxes through its stale-document guard,
or discards the result. Empty corrections remove words. Searchable acceptance is one undo
step; editable acceptance writes a separate file. `ocr_dictionary` reads or atomically
updates the UI's per-language dictionary format through an explicitly root-confined path.

## The PP-OCR engine

`ppocr.rs` runs PaddleOCR's PP-OCRv5 mobile models, the PaddleOCR team's own ONNX exports
(Apache-2.0, pinned by Hugging Face commit and SHA-256 in ATTRIBUTION.toml), on `rten`. The pre-
and post-processing are written here from the model cards and the DB and CRNN/CTC papers, not
from PaddleOCR's code:

- **Detection** (DB): the page is scaled so its long side is at most 2048 pixels, each side a
  multiple of 32, normalised with the ImageNet mean and deviation in BGR order. The probability
  map is binarised at 0.3; the outer contour of each component becomes its smallest rotated
  rectangle (a monotone-chain hull, then a search over the hull's edges); boxes under a mean
  probability of 0.6, or thinner than 3 map pixels, are dropped; each is grown by
  `area × 1.5 / perimeter` on every side.
- **Reading order**: in a frame turned by the page's median text angle (a page scanned a little
  askew still has gaps between its lines there), the boxes are cut recursively, with an
  explicit stack: at an empty vertical gutter about two lines wide (columns, left to right),
  else at the widest empty horizontal band (top to bottom); a region with neither is one line.
- **Recognition** (CTC): each box is cut out of the original raster, upright, 48 pixels tall at
  its own width (supersampled when shrinking), normalised to −1…1, and read in batches of
  similar width. Greedy CTC decoding: class 0 is the blank, 1…N the dictionary (the model
  card's `character_dict`), N+1 the space. A recogniser whose class count is not its
  dictionary's plus two is refused at load.
- **Words**: the time step of each character places it along the line, so each line is split at
  its spaces into words with their own boxes, within the box as found; Chinese and Japanese
  become one word per character, and a check box or bullet read as a character (`□`) is a word
  of its own. A word's confidence is the mean probability of its characters, 0–100.
- **Recognisers by language**: Latin (`en de fr es it pt nl pl cs hu sv`: the accented Latin
  alphabets, and Greek letters without tonos), Greek (`el`) and Chinese/Japanese (`zh ja`). A
  language whose recogniser is not installed is an error that names the installed ones.
- **Models**: the detector and the Latin recogniser (its `.onnx` and its `.yml` model card) must
  be in one directory, or there are none; the Greek and Chinese recognisers are used when both
  their files are there. Every file is checked against its SHA-256 before `rten` parses it.
  Sizes: detector 4.8 MB, Latin 8.0 MB, Greek 7.8 MB, Chinese/Japanese 16.7 MB (37 MB in all,
  25 MB more than the previous engine's 12 MB). The web build loads the detector and the Latin
  recogniser only (13 MB).
- **Limits**: the image caps of `OcrImage`; at most 20 000 contours looked at and 5 000 boxes
  kept per page; at most 2 000 000 line-image columns read per page; 1 000 characters per
  line; 50 000 words per page; dictionaries at most 4 MiB and 30 000 entries, plain or
  single-quoted items only; model files at most 256 MiB. One page is read at a time.

### Benchmark

`crates/engine/tests/it/ocr_bench.rs` (`crates/engine/tests/data/ocr_bench/README.md`) typesets
five pages of mathematical prose with pdfTeX (three development pages, two held out and never
tuned on), takes the truth from the DVI, renders the PDF at 300 dpi and degrades it. Word and
character error rate (WER, CER) in reading order, and the accented or Greek words read intact
(49 on the development pages, 6 held out). Measured on 2026-11-20, Linux x86-64, language `en`:

| Condition | Set | ocrs WER | ocrs CER | ocrs accented/Greek | PP-OCR WER | PP-OCR CER | PP-OCR accented/Greek |
|---|---|---|---|---|---|---|---|
| clean | dev | 8.1% | 2.1% | 0/49 | 0.4% | 0.1% | 46/49 |
| clean | held out | 3.7% | 0.9% | 0/6 | 0.0% | 0.0% | 6/6 |
| rotate 0.5° | dev | 23.1% | 18.5% | 0/49 | 0.3% | 0.0% | 47/49 |
| rotate 0.5° | held out | 3.7% | 0.9% | 0/6 | 0.0% | 0.0% | 6/6 |
| blur | dev | 19.0% | 12.6% | 0/49 | 0.1% | 0.0% | 48/49 |
| blur | held out | 3.0% | 0.8% | 0/6 | 0.0% | 0.0% | 6/6 |
| noise | dev | 11.7% | 3.5% | 0/49 | 0.6% | 0.1% | 44/49 |
| noise | held out | 6.2% | 1.6% | 0/6 | 0.0% | 0.0% | 6/6 |
| JPEG q40 | dev | 15.3% | 9.1% | 0/49 | 0.3% | 0.0% | 47/49 |
| JPEG q40 | held out | 3.2% | 0.8% | 0/6 | 0.0% | 0.0% | 6/6 |
| all four | dev | 8.3% | 2.4% | 0/49 | 0.5% | 0.1% | 46/49 |
| all four | held out | 5.0% | 1.3% | 0/6 | 0.0% | 0.0% | 6/6 |

ocrs turned every accented letter into `?` and read math-italic Greek as Latin look-alikes.
PP-OCR's remaining errors are look-alikes on the Greek page (ν read as v, μ as the micro sign µ,
Λ as A). It is slower: about 2.3 s a page against 0.7 s for ocrs on this machine (recognition alone,
rten's default threads). The pages are synthetic renders of Computer Modern, so the held-out set scoring 0 % says these pages are now
too easy to tell further improvements apart; real scans and other fonts are not measured here.

Not yet: Cyrillic, Arabic, Hebrew and Indic scripts (PaddleOCR publishes recognisers for them
that this engine could load); measured quality for Greek, Chinese and Japanese; vertical
Japanese; the text-direction classifier (not needed for the benchmark's pages: a 0.5° turn reads
as well as a straight page); a browser test of the web build; the deep scan-cleanup options
(descreen, background removal, edge-shadow removal); and editable glyph coverage beyond the
available attributed faces.
