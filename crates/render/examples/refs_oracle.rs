//! The reference oracle's report (`cargo xtask refs-oracle report`): scores of an analyser on the
//! synthetic papers of `tests/data/refs/oracle`, development and held out apart, per statement
//! kind, mention target, head style and layout class, with wrong links counted on their own.
//!
//! Arguments: `--analyser NAME` (`refs`, the default, or a broken one: `broken-trust-numbers`,
//! `broken-every-tag`, `broken-endless-proofs`; `all` for every one), `--failures` (the wrong
//! statements and mentions of the development papers; never of the held-out ones), `--dev` (score
//! the development papers alone, while the analysis is being changed).

#[path = "../tests/it/refs_oracle/harness.rs"]
mod harness;

use harness::{ANALYSERS, Analyser, Paper, analyser, found, grouped, linked, load, papers, set_name};
use pdfcraft_oracle::tex::refs::{RefsTally, RefsTruth};
use pdfcraft_render::text::page_layout::refs::Target;
use pdfcraft_render::{DestView, LinkTarget};

fn pct(a: usize, b: usize) -> String {
    if b == 0 { "–".to_owned() } else { format!("{:.1}%", 100.0 * a as f64 / b as f64) }
}

fn print_tally(name: &str, t: &RefsTally) {
    let s = t.statements.get("*").copied().unwrap_or_default();
    let m = t.mentions.get("*").copied().unwrap_or_default();
    let p = &t.proofs;
    println!("  {name}: {} papers", t.docs);
    println!(
        "    statements and proofs: detected {}/{} ({}), word {}, number {}, name {}, extent F1>=0.9 {}; spurious {} of {} found",
        s.detected,
        s.truth,
        pct(s.detected, s.truth),
        pct(s.kind_right, s.detected),
        pct(s.number_right, s.detected),
        pct(s.name_right, s.detected),
        pct(s.extent_right, s.truth),
        s.spurious,
        s.found
    );
    println!(
        "    proofs: {} detected; proves the right statement {}, a wrong one {}, none {}; end mark found {}/{}, a mark where there is none {}/{}",
        p.detected, p.proves_right, p.proves_wrong, p.proves_none, p.marked_found, p.marked, p.unmarked_found, p.unmarked
    );
    println!(
        "    mentions: recall {}/{} ({}), precision {}/{} ({}); resolved right {}, none where none {}, missed {}",
        m.matched,
        m.truth,
        pct(m.matched, m.truth),
        t.mentions_found_matched,
        t.mentions_found,
        pct(t.mentions_found_matched, t.mentions_found),
        m.right,
        m.none_right,
        m.missed
    );
    println!("    WRONG LINKS: {} ({} on matched mentions, {} where the text has no mention)", t.wrong_links(), m.wrong, t.spurious_links);
}

fn print_kinds(t: &RefsTally) {
    println!("    by statement word: truth / detected / word / number / extent");
    for (k, c) in t.statements.iter().filter(|(k, _)| k.as_str() != "*") {
        if c.truth > 0 {
            println!(
                "      {k:<14} {:>3} {:>3} {:>3} {:>3} {:>3}   (found {}, spurious {})",
                c.truth, c.detected, c.kind_right, c.number_right, c.extent_right, c.found, c.spurious
            );
        }
    }
    println!("    mentions by target: truth / matched / right / none-right / WRONG / missed");
    for (k, c) in t.mentions.iter().filter(|(k, _)| k.as_str() != "*") {
        println!("      {k:<14} {:>3} {:>3} {:>3} {:>3} {:>3} {:>3}", c.truth, c.matched, c.right, c.none_right, c.wrong, c.missed);
    }
}

