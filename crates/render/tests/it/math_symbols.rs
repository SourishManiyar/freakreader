//! The math symbol table (`src/text/math/symbols`): it covers what the oracle's pages draw, and every
//! entry is what TeX sets for the LaTeX that names it.
//!
//! The tests that read committed files run everywhere. The round trip needs TeX Live
//! (`cargo xtask math-oracle test` runs it); like the oracle's, it is `#[ignore]`d in the plain run
//! and a missing TeX is a skip, or a failure under `PDFCRAFT_ORACLES_REQUIRED`.

use crate::math_oracle::harness;
use harness::load;
use pdfcraft_oracle::tex::{Env, Mark, Tex};
use pdfcraft_render::text::math::symbols::{Lookup, family_of, table};
use std::collections::BTreeSet;

fn table_text() -> String {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/text/math/symbols/table.tsv");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Every (font, code) the oracle's equations draw is a symbol the table knows, or part of a compound.
#[test]
fn the_table_covers_every_glyph_the_oracle_draws() {
    let truth = load("truth.tsv").unwrap();
    let t = table();
    let mut known_in_compounds = BTreeSet::new();
    for c in t.all_compounds() {
        for p in &c.parts {
            known_in_compounds.insert((p.family, p.code));
        }
    }
    let mut missing = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for page in &truth.pages {
        for (item, owner) in &page.items {
            // Running text and equation numbers are prose, in text fonts.
            if owner == "-" || owner == "tag" {
                continue;
            }
            let Mark::Glyph { font, code, .. } = &item.mark else { continue };
            seen.insert((font.clone(), *code));
            let Some(id) = family_of(font).and_then(|f| t.family(&f)) else {
                missing.insert((font.clone(), *code, "font"));
                continue;
            };
            if !matches!(t.lookup_id(id, *code), Lookup::Known(_)) && !known_in_compounds.contains(&(id, *code)) {
                missing.insert((font.clone(), *code, "code"));
            }
        }
    }
    assert!(seen.len() > 200, "{}", seen.len());
    assert!(missing.is_empty(), "the table does not know: {missing:?}");
}

/// The glyphs of the oracle's pages as the page reads them resolve through the font names the PDF
/// really carries (subset prefix and all), not only the TFM names of the truth.
#[test]
fn every_glyph_of_the_oracle_pdfs_resolves_to_a_symbol_or_a_compound_part() {
    let truth = load("truth.tsv").unwrap();
    let mut pages = harness::Pages::default();
    let t = table();
    let mut parts = BTreeSet::new();
    for c in t.all_compounds() {
        for p in &c.parts {
            parts.insert((p.family, p.code));
        }
    }
    let mut checked = 0;
    for e in &truth.equations {
        let page = pages.get(&e.doc, e.page).unwrap();
        for g in harness::glyphs_in(page, harness::region_of(e)) {
            let name = page.fonts.get(g.font as usize).and_then(|f| f.base_font.clone()).unwrap_or_default();
            let ok = match t.lookup(&name, g.code) {
                Lookup::Known(_) => true,
                Lookup::UnknownFont => false,
                Lookup::UnknownCode => family_of(&name).and_then(|f| t.family(&f)).is_some_and(|id| parts.contains(&(id, g.code))),
            };
            assert!(ok, "{} ({}): {name} code {} is unknown", e.id, e.doc, g.code);
            checked += 1;
        }
    }
    assert!(checked > 1000, "{checked}");
}

const NEEDS_TEX: &str = "needs TeX Live";

fn tex_or_skip() -> Option<Tex> {
    match Tex::find() {
        Ok(t) => Some(t),
        Err(absent) => {
            assert!(!absent.required(), "{absent}");
            eprintln!("{absent}");
            None
        }
    }
}

/// With TeX: typesetting the LaTeX of every entry gives its font and code (for a compound, its
/// glyphs in their places).
#[test]
#[ignore = "needs TeX Live: cargo xtask math-oracle test"]
fn tex_sets_every_table_entry_as_the_table_says() {
    let Some(tex) = tex_or_skip() else { return };
    let text = table_text();
    /// A glyph a row expects: family, code, and for a compound its place per mille of the first glyph's size.
    type Expect = (String, u32, Option<(i64, i64)>);
    struct Row<'a> {
        what: String,
        env: Env,
        tex: &'a str,
        /// (family, code) of each glyph; for a compound with its relative place per mille.
        expect: Vec<Expect>,
        single: bool,
        exact: bool,
    }
    let mut rows = Vec::new();
    for line in text.lines().filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let f: Vec<&str> = line.split('\t').collect();
        let env = |s: &str| if s == "i" { Env::Inline } else { Env::Display };
        match f[0] {
            "S" => rows.push(Row {
                what: format!("{} {} {}", f[1], f[2], f[3]),
                env: env(f[12]),
                tex: f[13],
                expect: vec![(f[1].to_owned(), f[2].parse().unwrap(), None)],
                single: true,
                // A template with a rule in it (a delimiter around a rule, an accent) sets more.
                exact: !f[13].contains("\\vrule"),
            }),
            "C" => {
                let expect = f[8]
                    .split(';')
                    .map(|p| {
                        let a: Vec<&str> = p.split(':').collect();
                        (a[0].to_owned(), a[1].parse().unwrap(), Some((a[2].parse().unwrap(), a[3].parse().unwrap())))
                    })
                    .collect();
                rows.push(Row { what: format!("compound {}", f[1]), env: env(f[6]), tex: f[7], expect, single: false, exact: true });
            }
            other => panic!("row kind {other}"),
        }
    }
    assert!(rows.len() > 800, "{}", rows.len());
    let mut bad = Vec::new();
    for chunk in rows.chunks(200) {
        let entries: Vec<(Env, &str)> = chunk.iter().map(|r| (r.env, r.tex)).collect();
        for (row, result) in chunk.iter().zip(tex.typeset(&entries).unwrap()) {
            let items = match result {
                Ok(items) => items,
                Err(why) => {
                    bad.push(format!("{}: `{}` does not typeset: {}", row.what, row.tex, why.lines().next().unwrap_or("")));
                    continue;
                }
            };
            let glyphs: Vec<(String, i64, i64, i64, u32)> = items
                .iter()
                .filter_map(|i| match &i.mark {
                    Mark::Glyph { font, size, code } => {
                        Some((font.trim_end_matches(|c: char| c.is_ascii_digit()).to_owned(), i.h, i.v, *size, *code))
                    }
                    Mark::Rule { .. } => None,
                })
                .collect();
            if row.single {
                let want = &row.expect[0];
                let found = glyphs.iter().any(|g| g.0 == want.0 && g.4 == want.1);
                let count_ok = !row.exact || glyphs.len() == 1;
                if !found || !count_ok {
                    bad.push(format!("{}: `{}` sets {:?}", row.what, row.tex, glyphs.iter().map(|g| (&g.0, g.4)).collect::<Vec<_>>()));
                }
            } else {
                let ok = glyphs.len() == row.expect.len()
                    && glyphs.first().is_some_and(|first| {
                        glyphs.iter().zip(&row.expect).all(|(g, e)| {
                            let (dx, dy) = e.2.unwrap_or((0, 0));
                            let per = |v: i64| (v * 1000 + first.3 / 2).div_euclid(first.3.max(1));
                            g.0 == e.0 && g.4 == e.1 && (per(g.1 - first.1) - dx).abs() <= 1 && (per(g.2 - first.2) - dy).abs() <= 1
                        })
                    });
                if !ok {
                    bad.push(format!("{}: `{}` sets {:?}", row.what, row.tex, glyphs.iter().map(|g| (&g.0, g.4)).collect::<Vec<_>>()));
                }
            }
        }
    }
    assert!(bad.is_empty(), "{NEEDS_TEX}: {} entries do not round-trip:\n{}", bad.len(), bad.join("\n"));
}
