# pdfcraft-automation

Agent control for PdfCraft: a headless tool table over the engine, and an opt-in MCP server.

- **Layer:** L7 (architecture §13), but headless. It depends on `engine`, `render` and `organize`, never on a UI toolkit.
- **Status:** over a hundred tools; `pdfcraft-cli tools` lists the current set with their schemas (the count isn't repeated here, so it can't go stale). They cover inspection, rendering, text, page edits and organizing, page labels, bookmarks, comments and their review, form filling and authoring (fields, properties, scripts, data exchange, detection), redaction and sanitizing, password protection and signatures, calibrated 2D measurements and CSV export, adding text, images, links and stamps, optimizing, OCR, printing, export, metadata, undo/redo, save, combine, extract and split. Comment tools take geometry in the same top-left-origin points as everything else, and `comment_add` can mark text by searching for it (`find`). The MCP server also serves the open documents as resources, page images included. The running app's UI is driven separately, through the `ui.*` control channel in `pdfcraft-ui-egui`.

`fill_sign_add` text accepts line breaks, including blank lines. Its typewriter box grows to hold all wrapped rows, and saving/reopening preserves the contents and appearance.

## API

```rust
let mut a = Automation::new().with_root("/work")?;      // optional: confine all paths
let doc = a.call("doc_open", &json!({ "path": "in.pdf" }))?;   // Vec<Content>
a.write_output("page1.png", &png)?;                      // save a result yourself, under the same root rules
tools() -> Vec<ToolDef>                                  // name, title, description, input_schema, read_only, destructive, command
mcp::McpServer::new(a).serve(stdin, stdout)?             // newline-delimited JSON-RPC 2.0
```

`content_update.rotate` rotates an added image counter-clockwise by an integer multiple of
90 degrees. Large positive and negative signed 64-bit angles are normalized before combining
them with the image's current orientation. Full turns retain its appearance, rectangle, crop
and flips. Angles that are not multiples of 90 are refused without changing the document or
its undo/redo history.

`page_add_text`, `content_update` and `text_edit` take per-line `aligns`. An `"inherit"` entry
follows the box-level `align`; legacy `null` entries remain accepted with the same meaning.

`page_add_text` and `content_update` take one of `color` (`#RRGGBB` or a name, painted in
DeviceRGB), `gray` (0 black to 1 white, DeviceGray `g`) and `cmyk` (`[c, m, y, k]`, each 0 to 1,
DeviceCMYK `k`), so text added to a print file can be 100 K black. `content_list` reports `gray` or
`cmyk` for such text.