/// Where a link goes and where the inferred mention under it goes agree: same page, and the
/// destination's height inside the target block (with a line of slack above it).
fn agreement(paper: &Paper, a: &harness::Analysis) -> (usize, usize, usize, usize) {
    let (mut links, mut covered, mut agree, mut disagree) = (0, 0, 0, 0);
    for link in &paper.doc.links {
        let LinkTarget::Page(target, view) = link.target else { continue };
        let Some(page) = paper.doc.pages.get(link.page) else { continue };
        if link.dest_name.as_deref().is_some_and(|n| n.starts_with("cite.")) {
            continue;
        }
        links += 1;
        let a0 = page.user_to_view(link.rect[0], link.rect[1]);
        let a1 = page.user_to_view(link.rect[2], link.rect[3]);
        let r = [a0[0].min(a1[0]), a0[1].min(a1[1]), a0[0].max(a1[0]), a0[1].max(a1[1])];
        let Some(m) =
            a.refs.mentions.iter().find(|m| {
                m.pieces.iter().any(|p| p.page == link.page && p.rect[0] < r[2] && r[0] < p.rect[2] && p.rect[1] < r[3] && r[1] < p.rect[3])
            })
        else {
            continue;
        };
        covered += 1;
        let top = match view {
            DestView::Xyz { left, top, .. } => paper.doc.pages.get(target).and_then(|tp| tp.dest_fraction(left, top, 0)[1].map(|f| f * tp.height)),
            DestView::FitH { top } => paper.doc.pages.get(target).and_then(|tp| tp.dest_fraction(None, top, 0)[1].map(|f| f * tp.height)),
            _ => None,
        };
        let block = m.target.and_then(|t| match t {
            Target::Statement(k) => a.refs.statements.get(k).and_then(|s| s.extent.iter().find(|p| p.page == target)).map(|p| (target, p.rect)),
            Target::Heading { page, block } | Target::Equation { page, block } | Target::Float { page, block } => {
                a.layouts.get(page).and_then(|l| l.blocks.get(block)).map(|b| (page, b.rect))
            }
        });
        match (block, top) {
            (Some((p, rect)), Some(y)) if p == target && y >= rect[1] - 24.0 && y <= rect[3] + 2.0 => agree += 1,
            _ => {
                disagree += 1;
                if std::env::args().any(|a| a == "--failures") && !paper.info.holdout {
                    println!(
                        "  disagree: link {:?} -> page {target} y {top:?}; inferred {} {} -> {:?} {block:?}",
                        link.dest_name, m.word, m.number, m.target
                    );
                }
            }
        }
    }
    (links, covered, agree, disagree)
}

