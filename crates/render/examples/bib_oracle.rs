//! The bibliography oracle's report (`cargo xtask bib-oracle report`): scores of an analyser on the
//! synthetic papers of `tests/data/bib/oracle`, development and held out apart, per BibTeX style,
//! citation mode and layout class, with wrong links and wrong fields counted on their own.
//!
//! Arguments: `--analyser NAME` (`bib`, the default, or a broken one: `broken-split-every-line`,
//! `broken-every-bracket`, `broken-ignore-year`; `all` for every one), `--failures` (the wrong
//! entries, fields and citations of the development papers; never of the held-out ones), `--dev`
//! (score the development papers alone, while the analysis is being changed).

#[path = "../tests/it/bib_oracle/harness.rs"]
mod harness;

use harness::{ANALYSERS, Analyser, Paper, agreement, analyser, found, grouped, item_text, linked, load, papers, set_name};
use pdfcraft_oracle::tex::bib::{BibTruth, CITE_F1, DETECT_F1, FIELDS, field_matches};
use std::collections::{HashMap, HashSet};

fn failures(paper: &Paper, truth: &BibTruth, a: &dyn Analyser) {
    let an = a.analyse(paper);
    let f = found(paper, &an);
    let doc = &paper.info.doc;
    println!("--- {doc} ({}, {})", paper.bib.style, paper.bib.mode);
    if f.region.is_none() {
        println!("  no list found; diagnostics {:?}", an.bib.diagnostics.iter().map(|d| d.code).collect::<Vec<_>>());
    }
    let te: Vec<_> = truth.entries.iter().filter(|e| e.doc == *doc).collect();
    let flat = |items: &Vec<(usize, Vec<usize>)>| items.iter().flat_map(|(p, l)| l.iter().map(move |&i| (*p, i))).collect::<HashSet<_>>();
    let mut matched: HashMap<usize, usize> = HashMap::new();
    for (ti, t) in te.iter().enumerate() {
        let set = flat(&t.items);
        let best = f
            .entries
            .iter()
            .enumerate()
            .map(|(fi, e)| {
                let common = e.items.iter().filter(|i| set.contains(i)).count();
                (fi, 2.0 * common as f64 / (set.len() + e.items.len() + e.unmatched).max(1) as f64)
            })
            .max_by(|a, b| a.1.total_cmp(&b.1));
        let text = item_text(paper, &set.iter().copied().collect::<Vec<_>>());
        match best {
            Some((fi, s)) if s >= DETECT_F1 => {
                matched.insert(fi, ti);
                let e = &f.entries[fi];
                if s < 0.95 {
                    println!(
                        "  entry {} boundary F1 {s:.2}: truth \"{}…\" found \"{}…\"",
                        t.spec.key,
                        text.chars().take(50).collect::<String>(),
                        item_text(paper, &e.items).chars().take(50).collect::<String>()
                    );
                }
                let fold = |s: &str| pdfcraft_oracle::tex::bib::fold(s).replace(' ', "");
                if t.spec.label.as_deref().map(fold) != e.label.as_deref().map(fold) {
                    println!("  entry {} label: truth {:?} found {:?}", t.spec.key, t.spec.label, e.label);
                }
                for name in FIELDS {
                    let want = t.spec.fields.get(name);
                    let got = e.fields.get(name);
                    let ok = match (&want, got) {
                        (Some(w), Some(g)) => field_matches(name, w, g),
                        (None, None) => true,
                        _ => false,
                    };
                    if !ok {
                        println!("  entry {} {name}: truth {want:?} found {got:?}", t.spec.key);
                    }
                }
            }
            _ => println!("  entry {} missed: \"{}\"", t.spec.key, text.chars().take(70).collect::<String>()),
        }
    }
    for (fi, e) in f.entries.iter().enumerate() {
        if !matched.contains_key(&fi) {
            println!("  spurious entry: \"{}\"", item_text(paper, &e.items).chars().take(70).collect::<String>());
        }
    }
    let tc: Vec<_> = truth.cites.iter().filter(|c| c.doc == *doc && c.spec.serial.is_some()).collect();
    let mut used = HashSet::new();
    for t in &tc {
        let set = flat(&t.items);
        let best = f
            .cites
            .iter()
            .enumerate()
            .map(|(fi, c)| {
                (fi, 2.0 * c.items.iter().filter(|i| set.contains(i)).count() as f64 / (set.len() + c.items.len() + c.unmatched).max(1) as f64)
            })
            .max_by(|a, b| a.1.total_cmp(&b.1));
        let text = item_text(paper, &set.iter().copied().collect::<Vec<_>>());
        match best {
            Some((fi, s)) if s >= CITE_F1 => {
                used.insert(fi);
                let target = f.cites[fi].target.and_then(|e| matched.get(&e)).and_then(|ti| te.get(*ti)).map(|e| e.spec.key.as_str());
                if target != Some(t.spec.key.as_str()) {
                    println!("  citation \"{text}\" ({}) of {}: linked to {target:?}", t.spec.form, t.spec.key);
                }
            }
            Some((fi, s)) if s > 0.0 => println!(
                "  citation \"{text}\" ({}) of {}: glyphs F1 {s:.2}, found \"{}\"",
                t.spec.form,
                t.spec.key,
                item_text(paper, &f.cites[fi].items)
            ),
            _ => println!("  citation \"{text}\" ({}) of {}: missed", t.spec.form, t.spec.key),
        }
    }
    for (fi, c) in f.cites.iter().enumerate() {
        if !used.contains(&fi) && !c.items.is_empty() {
            println!("  spurious citation \"{}\" -> {:?}", item_text(paper, &c.items), c.target);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let name = args.iter().position(|a| a == "--analyser").and_then(|i| args.get(i + 1)).map_or("bib", String::as_str);
    let names: Vec<&str> = if name == "all" { ANALYSERS.to_vec() } else { vec![name] };
    let dev_only = args.iter().any(|a| a == "--dev");
    let truth = match load() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let all = match papers(&truth) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let ps: Vec<Paper> = all.into_iter().filter(|p| !(dev_only && p.info.holdout)).collect();
    for n in names {
        let Some(a) = analyser(n) else {
            eprintln!("no analyser `{n}`; one of {ANALYSERS:?} or all");
            std::process::exit(2);
        };
        println!("##### analyser {n}");
        for (set, t) in grouped(&ps, &truth, a.as_ref(), set_name) {
            print!("{}", t.render(&set));
        }
        for (label, key) in [
            ("BibTeX style", &(|p: &Paper| format!("{} {}", set_name(p), p.bib.style)) as &dyn Fn(&Paper) -> String),
            ("citation mode", &|p: &Paper| format!("{} {}", set_name(p), p.bib.mode)),
            ("layout class", &|p: &Paper| format!("{} {} {} {}", set_name(p), p.info.class, p.info.columns, p.info.size)),
        ] {
            println!("--- by {label}: wrong links / wrong fields / entries exact / citations matched / right");
            for (k, t) in grouped(&ps, &truth, a.as_ref(), key) {
                let c = t.cites.get("*").copied().unwrap_or_default();
                println!(
                    "  {k:<34} {:>3} {:>3}   {:>3}/{:<3}   {:>3}/{:<3}   {:>3}",
                    t.wrong_links(),
                    t.wrong_fields(),
                    t.entries.exact,
                    t.entries.truth,
                    c.matched,
                    c.truth,
                    c.right
                );
            }
        }
        if let Ok(lp) = linked(&truth) {
            for p in lp.iter().filter(|p| !(dev_only && p.info.holdout)) {
                let g = agreement(p, &a.analyse(p));
                println!(
                    "linked {}: {} citation links, {} with a citation read under them, {} agree, {} DISAGREE",
                    p.info.doc, g.links, g.covered, g.agree, g.disagree
                );
                if args.iter().any(|x| x == "--failures") && !p.info.holdout {
                    for d in g.details {
                        println!("    {d}");
                    }
                }
            }
        }
        if args.iter().any(|x| x == "--failures") {
            for p in ps.iter().filter(|p| !p.info.holdout) {
                failures(p, &truth, a.as_ref());
            }
        }
    }
}