`Content` is `Json(Value)` or `Png { data, width, height }`. Errors are `UnknownTool`, `InvalidArgs` (the call didn't match the schema) or `Failed` (a readable message for the agent).

## Conventions

- Tool names are `snake_case` (`page_rotate`): MCP clients reject dots. `ToolDef::command` links a tool to the registry id it automates (`page.rotate`), and `command_list` reports the link.
- Pages and positions are **1-based**. Rectangles are PDF points with the origin at the top-left of the displayed page.
- Unknown arguments are rejected, so typos fail loudly.
- Tools that change a document return its summary (`doc`, `pages`, `dirty`, `undo`, `redo`, …).
- `comment_image_preview` returns JSON and a PNG: `layer: "background"` (the default) gives the page without the selected image signature/initials; `layer: "image"` gives its embedded image with alpha. The metadata supplies its displayed rectangle, document rotation, `image_rotation` (how far clockwise the image is shown turned: 0 for one placed by PdfCraft, which keeps it upright as displayed) and annotation opacity. Each result can be saved with the CLI's `--out`. Cache the background and transform the image for live movement/resizing; `comment_edit` commits the final geometry in one undo step. Previewing never changes the document.
- For proportional image signature resizing, use the embedded image layer's width/height ratio rather than a previously stretched annotation rectangle. The GUI preserves this ratio at corners with the opposite corner anchored; edge midpoint handles change only their own axis. `comment_edit` accepts the resulting rectangle.
- `doc_close` refuses to drop unsaved changes unless `discard_changes: true`. `doc_save` writes atomically, incrementally in place, and in full for a new path. Atomic output writes call `sync_all` on the staging file before replacing the destination. A write or sync error leaves the destination intact, removes staging and leaves the document dirty for retry. A filesystem that does not support syncing returns an error; there is no unsynced fallback. This flushes file contents, but does not guarantee persistence of the parent-directory rename across power loss.
- With a root set, every read and write path must resolve inside it (symlinks and `..` included). Relative paths resolve inside the root, and `..` is resolved by name before the check. Every path outside the root gets the same refusal (`<path> is outside the allowed directory <root>`), whether or not it exists, so a confined agent can't probe the rest of the disk; on Windows another network share or device path (`\\host\share`, `\\?\UNC\…`, `\\.\…`) is refused without being contacted. "Not found" and other filesystem errors are reported only for paths inside the root. Tools that write several files into a folder name them after the document or input file, with separators, colons and control characters replaced by `_`, so a name can't lead them out of the folder. The root itself must be a folder.
- `Automation::with_automation_roots` holds a read folder and a write folder (`pdfcraft-cli mcp --automation-read-root DIR --automation-write-root DIR`, see [docs/mcp.md](../../docs/mcp.md)). Reads must lie inside the read root and writes inside the write root, links followed. A root left out grants none of its access. Refusals read `automation path rejected: outside the read root <root>: <path>`. As with a root set, on Windows a path on another network share or device path is refused with that refusal without being contacted (#4003). It applies on top of `with_root`.

## Glyph details for mathematics

`text_glyph_details {doc, pages?}` reads pages glyph by glyph, beside the text layer (which it does
not change): per glyph the character code, a font index (the page's `fonts` list base font, name
without subset prefix, program kind and size), glyph id and name, Unicode (`text`, null with
`unmapped: true` when there is none: text extraction drops such glyphs, this keeps them), the em size,
the em box, the exact bounds of the outline (`ink`), the baseline segment and the writing direction;
and per page the rules (thin axis-aligned filled rectangles and straight solid strokes: fraction bars,
radical overlines, table lines). Coordinates are points from the top-left of the displayed page, in
content order. Limits: 100,000 glyphs, 20,000 rules and 2,048 fonts per page (`capped: true`), 100,000
glyphs per call in whole pages (`next_page`); a page that cannot be read fails the call. Symbols of
fonts such as CMEX have private-use Unicode: key them by font name and code. Not a math parser; see
`pdfcraft_render::text::math` for the record and `crates/render/tests/data/math` for the LaTeX test
pages and the TeX-made ground truth they are checked against.

## Text search and cache limits

`text_word_count` counts the current snapshot without filling the text cache. It returns totals
and per-page Unicode word/character counts, with optional tagged-table, tagged-code and
image/OCR exclusions. See [word count](../../docs/word-count.md) for counting rules and limits.

Extracted page text is reused in a session-wide LRU cache: at most **64 MiB** of estimated owned
text storage and **4,096 pages**, shared by all open documents. Accounting includes glyph-vector
capacity, each glyph string's capacity, line indexes and word-break flags. Cache hits share the
renderer result instead of copying glyphs. Oversized pages remain usable by the current call but
are not cached; evicted pages can be extracted again. A changed document version invalidates its
cached text on the next text request, and closing a document removes its entries. Cache entries
do not keep an old PDF byte buffer alive.

This bounds retained text-cache data, not total process memory: active extraction batches,
renderers, document buffers and the requested output have separate lifetimes. Search still reads
growing batches (8 to 128 pages), with one lookahead match to determine continuation, unless
`stop_at_limit` is set (below), which starts at one page and looks no further than its limit.

`text_find` keeps matches from readable pages when another reached page fails extraction. It
returns `warnings` with 1-based `page`, `error` and `error_truncated`, plus `warning_count` and
`warnings_truncated`. At most 100 warnings and 1,024 UTF-8 bytes per error are returned. Failures
are retried on later calls; they are not cached as empty pages. Warnings describe pages visited in
search order, up to the continuation point, not failures in unvisited later pages.

`complete: true` means the requested search was exhausted with no page failures and no further
matches. `next: {start_page, skip}` exists only when another match would exceed `limit`. A search
that reaches the end with warnings therefore has `complete: false` **without** `next`. Callers
should follow `next` when present and separately inspect warnings. A capped search can return
both. The existing `query`, `count`, `matches` and rectangle semantics are unchanged.

That lookahead means a search with exactly `limit` matches reads on to the end of the document to
learn that no further match exists. `stop_at_limit: true` skips it: the search ends at the
`limit`th match. It reads one page first, then batches of 2, 4, … up to 128 pages, so nothing past
the batch holding that match is read or can add warnings (a match on the first page reads only that
page). The answer carries
`stopped_at_limit`. When that is `true`, `complete` is `false` and `next` is only the position just
past the last match, not a promise of another one; following it may return no matches and
`complete: true`. When it is `false`, the search ran out of pages first and `complete` means what
it always does.

`text_extract`, comment placement and redaction keep strict all-requested-pages-must-succeed
semantics; they never silently act on partial text.

## Shared field properties

For an individual push button, `form_set_props` also edits its visible normal label:

```json
{"doc": 1, "field": "clear_button", "caption": "Clear Form", "widget": 1}
```

`widget` is a 1-based index (default 1), used only with `caption`; an empty caption clears the
label. Neither renames the field. `form_fields` returns the first widget's `caption` and all
`captions` as `[{"widget": 1, "caption": "Clear Form"}]`. Other widgets and icon-only layouts
keep their settings. CLI and MCP use the same edit, including undo/redo and save/reopen.
The desktop exposes it as **Button Properties → Options → Label**. Rollover/down labels and
icon layout are not edited here; the existing caption font and unsupported-character warning
policy apply.

`form_set_props` accepts either `field` (one name) or `fields` (1–1000 unique names):

```json
{"doc": 1, "fields": ["first_name", "last_name"], "required": true,
 "appearance": {"width": 3, "border": "#1473E6"}}
```

The batch is atomic and creates one undo step. A missing or locked field, invalid option,
unknown argument or malformed appearance refuses the whole edit. Set `locked: false` to
unlock and change fields together. Only supplied properties change: each field/widget keeps
its other colours, fonts, values and unknown PDF dictionary keys. `"none"` removes a border
or fill colour. Names, rectangles, rotations and captions require the single-field form. Results keep
`field` for single edits and return `fields` for batches. CLI and MCP use the same tool.

## MCP server

**Opt-in only.** Nothing starts it automatically, and it opens no port. It runs while `pdfcraft-cli mcp [--root DIR]` runs, normally launched by an agent from its MCP configuration. It stops when stdin closes. The CLI's `mcp` Cargo feature (on by default) compiles it out entirely when disabled.

`pdfcraft-cli mcp --compact` (or `McpServer::with_compact(true)`) keeps `tools/list` short for agents with small context budgets. It lists only the core tools (`mcp::COMPACT_CORE_TOOLS`: open, info, save, close, render, text extract and find, combine, split, undo) plus the common command/inspection/preview tools and two meta tools: `tool_search` (optional `query` and `category`, the name prefix; or `name` for one tool's full `input_schema`) and `tool_call` (`name` and `arguments`, run through the same `Automation::call`). `tools/call` by name keeps working for every tool, and the server instructions mention the meta tools. Without the flag nothing changes.

Implemented: `initialize` (protocol 2025-06-18, 2025-03-26, 2024-11-05), `ping`, `tools/list` (with `readOnlyHint`/`destructiveHint` annotations), `tools/call` (JSON results also returned as `structuredContent`; images as `image/png`), and `resources/list`, `resources/templates/list` and `resources/read`. The resources expose the open documents read-only: `pdfcraft://doc/{doc}/info` (JSON), `…/text`, `…/page/{page}/text` and `…/page/{page}/image{?dpi}` (PNG, 1–600 dpi). Tool failures come back as `isError: true` results, so the agent can read and recover from them.

### From an installed release

The release packages ship `pdfcraft-cli` alongside the desktop app, so no build is needed:

| Install | CLI |
|---|---|
| Windows (MSI) | `%LOCALAPPDATA%\Programs\ArtCraft\PdfCraft\pdfcraft-cli.exe` by default (`C:\Program Files\ArtCraft\PdfCraft\pdfcraft-cli.exe` for an all-users install, under `Program Files (x86)` instead for the 32-bit build, or wherever you installed it otherwise), not on `PATH` |
| Linux (deb, rpm) | `/usr/bin/pdfcraft-cli` |
| macOS | the separate `pdfcraft-cli-<version>-macos-<arch>.zip` release asset (the `.app` holds only the desktop app) |

```sh
# Windows (PowerShell), default install folder
claude mcp add pdfcraft -- "$env:LOCALAPPDATA\Programs\ArtCraft\PdfCraft\pdfcraft-cli.exe" mcp --root <dir>
# Linux, or macOS with the CLI unzipped onto PATH
claude mcp add pdfcraft -- pdfcraft-cli mcp --root <dir>
```

## Page layout and sections

`page_layout {doc, pages?, text?, lines?}` analyses pages of a typeset document (a paper) from their
geometry: `columns` (box, zone, index, how many side by side), `blocks` with `kind` (`title`,
`abstract`, `heading`, `paragraph`, `display_equation`, `float`, `caption`, `footnote`,
`running_header`, `running_footer`, `page_number`, `list_item`, `other`), box, place in
`reading_order`, column, `confidence` and text, a heading's `level`, `number`, `level_guessed` and
`appendix`, a display's `equation_tag`, and a float's caption (and back). `lines: true` adds each
line's baseline and glyph indices. Each page carries `diagnostics` with a code (`no_text`,
`columns_undecided`, `heading_level_guessed`, `running_unconfirmed`, …) for what it is unsure of; a
scan or a blank page is an empty layout with `no_text`, not an error. Running heads and page numbers
are confirmed by repetition across pages, reading neighbouring pages when fewer than five are asked
for. At most 200 pages per call.

`doc_sections {doc}` returns the section tree from the headings: page, block, level, number, title,
parent, appendix and `level_guessed`, with the running elements found and how many pages were read
(at most 2000). Both need copy permission and read the current snapshot, unsaved edits included.

The analysis is scored by the layout oracle (`crates/render/tests/data/layout/oracle/README.md`):
synthetic LaTeX papers whose structure TeX itself reports. `cargo run -p pdfcraft-engine --example
layout_overlay -- IN.pdf PAGE OUT.png` draws what it found onto the page.

## Statements and references

`doc_blocks {doc, kinds?}` lists a typeset paper's referable blocks: theorem-like statements (the
printed word as `label`, an open set read from the text: Theorem, Lemma, Definition, Remark, Main
Theorem, …; `number` as printed; `name` in parentheses), proofs (`proves`: the statement their head
names, else the one right before them; `end_mark`: `box`, `filled_square`, `square`, `qed`, or none),
numbered headings, numbered displays, figures and tables, each with its extent page by page.
`doc_references {doc, source?, min_confidence?, resolved?, page?}` lists where the text mentions them
(`Lemma 2.1`, `Lemmas 2.1 and 2.3`, `Lemmas 2.1–2.3`, `Thm. 3`, `Section 3`, `§ 3`, `Figure 2`,
`Eq. (3.4)`, `(3.4)` after `by`, `see`, `from`, `in`, …): each with its clickable extent, its `target`
block (`null` with `unresolved` `external`, `missing` or `ambiguous` when it names another work, a
block that is not there, or two), and its `source`: `link` where a link annotation covers it (the link
decides; a `link_disagrees` diagnostic says when the text read otherwise) or `inferred`, with a
`confidence` (the viewer previews inferred mentions at 0.7 or more). The kind always comes from the
word, never from the number: Lemma 2.1 and Theorem 2.1 may share a counter. Both read at most 2000
pages (`truncated`), are read-only and need copy permission. The analysis is
`pdfcraft_render::text::page_layout::refs`, the graph `pdfcraft_engine::references`; scores against
TeX's own answer are in `crates/render/tests/data/refs/oracle/README.md` (`cargo xtask refs-oracle
report`). Citations of the bibliography are mentions too (below); `citations: false` leaves them out,
and `doc_blocks` lists bibliography entries only when `kinds` names `bib_entry`.

## Bibliography and citations

`doc_bibliography {doc, citations?, entry?}` reads a paper's reference list without help from links:
its `region` (the heading — References, Bibliography, Literature, Works cited, numbered or not — and
its first and last page; `found: false` with a `no_bibliography` diagnostic when nothing names a list
and the paper does not end with one numbered `[1]`, `[2]`, `[3]` in order), and its `entries` in
order, each with its `label` (`12`, `BKM20`; `printed_label` `[12]`), `label_style`, `extent` per page
and column, `text`, `parsed`, `confidence`, how often it is `cited`, and its `fields` — `authors`
(family and given names), `et_al`, `title`, `venue`, `year` (with a suffix: `2020a`), `volume`,
`number`, `pages`, `doi`, `arxiv`, `url`, `mr`, `zbl` — each a value with a confidence, or null when
the entry does not show it (never guessed). The fields are shaped like a BibTeX record's.
`citations: true` adds the citations; `entry` narrows to one entry. Citations also come through
`doc_references` (`kind: "citation"`): brackets of the list's labels (`[12]`, `[1, 3–5]`, `[BKM20]`,
`[12, Thm. 3.1]` with the locator apart) and, for a list without labels, a name with a year
(`Smith (2020)`, `(Smith and Jones, 2020a; Kay et al., 2019)`), each citation of a group with its own
`extent` and a shared `group`; the keys inside a range have no extent (`page: null`). A bracket in a
mathematics font, after `Section` or `Figure`, or whose parts are no labels of the list is no
citation; a key with no entry, or more than one, has no `target` (`unresolved`: `no_entry`,
`ambiguous`). A `cite.KEY` link decides where it has one (`citation_link_disagrees` when the text read
otherwise). Read-only; copy permission is required. The analysis is
`pdfcraft_render::text::page_layout::bib`; scores against TeX's and BibTeX's own answer are in
`crates/render/tests/data/bib/oracle/README.md` (`cargo xtask bib-oracle report`).

## Adding a tool

1. Add the engine capability first, with its tests (the tool is a thin adapter).
2. Add a `ToolDef` in `src/tools.rs`: a precise description, the schema, `ro()`/`destructive()`, and `cmd()` if a registry command exists.
3. Handle it in `Automation::call`, validating pages and positions with the existing helpers.
4. Add an end-to-end test in `tests/it/automation.rs`. `tool_table_is_well_formed` checks names, schemas and command links.


## Splitting two-page spreads

`page_split_spreads {doc, pages?, landscape_only?, min_ratio?, position?, margin?, order?, dry_run?}`
turns each scanned page that holds two book pages side by side into two pages, as Organize ▸ Split
two-page spreads does. Each half is a crop box on the same page content: nothing is re-encoded, the
scan is stored once, and the other half stays in the file behind the crop box.

| Argument | Default | Meaning |
| --- | --- | --- |
| `pages` | all | 1-based pages to consider. |
| `landscape_only` | `true` | Only pages whose width ÷ height as displayed is above `min_ratio`. A portrait page is split only when `pages` names it and this is `false`. |
| `min_ratio` | `1.2` | 1 to 10. |
| `position` | `"auto"` | `"auto"` looks for the gutter in a render of at most 100 dpi (the emptiest column in the middle 40% of the width, or the centre when none stands out); a number is a fixed fraction of the width from the left edge, 0.1 to 0.9. |
| `margin` | `0` | Points each half keeps from the split line, 0 to 1000. |
| `order` | `"ltr"` | `"ltr"`: left half first; `"rtl"`: right half first. |
| `dry_run` | `false` | Only report where each page would be split. |

The result lists, per split page, its 1-based `page`, `position`, `confidence`, `source`
(`gutter`, `centre` or `fixed`, with a `note` when it fell back to the centre), the new
`first_page` and `second_page`, `moved_annotations`, `annotations_on_both` and `first_half_only`
(what the second half does not carry, such as article beads or tagged structure). Bookmarks, links,
named destinations and page labels that went to a split page go to its first half. The whole
operation is one undo step, and a failure on any page leaves the document unchanged.

```json
[
  { "tool": "doc_open", "args": { "path": "scan.pdf" } },
  { "tool": "page_split_spreads", "args": { "doc": 1, "margin": 4 } },
  { "tool": "doc_save", "args": { "doc": 1, "path": "book.pdf" } }
]
```

## Moving several page objects

`object_list {doc, page}` inventories recognised paragraphs, Image/Form artwork and added items
with generation-scoped kind/index references. `object_move {doc, page, objects, offset, generation?}`
moves 1–1000 references atomically in displayed points (right/down), preserving source glyph bytes
and fonts. A refusal changes nothing; success is one undo step. `all: true` takes every object on the
page instead, as ⌘A does. `object_copy {doc, page, objects | all, cut?}` copies them with every
resource they use and `object_paste {doc, page}` pastes them as page content that draws the same,
into the same or another open document, stepped as the app steps a paste. See
[the selection and movement guide](../../docs/editing-multiple-objects.md) for limits and examples.

## Accessibility review record

Open the original and edited PDFs as separate documents, then call
`accessibility_review_record {before, after, path}`. The tool writes a JSON record atomically within
`--root`: SHA-256 hashes of both working PDFs, rule statuses and finding counts before and after,
changed rules, and rules still failed or requiring manual review. `all: true` also includes the
colour-contrast rule. It does not change either PDF or certify accessibility. Keep the original open
while making edits to the second document so the comparison has a stable baseline.

## Common command tools

Following FilmCraft's MCP conventions, `command_list {doc?, filter?, enabled_only?}` keeps its
`{commands: [...]}` result and adds each mapped tool's parameter schema. `command_run {id, params?}`
runs that command through its existing headless tool: include `doc` and other tool arguments in
`params`. Commands without a headless mapping return a tool error. Unknown parameter keys are
ignored with a `warnings` array; required values and paths still go through the original validation.

`command_batch {steps: [{id, params?}], stop_on_error?}` returns `completed`, `failed`, and per-step
`{ok, result?, error?}` entries. The default is to stop at the first error; earlier edits remain.
MCP marks a batch with any failed steps as `isError`, including through compact `tool_call`.
`doc_inspect {doc?}` returns one document's info, or `{documents: [...]}` for all open documents.
`render_preview {doc?, page?, max_side?}` returns a PNG (page defaults to 1, maximum side defaults
to 1024, allowed 1–4096). Omit `doc` only when exactly one document is open.

All task tools remain available; compact mode includes the five common tools. Every MCP tool has
all four annotation hints. Unknown top-level argument keys return JSON-RPC `-32602` with the
accepted keys; failures in the invoked tool remain `isError` results. An escaped tool panic is
reported as an internal tool error, and the server continues serving.

Exports complete synchronously. `progressToken` and cancellation notifications are harmlessly
ignored; no background-job or cancellation behavior is added.

Session resources `pdfcraft://document` and `pdfcraft://commands`, and the refusal of unknown argument keys, are described in [docs/mcp.md](../../docs/mcp.md).

For inline editor previews, `page_render` accepts `hide_text_block` (a 1-based index from
`text_paragraphs`). It renders the page without that paragraph’s source glyphs and leaves the
working document unchanged.

`page_render.hide_added_text` does the same for an added text item's 1-based index from
`content_list` or `object_list`. It removes only that item in a read-only preview and is
mutually exclusive with `hide_text_block`.

When the renderer reports a warning for the page, `page_render` and `render_preview` return a
JSON part after the PNG, `{ "page": 1, "warnings": [...] }`, with the sentences
`pdfcraft-cli render` prints, such as "content past the page's safety budget, or a picture whose
JBIG2 mask is past a decoder limit, was skipped" and "a colour that could not be computed was
painted in a fallback colour". A page without warnings returns
the PNG alone.


## Existing text and document fonts

`text_font_files` discovers eligible installed files on request (or specified confined folders),
`text_font_import` registers a full document face, and `text_fonts` lists source/full resources.
Use `text_edit.font_resource` to choose a resource, or keep source faces and let matching full
faces complete absent subset glyphs. `text_edit.ranges` accepts disjoint UTF-8 byte ranges;
`text_edit_layout` measures the same resource/range draft without committing. All changes are
undoable and survive saving/reopening. `image_hit` shares the UI's curve and hole hit testing.
The bounds, limits and font requirements are in [existing-text editing](../../docs/editing-existing-text.md)
and [image editing](../../docs/editing-images.md).

`text_fonts` also lists `builtin:thai` when the build includes the bundled Noto Sans Thai face
(the optional craft-fonts input). Passing it as `text_edit.font_resource` explicitly replaces a
paragraph or range with that face; the source font is never swapped for it silently. A build
without the face never lists it, and `text_edit` refuses it there. A page whose own fonts fill
the listing's 32 MiB preview budget can leave it out too; `text_edit` still accepts it then.

### Preparing grouped text

`text_prepare {doc, page}` makes supported grouped Form text editable on one page, as one click
on grouped text in the GUI's Edit text does. It keeps the group's original glyph operators and shared source
objects, records preparation and map repairs in the repair log, and is one undoable step. List
`text_paragraphs` afterwards; its indices belong to the prepared page. Besides the document
summary it returns what this edit did:

| Field | Type | Meaning |
| --- | --- | --- |
| `prepared_groups` | integer | Form placements drawn in place, so their text is now the page's own. |
| `repaired_fonts` | integer | Page fonts whose damaged legacy THSarabunPSK Thai map was repaired on a page-owned copy. |
| `refused_groups` | array of strings | One `name: reason` per group left as it was: unsafe compositing, tags, malformed content, or text grouped more than eight levels deep. |

The page's own fonts with that damaged map are repaired too, whether their text was grouped or
not. It fails, changing nothing, when the page has nothing to prepare or repair (the error gives
the first reason a group was left as it was) or the page is out of range.

### Superscript and subscript normalization

`text_script` changes characters to their Unicode superscript or subscript forms; it does not
change the font size or baseline. For characters that have such forms, uniformly plain text
toggles to the requested script, and uniformly scripted text toggles back to plain text.
Characters without a matching form stay unchanged and do not determine the selection's state.

A selection mixing plain and target-script characters normalizes to the requested script.
For example, subscript changes `H₂2` to `H₂₂`, then to `H22` on the next call; superscript
changes `Q²2` to `Q²²`, then to `Q22`. A second toggle does not reconstruct the original
mixture. Each call is one undoable text edit: `edit_undo` restores the exact prior text,
including its original mixture. Save/reopen preserves the resulting normalized characters.

## Opening local GoToR links

`link_open {doc, page, index, password?}` follows a local PDF link using `link_list`
numbering. Relative file specifications open normally. An **absolute** file specification
requires `allow_absolute: true`, after reviewing the target reported by `link_list`.
Omitting the argument or passing false refuses the request before checking or reading the
target file. This is an intentional compatibility change for callers that previously opened
absolute GoToR targets without explicit consent. Opting in does not bypass `--root`, symlink
confinement, regular-file, PDF-extension, size, or password validation. Consent applies only
to that call, including each password retry; it is never remembered for other links.

The desktop UI asks Allow/Cancel before following absolute GoToR targets and displays the
complete path. Relative links retain their existing behavior. Named destinations remain
unapplied with a warning.

## Reference previews and named destinations

`link_list` reports, for a link that goes through a named destination, its `dest_name` with the
`dest_kind` and `dest_number` its hyperref name gives (`equation.4.7` is an equation numbered
4.7). `dest_list {doc, from?, limit?}` lists every named destination of the document in page
order, with its page, page label, position, kind and number, at most `limit` (default 1000) per
call. `link_preview {doc, page, index}` or `link_preview {doc, dest}` renders what the viewer's
reference preview shows for a link or a destination: with `fit` (the default) the block the
destination names, read from the target page's glyphs (an equation's display with every row of
its `align`, a statement to its end, one bibliography entry, a heading and its first lines, a
float with its caption), else the band below the destination. The result says which (`fitted`)
and gives the window's caption (`Equation 4.7 · page 12`).

