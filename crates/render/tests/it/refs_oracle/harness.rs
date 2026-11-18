//! The reference oracle's harness, shared by its tests (`tests/it/refs_oracle.rs`) and its report
//! (`examples/refs_oracle.rs`): the committed truth, the papers it describes, the analysers scored
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

use pdfcraft_oracle::tex::layout::{LayoutPage, PaperInfo};
use pdfcraft_oracle::tex::refs::{FoundMention, FoundStatement, FoundTarget, ItemRef, RefsPaper, RefsTally, RefsTruth};
use pdfcraft_render::text::math::MathPage;
use pdfcraft_render::text::page_layout::refs::{self, MentionKind, PageWords, References, Target};
use pdfcraft_render::text::page_layout::{self, BlockKind, DocumentStructure, PageContext, PageLayout};
use pdfcraft_render::{DocInfo, PageRenderer, RenderConfig};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

pub fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/refs/oracle")
}

pub fn load() -> Result<RefsTruth, String> {
    let path = data_dir().join("truth.tsv");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    RefsTruth::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// One paper as read from its PDF.
pub struct Paper {
    pub info: PaperInfo,
    pub refs: RefsPaper,
    pub truth: Vec<LayoutPage>,
    pub pages: Vec<MathPage>,
    pub sizes: Vec<[f32; 2]>,
    pub doc: DocInfo,
    /// Per page: the truth item each glyph is, and each rule.
    pub glyph_item: Vec<Vec<Option<usize>>>,
    pub rule_item: Vec<Vec<Option<usize>>>,
}

pub fn papers(truth: &RefsTruth) -> Result<Vec<Paper>, String> {
    let mut out = Vec::new();
    for info in &truth.layout.papers {
        let path = data_dir().join(format!("{}.pdf", info.doc));
        let bytes = Arc::new(std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?);
        let doc = pdfcraft_render::inspect(bytes.clone(), None).map_err(|e| format!("{}: {e}", info.doc))?;
        let mut renderer = PageRenderer::new(bytes, RenderConfig::default());
        let refs = truth.paper(&info.doc).cloned().ok_or_else(|| format!("{}: no Y row", info.doc))?;
        let mut paper = Paper {
            info: info.clone(),
            refs,
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

/// The linked papers (with hyperref): no DVI truth, their links say where references go.
pub fn linked(truth: &RefsTruth) -> Result<Vec<Paper>, String> {
    let mut out = Vec::new();
    for refs in truth.papers.iter().filter(|p| p.hyperref) {
        let path = data_dir().join(format!("{}.pdf", refs.doc));
        let bytes = Arc::new(std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?);
        let doc = pdfcraft_render::inspect(bytes.clone(), None).map_err(|e| format!("{}: {e}", refs.doc))?;
        let mut renderer = PageRenderer::new(bytes, RenderConfig::default());
        let mut pages = Vec::new();
        for i in 0..doc.pages.len() {
            pages.push(renderer.glyph_details(i)?);
        }
        let info = PaperInfo {
            doc: refs.doc.clone(),
            holdout: refs.doc.starts_with("ho_"),
            class: String::new(),
            columns: String::new(),
            size: String::new(),
        };
        let sizes = doc.pages.iter().map(|p| [p.width, p.height]).collect();
        out.push(Paper { info, refs: refs.clone(), truth: Vec::new(), pages, sizes, doc, glyph_item: Vec::new(), rule_item: Vec::new() });
    }
    Ok(out)
}

/// What an analysis found in a paper.
pub struct Analysis {
    pub layouts: Vec<PageLayout>,
    pub words: Vec<PageWords>,
    pub structure: DocumentStructure,
    pub refs: References,
}

pub trait Analyser {
    fn name(&self) -> &'static str;
    fn analyse(&self, paper: &Paper) -> Analysis;
}

/// The analysis as shipped.
pub struct Real;

impl Analyser for Real {
    fn name(&self) -> &'static str {
        "refs"
    }
    fn analyse(&self, paper: &Paper) -> Analysis {
        let mut layouts: Vec<PageLayout> = paper
            .pages
            .iter()
            .zip(&paper.sizes)
            .enumerate()
            .map(|(i, (p, s))| page_layout::analyse_page(p, PageContext { index: i, width: s[0], height: s[1] }))
            .collect();
        let structure = page_layout::analyse_document(&mut layouts);
        let words: Vec<PageWords> = paper.pages.iter().zip(&layouts).map(|(p, l)| refs::page_words(p, l)).collect();
        let refs = refs::analyse_references(&layouts, &words, &structure);
        Analysis { layouts, words, structure, refs }
    }
}

/// Broken on purpose: a statement mention goes to any statement with its number, whatever the word.
pub struct TrustNumbers;

impl Analyser for TrustNumbers {
    fn name(&self) -> &'static str {
        "broken-trust-numbers"
    }
    fn analyse(&self, paper: &Paper) -> Analysis {
        let mut a = Real.analyse(paper);
        let statements = a.refs.statements.clone();
        for m in a.refs.mentions.iter_mut().filter(|m| m.kind == MentionKind::Statement && m.unresolved != Some(refs::Unresolved::External)) {
            let k = statements.iter().position(|s| !s.proof && s.number.as_deref() == Some(m.number.as_str()));
            m.target = k.map(Target::Statement);
        }
        a
    }
}

/// Broken on purpose: every equation number in parentheses in the text is a link to its display.
pub struct EveryTag;

impl Analyser for EveryTag {
    fn name(&self) -> &'static str {
        "broken-every-tag"
    }
    fn analyse(&self, paper: &Paper) -> Analysis {
        let mut a = Real.analyse(paper);
        let mut tags: HashMap<String, (usize, usize)> = HashMap::new();
        for l in &a.layouts {
            for (bi, b) in l.blocks.iter().enumerate() {
                if let Some(t) = &b.equation_tag {
                    tags.entry(t.trim().to_owned()).or_insert((l.page, bi));
                }
            }
        }
        let taken: HashSet<(usize, u32)> =
            a.refs.mentions.iter().flat_map(|m| m.pieces.iter().flat_map(|p| p.glyphs.iter().map(move |g| (p.page, *g)))).collect();
        for pw in &a.words {
            for line in pw.lines.iter().filter(|l| l.kind != BlockKind::DisplayEquation) {
                for w in &line.words {
                    let t = w.text.trim_end_matches([',', '.', ';', ':']);
                    let Some(&(page, block)) = tags.get(t) else { continue };
                    if w.glyphs.iter().any(|g| taken.contains(&(line.page, *g))) {
                        continue;
                    }
                    a.refs.mentions.push(refs::Mention {
                        kind: MentionKind::Equation,
                        word: String::new(),
                        statement_kind: None,
                        number: t.to_owned(),
                        extra: None,
                        pieces: vec![refs::Piece {
                            page: line.page,
                            glyphs: w.glyphs.iter().take(t.chars().count()).copied().collect(),
                            rules: Vec::new(),
                            rect: w.rect,
                        }],
                        target: Some(Target::Equation { page, block }),
                        unresolved: None,
                        confidence: 0.5,
                    });
                }
            }
        }
        a
    }
}

/// Broken on purpose: a proof runs on to the next statement or heading, end mark or not.
pub struct EndlessProofs;

impl Analyser for EndlessProofs {
    fn name(&self) -> &'static str {
        "broken-endless-proofs"
    }
    fn analyse(&self, paper: &Paper) -> Analysis {
        let mut a = Real.analyse(paper);
        let order: Vec<(usize, usize)> = a.words.iter().enumerate().flat_map(|(p, w)| (0..w.lines.len()).map(move |l| (p, l))).collect();
        let starts: Vec<(usize, usize)> = a.refs.statements.iter().map(|s| s.first_line).collect();
        for s in a.refs.statements.iter_mut().filter(|s| s.proof) {
            let Some(from) = order.iter().position(|&x| x == s.last_line) else { continue };
            for &(p, l) in order.iter().skip(from + 1) {
                let Some(line) = a.words.get(p).and_then(|w| w.lines.get(l)) else { break };
                if starts.contains(&(p, l)) || line.kind == BlockKind::Heading {
                    break;
                }
                if !matches!(line.kind, BlockKind::Paragraph | BlockKind::ListItem | BlockKind::DisplayEquation | BlockKind::Other) {
                    continue;
                }
                if s.extent.last().is_none_or(|x| x.page != p) {
                    s.extent.push(refs::Piece { page: p, ..Default::default() });
                }
                if let Some(x) = s.extent.last_mut() {
                    x.glyphs.extend(line.words.iter().flat_map(|w| w.glyphs.iter().copied()));
                }
            }
        }
        a
    }
}

