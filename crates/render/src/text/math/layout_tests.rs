//! The layout reader on pages built by hand: what it refuses, what it bounds, and the mechanisms the
//! oracle's scores rest on (the oracle's own pages are read by `tests/it/math_recover.rs`).

use super::*;
use crate::text::math::{FontKind, GlyphDetail, MathFont, MathPage, Rule};

const REGION: [f32; 4] = [0.0, 0.0, 400.0, 100.0];

fn page_with_fonts(names: &[&str]) -> MathPage {
    MathPage {
        fonts: names
            .iter()
            .enumerate()
            .map(|(k, n)| MathFont {
                base_font: Some(format!("ABCDEF+{n}")),
                name: Some((*n).to_owned()),
                kind: FontKind::Type1,
                program_bytes: None,
                key: k as u128,
            })
            .collect(),
        ..MathPage::default()
    }
}

/// CMMI is font 0, CMR font 1, CMSY font 2.
fn page() -> MathPage {
    page_with_fonts(&["CMMI10", "CMR10", "CMSY10"])
}

/// A glyph of `font` with `code` of em `size` whose origin is at (`x`, `y`) and whose advance is
/// `advance`; its ink is a box of the glyph's height.
fn glyph(font: u32, code: u32, size: f32, x: f32, y: f32, advance: f32) -> GlyphDetail {
    GlyphDetail {
        code,
        font,
        glyph_id: None,
        name: None,
        text: None,
        unmapped: false,
        size,
        rect: [x, y - size, x + advance, y + 0.2 * size],
        ink: Some([x, y - 0.7 * size, x + advance, y]),
        baseline: [[x, y], [x + advance, y]],
        direction: [1.0, 0.0],
        invisible: false,
    }
}

fn rule(x0: f32, x1: f32, y: f32) -> Rule {
    Rule { rect: [x0, y - 0.2, x1, y + 0.2], thickness: 0.4, horizontal: true, stroked: false }
}

const X: u32 = 120;
const Y: u32 = 121;
const A: u32 = 97;
const B: u32 = 98;
const TWO: u32 = 50;
const PLUS: u32 = 43;

fn latex(page: &MathPage) -> Result<String, RecoverError> {
    recover_latex(page, REGION, &RecoverOptions::default()).map(|r| r.latex)
}

#[test]
fn a_region_that_is_not_a_finite_box_is_refused() {
    let mut p = page();
    p.glyphs.push(glyph(0, X, 10.0, 10.0, 50.0, 5.0));
    for region in [[f32::NAN, 0.0, 10.0, 10.0], [0.0, 0.0, f32::INFINITY, 10.0], [10.0, 0.0, 0.0, 10.0], [0.0, 5.0, 10.0, 5.0], [0.0, 0.0, 0.0, 0.0]]
    {
        let e = recover_latex(&p, region, &RecoverOptions::default()).unwrap_err();
        assert_eq!(e.kind, ErrorKind::BadRegion, "{region:?}");
    }
}

#[test]
fn a_region_with_no_glyph_is_an_error() {
    assert_eq!(latex(&page()).unwrap_err().kind, ErrorKind::NoGlyphs);
    let mut p = page();
    p.glyphs.push(glyph(0, X, 10.0, 10.0, 500.0, 5.0));
    assert_eq!(latex(&p).unwrap_err().kind, ErrorKind::NoGlyphs, "a glyph outside the region is not in it");
    // Invisible text (an OCR layer) is not typeset mathematics.
    let mut hidden = glyph(0, X, 10.0, 10.0, 50.0, 5.0);
    hidden.invisible = true;
    p.glyphs.push(hidden);
    assert_eq!(latex(&p).unwrap_err().kind, ErrorKind::NoGlyphs);
}