The kinds come from hyperref's names: statements that share a counter (`\newtheorem{lemma}[theorem]`)
are all `theorem.N`, so `theorem_like` cannot say Lemma or Theorem; the statement's own text does.
A number is taken from a name only when it follows hyperref's `counter.number` pattern. A page
the block cannot be read from (no text, a scan, rotated text) shows the band. The blocks are
tuned on article-like LaTeX (`crates/render/tests/data/hyperref`); other classes and layouts may
cut differently.

## Folder batches

`doc_batch` runs conversion, OCR and compression over files and optionally recursive
folders. The CLI also exposes it as `pdfcraft-cli batch`. See the
[batch guide](../../docs/cli-batch.md) for examples, output protection, errors and limits.

## Editing button export values

Use `form_set_props` with `field`, `export_value` and optional `widget`
(1-based, default 1) to change a checkbox or radio button export value.
`form_fields` returns `export_values` in widget order. The selected/default
appearance states are preserved; the operation is undoable and survives saving.
Widgets sharing an appearance state keep one export value. Bulk edits are not
supported for this widget-specific property. Empty values, `Off`, conflicting
choices and values over 4096 UTF-8 bytes return an error.

Example: `form_set_props doc=1 field=agree export_value=Accepted widget=1`.

## Rendering a 3D model

