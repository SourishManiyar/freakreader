//! Create Table of Contents: bookmarks made from what a document already says about its own
//! structure, for the user to review before they are written.
//!
//! In order of preference:
//!
//! 1. **Its table of contents.** Contents pages are found from the text layer near the front of
//!    the document (and the back, where some books put them): a "Contents" heading, or many rows
//!    ending in page numbers after dot leaders ([`detect`]). Their entries are read in reading
//!    order, two-column tables column by column, wrapped titles joined, and nested by numbering
//!    or indent. Printed page numbers are mapped to pages by finding each title as a heading,
//!    by page labels, and by the offset the evidence agrees on ([`mapping`]).
//! 2. **Its tagged headings** (H, H1–H6), each linked to its structure element.
//! 3. **Its headings by how they look** ([`headings`]): larger or bolder rows that read as
//!    headings, nested by their numbering or style. This is what most research papers need.
//!
//! Everything is bounded: pages read ([`MAX_FRONT_PAGES`], [`MAX_BACK_PAGES`],
//! [`mapping::MAX_READ_PAGES`], [`MAX_HEADING_PAGES`]), rows per page, entries and title lengths.
//! Text from the document is never trusted: garbled or hostile text gives fewer entries, never
//! an error or a panic.

mod detect;
mod headings;
mod layout;
mod mapping;
pub mod parse;

use std::collections::HashMap;

use pdfcraft_cos::{Document, ObjRef};

pub use detect::MAX_ENTRIES;
pub use layout::Seg;
pub use parse::{MAX_PLAIN_LINES, PageRef, PlainEntry, parse_outline_text};

/// Pages at the front of a document searched for a table of contents.
pub const MAX_FRONT_PAGES: usize = 60;
/// Pages at the back of a document searched for a table of contents.
pub const MAX_BACK_PAGES: usize = 20;
/// Most pages read looking for headings by how they look.
pub const MAX_HEADING_PAGES: usize = 1500;

/// Where a proposal's entries came from.
#[derive(Clone, Debug, PartialEq)]
pub enum TocSource {
    /// The document's own table of contents, on these pages (0-based).
    Contents { pages: Vec<usize> },
    /// Its tagged headings.
    Structure,
    /// Its headings found by how they look.
    Headings,
}

/// One proposed bookmark.
#[derive(Clone, Debug, PartialEq)]
pub struct TocEntry {
    /// 0 for the top level; at most one deeper than the entry before.
    pub level: usize,
    pub title: String,
    /// The 0-based page it goes to; `None` when it could not be placed.
    pub page: Option<usize>,
    /// Where its heading is on that page (user space), when known: the left (which a turned
    /// page scrolls along) and the top.
    pub left: Option<f64>,
    pub top: Option<f64>,
    /// The page number the table of contents printed for it.
    pub printed: Option<String>,
    /// Its title was found on its page (always true for tags and headings, which come from the
    /// page itself).
    pub verified: bool,
    /// The structure element it stands for, for tagged headings.
    pub element: Option<ObjRef>,
}

/// What Create Table of Contents found.
#[derive(Clone, Debug, PartialEq)]
pub struct TocProposal {
    pub source: TocSource,
    pub entries: Vec<TocEntry>,
}

impl TocProposal {
    /// Entries placed on a page their title was not found on.
    pub fn unverified(&self) -> usize {
        self.entries.iter().filter(|e| e.page.is_some() && !e.verified).count()
    }

    /// Entries that go nowhere: their printed page could not be mapped to a page.
    pub fn unplaced(&self) -> usize {
        self.entries.iter().filter(|e| e.page.is_none()).count()
    }

    /// The entries as new rows of a list edit ([`pdfcraft_organize::rewrite_outline`]).
    pub fn rows(&self) -> Vec<pdfcraft_organize::OutlineRow> {
        self.entries
            .iter()
            .map(|e| {
                let target = match e.page {
                    Some(page) => pdfcraft_organize::RowTarget::Page { page, left: e.left, top: e.top },
                    None => pdfcraft_organize::RowTarget::Nowhere,
                };
                pdfcraft_organize::OutlineRow { level: e.level, title: e.title.clone(), target, origin: None, open: None, element: e.element }
            })
            .collect()
    }
}

