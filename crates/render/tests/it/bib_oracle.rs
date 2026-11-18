//! The bibliography oracle (`tests/data/bib/oracle/README.md`).
//!
//! These tests read only committed files: the PDFs draw exactly the glyphs and rules the DVI lists,
//! the truth's own entries and citations score perfectly, deliberately broken analysers score
//! badly, the analysis keeps the development scores it reached, and on the linked papers
//! (hyperref) the citations read from the text go where the real links go. The held-out papers are
//! scored by their rates alone (`cargo xtask bib-oracle report`); only their floor is tested here.

pub(crate) mod harness;

use harness::{Analyser, EveryBracket, IgnoreYear, Real, SplitEveryLine, agreement, grouped, linked, load, papers};
use pdfcraft_oracle::tex::bib::{BibTally, FoundBib, FoundCite, FoundEntry};
use std::collections::BTreeMap;

/// pdfTeX in PDF mode drew the glyphs and rules the DVI lists, nothing else.
#[test]
fn the_pdfs_hold_exactly_what_the_dvi_lists() {
    let truth = load().unwrap();
    let papers = papers(&truth).unwrap();
    assert!(papers.len() >= 12 && papers.iter().filter(|p| p.info.holdout).count() * 3 >= papers.len());
    let mut items = 0;
    for p in &papers {
        for (k, t) in p.truth.iter().enumerate() {
            let page = &p.pages[k];
            assert!(!page.truncated && !page.capped);
            let (g, r) = (&p.glyph_item[k], &p.rule_item[k]);
            let missing = g.iter().filter(|x| x.is_none()).count() + r.iter().filter(|x| x.is_none()).count();
            assert_eq!(missing, 0, "{} page {}: glyphs or rules the DVI does not list", p.info.doc, t.page);
            assert_eq!(g.len() + r.len(), t.items.len(), "{} page {}: the PDF misses items the DVI lists", p.info.doc, t.page);
            items += t.items.len();
        }
    }
    assert!(items > 50_000, "{items}");
}

/// Scoring the truth's own entries and citations gives full marks: the harness can tell right from
/// wrong.
#[test]
fn the_truth_scores_perfectly() {
    let truth = load().unwrap();
    let flat = |items: &Vec<(usize, Vec<usize>)>| items.iter().flat_map(|(p, l)| l.iter().map(move |&i| (*p, i))).collect::<Vec<_>>();
    let mut tally = BibTally::default();
    for doc in truth.layout.papers.iter().map(|p| p.doc.clone()) {
        let te: Vec<_> = truth.entries.iter().filter(|e| e.doc == doc).collect();
        let entries: Vec<FoundEntry> = te
            .iter()
            .map(|e| {
                let mut fields = BTreeMap::new();
                for f in pdfcraft_oracle::tex::bib::FIELDS {
                    if let Some(v) = e.spec.fields.get(f) {
                        fields.insert(f.to_owned(), v);
                    }
                }
                FoundEntry { items: flat(&e.items), unmatched: 0, label: e.spec.label.clone(), fields }
            })
            .collect();
        let mut region: Vec<(usize, usize)> = truth.headings.iter().filter(|(d, _)| *d == doc).flat_map(|(_, i)| flat(i)).collect();
        region.extend(entries.iter().flat_map(|e| e.items.iter().copied()));
        let cites = truth
            .cites
            .iter()
            .filter(|c| c.doc == doc)
            .map(|c| FoundCite { items: flat(&c.items), unmatched: 0, target: te.iter().position(|e| e.spec.key == c.spec.key) })
            .collect();
        tally.add_document(&truth, &doc, &FoundBib { region: Some(region), entries, cites });
    }
    let e = tally.entries;
    assert_eq!((e.detected, e.exact, e.spurious, e.label_right, e.label_wrong), (e.truth, e.truth, 0, e.labelled, 0));
    let c = tally.cites["*"];
    assert_eq!((c.matched, c.right, c.wrong, c.inner_right, tally.spurious_links), (c.truth, c.truth, 0, c.inner, 0));
    assert_eq!(tally.wrong_fields(), 0);
    assert_eq!((tally.region_found, tally.region_common), (tally.with_list, tally.region_truth));
    assert!(e.truth >= 300 && c.truth >= 150 && tally.traps >= 30, "{e:?} {c:?}");
    for f in ["authors", "title", "year", "doi", "arxiv", "mr"] {
        assert!(tally.fields[f].truth >= 10, "{f}: {:?}", tally.fields[f]);
    }
}