`model_render` draws a page's 3D model (a 3D annotation's PRC stream, directly or through a 3D
reference) to a PNG, without the viewer. `model` picks the page's Nth 3D model (default 1);
`width` and `height` are 1 to 4096 (default 512); `direction` is the view direction from the eye
(default `[-1, 1, -1]`, with Z up unless the view looks along Z); `perspective` defaults to true.
The model is framed to fill the image and shaded by a headlight on white. `orbit`, instead of
`direction`, frames it as a viewer's orbit control would: `{view, up, front, turn, zoom,
zoom_at, pan}`, all optional. `view` is `iso` (default), `front`, `right`, `top` or `back`
about the `up` axis (default `+z`) and the `front` axis (perpendicular to `up`); `turn` is
`[yaw, pitch]` in degrees, pitch down; `zoom` is a positive factor, the result held within 0.05 to 50, at `zoom_at` `[right, up]`
in picture heights from the centre, where the point under it stays still; `pan` `[right, up]`
moves the target, in model radii, so the model moves the other way. Every view frames the model
at the same size, whatever its direction. U3D models are refused
with an error, and the annotation's own views, lighting and render modes are not applied yet.

## Viewer navigation history

A running PdfCraft instance launched with `--control FILE` exposes its per-tab history through
`pdfcraft-cli ui --control FILE command id=view.previous_view` and
`pdfcraft-cli ui --control FILE command id=view.next_view`. `commands` reports whether each action
is enabled for the active document. These are the same actions as the rail buttons, View → Page
navigation, and Ctrl+[ / Ctrl+] (⌘[ / ⌘] on macOS).

Explicit page jumps, including search-result selection and Next/Previous match, enter history
when the page changes. Automatic search refresh, continuous scrolling, and within-page scroll
movement do not. Existing page-step commands and single-page wheel turns retain their history
behavior. Back/Forward
returns to page tops, does not restore zoom, does not switch documents, and is not saved across
restarts. Standalone engine/MCP sessions have no viewport, so they do not have a separate
navigation-history tool; use the opt-in UI control channel to navigate the displayed document.

## Fill & Sign text appearance

`fill_sign_add` accepts `font_size` (4–300 pt, default 10) and `color`
(`#RRGGBB`, default black) for `text` and `date`. The colour is saved in the
FreeText appearance and survives reopening. Other mark/signature types reject
`color`; their appearance controls are separate.

