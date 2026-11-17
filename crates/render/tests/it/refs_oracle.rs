//! The statement and reference oracle (`tests/data/refs/oracle/README.md`).
//!
//! These tests read only committed files: the PDFs draw exactly the glyphs and rules the DVI lists,
//! the truth's own statements and mentions score perfectly, deliberately broken analysers score
//! badly, the analysis keeps the development scores it reached, and on the linked papers (hyperref)
//! the inferred mentions go where the real links go. The held-out papers are scored by their rates
//! alone (`cargo xtask refs-oracle report`); only their overall floor is tested here.

pub(crate) mod harness;

use harness::{Analyser, EndlessProofs, EveryTag, Real, TrustNumbers, grouped, linked, load, papers};
use pdfcraft_oracle::tex::refs::{FoundMention, FoundStatement, FoundTarget, RefsTally};
use pdfcraft_render::text::page_layout::refs::{Target, Unresolved};
use pdfcraft_render::{DestView, LinkTarget};

fn dev_and_holdout(analyser: &dyn Analyser) -> (RefsTally, RefsTally) {
    let truth = load().unwrap();
    let papers = papers(&truth).unwrap();
    let mut by_set = grouped(&papers, &truth, analyser, harness::set_name);
    (by_set.remove("dev").unwrap_or_default(), by_set.remove("holdout").unwrap_or_default())
}

/// pdfTeX in PDF mode drew the glyphs and rules the DVI lists, nothing else.
#[test]
fn the_pdfs_hold_exactly_what_the_dvi_lists() {
    let truth = load().unwrap();
    let papers = papers(&truth).unwrap();
    assert!(papers.len() >= 8 && papers.iter().filter(|p| p.info.holdout).count() * 3 >= papers.len());
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
    assert!(items > 20_000, "{items}");
}

/// Scoring the truth's own statements and mentions gives full marks: the harness can tell right
/// from wrong.
#[test]
fn the_truth_scores_perfectly() {
    let truth = load().unwrap();
    let flat = |items: &Vec<(usize, Vec<usize>)>| items.iter().flat_map(|(p, l)| l.iter().map(move |&i| (*p, i))).collect::<Vec<_>>();
    let mut tally = RefsTally::default();
    for paper in truth.layout.papers.iter().map(|p| p.doc.clone()) {
        let ts: Vec<_> = truth.statements.iter().filter(|s| s.doc == paper).collect();
        let statements: Vec<FoundStatement> = ts
            .iter()
            .map(|s| FoundStatement {
                kind: s.spec.kind.clone(),
                number: s.number.clone(),
                name: s.spec.name.clone(),
                proof: s.spec.proof,
                items: flat(&s.items),
                unmatched: 0,
                proves: s.spec.proves.as_ref().and_then(|id| ts.iter().position(|x| &x.spec.id == id)),
                end_mark: s.spec.end_mark,
            })
            .collect();
        let mentions: Vec<FoundMention> = truth
            .mentions
            .iter()
            .filter(|m| m.doc == paper)
            .map(|m| FoundMention {
                items: flat(&m.items),
                unmatched: 0,
                target: m
                    .target
                    .as_ref()
                    .and_then(|t| truth.targets.iter().find(|x| x.doc == paper && &x.id == t))
                    .map(|t| FoundTarget { items: flat(&t.items), unmatched: 0 }),
            })
            .collect();
        tally.add_document(&truth, &paper, &statements, &mentions);
    }
    let s = tally.statements["*"];
    assert_eq!(
        (s.detected, s.kind_right, s.number_right, s.name_right, s.extent_right, s.spurious),
        (s.truth, s.truth, s.truth, s.truth, s.truth, 0)
    );
    let p = tally.proofs;
    assert_eq!((p.proves_right, p.marked_found, p.unmarked_found), (p.detected, p.marked, 0));
    let m = tally.mentions["*"];
    assert_eq!((m.matched, m.right + m.none_right, m.wrong, tally.spurious_links), (m.truth, m.truth, 0, 0));
    assert!(s.truth >= 70 && p.detected >= 25 && m.truth >= 100, "{s:?} {p:?} {m:?}");
}

