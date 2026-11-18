//! A document's reference graph: its referable blocks (theorem-like statements, proofs, numbered
//! headings, numbered displays, figures and tables, and the entries of its bibliography) and the
//! places its text mentions or cites them, each with where it points and why it is believed: a real
//! link annotation ([`Source::Link`]) or the analysis of the text ([`Source::Inferred`],
//! `pdfcraft_render::text::page_layout::refs` and `…::bib`).
//!
//! Where a link and the text both say something about one mention and disagree, the link wins and
//! a `link_disagrees` diagnostic says what the text said. A link with no mention found under it
//! still enters the graph, with its own target. It is a plain structure of numbers and strings, for
//! the tools (`doc_blocks`, `doc_references`), the viewer's previews of inferred mentions, and what
//! later reads the graph (a dependency graph, outline filters).
//!
//! Read-only. Pages are read without rasterizing them, at most [`MAX_REFERENCE_PAGES`]; past that
//! the rest are left out and the graph says so. Work is linear in the glyphs, links and mentions.

use pdfcraft_render::text::page_layout::bib::{self, Bibliography};
use pdfcraft_render::text::page_layout::refs::{self, PageWords, References, Target};
use pdfcraft_render::text::page_layout::{self, BlockKind as LayoutKind, Diagnostic, DocumentStructure, PageContext, PageLayout};
use pdfcraft_render::{DestView, Link, LinkTarget, PageInfo, PageRenderer};
use std::collections::HashMap;

use crate::export::ExportSource;
use crate::link_preview::{RefKind, ref_name};

/// Most pages read for a reference graph.
pub const MAX_REFERENCE_PAGES: usize = 2_000;
/// The least confidence an inferred mention needs before the viewer previews its target.
pub const PREVIEW_CONFIDENCE: f32 = 0.7;
/// Most links taken into the graph.
pub const MAX_GRAPH_LINKS: usize = 200_000;
/// How far above a block a link's destination may sit and still name it, in points: hyperref puts
/// a statement's anchor a line above its head.
const DEST_ABOVE: f32 = 24.0;

/// What a block of the graph is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlockKind {
    Statement,
    Proof,
    Heading,
    Equation,
    Float,
    /// An entry of the bibliography.
    BibEntry,
}

impl BlockKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BlockKind::Statement => "statement",
            BlockKind::Proof => "proof",
            BlockKind::Heading => "heading",
            BlockKind::Equation => "equation",
            BlockKind::Float => "float",
            BlockKind::BibEntry => "bib_entry",
        }
    }

    pub const ALL: [BlockKind; 6] =
        [BlockKind::Statement, BlockKind::Proof, BlockKind::Heading, BlockKind::Equation, BlockKind::Float, BlockKind::BibEntry];

    pub fn from_name(s: &str) -> Option<BlockKind> {
        BlockKind::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// Part of a block or mention on one page: its box in the page's view space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extent {
    pub page: usize,
    pub rect: [f32; 4],
}

/// A block of the graph.
#[derive(Clone, Debug, PartialEq)]
pub struct RefBlock {
    pub id: usize,
    pub kind: BlockKind,
    /// The printed word (`Lemma`, `Main Theorem`, `Proof`), or `Section`, `Equation`, `Figure`,
    /// `Table`; for a bibliography entry its printed label (`[12]`, `[BKM20]`, `12.`) or, in a list
    /// without labels, its first author and year (`Okafor 2020a`).
    pub label: String,
    /// The number as printed (`2.1`, `(3.4)` for an equation).
    pub number: Option<String>,
    /// A statement's name, a heading's title, a proof's rest of head (`of Lemma 2.1`).
    pub name: Option<String>,
    /// Page by page, in reading order.
    pub extent: Vec<Extent>,
    /// A statement's head.
    pub head: Option<Extent>,
    /// A proof's statement (a block id).
    pub proves: Option<usize>,
    /// A proof's end mark (`box`, `filled_square`, `square`, `qed`).
    pub end_mark: Option<&'static str>,
    pub confidence: f32,
}

impl RefBlock {
    /// The page it starts on.
    pub fn page(&self) -> usize {
        self.extent.first().map_or(0, |e| e.page)
    }

    /// Its box on its first page.
    pub fn first_rect(&self) -> [f32; 4] {
        self.extent.first().map_or([0.0; 4], |e| e.rect)
    }

