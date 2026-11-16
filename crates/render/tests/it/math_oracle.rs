//! The equation-level oracle for LaTeX recovery (`tests/data/math/oracle/README.md`).
//!
//! The tests without `#[ignore]` read only committed files: the PDFs hold exactly the glyphs and
//! rules the DVI lists, each equation's region holds its glyphs and nothing else, the identity
//! recoverer finds every equation through the page, and every committed deliberate breakage is
//! told apart by the typeset comparison. The `#[ignore]`d ones run TeX (`cargo xtask math-oracle
//! test`): the CI runners require every oracle and install no TeX Live.

pub(crate) mod harness;

use harness::{Broken, Identity, Pages, answers, glyphs_in, is_glyph, load, outcomes, region_of, rule_rect, same_glyphs};
use pdfcraft_oracle::tex::{self, ALONE, EquationTruth, Mark, Mutation, Report, Tex, Verdict};

/// How far an equation's ink may stick out of its box, in bp. The box is TeX's (character widths,
/// heights and depths, with italic correction); outlines overshoot it a little, and some glyphs
/// (the italic f, the tail of a big operator) more.
const INK_MARGIN_BP: f32 = 1.0;

/// Each mutation applies to at least this many equations of the list, so the discrimination test
/// is not vacuous.
const MIN_MUTANTS: usize = 10;

fn glyph_code(item: &tex::Item) -> Option<u32> {
    if let Mark::Glyph { code, .. } = item.mark { Some(code) } else { None }
}

fn alone(truths: &[EquationTruth]) -> Vec<&EquationTruth> {
    truths.iter().filter(|t| t.context == ALONE).collect()
}

/// pdfTeX in PDF mode set the glyphs and rules the DVI lists, and nothing else: the brackets that
/// say which equation owns what did not change the PDF.
#[test]
fn the_pdfs_hold_exactly_what_the_dvi_lists() {
    let truth = load("truth.tsv").unwrap();
    let mut pages = Pages::default();
    let mut glyphs = 0;
    for p in &truth.pages {
        let page = pages.get(&p.doc, p.page).unwrap();
        assert!(!page.truncated && !page.capped, "{} page {}", p.doc, p.page);
        let mut taken = vec![false; page.glyphs.len()];
        let mut rules_taken = vec![false; page.rules.len()];
        for (item, owner) in &p.items {
            if let Some(rect) = rule_rect(item) {
                let found = page.rules.iter().enumerate().position(|(k, r)| {
                    !rules_taken[k] && r.rect.iter().zip(rect).all(|(a, b)| (f64::from(*a) - b).abs() <= harness::PLACE_TOLERANCE_BP)
                });
                let k = found.unwrap_or_else(|| panic!("{} page {}: no rule like {rect:?} ({owner})", p.doc, p.page));
                rules_taken[k] = true;
            } else {
                let found = page.glyphs.iter().enumerate().position(|(k, g)| !taken[k] && is_glyph(page, g, item));
                let k = found.unwrap_or_else(|| {
                    let near: Vec<_> = page.glyphs.iter().filter(|g| Some(g.code) == glyph_code(item)).map(|g| (g.baseline[0], g.size)).collect();
                    let [x, y] = [tex::view_bp(item.h), tex::view_bp(item.v)];
                    panic!("{} page {}: no glyph like {item:?} at ({x:.3}, {y:.3}) ({owner}); with its code: {near:?}", p.doc, p.page)
                });
                taken[k] = true;
                glyphs += 1;
            }
        }
        assert!(taken.iter().all(|t| *t), "{} page {}: the PDF draws glyphs TeX did not set", p.doc, p.page);
        assert!(rules_taken.iter().all(|t| *t), "{} page {}: the PDF draws rules TeX did not set", p.doc, p.page);
    }
    assert!(truth.pages.len() > 100 && glyphs > 2000, "{} pages, {glyphs} glyphs", truth.pages.len());
}

/// A recoverer is handed an equation's box. It holds the equation's glyphs (by baseline origin) and
/// no other, its rules and no other, and the equation's ink lies within it.
#[test]
fn every_region_holds_its_equation_and_nothing_else() {
    let truth = load("truth.tsv").unwrap();
    let mut pages = Pages::default();
    for t in &truth.equations {
        let page = pages.get(&t.doc, t.page).unwrap();
        let region = region_of(t);
        let found = glyphs_in(page, region);
        if !same_glyphs(page, t, &found) {
            let extra: Vec<_> =
                found.iter().filter(|g| !t.items.iter().any(|i| is_glyph(page, g, i))).map(|g| (g.text.clone(), g.baseline[0])).collect();
            let missing: Vec<_> = t.items.iter().filter(|i| glyph_code(i).is_some() && !found.iter().any(|g| is_glyph(page, g, i))).collect();
            panic!("{}: the region {region:?} holds {} glyphs; not the equation's: {extra:?}; missing: {missing:?}", t.id, found.len());
        }
        let [x0, y0, x1, y1] = region;
        let rules_in = page.rules.iter().filter(|r| r.rect[0] < x1 && r.rect[2] > x0 && r.rect[1] < y1 && r.rect[3] > y0).count();
        let own_rules = t.items.iter().filter(|i| matches!(i.mark, Mark::Rule { .. })).count();
        assert_eq!(rules_in, own_rules, "{}: rules crossing the region", t.id);
        for g in &found {
            let Some([a, b, c, d]) = g.ink else { continue };
            let m = INK_MARGIN_BP;
            assert!(a >= x0 - m && b >= y0 - m && c <= x1 + m && d <= y1 + m, "{}: ink {:?} outside the box {region:?}", t.id, g.ink);
        }
    }
}

