//! The bibliography oracle (`tests/data/bib/oracle/README.md`).
//!
//! These tests read only committed files: the PDFs draw exactly the glyphs and rules the DVI lists,
//! and the truth's own entries and citations score perfectly. The held-out papers are scored by
//! their rates alone (`cargo xtask bib-oracle report`).

pub(crate) mod harness;

use harness::{load, papers};
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