/// Where a pasted entry goes: (page, left, top, its title found there).
pub type PlacedEntry = (Option<usize>, Option<f64>, Option<f64>, bool);

/// Why nothing was proposed.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum TocError {
    #[error("no table of contents, tagged headings or headings were found in this document's text")]
    NothingFound,
}

/// Where Create Table of Contents reads a document's text: each page's pieces of text in a
/// y-up frame, and how a height in that frame becomes a destination on the page.
pub trait PageSource {
    fn count(&self) -> usize;
    /// The page's pieces of text; empty for a page that can't be read.
    fn segs(&mut self, page: usize) -> Vec<Seg>;
    /// The user-space (`left`, `top`) of a destination at height `top` of the source's frame.
    fn to_user(&self, page: usize, top: f64) -> (Option<f64>, Option<f64>);
}

/// The text layer the editing tools read ([`pdfcraft_edit::text_lines`]), in user space: each
/// run of each line, split into its words when it draws them apart.
pub struct LinesSource<'a> {
    doc: &'a Document,
    count: usize,
}

impl<'a> LinesSource<'a> {
    pub fn new(doc: &'a Document) -> Self {
        Self { doc, count: pdfcraft_organize::page_count(doc).unwrap_or(0) }
    }
}

impl PageSource for LinesSource<'_> {
    fn count(&self) -> usize {
        self.count
    }

    fn segs(&mut self, page: usize) -> Vec<Seg> {
        let Ok(lines) = pdfcraft_edit::text_lines(self.doc, page) else { return Vec::new() };
        let mut out = Vec::new();
        for l in lines {
            if out.len() >= layout::MAX_SEGS_PER_PAGE {
                break;
            }
            let [x0, y0, x1, y1] = l.rect;
            let (size, bold) = (l.size.abs(), l.bold || l.base_font.to_lowercase().contains("bold"));
            let mut any_run = false;
            for (text, ink, _) in l.runs() {
                any_run = true;
                // Each run's words, or the run whole when its text and boxes don't pair up.
                let words: Vec<&str> = text.split_whitespace().collect();
                if words.len() == ink.len() && !words.is_empty() {
                    for (w, [a, b, c, d]) in words.iter().zip(ink) {
                        out.push(Seg { text: (*w).to_string(), x0: a.min(*c), x1: a.max(*c), top: b.max(*d), bottom: b.min(*d), size, bold });
                    }
                } else if let Some(r) = ink.iter().copied().reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]) {
                    out.push(Seg { text: text.to_string(), x0: r[0], x1: r[2], top: r[3], bottom: r[1], size, bold });
                }
            }
            if !any_run {
                out.push(Seg { text: l.text.clone(), x0: x0.min(x1), x1: x0.max(x1), top: y0.max(y1), bottom: y0.min(y1), size, bold });
            }
        }
        out
    }

    fn to_user(&self, _page: usize, top: f64) -> (Option<f64>, Option<f64>) {
        (None, Some(top))
    }
}

/// The text layer the viewer finds and selects in: the renderer's words, which are spaced by
/// position as well as by space characters (as TeX sets them), read in view space so a turned
/// page reads upright. Bold comes from the fonts the editing text layer reads. Text that does
/// not run left to right on screen, such as an arXiv identifier up the margin, is left out.
pub struct RenderSource<'a> {
    renderer: pdfcraft_render::PageRenderer,
    pages: &'a [pdfcraft_render::PageInfo],
    doc: &'a Document,
}

impl<'a> RenderSource<'a> {
    pub fn new(
        bytes: std::sync::Arc<Vec<u8>>,
        config: pdfcraft_render::RenderConfig,
        pages: &'a [pdfcraft_render::PageInfo],
        doc: &'a Document,
    ) -> Self {
        Self { renderer: pdfcraft_render::PageRenderer::new(bytes, config), pages, doc }
    }
}