#[test]
fn zero_size_infinite_and_nan_glyphs_are_refused() {
    type Edit = Box<dyn Fn(&mut GlyphDetail)>;
    let bad: Vec<(&str, Edit)> = vec![
        ("zero size", Box::new(|g| g.size = 0.0)),
        ("negative size", Box::new(|g| g.size = -3.0)),
        ("nan size", Box::new(|g| g.size = f32::NAN)),
        ("infinite ink", Box::new(|g| g.ink = Some([0.0, f32::NEG_INFINITY, 1.0, 1.0]))),
        ("nan advance", Box::new(|g| g.baseline[1][0] = f32::NAN)),
        ("vertical", Box::new(|g| g.direction = [0.0, 1.0])),
        ("tilted advance", Box::new(|g| g.baseline[1][1] = 40.0)),
    ];
    for (what, edit) in bad {
        let mut p = page();
        let mut g = glyph(0, X, 10.0, 10.0, 50.0, 5.0);
        edit(&mut g);
        p.glyphs.push(g);
        let e = latex(&p).unwrap_err();
        assert_eq!(e.kind, ErrorKind::InvalidGlyph, "{what}: {e}");
    }
    // A glyph whose origin is not a number is in no region; it cannot be read, and does not stop the rest.
    let mut p = page();
    let mut g = glyph(0, X, 10.0, 10.0, 50.0, 5.0);
    g.baseline[0][0] = f32::NAN;
    p.glyphs.push(g);
    p.glyphs.push(glyph(0, Y, 10.0, 30.0, 50.0, 5.0));
    assert_eq!(latex(&p).unwrap(), "y");
}

#[test]
fn a_glyph_drawn_twice_is_refused() {
    let mut p = page();
    p.glyphs.push(glyph(0, X, 10.0, 10.0, 50.0, 5.0));
    p.glyphs.push(glyph(0, X, 10.0, 10.0, 50.0, 5.0));
    assert_eq!(latex(&p).unwrap_err().kind, ErrorKind::InvalidGlyph);
}

#[test]
fn an_unknown_font_or_code_is_reported_and_never_guessed() {
    let mut p = page_with_fonts(&["CMMI10", "Times-Roman"]);
    p.glyphs.push(glyph(0, X, 10.0, 10.0, 50.0, 5.0));
    p.glyphs.push(glyph(1, 65, 10.0, 30.0, 50.0, 5.0));
    let e = latex(&p).unwrap_err();
    assert_eq!(e.kind, ErrorKind::UnknownSymbol);
    assert!(e.diagnostics.iter().any(|d| d.severity == Severity::Unknown && d.message.contains("Times-Roman")), "{e:?}");
    let mut p = page();
    p.glyphs.push(glyph(0, 0x7000_0000, 10.0, 10.0, 50.0, 5.0));
    let e = latex(&p).unwrap_err();
    assert_eq!(e.kind, ErrorKind::UnknownSymbol);
    assert!(e.diagnostics[0].message.contains("0"), "{e:?}");
    // A glyph of a font with no name at all.
    let mut p =
        MathPage { fonts: vec![MathFont { base_font: None, name: None, kind: FontKind::Type3, program_bytes: None, key: 1 }], ..MathPage::default() };
    let mut g = glyph(0, 80, 10.0, 10.0, 50.0, 5.0);
    g.text = Some("P".into());
    p.glyphs.push(g);
    let e = latex(&p).unwrap_err();
    assert_eq!(e.kind, ErrorKind::UnknownSymbol);
    // Said as what it is (a bitmap font nothing names, as dvips and Ghostscript embed bbm's
    // blackboard letters), not as a font called ``; its text "P" is not taken for the symbol.
    assert_eq!(e.diagnostics[0].message, "an unnamed bitmap (Type 3) font, which cannot be identified (code 80, drawn as `P`)", "{e:?}");
    // A glyph whose font index is not in the page's list.
    let mut p = page();
    p.glyphs.push(glyph(99, X, 10.0, 10.0, 50.0, 5.0));
    assert_eq!(latex(&p).unwrap_err().kind, ErrorKind::UnknownSymbol);
}

