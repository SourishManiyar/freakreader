//! The layout oracle's harness, shared by its tests (`tests/it/layout_oracle.rs`) and its report
//! (`examples/layout_oracle.rs`): the committed truth, the pages it describes, the analysers scored
//! (the real one and deliberately broken ones), and scoring.
// Each of the two users needs part of this.
#![allow(dead_code)]

use pdfcraft_oracle::tex::layout::{FoundBlock, FoundSection, Kind, LayoutPage, LayoutTally, LayoutTruth, PaperInfo};
use pdfcraft_oracle::tex::{self, Mark};
use pdfcraft_render::text::math::MathPage;
use pdfcraft_render::text::page_layout::{self, Block, BlockKind, DocumentStructure, PageContext, PageLayout};
use pdfcraft_render::{PageRenderer, RenderConfig};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

/// How far a glyph's baseline origin or a rule's edge may sit from where TeX put it, in bp.
pub const PLACE_TOLERANCE_BP: f64 = 0.02;

pub fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/layout/oracle")
}

pub fn load() -> Result<LayoutTruth, String> {
    let path = data_dir().join("truth.tsv");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    LayoutTruth::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// One paper as read from its PDF: per page the glyphs and rules, and the displayed size.
pub struct Paper {
    pub info: PaperInfo,
    pub truth: Vec<LayoutPage>,
    pub pages: Vec<MathPage>,
    pub sizes: Vec<[f32; 2]>,
    /// Per page: the truth item each glyph is, and each rule.
    pub glyph_item: Vec<Vec<Option<usize>>>,
    pub rule_item: Vec<Vec<Option<usize>>>,
}

fn font_name(page: &MathPage, font: u32) -> String {
    page.fonts.get(font as usize).and_then(|f| f.name.clone()).unwrap_or_default().to_uppercase()
}

/// Matches every glyph and rule of a page to the truth item it is (font, size, code and baseline
/// origin; a rule by its box). Unmatched ones stay `None`.
pub fn match_items(page: &MathPage, truth: &LayoutPage) -> (Vec<Option<usize>>, Vec<Option<usize>>) {
    let mut by_code: HashMap<(String, u32), Vec<usize>> = HashMap::new();
    let mut rules: Vec<(usize, [f64; 4])> = Vec::new();
    for (i, item) in truth.items.iter().enumerate() {
        match &item.mark {
            Mark::Glyph { font, code, .. } => by_code.entry((font.to_uppercase(), *code)).or_default().push(i),
            Mark::Rule { width, height } => {
                let (x0, y1) = (tex::view_bp(item.h), tex::view_bp(item.v));
                rules.push((i, [x0, y1 - tex::size_bp(*height), x0 + tex::size_bp(*width), y1]));
            }
        }
    }
    let mut taken = vec![false; truth.items.len()];
    let glyphs = page
        .glyphs
        .iter()
        .map(|g| {
            let cands = by_code.get(&(font_name(page, g.font), g.code))?;
            let k = cands.iter().copied().find(|&k| {
                let Some(item) = truth.items.get(k) else { return false };
                let Mark::Glyph { size, .. } = item.mark else { return false };
                !taken[k]
                    && (f64::from(g.size) - tex::size_bp(size)).abs() < 1e-3
                    && (f64::from(g.baseline[0][0]) - tex::view_bp(item.h)).abs() <= PLACE_TOLERANCE_BP
                    && (f64::from(g.baseline[0][1]) - tex::view_bp(item.v)).abs() <= PLACE_TOLERANCE_BP
            })?;
            taken[k] = true;
            Some(k)
        })
        .collect();
    let rules = page
        .rules
        .iter()
        .map(|r| {
            let k =
                rules.iter().find(|(k, rect)| !taken[*k] && r.rect.iter().zip(rect).all(|(a, b)| (f64::from(*a) - b).abs() <= PLACE_TOLERANCE_BP))?.0;
            taken[k] = true;
            Some(k)
        })
        .collect();
    (glyphs, rules)
}

/// Every paper of the truth, read from its PDF.
pub fn papers(truth: &LayoutTruth) -> Result<Vec<Paper>, String> {
    let mut out = Vec::new();
    for info in &truth.papers {
        let path = data_dir().join(format!("{}.pdf", info.doc));
        let bytes = Arc::new(std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?);
        let doc = pdfcraft_render::inspect(bytes.clone(), None).map_err(|e| format!("{}: {e}", info.doc))?;
        let mut renderer = PageRenderer::new(bytes, RenderConfig::default());
        let tp: Vec<LayoutPage> = truth.pages.iter().filter(|p| p.doc == info.doc).cloned().collect();
        let mut paper =
            Paper { info: info.clone(), truth: Vec::new(), pages: Vec::new(), sizes: Vec::new(), glyph_item: Vec::new(), rule_item: Vec::new() };
        for t in tp {
            let page = renderer.glyph_details(t.page)?;
            let (g, r) = match_items(&page, &t);
            let size = doc.pages.get(t.page).map_or([612.0, 792.0], |p| [p.width, p.height]);
            paper.glyph_item.push(g);
            paper.rule_item.push(r);
            paper.pages.push(page);
            paper.sizes.push(size);
            paper.truth.push(t);
        }
        out.push(paper);
    }
    Ok(out)
}

pub fn kind_of(k: BlockKind, level: Option<u8>) -> Kind {
    match k {
        BlockKind::Title => Kind::Title,
        BlockKind::Abstract => Kind::Abstract,
        BlockKind::Heading => Kind::Heading(level.unwrap_or(0)),
        BlockKind::Paragraph => Kind::Paragraph,
        BlockKind::DisplayEquation => Kind::Display,
        BlockKind::Float => Kind::Float,
        BlockKind::Caption => Kind::Caption,
        BlockKind::Footnote => Kind::Footnote,
        BlockKind::RunningHeader => Kind::RunningHeader,
        BlockKind::RunningFooter => Kind::RunningFooter,
        BlockKind::PageNumber => Kind::PageNumber,
        BlockKind::ListItem => Kind::ListItem,
        BlockKind::Other => Kind::Other,
    }
}

/// An analysis of a whole paper.
pub trait Analyser {
    fn name(&self) -> &'static str;
    fn analyse(&self, paper: &Paper) -> (Vec<PageLayout>, DocumentStructure);
}

/// The analysis as shipped: [`page_layout::analyse_page`] per page, then [`page_layout::analyse_document`].
pub struct Real;

impl Analyser for Real {
    fn name(&self) -> &'static str {
        "layout"
    }
    fn analyse(&self, paper: &Paper) -> (Vec<PageLayout>, DocumentStructure) {
        let mut pages: Vec<PageLayout> = paper
            .pages
            .iter()
            .zip(&paper.sizes)
            .enumerate()
            .map(|(i, (p, s))| page_layout::analyse_page(p, PageContext { index: i, width: s[0], height: s[1] }))
            .collect();
        let doc = page_layout::analyse_document(&mut pages);
        (pages, doc)
    }
}

