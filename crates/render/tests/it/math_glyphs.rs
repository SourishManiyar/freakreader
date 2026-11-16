//! Glyph-level math data (`PageRenderer::glyph_details`) against what TeX itself typeset.
//!
//! The three PDFs in `tests/data/math` are LaTeX output (pdfTeX, Computer Modern Type 1). Their
//! `.truth.tsv` lists were not read from the PDFs: `generate.sh` runs the same source to DVI and
//! lets `dvitype` list every glyph (font, character code, position) and rule as TeX placed it.

use pdfcraft_render::text::TextFilter;
use pdfcraft_render::text::math::{FontKind, GlyphDetail, MathPage};
use pdfcraft_render::{PageRenderer, RenderConfig};
use std::sync::Arc;

/// TeX points per big point: pdfTeX writes positions in bp.
const BP_PER_PT: f64 = 72.0 / 72.27;
/// The DVI origin sits one inch from the page's top-left corner (pdfTeX's `\pdfhorigin`).
const ORIGIN_BP: f64 = 72.0;
const SP_PER_PT: f64 = 65536.0;
/// How far a glyph or rule edge may sit from where TeX put it, in points. pdfTeX writes three decimals.
const TOLERANCE: f64 = 0.02;

fn bp(sp: i64) -> f64 {
    ORIGIN_BP + sp as f64 / SP_PER_PT * BP_PER_PT
}

fn size_bp(sp: i64) -> f64 {
    sp as f64 / SP_PER_PT * BP_PER_PT
}

struct TruthGlyph {
    font: String,
    code: u32,
    x: f64,
    y: f64,
}

struct TruthRule {
    /// `[x0, y0, x1, y1]` in view space.
    rect: [f64; 4],
}

fn data(name: &str) -> String {
    format!("{}/tests/data/math/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn truth(name: &str) -> (Vec<TruthGlyph>, Vec<TruthRule>) {
    let tsv = std::fs::read_to_string(data(&format!("{name}.truth.tsv"))).expect("the truth list is next to the PDF");
    let (mut glyphs, mut rules) = (Vec::new(), Vec::new());
    for line in tsv.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let n = |i: usize| f[i].parse::<i64>().expect("an integer column");
        match f[0] {
            "G" => glyphs.push(TruthGlyph { font: f[1].to_uppercase(), code: n(2) as u32, x: bp(n(3)), y: bp(n(4)) }),
            // Reference point: the bottom-left corner.
            "R" => {
                let (x0, y1) = (bp(n(1)), bp(n(2)));
                rules.push(TruthRule { rect: [x0, y1 - size_bp(n(4)), x0 + size_bp(n(3)), y1] });
            }
            other => panic!("unknown truth row {other}"),
        }
    }
    (glyphs, rules)
}

fn details(name: &str) -> MathPage {
    let bytes = std::fs::read(data(&format!("{name}.pdf"))).expect("the PDF is in tests/data/math");
    PageRenderer::new(Arc::new(bytes), RenderConfig::default()).glyph_details(0).expect("the page is read")
}

/// Every glyph TeX typeset is in the page's list, once, at its place, in its font and code, and the
/// list has nothing else; likewise the rules.
fn matches_truth(name: &str) -> MathPage {
    let page = details(name);
    let (glyphs, rules) = truth(name);
    assert!(!page.truncated && !page.capped, "{name}: nothing was cut");
    let mut taken = vec![false; page.glyphs.len()];
    for t in &glyphs {
        let found = page.glyphs.iter().enumerate().position(|(i, g)| {
            let font = page.fonts[g.font as usize].name.as_deref().unwrap_or("").to_uppercase();
            !taken[i]
                && g.code == t.code
                && font == t.font
                && (f64::from(g.baseline[0][0]) - t.x).abs() <= TOLERANCE
                && (f64::from(g.baseline[0][1]) - t.y).abs() <= TOLERANCE
        });
        match found {
            Some(i) => taken[i] = true,
            None => panic!("{name}: TeX set {} code {} at ({:.3}, {:.3}); the page lists no such glyph", t.font, t.code, t.x, t.y),
        }
    }
    assert_eq!(page.glyphs.len(), glyphs.len(), "{name}: the page lists glyphs TeX did not set");
    let mut taken = vec![false; page.rules.len()];
    for t in &rules {
        let found = page
            .rules
            .iter()
            .enumerate()
            .position(|(i, r)| !taken[i] && r.rect.iter().zip(t.rect).all(|(a, b)| (f64::from(*a) - b).abs() <= TOLERANCE));
        match found {
            Some(i) => taken[i] = true,
            None => panic!("{name}: TeX set a rule {:?}; the page lists none like it ({:?})", t.rect, page.rules),
        }
    }
    assert_eq!(page.rules.len(), rules.len(), "{name}: the page lists rules TeX did not set: {:?}", page.rules);
    page
}