/// What Create Table of Contents reads, owned, so it can run off the UI thread
/// ([`crate::Document::toc_job`]): a snapshot of the document as it was when asked. Running it
/// reads the same pages, within the same bounds, as [`crate::Document::create_toc`].
pub struct TocJob {
    pub(crate) doc: Document,
    pub(crate) bytes: std::sync::Arc<Vec<u8>>,
    pub(crate) config: pdfcraft_render::RenderConfig,
    pub(crate) pages: Vec<pdfcraft_render::PageInfo>,
}

impl TocJob {
    /// Find the proposal, asking `keep_going` before each page is read, and counting the pages
    /// read in `pages_read`. `None` once `keep_going` said no: the pages it skipped would have
    /// changed what was found.
    pub fn run(&self, keep_going: &dyn Fn() -> bool, pages_read: &std::sync::atomic::AtomicUsize) -> Option<Result<TocProposal, TocError>> {
        let inner = RenderSource::new(self.bytes.clone(), self.config.clone(), &self.pages, &self.doc);
        let mut source = Stoppable { inner, keep_going, pages_read, stopped: false };
        let found = create_toc_from(&self.doc, &mut source);
        (!source.stopped && keep_going()).then_some(found)
    }
}

/// A source that reads no more pages once told to stop.
struct Stoppable<'a, S> {
    inner: S,
    keep_going: &'a dyn Fn() -> bool,
    pages_read: &'a std::sync::atomic::AtomicUsize,
    stopped: bool,
}

impl<S: PageSource> PageSource for Stoppable<'_, S> {
    fn count(&self) -> usize {
        self.inner.count()
    }

    fn segs(&mut self, page: usize) -> Vec<Seg> {
        if self.stopped || !(self.keep_going)() {
            self.stopped = true;
            return Vec::new();
        }
        self.pages_read.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.inner.segs(page)
    }

    fn to_user(&self, page: usize, top: f64) -> (Option<f64>, Option<f64>) {
        self.inner.to_user(page, top)
    }
}

/// A word being built from glyphs: its text, view-space box, the sum of its glyphs' heights and
/// how many glyphs it has.
type Word = (String, [f32; 4], f32, usize);

