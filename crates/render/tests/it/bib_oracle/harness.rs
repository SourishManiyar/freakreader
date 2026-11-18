//! The bibliography oracle's harness, shared by its tests (`tests/it/bib_oracle.rs`) and its report
//! (`examples/bib_oracle.rs`): the committed truth, the papers it describes, the analysers scored
//! (the real one and deliberately broken ones), and scoring.
// Each of the two users needs part of this.
#![allow(dead_code)]

// The layout oracle's harness matches glyphs to the truth's items: in the test crate it is a
// module already, in the report it is brought in here.
#[cfg(not(test))]
#[path = "../layout_oracle/harness.rs"]
mod layout;
#[cfg(test)]
use crate::layout_oracle::harness as layout;

use pdfcraft_oracle::tex::bib::{BibPaper, BibTally, BibTruth, FoundBib, FoundCite, FoundEntry, ItemRef};
use pdfcraft_oracle::tex::layout::{LayoutPage, PaperInfo};
use pdfcraft_render::text::math::MathPage;
use pdfcraft_render::text::page_layout::bib::{self, Bibliography, CiteKind, LabelStyle};
use pdfcraft_render::text::page_layout::refs::{self, PageWords, Piece};
use pdfcraft_render::text::page_layout::{self, BlockKind, DocumentStructure, PageContext, PageLayout};
use pdfcraft_render::{DocInfo, PageRenderer, RenderConfig};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

pub fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/bib/oracle")
}

pub fn load() -> Result<BibTruth, String> {
    let path = data_dir().join("truth.tsv");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    BibTruth::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// One paper as read from its PDF.
pub struct Paper {
    pub info: PaperInfo,
    pub bib: BibPaper,
    pub truth: Vec<LayoutPage>,
    pub pages: Vec<MathPage>,
    pub sizes: Vec<[f32; 2]>,
    pub doc: DocInfo,
    /// Per page: the truth item each glyph is, and each rule.
    pub glyph_item: Vec<Vec<Option<usize>>>,
    pub rule_item: Vec<Vec<Option<usize>>>,
}

fn read(doc: &str) -> Result<(DocInfo, PageRenderer), String> {
    let path = data_dir().join(format!("{doc}.pdf"));
    let bytes = Arc::new(std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?);
    let info = pdfcraft_render::inspect(bytes.clone(), None).map_err(|e| format!("{doc}: {e}"))?;
    Ok((info, PageRenderer::new(bytes, RenderConfig::default())))
}

/// The papers with DVI truth.
pub fn papers(truth: &BibTruth) -> Result<Vec<Paper>, String> {
    let mut out = Vec::new();
    for info in &truth.layout.papers {
        let (doc, mut renderer) = read(&info.doc)?;
        let bib = truth.paper(&info.doc).cloned().ok_or_else(|| format!("{}: no Q row", info.doc))?;
        let mut paper = Paper {
            info: info.clone(),
            bib,
            truth: Vec::new(),
            pages: Vec::new(),
            sizes: Vec::new(),
            doc: DocInfo::default(),
            glyph_item: Vec::new(),
            rule_item: Vec::new(),
        };
        for t in truth.layout.pages.iter().filter(|p| p.doc == info.doc) {
            let page = renderer.glyph_details(t.page)?;
            let (g, r) = layout::match_items(&page, t);
            paper.sizes.push(doc.pages.get(t.page).map_or([612.0, 792.0], |p| [p.width, p.height]));
            paper.glyph_item.push(g);
            paper.rule_item.push(r);
            paper.pages.push(page);
            paper.truth.push(t.clone());
        }
        paper.doc = doc;
        out.push(paper);
    }
    Ok(out)
}

/// The linked papers (with hyperref): no DVI truth, their links say where citations go.
pub fn linked(truth: &BibTruth) -> Result<Vec<Paper>, String> {
    let mut out = Vec::new();
    for bib in truth.papers.iter().filter(|p| p.hyperref) {
        let (doc, mut renderer) = read(&bib.doc)?;
        let mut pages = Vec::new();
        for i in 0..doc.pages.len() {
            pages.push(renderer.glyph_details(i)?);
        }
        let info = PaperInfo {
            doc: bib.doc.clone(),
            holdout: bib.doc.starts_with("ho_"),
            class: String::new(),
            columns: String::new(),
            size: String::new(),
        };
        let sizes = doc.pages.iter().map(|p| [p.width, p.height]).collect();
        out.push(Paper { info, bib: bib.clone(), truth: Vec::new(), pages, sizes, doc, glyph_item: Vec::new(), rule_item: Vec::new() });
    }
    Ok(out)
}

/// What an analysis found in a paper.
pub struct Analysis {
    pub layouts: Vec<PageLayout>,
    pub words: Vec<PageWords>,
    pub structure: DocumentStructure,
    pub bib: Bibliography,
}

pub trait Analyser {
    fn name(&self) -> &'static str;
    fn analyse(&self, paper: &Paper) -> Analysis;
}

/// The analysis as shipped (as the engine runs it: the document's view of each line's kind).
pub struct Real;

impl Analyser for Real {
    fn name(&self) -> &'static str {
        "bib"
    }
    fn analyse(&self, paper: &Paper) -> Analysis {
        let mut layouts: Vec<PageLayout> = paper
            .pages
            .iter()
            .zip(&paper.sizes)
            .enumerate()
            .map(|(i, (p, s))| page_layout::analyse_page(p, PageContext { index: i, width: s[0], height: s[1] }))
            .collect();
        let mut words: Vec<PageWords> = paper.pages.iter().zip(&layouts).map(|(p, l)| refs::page_words(p, l)).collect();
        let structure = page_layout::analyse_document(&mut layouts);
        for w in &mut words {
            for line in &mut w.lines {
                if let Some(b) = layouts.get(line.page).and_then(|l| l.blocks.get(line.block)) {
                    line.kind = b.kind;
                }
            }
        }
        let bib = bib::analyse_bibliography(&layouts, &words, &structure);
        Analysis { layouts, words, structure, bib }
    }
}