/// The identity recoverer, handed only the page and the region, answers every equation's source:
/// no edit distance anywhere. This runs without TeX.
#[test]
fn the_identity_recoverer_finds_every_equation_through_the_page() {
    let truth = load("truth.tsv").unwrap();
    let mut pages = Pages::default();
    let got = answers(&Identity { truths: &truth.equations }, &truth.equations, &mut pages).unwrap();
    for (t, a) in truth.equations.iter().zip(&got) {
        assert_eq!(a.as_deref(), Some(t.source.as_str()), "{}", t.id);
    }
    let report = Report::new(&outcomes(&truth.equations, got, None).unwrap());
    assert_eq!((report.total.count, report.total.answered, report.total.mean_distance()), (truth.equations.len(), truth.equations.len(), Some(0.0)));
    assert_eq!(report.total.checked, 0, "no TeX, no typeset verdicts");
    // The list's spread: every category and tier, alone and in running text.
    assert!(report.by_category.len() >= 19 && report.by_tier.len() == 3, "{:?}", report.by_category.keys());
    assert_eq!(report.by_context.keys().collect::<Vec<_>>(), ["1col", "2col", "alone"]);
}

/// Every deliberate breakage the generator typeset (`mutants.tsv`) is told apart from the
/// original by the typeset comparison, and the breakages are the ones the mutation code makes
/// today. Runs without TeX. If [`tex::same_typeset`] is weakened (ignoring places, codes or
/// fonts), this fails.
#[test]
fn every_committed_breakage_typesets_differently() {
    let truth = load("truth.tsv").unwrap();
    let mutants = load("mutants.tsv").unwrap();
    let mut counts = [0usize; 3];
    for t in alone(&truth.equations) {
        // Control: the original against its own typeset, moved.
        let moved: Vec<_> = t.reference.iter().map(|i| tex::Item { h: i.h + 1_000_000, v: i.v - 5_000, ..i.clone() }).collect();
        assert_eq!(tex::same_typeset(&t.reference, &moved), Verdict::Equal, "{}", t.id);
        for (k, m) in Mutation::ALL.into_iter().enumerate() {
            let id = format!("{}/{}", t.id, m.name());
            let row = mutants.mutants.iter().find(|r| r.id == id);
            match (m.apply(&t.source), row) {
                (None, None) => {}
                (Some(src), Some(row)) => {
                    assert_eq!((row.base.as_str(), row.source.as_str()), (t.id.as_str(), src.as_str()), "{id}");
                    let verdict = tex::same_typeset(&t.reference, &row.items);
                    assert!(matches!(verdict, Verdict::Different(_)), "{id}: `{src}` passes for `{}`", t.source);
                    counts[k] += 1;
                }
                (src, row) => {
                    panic!("{id}: the mutation gives {src:?}, mutants.tsv has {:?}; run cargo xtask math-oracle generate", row.map(|r| &r.source))
                }
            }
        }
    }
    assert_eq!(counts.iter().sum::<usize>(), mutants.mutants.len(), "mutants.tsv has rows for no equation");
    assert!(counts.iter().all(|&n| n >= MIN_MUTANTS), "{counts:?}");
}