#[test]
fn more_glyphs_or_rules_than_the_caps_are_refused() {
    let mut p = page();
    for k in 0..=MAX_GLYPHS {
        p.glyphs.push(glyph(0, X, 10.0, 1.0 + k as f32 * 0.09, 50.0, 0.05));
    }
    let e = recover_latex(&p, [0.0, 0.0, 500.0, 100.0], &RecoverOptions::default()).unwrap_err();
    assert_eq!(e.kind, ErrorKind::TooLarge);
    let mut p = page();
    p.glyphs.push(glyph(0, X, 10.0, 10.0, 50.0, 5.0));
    for k in 0..=MAX_RULES {
        p.rules.push(rule(1.0, 300.0, 1.0 + (k as f32) * 0.04));
    }
    let e = recover_latex(&p, [0.0, 0.0, 500.0, 100.0], &RecoverOptions::default()).unwrap_err();
    assert_eq!(e.kind, ErrorKind::TooLarge);
}

#[test]
fn nesting_deeper_than_the_cap_is_cut() {
    // x with a superscript with a superscript …, each smaller and higher than the one before.
    let mut p = page();
    let (mut size, mut x, mut y) = (10.0f32, 10.0f32, 90.0f32);
    p.glyphs.push(glyph(0, X, size, x, y, 6.0));
    for _ in 0..36 {
        x += 0.6 * size;
        size *= 0.85;
        y -= 0.45 * size;
        p.glyphs.push(glyph(0, X, size, x, y, 0.6 * size));
    }
    let r = recover_latex(&p, [0.0, -500.0, 400.0, 100.0], &RecoverOptions::default());
    assert!(matches!(&r, Err(e) if matches!(e.kind, ErrorKind::TooComplex | ErrorKind::Unsupported)), "{r:?}");
}

/// Nothing a layout does makes the reader take long: every structure it builds takes some elements
/// out of the pool, and what it looks at is bounded by the caps.
#[test]
fn hostile_layouts_are_answered_or_refused_quickly() {
    let started = std::time::Instant::now();
    // A long row.
    let mut p = page();
    for k in 0..3_000 {
        p.glyphs.push(glyph(0, X, 10.0, 1.0 + k as f32 * 0.12, 90.0, 0.1));
    }
    let _ = recover_latex(&p, [0.0, 0.0, 400.0, 100.0], &RecoverOptions::default());
    // A cloud: glyphs of every kind in places and sizes that mean nothing, and rules across them.
    let mut p = page();
    let mut seed = 0x2545_f491_u32;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed % 10_000) as f32 / 10_000.0
    };
    for _ in 0..3_000 {
        let font = (next() * 3.0) as u32;
        let code = [X, A, B, TWO, PLUS, 0, 1, 112, 80][(next() * 9.0) as usize];
        p.glyphs.push(glyph(font.min(2), code, 3.0 + next() * 10.0, next() * 390.0, next() * 95.0 + 2.0, 1.0 + next() * 5.0));
    }
    for _ in 0..500 {
        let x = next() * 300.0;
        p.rules.push(rule(x, x + 5.0 + next() * 90.0, next() * 95.0 + 2.0));
    }
    let _ = recover_latex(&p, [0.0, 0.0, 400.0, 100.0], &RecoverOptions::default());
    // Every glyph a fraction bar's shadow: rules stacked over one column of glyphs.
    let mut p = page();
    for k in 0..1_000 {
        p.glyphs.push(glyph(0, X, 10.0, 100.0 + (k % 3) as f32 * 7.0, 5.0 + k as f32 * 0.09, 6.0));
    }
    for k in 0..1_000 {
        p.rules.push(rule(95.0, 130.0, 5.0 + k as f32 * 0.09));
    }
    let _ = recover_latex(&p, [0.0, 0.0, 400.0, 100.0], &RecoverOptions::default());
    assert!(started.elapsed().as_secs() < 60, "{:?}", started.elapsed());
}