/// Broken on purpose: never finds columns. Lines run across the whole page and blocks are cut only
/// by vertical gaps, read top to bottom.
pub struct NoColumns;

impl Analyser for NoColumns {
    fn name(&self) -> &'static str {
        "broken-no-columns"
    }
    fn analyse(&self, paper: &Paper) -> (Vec<PageLayout>, DocumentStructure) {
        let pages = paper
            .pages
            .iter()
            .enumerate()
            .map(|(pi, p)| {
                let mut idx: Vec<u32> = (0..p.glyphs.len() as u32).collect();
                idx.sort_by(|&a, &b| p.glyphs[a as usize].baseline[0][1].total_cmp(&p.glyphs[b as usize].baseline[0][1]));
                let mut blocks: Vec<Block> = Vec::new();
                let mut last: Option<f32> = None;
                for g in idx {
                    let y = p.glyphs[g as usize].baseline[0][1];
                    let new = last.is_none_or(|l| y - l > 18.0);
                    if new {
                        blocks.push(bare_block(BlockKind::Paragraph));
                    }
                    if let Some(b) = blocks.last_mut() {
                        b.lines[0].glyphs.push(g);
                    }
                    last = Some(y);
                }
                let n = blocks.len();
                PageLayout { page: pi, blocks, reading_order: (0..n).collect(), ..Default::default() }
            })
            .collect();
        (pages, DocumentStructure::default())
    }
}

fn bare_block(kind: BlockKind) -> Block {
    Block {
        kind,
        rect: [0.0; 4],
        lines: vec![page_layout::Line { glyphs: Vec::new(), baseline: 0.0, rect: [0.0; 4] }],
        rules: Vec::new(),
        text: String::new(),
        order: 0,
        column: None,
        confidence: 0.0,
        style: Default::default(),
        heading: None,
        equation_tag: None,
        partner: None,
    }
}

/// Broken on purpose: merges each paragraph into the paragraph before it in reading order.
pub struct MergeParagraphs;