/// Broken on purpose: every line of the list is an entry of its own.
pub struct SplitEveryLine;

impl Analyser for SplitEveryLine {
    fn name(&self) -> &'static str {
        "broken-split-every-line"
    }
    fn analyse(&self, paper: &Paper) -> Analysis {
        let mut a = Real.analyse(paper);
        let mut out = Vec::new();
        for e in &a.bib.entries {
            for (k, &(p, l)) in e.lines.iter().enumerate() {
                let Some(line) = a.words.get(p).and_then(|w| w.lines.get(l)) else { continue };
                let glyphs: Vec<u32> = line.words.iter().flat_map(|w| w.glyphs.iter().copied()).collect();
                let mut n = e.clone();
                n.id = out.len();
                n.lines = vec![(p, l)];
                n.pieces = vec![Piece { page: p, glyphs, rules: Vec::new(), rect: line.rect }];
                if k > 0 {
                    n.label = None;
                    n.fields = Default::default();
                }
                out.push(n);
            }
        }
        // Citations keep pointing at the first line of their entry.
        let first: Vec<usize> = a
            .bib
            .entries
            .iter()
            .scan(0usize, |at, e| {
                let here = *at;
                *at += e.lines.len().max(1);
                Some(here)
            })
            .collect();
        for c in &mut a.bib.citations {
            c.target = c.target.and_then(|t| first.get(t).copied());
        }
        a.bib.entries = out;
        a
    }
}

/// Broken on purpose: every `[n]` in the text is a citation of the entry labelled `n`, in math, in
/// a list's label, after `Section`, wherever it is.
pub struct EveryBracket;

impl Analyser for EveryBracket {
    fn name(&self) -> &'static str {
        "broken-every-bracket"
    }
    fn analyse(&self, paper: &Paper) -> Analysis {
        let mut a = Real.analyse(paper);
        let labels: HashMap<String, usize> = a.bib.entries.iter().filter_map(|e| e.label.clone().map(|l| (l, e.id))).collect();
        let region: Vec<(usize, u32)> =
            a.bib.entries.iter().flat_map(|e| e.pieces.iter().flat_map(|p| p.glyphs.iter().map(move |g| (p.page, *g)))).collect();
        let taken: std::collections::HashSet<(usize, u32)> =
            a.bib.citations.iter().flat_map(|c| c.pieces.iter().flat_map(|p| p.glyphs.iter().map(move |g| (p.page, *g)))).chain(region).collect();
        for pw in &a.words {
            for line in &pw.lines {
                for w in &line.words {
                    let t = w.text.trim_end_matches([',', '.', ';', ':', ')']);
                    let Some(inner) = t.strip_prefix('[').and_then(|r| r.strip_suffix(']')) else { continue };
                    let Some(&target) = labels.get(inner) else { continue };
                    if w.glyphs.iter().any(|g| taken.contains(&(line.page, *g))) {
                        continue;
                    }
                    let glyphs: Vec<u32> = w.glyphs.iter().skip(1).take(inner.chars().count()).copied().collect();
                    a.bib.citations.push(bib::Citation {
                        kind: CiteKind::Numeric,
                        text: inner.to_owned(),
                        locator: None,
                        pieces: vec![Piece { page: line.page, glyphs, rules: Vec::new(), rect: w.rect }],
                        group: usize::MAX,
                        target: Some(target),
                        unresolved: None,
                        in_range: false,
                        confidence: 0.5,
                    });
                }
            }
        }
        a
    }
}