    /// What it is called: `Lemma 2.1`, `Equation (3.4)`, `Section 3`, `Remark`, `[12]`.
    pub fn title(&self) -> String {
        match (&self.number, self.kind) {
            (_, BlockKind::BibEntry) | (None, _) => self.label.clone(),
            (Some(n), _) => format!("{} {n}", self.label),
        }
    }
}

/// Why a mention's target is believed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    /// A link annotation of the document.
    Link,
    /// The analysis of the text.
    Inferred,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Link => "link",
            Source::Inferred => "inferred",
        }
    }
}

/// A place the text mentions a block.
#[derive(Clone, Debug, PartialEq)]
pub struct RefMention {
    pub id: usize,
    /// `statement`, `section`, `equation`, `figure`, `table`, `citation`, or `link` for a link with
    /// no mention found under it.
    pub kind: &'static str,
    /// What it says (`Lemma 2.1`, `(3.4)`, `2.3`; a citation's label or names and year: `12`,
    /// `BKM20`, `Smith and Jones 2020a`).
    pub text: String,
    /// The clickable part, page by page; none for a key inside a range of citations (`2` of
    /// `[1–3]`).
    pub extent: Vec<Extent>,
    /// The block it points at (an id into [`ReferenceGraph::blocks`]).
    pub target: Option<usize>,
    pub source: Source,
    /// 1 for a link; the analysis's for an inferred mention; 0 without a target.
    pub confidence: f32,
    /// Why there is no target: `external`, `missing`, `ambiguous`, `no_block` (a link to a place no
    /// block of the graph covers).
    pub unresolved: Option<&'static str>,
    /// The link annotation: its page and `/Annots` index.
    pub link: Option<(usize, usize)>,
    /// For a citation: what follows its label in its bracket (`Thm. 3.1`).
    pub locator: Option<String>,
    /// For a citation: the citations of one bracket or parenthesis share it.
    pub group: Option<usize>,
}

impl RefMention {
    /// The page it is on (0 for a citation inside a range, which has no extent).
    pub fn page(&self) -> usize {
        self.extent.first().map_or(0, |e| e.page)
    }

    /// Whether the point `(x, y)` (view space of `page`) is on it.
    pub fn contains(&self, page: usize, x: f32, y: f32) -> bool {
        self.extent.iter().any(|e| e.page == page && x >= e.rect[0] && x <= e.rect[2] && y >= e.rect[1] && y <= e.rect[3])
    }
}

/// Something the graph is unsure of.
#[derive(Clone, Debug, PartialEq)]
pub struct RefDiagnostic {
    pub code: &'static str,
    pub message: String,
    pub page: Option<usize>,
}

/// A document's reference graph.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReferenceGraph {
    pub blocks: Vec<RefBlock>,
    /// In reading order.
    pub mentions: Vec<RefMention>,
    pub diagnostics: Vec<RefDiagnostic>,
    pub pages_read: usize,
    /// More than [`MAX_REFERENCE_PAGES`] pages: the rest were not read.
    pub truncated: bool,
    /// The bibliography as read from the text: its region, entries with their fields, and the
    /// citations (before the links were weighed; [`ReferenceGraph::mentions`] has the outcome).
    pub bibliography: Bibliography,
    /// Per bibliography entry, its block's id.
    pub entry_blocks: Vec<usize>,
    /// Per page, the mentions on it.
    by_page: HashMap<usize, Vec<usize>>,
}

impl ReferenceGraph {
    pub fn block(&self, id: usize) -> Option<&RefBlock> {
        self.blocks.get(id)
    }

    /// The mention at `(x, y)` on `page` (view space) whose target is known with at least
    /// `confidence`, if any.
    pub fn mention_at(&self, page: usize, x: f32, y: f32, confidence: f32) -> Option<&RefMention> {
        if !(x.is_finite() && y.is_finite()) {
            return None;
        }
        self.by_page
            .get(&page)?
            .iter()
            .filter_map(|&i| self.mentions.get(i))
            .find(|m| m.target.is_some() && m.confidence >= confidence && m.contains(page, x, y))
    }

    /// The mentions on `page`.
    pub fn mentions_on(&self, page: usize) -> impl Iterator<Item = &RefMention> {
        self.by_page.get(&page).into_iter().flatten().filter_map(|&i| self.mentions.get(i))
    }

    /// The mention made from the link at `/Annots` index `index` on `page`, if the graph holds it.
    pub fn mention_of_link(&self, page: usize, index: usize) -> Option<&RefMention> {
        self.by_page.get(&page)?.iter().filter_map(|&i| self.mentions.get(i)).find(|m| m.link == Some((page, index)))
    }

