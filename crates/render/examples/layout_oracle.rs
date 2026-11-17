//! The layout oracle's report (`cargo xtask layout-oracle report`): scores of an analyser on the
//! synthetic papers of `tests/data/layout/oracle`, development and held out apart, per kind, class
//! and column layout.
//!
//! Arguments: `--analyser NAME` (`layout`, the default, or a broken one: `broken-no-columns`,
//! `broken-merge-paragraphs`, `broken-no-running-heads`; `all` for every one), `--failures` (the
//! wrong blocks of the development papers, by page; never of the held-out ones), `--page DOC:N` (the
//! blocks found on page N, from 1, of a development paper).

#[path = "../tests/it/layout_oracle/harness.rs"]
mod harness;

use harness::{ANALYSERS, Analyser, analyser, found_blocks, grouped, load, papers, set_name};
use pdfcraft_oracle::tex::layout::{Kind, matches};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let name = args.iter().position(|a| a == "--analyser").and_then(|i| args.get(i + 1)).map_or("layout", String::as_str);
    let names: Vec<&str> = if name == "all" { ANALYSERS.to_vec() } else { vec![name] };
    let truth = match load() {
        Ok(t) => t,
        Err(e) => return eprintln!("layout-oracle: {e}"),
    };
    let papers = match papers(&truth) {
        Ok(p) => p,
        Err(e) => return eprintln!("layout-oracle: {e}"),
    };
    if let Some(spec) = args.iter().position(|a| a == "--page").and_then(|i| args.get(i + 1)) {
        let (doc, n) = spec.split_once(':').unwrap_or((spec, "1"));
        let n: usize = n.parse().unwrap_or(1);
        match papers.iter().find(|p| p.info.doc == doc && !p.info.holdout) {
            None => eprintln!("layout-oracle: no development paper `{doc}`"),
            Some(p) => {
                let (layouts, _) = harness::Real.analyse(p);
                if let Some(l) = layouts.get(n.saturating_sub(1)) {
                    println!("columns: {:?}\nbody {:?}; diagnostics: {:?}", l.columns, l.body_size, l.diagnostics);
                    for &i in &l.reading_order {
                        let b = &l.blocks[i];
                        let t: String = b.text.chars().take(60).collect();
                        println!(
                            "#{i} {:?} col {:?} rect {:?} lines {} {:?} conf {} {:?}",
                            b.kind,
                            b.column,
                            b.rect.map(|v| v.round()),
                            b.lines.len(),
                            b.style,
                            b.confidence,
                            t
                        );
                    }
                }
            }
        }
        return;
    }
    for name in names {
        let Some(a) = analyser(name) else {
            eprintln!("layout-oracle: no analyser `{name}`; one of {ANALYSERS:?} or all");
            return;
        };
        println!("== analyser {name} (a block is right when its glyphs and rules match the truth's with F1 >= 0.9, and its kind too)\n");
        for (set, t) in grouped(&papers, a.as_ref(), set_name) {
            println!("{}", t.render(&set, true));
        }
        let by = |f: fn(&pdfcraft_oracle::tex::layout::PaperInfo) -> String| {
            for (key, t) in grouped(&papers, a.as_ref(), f) {
                print!("{}", t.render(&key, false));
            }
            println!();
        };
        by(|p| format!("{} {}", set_name(p), p.class));
        by(|p| format!("{} {}", set_name(p), p.columns));
        by(|p| format!("{} {}", set_name(p), p.size));
        if args.iter().any(|x| x == "--failures") {
            for p in papers.iter().filter(|p| !p.info.holdout) {
                let (layouts, _) = a.analyse(p);
                for (k, (t, l)) in p.truth.iter().zip(&layouts).enumerate() {
                    let found = found_blocks(p, k, l);
                    let m = matches(t, &found);
                    for (bi, b) in t.blocks.iter().enumerate() {
                        if b.kind == Kind::Skip {
                            continue;
                        }
                        let got = m[bi].map(|f| found[f].kind);
                        if got.map(Kind::family) != Some(b.kind.family()) {
                            let holders: Vec<String> = found
                                .iter()
                                .enumerate()
                                .filter_map(|(fi, f)| {
                                    let c = f.items.iter().filter(|i| b.items.contains(i)).count();
                                    (c > 0).then(|| format!("#{fi} {} ({c}/{} of it, {} items)", f.kind, b.items.len(), f.items.len() + f.unmatched))
                                })
                                .collect();
                            println!("{} p{} {} {}: found {:?}; held by {}", p.info.doc, t.page, b.id, b.kind, got, holders.join(", "));
                        }
                    }
                }
            }
        }
    }
}