/// The analysis keeps the scores it reached: every statement and mention of the development papers
/// found and resolved, and no wrong link anywhere; the held-out papers above their floor.
#[test]
fn the_analysis_keeps_its_scores() {
    let (dev, holdout) = dev_and_holdout(&Real);
    for (name, t, floor) in [("dev", &dev, 1.0), ("holdout", &holdout, 0.9)] {
        let s = t.statements["*"];
        let m = t.mentions["*"];
        let rate = |a: usize, b: usize| a as f64 / b.max(1) as f64;
        assert!(rate(s.detected, s.truth) >= floor && rate(s.extent_right, s.truth) >= floor, "{name}: {s:?}");
        assert!(rate(t.proofs.proves_right, t.proofs.detected) >= floor, "{name}: {:?}", t.proofs);
        assert!(rate(m.matched, m.truth) >= floor && rate(t.mentions_found_matched, t.mentions_found) >= floor, "{name}: {m:?}");
        assert!(rate(m.right + m.none_right, m.matched) >= floor, "{name}: {m:?}");
        assert_eq!(t.wrong_links(), 0, "{name}: a wrong link: {m:?}, {} where the text has none", t.spurious_links);
    }
}

/// The harness tells a broken analysis from a working one.
#[test]
fn broken_analysers_score_badly() {
    let (real, _) = dev_and_holdout(&Real);
    let (numbers, _) = dev_and_holdout(&TrustNumbers);
    let (tags, _) = dev_and_holdout(&EveryTag);
    let (proofs, _) = dev_and_holdout(&EndlessProofs);
    assert_eq!(real.wrong_links(), 0);
    // Trusting the number links mentions to statements of another kind.
    assert!(numbers.wrong_links() >= 8, "{:?}", numbers.mentions["*"]);
    // Every `(n)` a link: list labels and cases go to equations.
    assert!(tags.wrong_links() >= 8 && tags.spurious_links >= 8, "{}", tags.spurious_links);
    // Proofs that never end swallow the text after them.
    let (r, p) = (real.statements["proof"], proofs.statements["proof"]);
    assert!(p.extent_right * 10 <= r.extent_right * 8, "{p:?} against {r:?}");
}

/// On the linked papers, every link to a block has an inferred mention under it, and both go to the
/// same place, except where the text names another work with a `\ref` (`Theorem 1.1 of [5]`):
/// the link is real, the inference says it points outside, and the reference graph lets the link
/// win and says so.
#[test]
fn inferred_mentions_go_where_the_links_go() {
    let truth = load().unwrap();
    let linked = linked(&truth).unwrap();
    assert!(linked.len() >= 2);
    let mut agreed = 0;
    for p in &linked {
        let a = Real.analyse(p);
        for link in &p.doc.links {
            let LinkTarget::Page(target, view) = link.target else { continue };
            let page = &p.doc.pages[link.page];
            let (a0, a1) = (page.user_to_view(link.rect[0], link.rect[1]), page.user_to_view(link.rect[2], link.rect[3]));
            let r = [a0[0].min(a1[0]), a0[1].min(a1[1]), a0[0].max(a1[0]), a0[1].max(a1[1])];
            let m = a.refs.mentions.iter().find(|m| {
                m.pieces.iter().any(|q| q.page == link.page && q.rect[0] < r[2] && r[0] < q.rect[2] && q.rect[1] < r[3] && r[1] < q.rect[3])
            });
            let m = m.unwrap_or_else(|| panic!("{}: no inferred mention under the link to {:?}", p.info.doc, link.dest_name));
            let DestView::Xyz { left, top, .. } = view else { panic!("hyperref writes /XYZ destinations") };
            let y = p.doc.pages[target].dest_fraction(left, top, 0)[1].unwrap() * p.doc.pages[target].height;
            let rect = match m.target {
                Some(Target::Statement(k)) => a.refs.statements[k].extent.iter().find(|x| x.page == target).map(|x| x.rect),
                Some(Target::Heading { page, block } | Target::Equation { page, block } | Target::Float { page, block }) => {
                    (page == target).then(|| a.layouts[page].blocks[block].rect)
                }
                None => {
                    assert_eq!(m.unresolved, Some(Unresolved::External), "{}: {} {} has no target", p.info.doc, m.word, m.number);
                    continue;
                }
            };
            let rect = rect.unwrap_or_else(|| panic!("{}: {} {} goes to another page than its link", p.info.doc, m.word, m.number));
            assert!(
                y >= rect[1] - 24.0 && y <= rect[3] + 2.0,
                "{}: {} {} goes elsewhere than its link: {y} not in {rect:?}",
                p.info.doc,
                m.word,
                m.number
            );
            agreed += 1;
        }
    }
    assert!(agreed >= 14, "{agreed}");
}