pub const ANALYSERS: [&str; 4] = ["refs", "broken-trust-numbers", "broken-every-tag", "broken-endless-proofs"];

pub fn analyser(name: &str) -> Option<Box<dyn Analyser>> {
    let all: Vec<Box<dyn Analyser>> = vec![Box::new(Real), Box::new(TrustNumbers), Box::new(EveryTag), Box::new(EndlessProofs)];
    all.into_iter().find(|a| a.name() == name)
}

fn items(paper: &Paper, page: usize, glyphs: &[u32], rules: &[u32], into: &mut Vec<ItemRef>) -> usize {
    let (Some(gi), Some(ri)) = (paper.glyph_item.get(page), paper.rule_item.get(page)) else { return glyphs.len() + rules.len() };
    let mut unmatched = 0;
    for &g in glyphs {
        match gi.get(g as usize).copied().flatten() {
            Some(k) => into.push((page, k)),
            None => unmatched += 1,
        }
    }
    for &r in rules {
        match ri.get(r as usize).copied().flatten() {
            Some(k) => into.push((page, k)),
            None => unmatched += 1,
        }
    }
    unmatched
}

fn block_target(paper: &Paper, a: &Analysis, page: usize, block: usize) -> FoundTarget {
    let mut out = Vec::new();
    let unmatched = a.layouts.get(page).and_then(|l| l.blocks.get(block)).map_or(0, |b| {
        let g: Vec<u32> = b.glyphs().collect();
        items(paper, page, &g, &b.rules, &mut out)
    });
    FoundTarget { items: out, unmatched }
}