impl PageSource for RenderSource<'_> {
    fn count(&self) -> usize {
        self.pages.len()
    }

    fn segs(&mut self, page: usize) -> Vec<Seg> {
        let Some(info) = self.pages.get(page) else { return Vec::new() };
        let res =
            self.renderer.render(pdfcraft_render::RenderRequest { page, kind: pdfcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() });
        let Some(text) = res.text else { return Vec::new() };
        // Bold lines, in user space.
        let bold: Vec<[f64; 4]> = pdfcraft_edit::text_lines(self.doc, page)
            .map(|lines| {
                lines
                    .into_iter()
                    .filter(|l| l.bold || l.base_font.to_lowercase().contains("bold"))
                    .map(|l| l.rect)
                    .take(layout::MAX_SEGS_PER_PAGE)
                    .collect()
            })
            .unwrap_or_default();
        let mut out: Vec<Seg> = Vec::new();
        let flush = |word: &mut Option<Word>, out: &mut Vec<Seg>| {
            let Some((text, r, heights, n)) = word.take() else { return };
            if text.trim().is_empty() || n == 0 || out.len() >= layout::MAX_SEGS_PER_PAGE {
                return;
            }
            let size = f64::from(heights / n as f32);
            let [cx, cy] = info.view_to_user((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
            let (cx, cy) = (f64::from(cx), f64::from(cy));
            let is_bold = bold.iter().any(|b| cx >= b[0].min(b[2]) && cx <= b[0].max(b[2]) && cy >= b[1].min(b[3]) && cy <= b[1].max(b[3]));
            // View space is y down; this frame is its negation, so higher on the page is larger.
            out.push(Seg { text, x0: f64::from(r[0]), x1: f64::from(r[2]), top: -f64::from(r[1]), bottom: -f64::from(r[3]), size, bold: is_bold });
        };
        let mut word: Option<Word> = None;
        for (i, g) in text.glyphs.iter().enumerate() {
            if out.len() >= layout::MAX_SEGS_PER_PAGE {
                break;
            }
            let upright = g.direction[0] > 0.9 && g.direction[1].abs() < 0.2 && g.rect.iter().all(|v| v.is_finite());
            let breaks = i == 0
                || text.space_before.get(i).copied().unwrap_or(false)
                || text.line_of.get(i) != i.checked_sub(1).and_then(|k| text.line_of.get(k));
            if breaks || !upright {
                flush(&mut word, &mut out);
            }
            if !upright {
                continue;
            }
            let h = (g.rect[3] - g.rect[1]).abs();
            match word.as_mut() {
                Some((t, r, heights, n)) => {
                    t.push_str(&g.text);
                    *r = [r[0].min(g.rect[0]), r[1].min(g.rect[1]), r[2].max(g.rect[2]), r[3].max(g.rect[3])];
                    *heights += h;
                    *n += 1;
                }
                None => word = Some((g.text.clone(), g.rect, h, 1)),
            }
        }
        flush(&mut word, &mut out);
        out
    }

    fn to_user(&self, page: usize, top: f64) -> (Option<f64>, Option<f64>) {
        match self.pages.get(page) {
            Some(info) => {
                let [left, top] = info.xyz_for_view(None, Some(-top as f32));
                (left.map(f64::from), top.map(f64::from))
            }
            None => (None, None),
        }
    }
}

/// A document's pages as the layout reads them, each read once.
struct DocPages<'a> {
    source: &'a mut dyn PageSource,
    count: usize,
    rows: HashMap<usize, Vec<layout::Row>>,
    bodies: HashMap<usize, Option<mapping::PageBody>>,
    reads: usize,
    /// Pages whose rows may be read at most.
    budget: usize,
}

impl<'a> DocPages<'a> {
    fn new(source: &'a mut dyn PageSource, budget: usize) -> Self {
        let count = source.count();
        Self { source, count, rows: HashMap::new(), bodies: HashMap::new(), reads: 0, budget }
    }

    fn rows(&mut self, page: usize) -> Option<&Vec<layout::Row>> {
        if page >= self.count {
            return None;
        }
        if !self.rows.contains_key(&page) {
            if self.reads >= self.budget {
                return None;
            }
            self.reads += 1;
            let rows = layout::rows(&self.source.segs(page));
            self.rows.insert(page, rows);
        }
        self.rows.get(&page)
    }

    /// The text of the first and last row of `page`, letters only: what a running head or foot
    /// repeats from page to page.
    fn edges(&mut self, page: usize) -> [Option<String>; 2] {
        let key = |r: &layout::Row| r.text.chars().filter(|c| c.is_alphabetic()).flat_map(char::to_lowercase).collect::<String>();
        match self.rows(page) {
            Some(rows) => [rows.first().map(key), rows.last().map(key)],
            None => [None, None],
        }
    }
}