/// The broken recoverers, too, go through the page: each answers a changed source for exactly the
/// equations its mutation applies to.
#[test]
fn the_broken_recoverers_answer_through_the_page() {
    let truth = load("truth.tsv").unwrap();
    let mut pages = Pages::default();
    for m in Mutation::ALL {
        let broken = Broken { identity: Identity { truths: &truth.equations }, mutation: m };
        let got = answers(&broken, &truth.equations, &mut pages).unwrap();
        for (t, a) in truth.equations.iter().zip(&got) {
            let changed = a.as_deref() != Some(t.source.as_str());
            assert_eq!(changed, m.apply(&t.source).is_some(), "{} {}", m.name(), t.id);
        }
    }
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

/// With TeX: the identity recoverer scores 100% typeset-equal, in running text too.
#[test]
#[ignore = "needs TeX Live: cargo xtask math-oracle test"]
fn tex_scores_the_identity_recoverer_at_100_percent() {
    let Some(tex) = tex_or_skip() else { return };
    let truth = load("truth.tsv").unwrap();
    let mut pages = Pages::default();
    let got = answers(&Identity { truths: &truth.equations }, &truth.equations, &mut pages).unwrap();
    let scored = outcomes(&truth.equations, got, Some(&tex)).unwrap();
    for o in &scored {
        assert_eq!(o.verdict, Some(Verdict::Equal), "{}", o.truth.id);
    }
    assert_eq!(Report::new(&scored).total.equal_rate(), Some(1.0), "{NEEDS_TEX}");
}

/// With TeX: each broken recoverer fails the typeset check on every equation its mutation applies
/// to, and passes the others.
#[test]
#[ignore = "needs TeX Live: cargo xtask math-oracle test"]
fn tex_rejects_every_broken_answer() {
    let Some(tex) = tex_or_skip() else { return };
    let truth = load("truth.tsv").unwrap();
    let mut pages = Pages::default();
    for m in Mutation::ALL {
        let broken = Broken { identity: Identity { truths: &truth.equations }, mutation: m };
        let got = answers(&broken, &truth.equations, &mut pages).unwrap();
        let scored = outcomes(&truth.equations, got, Some(&tex)).unwrap();
        let mut rejected = 0;
        for o in &scored {
            if m.apply(&o.truth.source).is_some() {
                assert!(matches!(o.verdict, Some(Verdict::Different(_))), "{} {}: {:?}", m.name(), o.truth.id, o.verdict);
                rejected += 1;
            } else {
                assert_eq!(o.verdict, Some(Verdict::Equal), "{} {}", m.name(), o.truth.id);
            }
        }
        assert!(rejected >= MIN_MUTANTS, "{}: {rejected}", m.name());
    }
}

/// With TeX: the installed TeX sets every equation exactly where the committed truth says (the
/// candidate template is the truth's), so a difference in TeX Live versions shows up here first.
#[test]
#[ignore = "needs TeX Live: cargo xtask math-oracle test"]
fn tex_reproduces_the_committed_truth() {
    let Some(tex) = tex_or_skip() else { return };
    let truth = load("truth.tsv").unwrap();
    let list = alone(&truth.equations);
    let entries: Vec<_> = list.iter().map(|t| (t.env, t.source.as_str())).collect();
    for (t, items) in list.iter().zip(tex.typeset(&entries).unwrap()) {
        assert_eq!(items.as_ref(), Ok(&t.reference), "{}", t.id);
    }
}

/// With TeX: spellings that typeset alike are equal, and others are not; a candidate TeX cannot
/// typeset fails with TeX's own message instead of panicking.
#[test]
#[ignore = "needs TeX Live: cargo xtask math-oracle test"]
fn tex_judges_spellings_by_what_they_typeset() {
    let Some(_tex) = tex_or_skip() else { return };
    let truth = load("truth.tsv").unwrap();
    let get = |id: &str| truth.equations.iter().find(|t| t.id == id).unwrap();
    for (id, candidate) in [
        ("frac_simple", r"\frac {a + b} c"),
        ("text_subscript", r"x_{\mathrm{max}} - x_{\mathrm{min}}"),
        ("dots_auto", r"a_1 + \cdots + a_n"),
        ("set_braces", r"\lbrace 1,2,3 \rbrace"),
        ("sup_squares@onecol_a", r"x^{2}+y^{2}=r^{2}"),
        // An operator name before a parenthesis gets no space, so plain upright letters match it.
        ("operatorname_trace", r"\mathrm{tr}(AB) = \mathrm{tr}(BA)"),
    ] {
        assert_eq!(tex::typeset_equal(get(id), candidate).unwrap(), Verdict::Equal, "{id}: {candidate}");
    }
    for (id, candidate) in [
        ("frac_simple", r"\dfrac{a+b}{c} + 0"),
        // Before a letter it gets a thin space, which upright letters do not.
        ("log_product", r"\mathrm{log}(xy) = \mathrm{log} x + \mathrm{log} y"),
        ("upright_d", r"\frac{dy}{dx}"),
        ("spaces_named", r"a\,b\,c\quad d\qquad e"),
    ] {
        assert!(matches!(tex::typeset_equal(get(id), candidate).unwrap(), Verdict::Different(_)), "{id}: {candidate}");
    }
    let Verdict::Failed(why) = tex::typeset_equal(get("frac_simple"), r"\frac{a+b}{c").unwrap() else { panic!("an unbalanced brace typesets?") };
    assert!(why.contains('!'), "{why}");
    assert!(matches!(tex::typeset_equal(get("frac_simple"), r"\undefinedmacro").unwrap(), Verdict::Failed(_)));
    assert!(matches!(tex::typeset_equal(get("frac_simple"), r"x\special{pdfcraft:eq:c0:end}").unwrap(), Verdict::Failed(_)));
}