/// Broken on purpose: an author-year citation goes to the first entry of its first author,
/// whatever the year.
pub struct IgnoreYear;

impl Analyser for IgnoreYear {
    fn name(&self) -> &'static str {
        "broken-ignore-year"
    }
    fn analyse(&self, paper: &Paper) -> Analysis {
        let mut a = Real.analyse(paper);
        let family = |s: &str| s.split_whitespace().next().unwrap_or("").to_lowercase();
        for c in a.bib.citations.iter_mut().filter(|c| c.kind == CiteKind::AuthorYear) {
            let name = family(&c.text);
            c.target = a.bib.entries.iter().find(|e| e.fields.authors.first().is_some_and(|x| x.family.to_lowercase() == name)).map(|e| e.id);
        }
        a
    }
}

pub const ANALYSERS: [&str; 4] = ["bib", "broken-split-every-line", "broken-every-bracket", "broken-ignore-year"];

pub fn analyser(name: &str) -> Option<Box<dyn Analyser>> {
    let all: Vec<Box<dyn Analyser>> = vec![Box::new(Real), Box::new(SplitEveryLine), Box::new(EveryBracket), Box::new(IgnoreYear)];
    all.into_iter().find(|a| a.name() == name)
}

fn items(paper: &Paper, page: usize, glyphs: &[u32], into: &mut Vec<ItemRef>) -> usize {
    let Some(gi) = paper.glyph_item.get(page) else { return glyphs.len() };
    let mut unmatched = 0;
    for &g in glyphs {
        match gi.get(g as usize).copied().flatten() {
            Some(k) => into.push((page, k)),
            None => unmatched += 1,
        }
    }
    unmatched
}

/// What the analysis found, as the scorer sees it.
pub fn found(paper: &Paper, a: &Analysis) -> FoundBib {
    let region = a.bib.region.as_ref().map(|r| {
        let mut it = Vec::new();
        if let Some(h) = &r.heading {
            items(paper, h.page, &h.glyphs, &mut it);
        }
        for e in &a.bib.entries {
            for p in &e.pieces {
                items(paper, p.page, &p.glyphs, &mut it);
            }
        }
        it.sort_unstable();
        it.dedup();
        it
    });
    let entries = a
        .bib
        .entries
        .iter()
        .map(|e| {
            let mut it = Vec::new();
            let mut unmatched = 0;
            for p in &e.pieces {
                unmatched += items(paper, p.page, &p.glyphs, &mut it);
            }
            let mut fields: BTreeMap<String, String> = e.fields.named().map(|(n, f)| (n.to_owned(), f.value.clone())).collect();
            if !e.fields.authors.is_empty() {
                fields.insert("authors".into(), e.fields.authors.iter().map(|x| x.family.as_str()).collect::<Vec<_>>().join("|"));
            }
            FoundEntry { items: it, unmatched, label: e.label.clone().filter(|_| e.label_style != LabelStyle::None), fields }
        })
        .collect();
    let cites = a
        .bib
        .citations
        .iter()
        .map(|c| {
            let mut it = Vec::new();
            let mut unmatched = 0;
            for p in &c.pieces {
                unmatched += items(paper, p.page, &p.glyphs, &mut it);
            }
            FoundCite { items: it, unmatched, target: c.target }
        })
        .collect();
    FoundBib { region, entries, cites }
}

pub fn score_paper(paper: &Paper, truth: &BibTruth, analyser: &dyn Analyser) -> BibTally {
    let a = analyser.analyse(paper);
    let f = found(paper, &a);
    let mut t = BibTally::default();
    t.add_document(truth, &paper.info.doc, &f);
    t
}