impl mapping::Pages for DocPages<'_> {
    fn count(&self) -> usize {
        self.count
    }

    fn body(&mut self, page: usize) -> Option<&mapping::PageBody> {
        if !self.bodies.contains_key(&page) {
            let rows = self.rows(page)?.clone();
            // A first or last row that the page before or after repeats is a running head or foot:
            // it names a chapter on every page, so it can't say where the chapter starts.
            let mine = self.edges(page);
            let before = page.checked_sub(1).map(|p| self.edges(p)).unwrap_or([None, None]);
            let after = self.edges(page.saturating_add(1));
            let repeated = |k: &Option<String>, i: usize| {
                k.as_ref().is_some_and(|k| !k.is_empty() && [before.get(i), after.get(i)].into_iter().flatten().any(|o| o.as_ref() == Some(k)))
            };
            let drop_first = repeated(&mine[0], 0);
            let drop_last = rows.len() > 1 && repeated(&mine[1], 1);
            let last_index = rows.len().saturating_sub(1);
            let mut number = None;
            let mut lines = Vec::new();
            for (i, r) in rows.iter().enumerate() {
                let edge = i == 0 || i == last_index;
                if edge && number.is_none() {
                    number = edge_number(&r.text);
                }
                if (i == 0 && drop_first) || (i == last_index && drop_last) {
                    continue;
                }
                lines.push((parse::compact(&r.text), r.top, r.size));
            }
            let mut all = String::new();
            for (l, _, _) in &lines {
                if all.len() < 200_000 {
                    all.push_str(l);
                }
            }
            self.bodies.insert(page, Some(mapping::PageBody { lines, all, number }));
        }
        self.bodies.get(&page).and_then(Option::as_ref)
    }
}

/// The page number printed in a header or footer row: the row itself, or its first or last word
/// set apart from the rest.
fn edge_number(text: &str) -> Option<PageRef> {
    let t = text.trim();
    if let Some(p) = parse::page_token(t, true) {
        return Some(p).filter(|p| !matches!(p, PageRef::Label(_)));
    }
    if t.split_whitespace().count() > 12 {
        return None;
    }
    let pieces: Vec<&str> = t.split(parse::GAP).map(str::trim).filter(|p| !p.is_empty()).collect();
    if pieces.len() < 2 {
        return None;
    }
    [pieces.first(), pieces.last()].into_iter().flatten().find_map(|p| parse::page_token(p, true).filter(|p| !matches!(p, PageRef::Label(_))))
}

/// Find the document's table of contents, else its tagged headings, else its headings by how
/// they look, as bookmarks to review, reading the text layer the editing tools read.
pub fn create_toc(doc: &Document) -> Result<TocProposal, TocError> {
    create_toc_from(doc, &mut LinesSource::new(doc))
}

/// [`create_toc`] reading the text from `source`.
pub fn create_toc_from(doc: &Document, source: &mut dyn PageSource) -> Result<TocProposal, TocError> {
    if let Some(p) = from_contents(doc, source) {
        return Ok(p);
    }
    let tagged = pdfcraft_a11y::headings(doc);
    if !tagged.is_empty() {
        let top = tagged.iter().map(|h| h.level).min().unwrap_or(1);
        let mut entries: Vec<TocEntry> = Vec::new();
        for h in tagged.into_iter().take(MAX_ENTRIES) {
            let deepest = entries.last().map_or(0, |e| e.level.saturating_add(1));
            let level = usize::from(h.level.saturating_sub(top)).min(deepest);
            entries.push(TocEntry {
                level,
                title: h.title,
                page: Some(h.page),
                left: None,
                top: None,
                printed: None,
                verified: true,
                element: h.obj,
            });
        }
        return Ok(TocProposal { source: TocSource::Structure, entries });
    }
    let mut pages = DocPages::new(source, MAX_HEADING_PAGES);
    let mut read = Vec::new();
    for page in 0..pages.count.min(MAX_HEADING_PAGES) {
        if let Some(rows) = pages.rows(page) {
            read.push((page, rows.clone()));
        }
    }
    let found = headings::headings(&read);
    if found.len() < 2 {
        return Err(TocError::NothingFound);
    }
    let entries = found
        .into_iter()
        .map(|h| {
            let (left, top) = pages.source.to_user(h.page, h.top);
            TocEntry { level: h.level, title: h.title, page: Some(h.page), left, top, printed: None, verified: true, element: None }
        })
        .collect();
    Ok(TocProposal { source: TocSource::Headings, entries })
}