/// The glyph of `font` (without subset prefix) and `code`, at the `n`th place it is drawn.
fn glyph<'a>(page: &'a MathPage, font: &str, code: u32, n: usize) -> &'a GlyphDetail {
    page.glyphs
        .iter()
        .filter(|g| g.code == code && page.fonts[g.font as usize].name.as_deref() == Some(font))
        .nth(n)
        .unwrap_or_else(|| panic!("no glyph {font} code {code} (#{n})"))
}

fn names<'a>(page: &'a MathPage, font: &str, code: u32) -> Vec<Option<&'a str>> {
    page.glyphs.iter().filter(|g| g.code == code && page.fonts[g.font as usize].name.as_deref() == Some(font)).map(|g| g.name.as_deref()).collect()
}

/// The 10 pt of TeX, in the bp the page is measured in.
const TEN_PT: f32 = (10.0 * BP_PER_PT) as f32;

#[test]
fn scripts_and_fractions_match_what_tex_typeset() {
    let page = matches_truth("scripts_fractions");
    // α is CMMI's code 11. The font is the embedded Type 1 program, found by its name without the
    // subset prefix.
    let alpha = glyph(&page, "CMMI10", 11, 0);
    assert_eq!((alpha.name.as_deref(), alpha.text.as_deref(), alpha.unmapped), (Some("alpha"), Some("α"), false));
    assert!((alpha.size - TEN_PT).abs() < 1e-3, "{}", alpha.size);
    // The superscript 2 of x² is set in CMR7, at seven points, and raised above the baseline of x.
    let (x, two) = (glyph(&page, "CMMI10", 120, 0), glyph(&page, "CMR7", 50, 0));
    assert!((two.size - (7.0 * BP_PER_PT) as f32).abs() < 1e-3, "{}", two.size);
    assert!(two.baseline[0][1] < x.baseline[0][1] - 3.0, "x² is raised: {:?} over {:?}", two.baseline, x.baseline);
    // Ink against the font's TeX metrics (cmmi10.tfm: x is 0.430555 em high with no depth; y is as
    // high with a depth of 0.194445 em): the outline's own bounds sit within an overshoot of them.
    let em = f64::from(x.size);
    let (top, bottom) = (f64::from(x.baseline[0][1] - x.ink.unwrap()[1]), f64::from(x.ink.unwrap()[3] - x.baseline[0][1]));
    assert!((top / em - 0.430555).abs() < 0.03 && bottom.abs() / em < 0.03, "x ink {:?} on baseline {:?}", x.ink, x.baseline);
    let y = glyph(&page, "CMMI10", 121, 0);
    let depth = f64::from(y.ink.unwrap()[3] - y.baseline[0][1]);
    assert!((depth / em - 0.194445).abs() < 0.03, "y hangs {depth} below its baseline");
    // The one fraction bar of the display is a rule, and so is nothing else on the page.
    assert_eq!((page.rules.len(), page.rules[0].horizontal), (1, true));
    for font in &page.fonts {
        assert_eq!(font.kind, FontKind::Type1, "{font:?}");
        assert!(font.program_bytes.is_some_and(|n| n > 1000), "{font:?}");
        assert!(font.base_font.as_deref().is_some_and(|b| b.contains('+') && b.ends_with(font.name.as_deref().unwrap_or("?"))), "{font:?}");
    }
}