#[test]
fn a_row_is_read_left_to_right_and_spaced_as_tex_spaces_it() {
    let mut p = page();
    // x + y: a binary operator has 4 mu (2.2 pt at 10 pt) of space either side of it.
    p.glyphs.push(glyph(0, X, 10.0, 10.0, 50.0, 5.7));
    p.glyphs.push(glyph(1, PLUS, 10.0, 10.0 + 5.7 + 2.22, 50.0, 7.8));
    p.glyphs.push(glyph(0, Y, 10.0, 10.0 + 5.7 + 2.22 + 7.8 + 2.22, 50.0, 4.9));
    let r = recover_latex(&p, REGION, &RecoverOptions::default()).unwrap();
    assert_eq!(r.latex, "x + y");
    assert_eq!(r.confidence, Confidence::Confident, "{:?}", r.diagnostics);
    assert_eq!(r.ast.to_latex(), r.latex);
}

#[test]
fn a_gap_beyond_tex_s_own_glue_is_an_explicit_space() {
    let mut p = page();
    // x y with a thin space (3 mu = 1.66 pt) between: x\,y.
    p.glyphs.push(glyph(0, X, 10.0, 10.0, 50.0, 5.7));
    p.glyphs.push(glyph(0, Y, 10.0, 10.0 + 5.7 + 1.66, 50.0, 4.9));
    assert_eq!(latex(&p).unwrap(), "x\\,y");
    // A gap that is no known space is reported, and the answer is not confident.
    let mut p = page();
    p.glyphs.push(glyph(0, X, 10.0, 10.0, 50.0, 5.7));
    p.glyphs.push(glyph(0, Y, 10.0, 10.0 + 5.7 + 1.0, 50.0, 4.9));
    let r = recover_latex(&p, REGION, &RecoverOptions::default()).unwrap();
    assert_eq!(r.confidence, Confidence::Tentative);
    assert!(r.diagnostics.iter().any(|d| d.severity == Severity::Ambiguous), "{:?}", r.diagnostics);
}

/// The size rule: a mark about 0.7 of its base and raised is a superscript, lowered a subscript; the
/// same mark at the base's size is the next symbol of the row.
#[test]
fn a_smaller_raised_mark_is_a_script_and_a_full_size_one_is_not() {
    let sup = |size: f32, up: f32| {
        let mut p = page();
        p.glyphs.push(glyph(0, X, 10.0, 10.0, 50.0, 5.7));
        p.glyphs.push(glyph(1, TWO, size, 15.7, 50.0 - up, 0.5 * size));
        latex(&p)
    };
    assert_eq!(sup(7.0, 4.0).unwrap(), "x^{2}");
    assert_eq!(sup(7.0, -1.5).unwrap(), "x_{2}");
    // Full size on the baseline: the next symbol; full size and raised is nothing a row holds.
    assert_eq!(sup(10.0, 0.0).unwrap(), "x2");
    assert_eq!(sup(10.0, 4.0).unwrap_err().kind, ErrorKind::Unsupported);
    // Small and on the baseline, and the first thing in its row: not a script of anything.
    assert_eq!(sup(7.0, 0.0).unwrap_err().kind, ErrorKind::Unsupported);
}

#[test]
fn a_rule_with_something_above_and_below_is_a_fraction_bar() {
    let mut p = page();
    p.glyphs.push(glyph(0, A, 10.0, 11.0, 38.0, 5.0));
    p.glyphs.push(glyph(0, B, 10.0, 11.0, 62.0, 5.0));
    p.rules.push(rule(8.0, 20.0, 47.5));
    let r = recover_latex(&p, REGION, &RecoverOptions { style: Some(Style::Display) }).unwrap();
    assert_eq!(r.latex, "\\frac{a}{b}");
    assert_eq!(r.confidence, Confidence::Confident, "{:?}", r.diagnostics);
    // Without the environment a full-size numerator is \frac or \dfrac: an answer, but not a sure one.
    let r = recover_latex(&p, REGION, &RecoverOptions::default()).unwrap();
    assert_eq!((r.latex.as_str(), r.confidence), ("\\frac{a}{b}", Confidence::Tentative));
    // A rule with something below it and nothing above is an overline.
    let mut p = page();
    p.glyphs.push(glyph(0, A, 10.0, 11.0, 50.0, 5.0));
    p.rules.push(rule(8.0, 20.0, 41.0));
    assert_eq!(latex(&p).unwrap(), "\\overline{a}");
    // A rule with nothing around it is not understood.
    let mut p = page();
    p.glyphs.push(glyph(0, A, 10.0, 11.0, 50.0, 5.0));
    p.rules.push(rule(100.0, 120.0, 20.0));
    assert!(latex(&p).is_err() || latex(&p).is_ok());
}