    /// The bibliography entry (index into [`Bibliography::entries`]) block `id` is, if one.
    pub fn entry_of_block(&self, id: usize) -> Option<usize> {
        self.entry_blocks.iter().position(|&b| b == id)
    }
}

fn layout_block(layouts: &[PageLayout], page: usize, block: usize) -> Option<&page_layout::Block> {
    layouts.get(page)?.blocks.get(block)
}

fn union(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
}

fn finite(r: [f32; 4]) -> bool {
    r.iter().all(|v| v.is_finite())
}

/// What [`blocks`] gives: the blocks, a map from a layout block to its block id, and per
/// bibliography entry its block id.
type Blocks = (Vec<RefBlock>, HashMap<(usize, usize), usize>, Vec<usize>);

/// The blocks: statements and proofs first (so a statement's index is its id), then numbered
/// headings, displays and floats, then the bibliography's entries. Returns them, a map from a
/// layout block to its id, and each entry's id.
fn blocks(layouts: &[PageLayout], structure: &DocumentStructure, r: &References, bib: &Bibliography) -> Blocks {
    let mut out: Vec<RefBlock> = r
        .statements
        .iter()
        .enumerate()
        .map(|(i, s)| RefBlock {
            id: i,
            kind: if s.proof { BlockKind::Proof } else { BlockKind::Statement },
            label: s.kind.clone(),
            number: s.number.clone(),
            name: s.name.clone(),
            extent: s.extent.iter().filter(|p| finite(p.rect)).map(|p| Extent { page: p.page, rect: p.rect }).collect(),
            head: Some(Extent { page: s.head.page, rect: s.head.rect }),
            proves: s.proves.map(|(k, _)| k),
            end_mark: s.end_mark.as_ref().map(|(_, m)| m.kind.as_str()),
            confidence: s.confidence,
        })
        .collect();
    let mut at: HashMap<(usize, usize), usize> = HashMap::new();
    let mut add = |out: &mut Vec<RefBlock>, page: usize, block: usize, kind: BlockKind, label: &str, number: Option<String>, name: Option<String>| {
        let Some(b) = layout_block(layouts, page, block) else { return };
        if at.contains_key(&(page, block)) || !finite(b.rect) {
            return;
        }
        // A float shows with its caption.
        let rect = match (kind, b.partner.and_then(|p| layout_block(layouts, page, p))) {
            (BlockKind::Float, Some(f)) if finite(f.rect) => union(b.rect, f.rect),
            _ => b.rect,
        };
        let id = out.len();
        at.insert((page, block), id);
        out.push(RefBlock {
            id,
            kind,
            label: label.to_owned(),
            number,
            name,
            extent: vec![Extent { page, rect }],
            head: None,
            proves: None,
            end_mark: None,
            confidence: b.confidence,
        });
    };
    for s in &structure.sections {
        if s.number.is_some() {
            add(&mut out, s.page, s.block, BlockKind::Heading, "Section", s.number.clone(), Some(s.title.clone()));
        }
    }
    for l in layouts {
        for (bi, b) in l.blocks.iter().enumerate() {
            match b.kind {
                LayoutKind::DisplayEquation if b.equation_tag.is_some() => {
                    add(&mut out, l.page, bi, BlockKind::Equation, "Equation", b.equation_tag.clone(), None);
                }
                LayoutKind::Caption => {
                    let t = b.text.trim_start();
                    let label = if t.starts_with("Figure") || t.starts_with("Fig.") {
                        "Figure"
                    } else if t.starts_with("Table") {
                        "Table"
                    } else {
                        continue;
                    };
                    let number =
                        t.split_whitespace().nth(1).map(|n| n.trim_end_matches([':', '.']).to_owned()).filter(|n| !n.is_empty() && n.len() <= 16);
                    add(&mut out, l.page, bi, BlockKind::Float, label, number, None);
                }
                _ => {}
            }
        }
    }
    // The bibliography's entries, last: their own extents, one per page and column.
    let mut entry_blocks = Vec::with_capacity(bib.entries.len());
    for e in &bib.entries {
        let id = out.len();
        entry_blocks.push(id);
        out.push(RefBlock {
            id,
            kind: BlockKind::BibEntry,
            label: entry_label(e),
            number: e.label.clone(),
            name: e.fields.title.as_ref().map(|t| t.value.clone()),
            extent: e.pieces.iter().filter(|p| finite(p.rect)).map(|p| Extent { page: p.page, rect: p.rect }).collect(),
            head: None,
            proves: None,
            end_mark: None,
            confidence: e.confidence,
        });
    }
    (out, at, entry_blocks)
}