impl Analyser for MergeParagraphs {
    fn name(&self) -> &'static str {
        "broken-merge-paragraphs"
    }
    fn analyse(&self, paper: &Paper) -> (Vec<PageLayout>, DocumentStructure) {
        let (mut pages, doc) = Real.analyse(paper);
        for p in &mut pages {
            let mut kept: Vec<Block> = Vec::new();
            for &i in &p.reading_order {
                let b = p.blocks[i].clone();
                match kept.last_mut() {
                    Some(prev) if prev.kind == BlockKind::Paragraph && b.kind == BlockKind::Paragraph => prev.lines.extend(b.lines),
                    _ => kept.push(b),
                }
            }
            p.reading_order = (0..kept.len()).collect();
            p.blocks = kept;
        }
        (pages, DocumentStructure { sections: Vec::new(), ..doc })
    }
}

/// Broken on purpose: takes running heads, footers and page numbers for text.
pub struct NoRunningHeads;

impl Analyser for NoRunningHeads {
    fn name(&self) -> &'static str {
        "broken-no-running-heads"
    }
    fn analyse(&self, paper: &Paper) -> (Vec<PageLayout>, DocumentStructure) {
        let (mut pages, doc) = Real.analyse(paper);
        for b in pages.iter_mut().flat_map(|p| p.blocks.iter_mut()) {
            if b.kind.is_running() {
                b.kind = BlockKind::Paragraph;
            }
        }
        (pages, doc)
    }
}

pub fn analyser(name: &str) -> Option<Box<dyn Analyser>> {
    let all: Vec<Box<dyn Analyser>> = vec![Box::new(Real), Box::new(NoColumns), Box::new(MergeParagraphs), Box::new(NoRunningHeads)];
    all.into_iter().find(|a| a.name() == name)
}

pub const ANALYSERS: [&str; 4] = ["layout", "broken-no-columns", "broken-merge-paragraphs", "broken-no-running-heads"];

/// The found blocks of one page as the scorer sees them, in reading order.
pub fn found_blocks(paper: &Paper, page: usize, layout: &PageLayout) -> Vec<FoundBlock> {
    let (Some(gi), Some(ri)) = (paper.glyph_item.get(page), paper.rule_item.get(page)) else { return Vec::new() };
    layout
        .reading_order
        .iter()
        .filter_map(|&i| layout.blocks.get(i))
        .map(|b| {
            let mut items = Vec::new();
            let mut unmatched = 0;
            for g in b.glyphs() {
                match gi.get(g as usize).copied().flatten() {
                    Some(k) => items.push(k),
                    None => unmatched += 1,
                }
            }
            for &r in &b.rules {
                match ri.get(r as usize).copied().flatten() {
                    Some(k) => items.push(k),
                    None => unmatched += 1,
                }
            }
            FoundBlock { kind: kind_of(b.kind, b.heading.as_ref().map(|h| h.level)), items, unmatched }
        })
        .collect()
}

/// Scores of one paper.
pub fn score_paper(paper: &Paper, analyser: &dyn Analyser) -> LayoutTally {
    let (pages, doc) = analyser.analyse(paper);
    let mut tally = LayoutTally::default();
    let mut found: Vec<Vec<FoundBlock>> = Vec::new();
    for (k, (truth, layout)) in paper.truth.iter().zip(&pages).enumerate() {
        let f = found_blocks(paper, k, layout);
        tally.add_page(truth, &f);
        found.push(f);
    }
    // Found sections: their block's place in the page's reading order.
    let sections: Vec<FoundSection> = doc
        .sections
        .iter()
        .filter_map(|s| {
            let layout = pages.get(s.page)?;
            let block = layout.reading_order.iter().position(|&i| i == s.block)?;
            Some(FoundSection { page: s.page, block, level: s.level, parent: s.parent })
        })
        .collect();
    let truth_pages: Vec<&LayoutPage> = paper.truth.iter().collect();
    tally.add_sections(&truth_pages, &found, &sections);
    tally
}

/// Tallies grouped by a key of the paper (set, class, columns…).
pub fn grouped(papers: &[Paper], analyser: &dyn Analyser, key: impl Fn(&PaperInfo) -> String) -> BTreeMap<String, LayoutTally> {
    let mut out: BTreeMap<String, LayoutTally> = BTreeMap::new();
    for p in papers {
        let t = score_paper(p, analyser);
        out.entry(key(&p.info)).or_default().add(&t);
    }
    out
}

pub fn set_name(p: &PaperInfo) -> String {
    if p.holdout { "holdout".into() } else { "dev".into() }
}
