//! The page-layout oracle (`tests/data/layout/oracle/README.md`).
//!
//! These tests read only committed files: the PDFs draw exactly the glyphs and rules the DVI lists,
//! the truth's own blocks score perfectly, deliberately broken analysers score badly, and the
//! analysis keeps the development scores it reached. The held-out papers are scored by their rates
//! alone (`cargo xtask layout-oracle report`); only their overall floor is tested here.

pub(crate) mod harness;

use harness::{Analyser, MergeParagraphs, NoColumns, NoRunningHeads, Real, grouped, load, papers, score_paper};
use pdfcraft_oracle::tex::layout::{FoundBlock, FoundSection, Kind, LayoutTally};

fn dev_and_holdout(analyser: &dyn Analyser) -> (LayoutTally, LayoutTally) {
    let truth = load().unwrap();
    let papers = papers(&truth).unwrap();
    let mut by_set = grouped(&papers, analyser, harness::set_name);
    (by_set.remove("dev").unwrap_or_default(), by_set.remove("holdout").unwrap_or_default())
}

/// pdfTeX in PDF mode drew the glyphs and rules the DVI lists, nothing else, and the analysis reads
/// them all.
#[test]
fn the_pdfs_hold_exactly_what_the_dvi_lists() {
    let truth = load().unwrap();
    let papers = papers(&truth).unwrap();
    assert!(papers.len() >= 9 && papers.iter().filter(|p| p.info.holdout).count() * 3 >= papers.len());
    let mut items = 0;
    for p in &papers {
        for (k, t) in p.truth.iter().enumerate() {
            let page = &p.pages[k];
            assert!(!page.truncated && !page.capped);
            let (g, r) = (&p.glyph_item[k], &p.rule_item[k]);
            assert!(g.iter().all(Option::is_some), "{} page {}: a glyph the DVI does not list", p.info.doc, t.page);
            assert!(r.iter().all(Option::is_some), "{} page {}: a rule the DVI does not list", p.info.doc, t.page);
            assert_eq!(g.len() + r.len(), t.items.len(), "{} page {}: the PDF misses items the DVI lists", p.info.doc, t.page);
            items += t.items.len();
        }
    }
    assert!(items > 20_000, "{items}");
}

/// Scoring the truth's own blocks gives full marks: the harness can tell right from wrong.
#[test]
fn the_truth_scores_perfectly() {
    let truth = load().unwrap();
    let mut tally = LayoutTally::default();
    for paper in truth.papers.iter().map(|p| p.doc.clone()) {
        let pages: Vec<_> = truth.pages.iter().filter(|p| p.doc == paper).collect();
        let mut found = Vec::new();
        for p in &pages {
            let f: Vec<FoundBlock> =
                p.blocks.iter().filter(|b| b.kind != Kind::Skip).map(|b| FoundBlock { kind: b.kind, items: b.items.clone(), unmatched: 0 }).collect();
            tally.add_page(p, &f);
            found.push(f);
        }
        let mut sections = Vec::new();
        let mut stack: Vec<(u8, usize)> = Vec::new();
        for (pi, f) in found.iter().enumerate() {
            for (bi, b) in f.iter().enumerate() {
                if let Kind::Heading(level) = b.kind {
                    while stack.last().is_some_and(|(l, _)| *l >= level) {
                        stack.pop();
                    }
                    sections.push(FoundSection { page: pi, block: bi, level, parent: stack.last().map(|s| s.1) });
                    stack.push((level, sections.len() - 1));
                }
            }
        }
        tally.add_sections(&pages, &found, &sections);
    }
    assert_eq!(tally.block_rate(), Some(1.0));
    assert_eq!(tally.order_exact, tally.pages);
    assert_eq!((tally.running_precision(), tally.running_recall()), (Some(1.0), Some(1.0)));
    assert_eq!(tally.sections_parent, tally.sections_truth);
    assert!(tally.sections_truth >= 30);
}

/// Each deliberately broken analyser loses on what it breaks.
#[test]
fn broken_analysers_score_badly() {
    let truth = load().unwrap();
    let papers = papers(&truth).unwrap();
    let all = |a: &dyn Analyser| {
        let mut t = LayoutTally::default();
        for p in &papers {
            t.add(&score_paper(p, a));
        }
        t
    };
    let real = all(&Real);
    let no_columns = all(&NoColumns);
    let merged = all(&MergeParagraphs);
    let no_heads = all(&NoRunningHeads);
    let para = |t: &LayoutTally| t.by_kind.get(&Kind::Paragraph).copied().unwrap_or_default();
    assert!(no_columns.block_rate().unwrap() < 0.5 * real.block_rate().unwrap(), "{}", no_columns.render("no columns", true));
    assert!(no_columns.order_exact < real.order_exact);
    assert!(para(&merged).boundary + 25 <= para(&real).boundary, "{}", merged.render("merged", true));
    assert!(para(&merged).merged >= para(&real).merged + 30);
    assert!(no_heads.block_rate().unwrap() + 0.05 < real.block_rate().unwrap());
    assert_eq!(no_heads.running_found, 0);
    assert!(real.running_recall().unwrap() > 0.8);
}

/// The scores the analysis reached on the development papers (measured 2026-11-17); a change that
/// loses ground fails here. Held-out papers: only an overall floor.
#[test]
fn the_analysis_keeps_its_scores() {
    let (dev, holdout) = dev_and_holdout(&Real);
    let report = format!("{}{}", dev.render("dev", true), holdout.render("holdout", true));
    let floor = |name: &str, value: Option<f64>, min: f64| assert!(value.unwrap_or(0.0) >= min, "{name}: {value:?} < {min}\n{report}");
    floor("dev blocks", dev.block_rate(), DEV_BLOCKS);
    floor("dev tau", dev.mean_tau(), DEV_TAU);
    floor("dev running precision", dev.running_precision(), DEV_RUNNING);
    floor("dev running recall", dev.running_recall(), DEV_RUNNING);
    floor("dev sections", Some(dev.sections_parent as f64 / dev.sections_truth.max(1) as f64), DEV_SECTIONS);
    floor("holdout blocks", holdout.block_rate(), HOLDOUT_BLOCKS);
}

/// Measured: 236/236 development blocks right in boundary and kind, tau 1.0, running elements
/// 100% precision and recall, 37/37 sections with level and parent; held out 95/99 blocks.
const DEV_BLOCKS: f64 = 0.98;
const DEV_TAU: f64 = 0.99;
const DEV_RUNNING: f64 = 0.95;
const DEV_SECTIONS: f64 = 0.95;
const HOLDOUT_BLOCKS: f64 = 0.9;