/// What an entry is called: its printed label (`[12]`), else its first author and year
/// (`Okafor 2020a`, `Smith and Jones 2019`, `Kay et al. 2019`), else `Reference N`.
pub fn entry_label(e: &bib::Entry) -> String {
    if let Some(l) = e.printed_label() {
        return l;
    }
    let year = e.fields.year.as_ref().map(|y| y.value.as_str());
    let names = match e.fields.authors.as_slice() {
        [] => None,
        [a] => Some(a.family.clone()),
        [a, b] if !e.fields.et_al => Some(format!("{} and {}", a.family, b.family)),
        [a, ..] => Some(format!("{} et al.", a.family)),
    };
    match (names, year) {
        (Some(n), Some(y)) => format!("{n} {y}"),
        (Some(n), None) => n,
        _ => format!("Reference {}", e.id + 1),
    }
}

fn target_id(t: Target, at: &HashMap<(usize, usize), usize>) -> Option<usize> {
    match t {
        Target::Statement(k) => Some(k),
        Target::Heading { page, block } | Target::Equation { page, block } | Target::Float { page, block } => at.get(&(page, block)).copied(),
    }
}

/// The block a destination at height `y` on page `page` names: among the blocks with a part on
/// that page whose box (with [`DEST_ABOVE`] of slack above it) holds `y`, the one whose top is
/// nearest `y` of the kind the destination's name says (none when no such block holds it), or of
/// any kind when the name says nothing.
fn block_at(blocks: &[RefBlock], page: usize, y: f32, kind: RefKind) -> Option<usize> {
    let want = |b: &RefBlock| match kind {
        RefKind::Equation => b.kind == BlockKind::Equation,
        RefKind::Section => b.kind == BlockKind::Heading,
        RefKind::Figure | RefKind::Table => b.kind == BlockKind::Float,
        RefKind::TheoremLike => b.kind == BlockKind::Statement,
        RefKind::Citation => b.kind == BlockKind::BibEntry,
        _ => false,
    };
    let holding: Vec<(&RefBlock, [f32; 4])> = blocks
        .iter()
        .filter(|b| b.kind != BlockKind::Proof)
        .filter_map(|b| b.extent.iter().find(|e| e.page == page).map(|e| (b, e.rect)))
        .filter(|(_, r)| y >= r[1] - DEST_ABOVE && y <= r[3] + 2.0)
        .collect();
    let area = |r: &[f32; 4]| (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0);
    // A destination marks the top of what it names (hyperref raises it a line above): the block
    // whose top is nearest wins, the smaller one of two at the same place.
    let rank = |r: &[f32; 4]| ((r[1] - y).abs(), area(r));
    let smallest = |list: &mut dyn Iterator<Item = &(&RefBlock, [f32; 4])>| {
        list.min_by(|a, b| {
            let (ra, rb) = (rank(&a.1), rank(&b.1));
            ra.0.total_cmp(&rb.0).then(ra.1.total_cmp(&rb.1))
        })
        .map(|(b, _)| b.id)
    };
    // A name that says what the destination is settles it: a block of another kind is no answer.
    match kind {
        RefKind::Equation | RefKind::Section | RefKind::Figure | RefKind::Table | RefKind::TheoremLike | RefKind::Citation => {
            smallest(&mut holding.iter().filter(|(b, _)| want(b)))
        }
        _ => smallest(&mut holding.iter()),
    }
}

/// The words under `rect` on `page`, joined.
fn words_under(words: &[PageWords], page: usize, rect: [f32; 4]) -> String {
    let mut out = String::new();
    for l in words.get(page).map(|w| w.lines.as_slice()).unwrap_or(&[]) {
        for w in &l.words {
            let r = w.rect;
            if r[0] < rect[2] && rect[0] < r[2] && r[1] < rect[3] && rect[1] < r[3] {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(&w.text);
            }
            if out.len() > 200 {
                return out;
            }
        }
    }
    out
}