/// The found statements and mentions of a paper as the scorer sees them.
pub fn found(paper: &Paper, a: &Analysis) -> (Vec<FoundStatement>, Vec<FoundMention>) {
    let statements: Vec<FoundStatement> = a
        .refs
        .statements
        .iter()
        .map(|s| {
            let mut it = Vec::new();
            let mut unmatched = 0;
            for p in &s.extent {
                unmatched += items(paper, p.page, &p.glyphs, &p.rules, &mut it);
            }
            FoundStatement {
                kind: s.kind.clone(),
                number: s.number.clone(),
                name: if s.proof { None } else { s.name.clone() },
                proof: s.proof,
                items: it,
                unmatched,
                proves: s.proves.map(|(k, _)| k),
                end_mark: s.end_mark.is_some(),
            }
        })
        .collect();
    let mentions = a
        .refs
        .mentions
        .iter()
        .map(|m| {
            let mut it = Vec::new();
            let mut unmatched = 0;
            for p in &m.pieces {
                unmatched += items(paper, p.page, &p.glyphs, &p.rules, &mut it);
            }
            let target = m.target.map(|t| match t {
                Target::Statement(k) => statements
                    .get(k)
                    .map_or(FoundTarget { items: Vec::new(), unmatched: 0 }, |s| FoundTarget { items: s.items.clone(), unmatched: s.unmatched }),
                Target::Heading { page, block } | Target::Equation { page, block } | Target::Float { page, block } => {
                    block_target(paper, a, page, block)
                }
            });
            FoundMention { items: it, unmatched, target }
        })
        .collect();
    (statements, mentions)
}

pub fn score_paper(paper: &Paper, truth: &RefsTruth, analyser: &dyn Analyser) -> RefsTally {
    let a = analyser.analyse(paper);
    let (s, m) = found(paper, &a);
    let mut t = RefsTally::default();
    t.add_document(truth, &paper.info.doc, &s, &m);
    t
}

/// Adds `b` into `a`.
pub fn merge(a: &mut RefsTally, b: &RefsTally) {
    a.docs += b.docs;
    for (k, v) in &b.statements {
        let c = a.statements.entry(k.clone()).or_default();
        c.truth += v.truth;
        c.detected += v.detected;
        c.kind_right += v.kind_right;
        c.number_right += v.number_right;
        c.name_right += v.name_right;
        c.extent_right += v.extent_right;
        c.found += v.found;
        c.spurious += v.spurious;
    }
    let (p, q) = (&mut a.proofs, &b.proofs);
    p.detected += q.detected;
    p.proves_right += q.proves_right;
    p.proves_wrong += q.proves_wrong;
    p.proves_none += q.proves_none;
    p.marked += q.marked;
    p.marked_found += q.marked_found;
    p.unmarked += q.unmarked;
    p.unmarked_found += q.unmarked_found;
    for (k, v) in &b.mentions {
        let c = a.mentions.entry(k.clone()).or_default();
        c.truth += v.truth;
        c.matched += v.matched;
        c.right += v.right;
        c.none_right += v.none_right;
        c.wrong += v.wrong;
        c.missed += v.missed;
    }
    a.mentions_found += b.mentions_found;
    a.mentions_found_matched += b.mentions_found_matched;
    a.spurious_links += b.spurious_links;
}

/// Tallies grouped by a key of the paper.
pub fn grouped(papers: &[Paper], truth: &RefsTruth, analyser: &dyn Analyser, key: impl Fn(&Paper) -> String) -> BTreeMap<String, RefsTally> {
    let mut out: BTreeMap<String, RefsTally> = BTreeMap::new();
    for p in papers {
        let t = score_paper(p, truth, analyser);
        merge(out.entry(key(p)).or_default(), &t);
    }
    out
}

pub fn set_name(p: &Paper) -> String {
    if p.info.holdout { "holdout".into() } else { "dev".into() }
}