#[test]
fn delimiters_and_radicals_match_what_tex_typeset() {
    let page = matches_truth("delimiters_radicals");
    // CMEX's extensible parenthesis: the tall matrix is closed by pieces (top 0x30, extension 0x42,
    // bottom 0x40 on the left; 0x31, 0x43, 0x41 on the right). They have names but, as glyphs of a
    // symbol font, Unicode only in the private-use area.
    assert_eq!(names(&page, "CMEX10", 48), [Some("parenlefttp")]);
    assert_eq!(names(&page, "CMEX10", 64), [Some("parenleftbt")]);
    assert_eq!(names(&page, "CMEX10", 66), vec![Some("parenleftex"); 6]);
    assert_eq!(names(&page, "CMEX10", 67), vec![Some("parenrightex"); 6]);
    // The stack of extension pieces is one column: the same left edge, each below the one before.
    let ext: Vec<&GlyphDetail> = page.glyphs.iter().filter(|g| g.code == 66).collect();
    assert!(ext.windows(2).all(|w| w[1].baseline[0][1] > w[0].baseline[0][1] && w[1].baseline[0][0] == w[0].baseline[0][0]));
    // Radical signs: the big one is CMEX 0x70, the small CMSY 0x70, each under a rule that is a
    // fraction-bar-thin line (0.398 bp, TeX's \fontdimen8 of the extension font).
    assert_eq!(names(&page, "CMEX10", 112), [Some("radicalbig")]);
    assert_eq!(names(&page, "CMSY10", 112), [Some("radical")]);
    assert!(page.rules.iter().all(|r| (r.thickness - 0.398).abs() < 0.01 && r.horizontal && r.stroked), "{:?}", page.rules);
    // The big operators of the display: ∑ as CMEX 0x50, ∫ as 0x5A (display size), ∏ as 0x59.
    assert_eq!(names(&page, "CMEX10", 80), [Some("summationtext")]);
    assert_eq!(names(&page, "CMEX10", 90), [Some("integraldisplay")]);
    assert_eq!(names(&page, "CMEX10", 89), [Some("productdisplay")]);
    // The ink of a big operator is much taller than a letter's, and inside the em box.
    let integral = glyph(&page, "CMEX10", 90, 0);
    let ink = integral.ink.expect("∫ has ink");
    assert!(ink[3] - ink[1] > 2.0 * integral.size, "{ink:?} is {} tall", ink[3] - ink[1]);
}

#[test]
fn symbols_and_arrays_match_what_tex_typeset() {
    let page = matches_truth("symbols_array");
    // CMSY: ≤ is 0x14, → 0x21, ∞ 0x31; CMMI ∂ is 0x40; the AMS blackboard R and Euler g come from
    // their own fonts.
    let expect =
        [("CMSY10", 20, "lessequal", "≤"), ("CMSY10", 33, "arrowright", "→"), ("CMSY10", 49, "infinity", "∞"), ("CMMI10", 64, "partialdiff", "∂")];
    for (font, code, name, text) in expect {
        let g = glyph(&page, font, code, 0);
        assert_eq!((g.name.as_deref(), g.text.as_deref(), g.unmapped), (Some(name), Some(text), false), "{font} {code}");
    }
    assert!(page.fonts.iter().any(|f| f.name.as_deref() == Some("MSBM10")) && page.fonts.iter().any(|f| f.name.as_deref() == Some("EUFM10")));
    // \hline and the column rules are rules: three horizontal lines and three vertical ones, plus the
    // lines the array's own rule widths make at each crossing.
    let (horizontal, vertical) = (page.rules.iter().filter(|r| r.horizontal).count(), page.rules.iter().filter(|r| !r.horizontal).count());
    assert!(horizontal >= 3 && vertical >= 3, "{horizontal} horizontal, {vertical} vertical: {:?}", page.rules);
}

/// The text layer is not changed by any of this: the glyphs it keeps are the glyphs of the richer
/// record that have Unicode, with the same em boxes and text.
#[test]
fn the_richer_record_sits_beside_the_text_layer() {
    for name in ["scripts_fractions", "delimiters_radicals", "symbols_array"] {
        let bytes = std::fs::read(data(&format!("{name}.pdf"))).unwrap();
        let mut renderer = PageRenderer::new(Arc::new(bytes), RenderConfig::default());
        let page = renderer.glyph_details(0).unwrap();
        let text = renderer.filtered_text(0, TextFilter::default()).unwrap();
        let mut mine: Vec<(String, [u32; 4])> =
            page.glyphs.iter().filter(|g| !g.unmapped).map(|g| (g.text.clone().unwrap(), g.rect.map(f32::to_bits))).collect();
        let mut theirs: Vec<(String, [u32; 4])> = text.glyphs.iter().map(|g| (g.text.clone(), g.rect.map(f32::to_bits))).collect();
        mine.sort();
        theirs.sort();
        assert_eq!(mine, theirs, "{name}");
    }
}