fn failures(papers: &[Paper], truth: &RefsTruth, an: &dyn Analyser) {
    for p in papers.iter().filter(|p| !p.info.holdout) {
        let a = an.analyse(p);
        println!("== {}", p.info.doc);
        for s in &a.refs.statements {
            println!(
                "  found {}{} {:?} name {:?} lines {:?}..{:?} proves {:?} mark {:?}",
                if s.proof { "proof " } else { "" },
                s.kind,
                s.number,
                s.name,
                s.first_line,
                s.last_line,
                s.proves.map(|x| x.0),
                s.end_mark.as_ref().map(|m| m.1.kind)
            );
        }
        let (fs, fm) = found(p, &a);
        let mut t = RefsTally::default();
        t.add_document(truth, &p.info.doc, &fs, &fm);
        for (m, f) in a.refs.mentions.iter().zip(&fm) {
            let text: String = format!("{} {}", m.word, m.number);
            println!("  mention {:<24} -> {:?} {:?} conf {:.2} ({} items)", text, m.target, m.unresolved, m.confidence, f.items.len());
        }
        for s in truth.statements.iter().filter(|s| s.doc == p.info.doc) {
            let set: std::collections::HashSet<(usize, usize)> = s.items.iter().flat_map(|(pg, l)| l.iter().map(move |&i| (*pg, i))).collect();
            let best = fs
                .iter()
                .map(|f| {
                    let c = f.items.iter().filter(|i| set.contains(i)).count();
                    (2.0 * c as f64 / (set.len() + f.items.len() + f.unmatched).max(1) as f64, f.items.len(), c)
                })
                .fold((0.0, 0, 0), |a, b| if b.0 > a.0 { b } else { a });
            println!(
                "  truth {} {:?} {:?} proves {:?} mark {}: best F1 {:.2} (found {} items, {} common, truth {})",
                s.spec.kind,
                s.number,
                s.spec.name,
                s.spec.proves,
                s.spec.end_mark,
                best.0,
                best.1,
                best.2,
                set.len()
            );
        }
        for m in truth.mentions.iter().filter(|m| m.doc == p.info.doc) {
            println!("  truth mention {} {} -> {:?} ({} items)", m.id, m.form, m.target, m.items.iter().map(|x| x.1.len()).sum::<usize>());
        }
        print_tally(&p.info.doc, &t);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let name = args.iter().position(|a| a == "--analyser").and_then(|i| args.get(i + 1)).map_or("refs", String::as_str);
    let names: Vec<&str> = if name == "all" { ANALYSERS.to_vec() } else { vec![name] };
    let truth = match load() {
        Ok(t) => t,
        Err(e) => return eprintln!("refs-oracle: {e}"),
    };
    let mut papers = match papers(&truth) {
        Ok(p) => p,
        Err(e) => return eprintln!("refs-oracle: {e}"),
    };
    // While the analysis is being changed, the held-out papers are not even scored.
    let dev_only = args.iter().any(|a| a == "--dev");
    if dev_only {
        papers.retain(|p| !p.info.holdout);
    }
    for n in names {
        let Some(an) = analyser(n) else {
            eprintln!("refs-oracle: no analyser `{n}`; one of {ANALYSERS:?}");
            continue;
        };
        if args.iter().any(|a| a == "--failures") {
            failures(&papers, &truth, an.as_ref());
            for p in linked(&truth).unwrap_or_default().iter().filter(|p| !p.info.holdout) {
                let a = an.analyse(p);
                println!("== linked {}: {:?}", p.info.doc, agreement(p, &a));
            }
            continue;
        }
        println!("refs-oracle: analyser {n}");
        let by_set = grouped(&papers, &truth, an.as_ref(), set_name);
        for (set, t) in &by_set {
            print_tally(set, t);
            print_kinds(t);
        }
        for (label, key) in [
            ("head style", Box::new(|p: &Paper| format!("{} {}", set_name(p), p.refs.heads)) as Box<dyn Fn(&Paper) -> String>),
            ("layout class", Box::new(|p: &Paper| format!("{} {} {}", set_name(p), p.info.class, p.info.columns))),
            ("numbering", Box::new(|p: &Paper| format!("{} {}", set_name(p), p.refs.numbering))),
        ] {
            println!("  by {label}:");
            for (k, t) in grouped(&papers, &truth, an.as_ref(), key) {
                let s = t.statements.get("*").copied().unwrap_or_default();
                let m = t.mentions.get("*").copied().unwrap_or_default();
                println!(
                    "    {k:<26} statements {}/{} extent {}; mentions {}/{} resolved right or none {}; WRONG {}",
                    s.detected,
                    s.truth,
                    pct(s.extent_right, s.truth),
                    m.matched,
                    m.truth,
                    pct(m.right + m.none_right, m.matched),
                    t.wrong_links()
                );
            }
        }
        // Linked papers: real links against inferred mentions.
        let linked = match linked(&truth) {
            Ok(l) => l,
            Err(e) => return eprintln!("refs-oracle: {e}"),
        };
        for p in linked.iter().filter(|p| !dev_only || !p.info.holdout) {
            let a = an.analyse(p);
            let (links, covered, agree, disagree) = agreement(p, &a);
            println!(
                "  hyperref {} ({}): {links} links to blocks, {covered} under an inferred mention, agree {agree}, disagree {disagree}",
                p.info.doc,
                set_name(p)
            );
        }
    }
}