## AcroForm script interactions

`js_run` accepts optional `answers` (strings or null, at most 32), consumed in popup, response and alert order. Missing responses cancel menus/input; explicit viewer requests remain data in automation. See [the runtime contract](../../docs/acroform-script-workflows.md).

For distinct PDF fields sharing a name, form_fields exposes a document-local integer field_id. Pass it with field to js_run to bind event.target to that exact field (including ordered answers); omit it for existing first-match behavior. Mismatched names/IDs, missing names and non-integer/out-of-range IDs are refused before execution.

`form_set_image` accepts the optional document-local `field_id` from `form_fields` alongside the matching `field` name, selecting an exact duplicate-name image button. Omit it for the legacy first-name match. IDs are unsigned 48-bit integers and must be rediscovered after structural edits or reopening.

## Reviewing OCR headlessly

`ocr_words {doc, retain_review: true}` reads words without editing the document and returns
`review`, plus pages and word boxes/confidences. Page and word positions are 1-based.
`region` can restrict recognition to one named page, using displayed-page points. Without
`retain_review`, word inspection remains stateless.

Apply reviewed text with `ocr_review_apply {doc, review, corrections: [{page: 1, word: 1,
text: "Corrected"}]}`. Omitted words keep their text; empty text removes a word. Geometry
comes from the retained recognition, never caller input. The captured document guard rejects
stale/foreign results. Successful application consumes the handle and is one undoable step.
Use `output_mode: "editable", out: "reviewed.pdf"` to write a separate editable document,
or `discard: true` to release the result without applying it. Failed validation preserves the
review for correction or discard. A new retained recognition replaces that document's prior
handle; closing the document releases it. At most eight documents may retain reviews, with
65,536-word/4-MiB input bounds and a 65,536-page cap.

`ocr_dictionary {path: "ocr-dictionary-en.txt", add: ["Quartz"], remove: ["old"]}` atomically
updates the same one-word-per-UTF-8-line format OCR Verify uses. Omit add/remove to read it.
The explicit path honors `--root`, including symlink confinement; choose the appropriate
per-language file used by the UI. Entries are case-insensitive and duplicate-free. Invalid
or oversized inputs fail before writing (65,536 words, 4 MiB, at most 1,024 bytes per word).

## Recognizing text in an area

`ocr_area_text {doc, page, region}` is Edit ▸ Recognize text in area without the GUI: the text
inside `region` ([x0, y0, x1, y1] in points from the top-left of the displayed page). Where the
page has its own text there it comes back as it is (`source: "text_layer"`); otherwise only that
area is rendered and recognised with the given `engine`, `language` and `dpi` (`source` is the
engine's id, `ppocr` by default), with `words` and `lowest_band` (`high`, `medium`, `low`, or
null when no word reported a confidence). It changes nothing and is refused when the document
forbids copying. The engine reads the area through the same crop as `ocr_recognize`'s and
`ocr_words`'s `region` (`OcrJob::region_pixels`).