/// The graph of a document already analysed: its pages' layouts and words, section tree and
/// references (the same pages, from the first), its links and its pages (all of them; links from
/// pages past those analysed are left out).
#[allow(clippy::too_many_arguments)]
pub fn build_graph(
    layouts: &[PageLayout],
    words: &[PageWords],
    structure: &DocumentStructure,
    r: &References,
    bib: &Bibliography,
    links: &[Link],
    pages: &[PageInfo],
) -> ReferenceGraph {
    let (blocks, at, entry_blocks) = blocks(layouts, structure, r, bib);
    let mut g = ReferenceGraph { pages_read: layouts.len(), ..Default::default() };
    let to_diag = |d: &Diagnostic| RefDiagnostic { code: d.code, message: d.message.clone(), page: None };
    g.diagnostics.extend(r.diagnostics.iter().map(to_diag));
    g.diagnostics.extend(bib.diagnostics.iter().map(to_diag));
    let mut mentions: Vec<RefMention> = r
        .mentions
        .iter()
        .map(|m| {
            let target = m.target.and_then(|t| target_id(t, &at));
            RefMention {
                id: 0,
                kind: m.kind.as_str(),
                text: format!("{} {}", m.word, m.number).trim().to_owned(),
                extent: m.pieces.iter().filter(|p| finite(p.rect)).map(|p| Extent { page: p.page, rect: p.rect }).collect(),
                target,
                source: Source::Inferred,
                confidence: if target.is_some() { m.confidence } else { 0.0 },
                unresolved: m.unresolved.map(refs::Unresolved::as_str).or(if target.is_none() { Some("missing") } else { None }),
                link: None,
                locator: None,
                group: None,
            }
        })
        .collect();
    // Citations, inferred.
    mentions.extend(bib.citations.iter().map(|c| {
        let target = c.target.and_then(|e| entry_blocks.get(e).copied());
        RefMention {
            id: 0,
            kind: "citation",
            text: c.text.clone(),
            extent: c.pieces.iter().filter(|p| finite(p.rect)).map(|p| Extent { page: p.page, rect: p.rect }).collect(),
            target,
            source: Source::Inferred,
            confidence: if target.is_some() { c.confidence } else { 0.0 },
            unresolved: c.unresolved.map(bib::CiteUnresolved::as_str).or(if target.is_none() { Some("no_entry") } else { None }),
            link: None,
            locator: c.locator.clone(),
            group: Some(c.group),
        }
    }));
    // Real links, in the pages read.
    for link in links.iter().filter(|l| l.page < layouts.len()).take(MAX_GRAPH_LINKS) {
        let LinkTarget::Page(target_page, view) = link.target else { continue };
        let reference = link.dest_name.as_deref().map(ref_name);
        let cites = reference.as_ref().is_some_and(|n| n.kind == RefKind::Citation);
        let (Some(here), Some(there)) = (pages.get(link.page), pages.get(target_page)) else { continue };
        let a = here.user_to_view(link.rect[0], link.rect[1]);
        let b = here.user_to_view(link.rect[2], link.rect[3]);
        let rect = [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])];
        if !finite(rect) {
            continue;
        }
        let y = match view {
            DestView::Xyz { left, top, .. } => there.dest_fraction(left, top, 0)[1],
            DestView::FitH { top } => there.dest_fraction(None, top, 0)[1],
            DestView::FitR { rect } => there.dest_fraction(Some(rect[0]), Some(rect[3]), 0)[1],
            DestView::Top | DestView::Fit | DestView::FitV { .. } => Some(0.0),
        }
        .map(|f| f * there.height);
        let kind = reference.as_ref().map_or(RefKind::Unknown, |n| n.kind);
        let dest = y.filter(|y| y.is_finite()).and_then(|y| block_at(&blocks, target_page, y, kind));
        // A citation link takes the citation under it; another link, a mention of a block.
        let under = mentions.iter_mut().find(|m| {
            m.link.is_none()
                && (m.kind == "citation") == cites
                && m.extent
                    .iter()
                    .any(|e| e.page == link.page && e.rect[0] < rect[2] && rect[0] < e.rect[2] && e.rect[1] < rect[3] && rect[1] < e.rect[3])
        });
        match under {
            Some(m) => {
                if m.target != dest {
                    let name = |t: Option<usize>| t.and_then(|t| blocks.get(t)).map_or_else(|| "nothing".to_owned(), RefBlock::title);
                    g.diagnostics.push(RefDiagnostic {
                        code: if m.kind == "citation" { "citation_link_disagrees" } else { "link_disagrees" },
                        message: format!(
                            "page {}: \"{}\" reads as {}, its link goes to {}; the link is taken",
                            link.page + 1,
                            m.text,
                            name(m.target),
                            name(dest)
                        ),
                        page: Some(link.page),
                    });
                }
                m.target = dest;
                m.source = Source::Link;
                m.confidence = if dest.is_some() { 1.0 } else { 0.0 };
                m.unresolved = if dest.is_some() { None } else { Some("no_block") };
                m.link = Some((link.page, link.index));
            }
            None => mentions.push(RefMention {
                id: 0,
                kind: if cites { "citation" } else { "link" },
                text: words_under(words, link.page, rect),
                extent: vec![Extent { page: link.page, rect }],
                target: dest,
                source: Source::Link,
                confidence: if dest.is_some() { 1.0 } else { 0.0 },
                unresolved: if dest.is_some() { None } else { Some("no_block") },
                link: Some((link.page, link.index)),
                locator: None,
                group: None,
            }),
        }
    }
    let key = |m: &RefMention| {
        let e = m.extent.first().copied().unwrap_or(Extent { page: usize::MAX, rect: [0.0; 4] });
        (e.page, (e.rect[1] * 4.0) as i64, (e.rect[0] * 4.0) as i64)
    };
    // Reading order, kept stable for mentions on one line.
    mentions.sort_by_key(key);
    for (i, m) in mentions.iter_mut().enumerate() {
        m.id = i;
        // A key inside a range has no extent and no page to be found on.
        if !m.extent.is_empty() {
            g.by_page.entry(m.page()).or_default().push(i);
        }
    }
    g.blocks = blocks;
    g.mentions = mentions;
    g.bibliography = bib.clone();
    g.entry_blocks = entry_blocks;
    g
}