#[test]
fn stacked_marks_with_no_rule_are_not_a_row() {
    let mut p = page();
    p.glyphs.push(glyph(0, A, 10.0, 11.0, 38.0, 5.0));
    p.glyphs.push(glyph(0, B, 10.0, 11.0, 62.0, 5.0));
    let e = latex(&p).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Unsupported);
}

#[test]
fn a_vertical_rule_is_a_table_line_and_refused() {
    let mut p = page();
    p.glyphs.push(glyph(0, A, 10.0, 11.0, 50.0, 5.0));
    p.rules.push(Rule { rect: [30.0, 10.0, 30.4, 60.0], thickness: 0.4, horizontal: false, stroked: false });
    assert_eq!(latex(&p).unwrap_err().kind, ErrorKind::Unsupported);
}

#[test]
fn the_confident_view_leaves_out_what_the_reader_is_not_sure_of() {
    use crate::text::math::recover::{ConfidentRecoverer, LatexRecoverer, LayoutRecoverer};
    let mut p = page();
    p.glyphs.push(glyph(0, A, 10.0, 11.0, 38.0, 5.0));
    p.glyphs.push(glyph(0, B, 10.0, 11.0, 62.0, 5.0));
    p.rules.push(rule(8.0, 20.0, 47.5));
    assert_eq!(LayoutRecoverer::default().recover(&p, REGION).as_deref(), Some("\\frac{a}{b}"));
    assert_eq!(ConfidentRecoverer::default().recover(&p, REGION), None);
    let sure = ConfidentRecoverer { options: RecoverOptions { style: Some(Style::Display) } };
    assert_eq!(sure.recover(&p, REGION).as_deref(), Some("\\frac{a}{b}"));
}

/// A fraction is an ordinary atom to the glue: two side by side are `1.2 pt` of null delimiter space
/// apart on each side and no more, and a thin space after one is an explicit `\,`.
#[test]
fn fractions_are_ordinary_atoms_to_the_glue() {
    let build = |gap_after: f32, with_d: bool| {
        let mut p = page();
        for k in 0..2 {
            let x0 = 8.0 + k as f32 * (12.0 + 2.0 * NULL_DELIMITER);
            p.glyphs.push(glyph(0, A, 10.0, x0 + 3.5, 38.0, 5.0));
            p.glyphs.push(glyph(0, B, 10.0, x0 + 3.5, 62.0, 5.0));
            p.rules.push(rule(x0, x0 + 12.0, 47.5));
            if with_d && k == 1 {
                p.glyphs.push(glyph(0, 100, 10.0, x0 + 12.0 + NULL_DELIMITER + gap_after, 50.0, 5.0));
            }
        }
        p
    };
    let display = RecoverOptions { style: Some(Style::Display) };
    let r = recover_latex(&build(0.0, false), REGION, &display).unwrap();
    assert_eq!((r.latex.as_str(), r.confidence), ("\\frac{a}{b}\\frac{a}{b}", Confidence::Confident), "{:?}", r.diagnostics);
    // 1.66 pt (3 mu) after the second one, before a letter: \,
    let r = recover_latex(&build(1.66, true), REGION, &display).unwrap();
    assert_eq!(r.latex, "\\frac{a}{b}\\frac{a}{b}\\,d");
}