/// The pages of the document's table of contents (empty when it has none), and their lines.
fn contents_pages(pages: &mut DocPages<'_>) -> (Vec<usize>, HashMap<usize, Vec<detect::Line>>) {
    let count = pages.count;
    let mut candidates: Vec<usize> = (0..count.min(MAX_FRONT_PAGES)).collect();
    candidates.extend(count.saturating_sub(MAX_BACK_PAGES).max(MAX_FRONT_PAGES.min(count))..count);
    let mut scored = Vec::new();
    let mut lines_of: HashMap<usize, Vec<detect::Line>> = HashMap::new();
    for page in candidates {
        let Some(rows) = pages.rows(page) else { continue };
        let (lines, s) = detect::score(rows);
        scored.push((page, s));
        lines_of.insert(page, lines);
    }
    (detect::best_run(&scored), lines_of)
}

/// The proposal from the document's own table of contents, if it has one.
fn from_contents(doc: &Document, source: &mut dyn PageSource) -> Option<TocProposal> {
    let mut pages = DocPages::new(source, mapping::MAX_READ_PAGES.saturating_add(MAX_FRONT_PAGES).saturating_add(MAX_BACK_PAGES));
    let (run, mut lines_of) = contents_pages(&mut pages);
    if run.is_empty() {
        return None;
    }
    let toc_lines: Vec<(usize, Vec<detect::Line>)> = run.iter().filter_map(|p| lines_of.remove(p).map(|l| (*p, l))).collect();
    let raw = detect::entries(&toc_lines);
    if raw.len() < 2 {
        return None;
    }
    let labels = page_labels_of(doc);
    let wanted: Vec<mapping::Wanted> = raw.iter().map(|e| mapping::Wanted { title: e.title.clone(), page: e.page.clone() }).collect();
    let placed = mapping::place(&mut pages, &wanted, labels.as_deref(), &run);
    let entries = raw
        .into_iter()
        .zip(placed)
        .map(|(e, p)| {
            let (left, top) = match (p.page, p.top) {
                (Some(page), Some(top)) => pages.source.to_user(page, top),
                _ => (None, None),
            };
            TocEntry {
                level: e.level,
                printed: e.page.as_ref().map(PageRef::printed),
                title: e.title,
                page: p.page,
                left,
                top,
                verified: p.verified,
                element: None,
            }
        })
        .collect();
    Some(TocProposal { source: TocSource::Contents { pages: run }, entries })
}

/// The document's own page labels, when it has `/PageLabels` (not the page numbers a viewer
/// shows by default).
fn page_labels_of(doc: &Document) -> Option<Vec<String>> {
    if pdfcraft_organize::page_label_ranges(doc).is_empty() {
        return None;
    }
    pdfcraft_organize::page_labels(doc).ok()
}

/// Map printed page numbers to pages as Create Table of Contents does (titles found as headings,
/// page labels, the offset the evidence agrees on): for outlines pasted with the page numbers a
/// book prints. One result per entry.
pub fn place_printed(doc: &Document, entries: &[(String, Option<PageRef>)]) -> Vec<PlacedEntry> {
    place_printed_from(doc, &mut LinesSource::new(doc), entries)
}

/// [`place_printed`] reading the text from `source`.
pub fn place_printed_from(doc: &Document, source: &mut dyn PageSource, entries: &[(String, Option<PageRef>)]) -> Vec<PlacedEntry> {
    let mut pages = DocPages::new(source, mapping::MAX_READ_PAGES.saturating_add(MAX_FRONT_PAGES).saturating_add(MAX_BACK_PAGES));
    let labels = page_labels_of(doc);
    // The contents pages list every title, so they are never where a title is found.
    let (toc, _) = contents_pages(&mut pages);
    let wanted: Vec<mapping::Wanted> = entries.iter().map(|(title, page)| mapping::Wanted { title: title.clone(), page: page.clone() }).collect();
    let placed = mapping::place(&mut pages, &wanted, labels.as_deref(), &toc);
    placed
        .into_iter()
        .map(|p| {
            let (left, top) = match (p.page, p.top) {
                (Some(page), Some(top)) => pages.source.to_user(page, top),
                _ => (None, None),
            };
            (p.page, left, top, p.verified)
        })
        .collect()
}