/// The reference graph of the snapshot, from its first [`MAX_REFERENCE_PAGES`] pages, with its
/// links (`links`) and pages (`pages`, as `DocInfo` gives them). `Err` only for a panic caught.
pub fn reference_graph(source: ExportSource, links: &[Link], pages: &[PageInfo]) -> Result<ReferenceGraph, String> {
    crate::guard(|| {
        let n = source.pages.min(MAX_REFERENCE_PAGES);
        let mut renderer = PageRenderer::new(source.bytes.clone(), source.config.clone());
        let mut layouts = Vec::with_capacity(n);
        let mut words = Vec::with_capacity(n);
        for page in 0..n {
            let (width, height) = source.sizes.get(page).copied().unwrap_or((0.0, 0.0));
            let ctx = PageContext { index: page, width, height };
            // A page that cannot be read gives an empty layout (and no words); the rest go on.
            let (layout, w) = match renderer.glyph_details(page) {
                Ok(math) => {
                    let l = page_layout::analyse_page(&math, ctx);
                    let w = refs::page_words(&math, &l);
                    (l, w)
                }
                Err(e) => (
                    PageLayout {
                        page,
                        size: [width, height],
                        diagnostics: vec![Diagnostic {
                            code: "unreadable",
                            message: format!("the page's content could not be read: {e}"),
                            block: None,
                        }],
                        ..Default::default()
                    },
                    PageWords { page, ..Default::default() },
                ),
            };
            layouts.push(layout);
            words.push(w);
        }
        let structure = page_layout::analyse_document(&mut layouts);
        // The document's view of running heads and headings decides what each line is.
        for w in &mut words {
            for line in &mut w.lines {
                if let Some(b) = layouts.get(line.page).and_then(|l| l.blocks.get(line.block)) {
                    line.kind = b.kind;
                }
            }
        }
        let r = refs::analyse_references(&layouts, &words, &structure);
        let b = bib::analyse_bibliography(&layouts, &words, &structure);
        let mut g = build_graph(&layouts, &words, &structure, &r, &b, links, pages);
        g.truncated = source.pages > n;
        if g.truncated {
            g.diagnostics.push(RefDiagnostic {
                code: "truncated",
                message: format!("only the first {n} of {} pages were read", source.pages),
                page: None,
            });
        }
        g
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_render::text::page_layout::refs::MentionKind;

    fn page() -> PageInfo {
        PageInfo { width: 612.0, height: 792.0, label: "1".into(), crop: [0.0, 0.0, 612.0, 792.0], rotation: 0 }
    }

    fn block(id: usize, kind: BlockKind, rect: [f32; 4]) -> RefBlock {
        RefBlock {
            id,
            kind,
            label: "Lemma".into(),
            number: Some("1".into()),
            name: None,
            extent: vec![Extent { page: 0, rect }],
            head: None,
            proves: None,
            end_mark: None,
            confidence: 0.9,
        }
    }

    #[test]
    fn a_destination_names_the_block_of_its_kind_it_falls_in() {
        let blocks = vec![block(0, BlockKind::Statement, [72.0, 100.0, 540.0, 300.0]), block(1, BlockKind::Equation, [200.0, 180.0, 540.0, 210.0])];
        assert_eq!(block_at(&blocks, 0, 190.0, RefKind::Equation), Some(1));
        assert_eq!(block_at(&blocks, 0, 85.0, RefKind::TheoremLike), Some(0), "an anchor a line above its head");
        assert_eq!(block_at(&blocks, 0, 190.0, RefKind::Unknown), Some(1), "the nearest top");
        // The next statement's anchor, a line above its head, inside the slack below this one.
        let two = vec![block(0, BlockKind::Statement, [72.0, 100.0, 540.0, 300.0]), block(1, BlockKind::Statement, [72.0, 310.0, 540.0, 400.0])];
        assert_eq!(block_at(&two, 0, 298.0, RefKind::TheoremLike), Some(1));
        assert_eq!(block_at(&blocks, 0, 500.0, RefKind::TheoremLike), None);
        assert_eq!(block_at(&blocks, 1, 190.0, RefKind::TheoremLike), None);
    }

    #[test]
    fn a_link_wins_over_the_text_and_says_so() {
        let mut r = References::default();
        r.mentions.push(refs::Mention {
            kind: MentionKind::Statement,
            word: "Theorem".into(),
            statement_kind: Some("Theorem".into()),
            number: "2".into(),
            extra: None,
            pieces: vec![refs::Piece { page: 0, glyphs: vec![1, 2], rules: Vec::new(), rect: [100.0, 400.0, 150.0, 410.0] }],
            target: None,
            unresolved: Some(refs::Unresolved::External),
            confidence: 0.0,
        });
        let layouts = vec![PageLayout {
            blocks: vec![page_layout::Block {
                kind: LayoutKind::DisplayEquation,
                rect: [200.0, 180.0, 540.0, 210.0],
                lines: Vec::new(),
                rules: Vec::new(),
                text: "x (2)".into(),
                order: 0,
                column: None,
                confidence: 0.8,
                style: Default::default(),
                heading: None,
                equation_tag: Some("(2)".into()),
                partner: None,
            }],
            reading_order: vec![0],
            ..Default::default()
        }];
        // In user space, y up: the mention's box is 382..392, the destination 602 (190 down).
        let link = Link {
            page: 0,
            index: 3,
            rect: [100.0, 382.0, 150.0, 392.0],
            quads: Vec::new(),
            target: LinkTarget::Page(0, DestView::Xyz { left: None, top: Some(602.0), zoom: None }),
            dest_name: Some("equation.2".into()),
        };
        let stray = Link { index: 4, rect: [300.0, 50.0, 320.0, 60.0], dest_name: None, ..link.clone() };
        let cite = Link { index: 5, dest_name: Some("cite.knuth".into()), ..link.clone() };
        let g = build_graph(&layouts, &[], &DocumentStructure::default(), &r, &Bibliography::default(), &[link, stray, cite], &[page()]);
        assert_eq!(g.blocks.len(), 1);
        // The link, the stray link, and the citation link (to no entry: the paper has no list).
        assert_eq!(g.mentions.len(), 3, "{:?}", g.mentions);
        let c = g.mentions.iter().find(|m| m.link == Some((0, 5))).expect("the citation link");
        assert_eq!((c.kind, c.target, c.unresolved), ("citation", None, Some("no_block")));
        let m = g.mentions.iter().find(|m| m.link == Some((0, 3))).expect("the mention under the link");
        assert_eq!((m.target, m.source, m.confidence, m.unresolved), (Some(0), Source::Link, 1.0, None));
        assert!(g.diagnostics.iter().any(|d| d.code == "link_disagrees"));
        let s = g.mentions.iter().find(|m| m.link == Some((0, 4))).expect("a link with no mention");
        assert_eq!((s.kind, s.target), ("link", Some(0)));
        // Hover finds the mention by its box, never one below the confidence asked for.
        assert_eq!(g.mention_at(0, 120.0, 405.0, PREVIEW_CONFIDENCE).map(|m| m.id), Some(m.id));
        assert!(g.mention_at(0, 120.0, 405.0, 1.1).is_none());
        assert!(g.mention_at(0, f32::NAN, 405.0, 0.0).is_none());
        assert!(g.mention_at(7, 120.0, 405.0, 0.0).is_none());
    }

    #[test]
    fn hostile_links_and_pages_give_a_graph() {
        let r = References::default();
        let bad = Link {
            page: 0,
            index: 0,
            rect: [f32::NAN, f32::INFINITY, 1e30, -1e30],
            quads: Vec::new(),
            target: LinkTarget::Page(99, DestView::Xyz { left: None, top: Some(f32::NAN), zoom: None }),
            dest_name: None,
        };
        let off = Link { page: 5, ..bad.clone() };
        let g = build_graph(&[PageLayout::default()], &[], &DocumentStructure::default(), &r, &Bibliography::default(), &[bad, off], &[page()]);
        assert!(g.mentions.is_empty() && g.blocks.is_empty());
    }

    fn entry(id: usize, label: &str, rect: [f32; 4]) -> bib::Entry {
        bib::Entry {
            id,
            label: Some(label.into()),
            label_style: bib::LabelStyle::Bracket,
            lines: Vec::new(),
            pieces: vec![refs::Piece { page: 0, glyphs: vec![100 + id as u32], rules: Vec::new(), rect }],
            text: String::new(),
            fields: Default::default(),
            parsed: false,
            confidence: 0.9,
        }
    }

    fn citation(text: &str, target: Option<usize>, rect: [f32; 4]) -> bib::Citation {
        bib::Citation {
            kind: bib::CiteKind::Numeric,
            text: text.into(),
            locator: None,
            pieces: vec![refs::Piece { page: 0, glyphs: vec![1], rules: Vec::new(), rect }],
            group: 1,
            target,
            unresolved: None,
            in_range: false,
            confidence: 0.9,
        }
    }

    #[test]
    fn citations_point_at_entries_and_a_citation_link_wins() {
        let b = Bibliography {
            entries: vec![entry(0, "1", [72.0, 600.0, 540.0, 620.0]), entry(1, "2", [72.0, 630.0, 540.0, 650.0])],
            citations: vec![
                citation("1", Some(0), [100.0, 400.0, 110.0, 410.0]),
                citation("2", Some(1), [200.0, 400.0, 210.0, 410.0]),
                bib::Citation { pieces: Vec::new(), in_range: true, ..citation("3", Some(1), [0.0; 4]) },
            ],
            ..Default::default()
        };
        let layouts = vec![PageLayout::default()];
        // The link on the second citation goes to the first entry (y 600 from the top: 192 up).
        let link = Link {
            page: 0,
            index: 7,
            rect: [200.0, 382.0, 210.0, 392.0],
            quads: Vec::new(),
            target: LinkTarget::Page(0, DestView::Xyz { left: None, top: Some(192.0), zoom: None }),
            dest_name: Some("cite.smith".into()),
        };
        let g = build_graph(&layouts, &[], &DocumentStructure::default(), &References::default(), &b, &[link], &[page()]);
        assert_eq!(g.blocks.iter().filter(|x| x.kind == BlockKind::BibEntry).count(), 2);
        assert_eq!(g.entry_blocks, [0, 1]);
        assert_eq!(g.blocks[0].title(), "[1]");
        let first = g.mentions.iter().find(|m| m.text == "1").expect("[1]");
        assert_eq!((first.kind, first.target, first.source), ("citation", Some(0), Source::Inferred));
        let second = g.mentions.iter().find(|m| m.link == Some((0, 7))).expect("the linked one");
        assert_eq!((second.target, second.source, second.confidence), (Some(0), Source::Link, 1.0));
        assert!(g.diagnostics.iter().any(|d| d.code == "citation_link_disagrees" && d.message.contains("[2]")));
        // A key inside a range is in the graph, on no page.
        let inner = g.mentions.iter().find(|m| m.text == "3").expect("inside a range");
        assert!(inner.extent.is_empty() && g.mentions_on(0).all(|m| m.id != inner.id));
        assert_eq!(g.mention_at(0, 105.0, 405.0, PREVIEW_CONFIDENCE).map(|m| m.target), Some(Some(0)));
        assert_eq!(g.mention_of_link(0, 7).map(|m| m.id), Some(second.id));
        assert_eq!(g.entry_of_block(1), Some(1));
    }
}