fn dev_and_holdout(analyser: &dyn Analyser) -> (BibTally, BibTally) {
    let truth = load().unwrap();
    let papers = papers(&truth).unwrap();
    let mut by_set = grouped(&papers, &truth, analyser, harness::set_name);
    (by_set.remove("dev").unwrap_or_default(), by_set.remove("holdout").unwrap_or_default())
}

fn rate(a: usize, b: usize) -> f64 {
    a as f64 / b.max(1) as f64
}

/// The analysis keeps the scores it reached: on the development papers every list, entry and label
/// found, at most one wrong field (a compound broken at its hyphen), no citation where there is
/// none and no wrong link; the held-out papers above their floor, with no wrong link either.
#[test]
fn the_analysis_keeps_its_scores() {
    let (dev, holdout) = dev_and_holdout(&Real);
    for (name, t, floor, fields) in [("dev", &dev, 0.98, 2), ("holdout", &holdout, 0.9, 12)] {
        let e = t.entries;
        let c = t.cites["*"];
        assert_eq!(t.region_found, t.with_list, "{name}: a list not found");
        assert_eq!(t.region_spurious, 0, "{name}");
        assert!(rate(e.exact, e.truth) >= floor && rate(e.label_right, e.labelled) >= floor, "{name}: {e:?}");
        assert!(t.wrong_fields() <= fields, "{name}: {} wrong fields: {:?}", t.wrong_fields(), t.fields);
        assert!(rate(c.matched, c.truth) >= floor - 0.05, "{name}: {c:?}");
        assert!(rate(t.cites_found - t.cites_spurious, t.cites_found) >= floor, "{name}: {} spurious of {}", t.cites_spurious, t.cites_found);
        assert_eq!(t.trap_hits, 0, "{name}: a look-alike read as a citation");
        assert_eq!(t.wrong_links(), 0, "{name}: a wrong link: {c:?}, {} where nothing is cited", t.spurious_links);
    }
}

/// The harness tells a broken analysis from a working one.
#[test]
fn broken_analysers_score_badly() {
    let (real, _) = dev_and_holdout(&Real);
    let (split, _) = dev_and_holdout(&SplitEveryLine);
    let (every, _) = dev_and_holdout(&EveryBracket);
    let (year, _) = dev_and_holdout(&IgnoreYear);
    assert_eq!(real.wrong_links(), 0);
    // Splitting at every line breaks most entries and sends citations to the wrong line.
    assert!(rate(split.entries.exact, split.entries.truth) < 0.3, "{:?}", split.entries);
    assert!(split.entries.spurious > 300 && split.wrong_links() > 10, "{:?}", split.entries);
    // Every `[n]` a citation: the traps become links.
    assert!(every.trap_hits >= 3 && every.wrong_links() >= 3, "{} {}", every.trap_hits, every.wrong_links());
    // An author-year citation by its first author alone goes to the wrong year.
    assert!(year.wrong_links() >= 10, "{}", year.wrong_links());
}

/// On the linked papers, every citation link has a citation read under it whose entry holds the
/// link's destination; none disagrees.
#[test]
fn inferred_citations_go_where_the_links_go() {
    let truth = load().unwrap();
    let papers = linked(&truth).unwrap();
    assert!(papers.len() >= 3);
    for p in &papers {
        let g = agreement(p, &Real.analyse(p));
        assert!(g.links >= 8, "{}: {g:?}", p.info.doc);
        assert_eq!(g.disagree, 0, "{}: {:?}", p.info.doc, g.details);
        // Held out: by its rate (a citation set in a way the analysis does not read gives no link).
        let least = if p.info.holdout { 0.75 } else { 1.0 };
        assert!(rate(g.covered, g.links) >= least, "{}: {g:?}", p.info.doc);
    }
}