/// Tallies grouped by a key of the paper.
pub fn grouped(papers: &[Paper], truth: &BibTruth, analyser: &dyn Analyser, key: impl Fn(&Paper) -> String) -> BTreeMap<String, BibTally> {
    let mut out: BTreeMap<String, BibTally> = BTreeMap::new();
    for p in papers {
        let t = score_paper(p, truth, analyser);
        out.entry(key(p)).or_default().add(&t);
    }
    out
}

pub fn set_name(p: &Paper) -> String {
    if p.info.holdout { "holdout".into() } else { "dev".into() }
}

/// The glyphs of `items` on `page` of a paper, as text (for failure reports).
pub fn item_text(paper: &Paper, items: &[(usize, usize)]) -> String {
    let mut out = String::new();
    let mut items = items.to_vec();
    items.sort_unstable();
    for &(page, item) in &items {
        let Some(gi) = paper.glyph_item.get(page) else { continue };
        let Some(g) = gi.iter().position(|x| *x == Some(item)) else { continue };
        if let Some(t) = paper.pages.get(page).and_then(|p| p.glyphs.get(g)).and_then(|g| g.text.as_deref()) {
            out.push_str(t);
        }
    }
    out
}

/// The kind of a line of the bibliography's text, for failure reports.
pub fn line_kind(a: &Analysis, page: usize, line: usize) -> Option<BlockKind> {
    a.words.get(page).and_then(|w| w.lines.get(line)).map(|l| l.kind)
}

/// How a paper's citation links (`cite.KEY`) and the citations read under them agree.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Agreement {
    /// Citation links; those with a citation read under them; of those, the ones whose entry holds
    /// the link's destination, and the ones whose entry does not (or that have none).
    pub links: usize,
    pub covered: usize,
    pub agree: usize,
    pub disagree: usize,
    /// What disagreed, for a report.
    pub details: Vec<String>,
}

/// Where each citation link goes and where the citation read under it goes agree: the target
/// entry has a piece on the destination's page whose box (with a line of slack above) holds the
/// destination's height.
pub fn agreement(paper: &Paper, a: &Analysis) -> Agreement {
    use pdfcraft_render::{DestView, LinkTarget};
    let mut out = Agreement::default();
    for link in &paper.doc.links {
        let LinkTarget::Page(target, view) = link.target else { continue };
        let Some(page) = paper.doc.pages.get(link.page) else { continue };
        if !link.dest_name.as_deref().is_some_and(|n| n.starts_with("cite.")) {
            continue;
        }
        out.links += 1;
        let a0 = page.user_to_view(link.rect[0], link.rect[1]);
        let a1 = page.user_to_view(link.rect[2], link.rect[3]);
        let r = [a0[0].min(a1[0]), a0[1].min(a1[1]), a0[0].max(a1[0]), a0[1].max(a1[1])];
        let Some(c) =
            a.bib.citations.iter().find(|c| {
                c.pieces.iter().any(|p| p.page == link.page && p.rect[0] < r[2] && r[0] < p.rect[2] && p.rect[1] < r[3] && r[1] < p.rect[3])
            })
        else {
            out.details.push(format!("no citation under {:?} on page {}", link.dest_name, link.page + 1));
            continue;
        };
        out.covered += 1;
        let top = match view {
            DestView::Xyz { left, top, .. } => paper.doc.pages.get(target).and_then(|tp| tp.dest_fraction(left, top, 0)[1].map(|f| f * tp.height)),
            DestView::FitH { top } => paper.doc.pages.get(target).and_then(|tp| tp.dest_fraction(None, top, 0)[1].map(|f| f * tp.height)),
            _ => None,
        };
        let entry = c.target.and_then(|t| a.bib.entries.get(t));
        let holds =
            entry.is_some_and(|e| e.pieces.iter().any(|p| p.page == target && top.is_some_and(|y| y >= p.rect[1] - 14.0 && y <= p.rect[3] + 2.0)));
        if holds {
            out.agree += 1;
        } else {
            out.disagree += 1;
            out.details.push(format!(
                "{:?} -> page {} y {top:?}; read \"{}\" -> {:?}",
                link.dest_name,
                target + 1,
                c.text,
                entry.map(|e| (e.label.clone(), e.text.chars().take(40).collect::<String>()))
            ));
        }
    }
    out
}
