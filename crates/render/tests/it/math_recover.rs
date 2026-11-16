//! The layout reader on the math oracle's pages (`tests/data/math/oracle/README.md`).
//!
//! The tests without `#[ignore]` read only committed files: exact answers for a set of development
//! equations, and a TeX-free closeness measure over every equation. The `#[ignore]`d ones run TeX
//! (`cargo xtask math-oracle test`) and hold the scores of `cargo xtask math-oracle report` to floors;
//! CI installs no TeX Live. The floors are where the reader stood when it was last worked on: raise
//! them with it, never lower one to make a number pass.

use crate::math_oracle::harness::{Pages, load, outcomes, region_of};
use pdfcraft_oracle::tex::{EquationTruth, Report, Tex, Verdict, edit_distance};
use pdfcraft_render::text::math::recover::{Confidence, RecoverOptions, Recovered, Severity, Style, recover_latex};

fn is_held_out(t: &EquationTruth) -> bool {
    t.base.starts_with("ho_")
}

fn read(pages: &mut Pages, t: &EquationTruth, style: Option<Style>) -> Result<Recovered, String> {
    let page = pages.get(&t.doc, t.page)?;
    recover_latex(page, region_of(t), &RecoverOptions { style }).map_err(|e| e.to_string())
}

fn style_of(t: &EquationTruth) -> Option<Style> {
    Some(if t.env.as_str() == "inline" { Style::Text } else { Style::Display })
}

/// Equations of the development list with the answer the reader gives (the environment known, as a
/// caller that knows where the region came from can say), checked against what TeX makes of each
/// answer when TeX is there. These pin the mechanisms: scripts, fractions, radicals, limits, delimiters,
/// accents, spaces, alphabets, function names.
const EXPECTED: [(&str, &str); 34] = [
    ("sup_squares", r"x^{2} + y^{2} = r^{2}"),
    ("sub_index", r"a_{n} + a_{n + 1} = a_{n + 2}"),
    ("sub_sup_both", r"x_{i}^{2} + y_{jk}^{3}"),
    ("sup_tower", r"2^{2^{n}} + 1"),
    ("sub_nested", r"a_{i_{j_{k}}}"),
    ("sup_gaussian", r"e^{- \frac{(x - m)^{2}}{2s^{2}}}"),
    ("frac_simple", r"\frac{a + b}{c}"),
    ("frac_text_style", r"\frac{1}{2} + \frac{1}{3} = \frac{5}{6}"),
    ("frac_nested", r"\frac{1}{1 + \frac{1}{x}}"),
    ("sqrt_two", r"\sqrt{2}"),
    ("sqrt_cube", r"\sqrt[3]{u + v}"),
    ("sqrt_nested", r"\sqrt{1 + \sqrt{1 + \sqrt{w}}}"),
    ("sqrt_tall", r"\sqrt{\frac{a}{b}}"),
    ("sqrt_inline", r"\sqrt{n}\log n"),
    ("sum_gauss", r"\sum_{i = 1}^{n}i = \frac{n(n + 1)}{2}"),
    ("sum_inline", r"\sum_{k = 0}^{\infty}z^{k}"),
    ("prod_display", r"\prod_{j = 1}^{m}(1 + t_{j})"),
    ("union_intersection", r"\bigcup_{n \geq 1}A_{n} \subseteq \bigcap_{m}B_{m}"),
    ("int_power", r"\int_{0}^{1}x^{2}\,dx = \frac{1}{3}"),
    ("int_gaussian", r"\int_{- \infty}^{\infty}e^{- x^{2}}\,dx = \sqrt{\pi}"),
    ("int_limits", r"\int\limits_{0}^{\infty}e^{- t}\,dt"),
    ("lim_inline", r"\lim_{x \rightarrow 0}\frac{\sin x}{x} = 1"),
    ("log_product", r"\log (xy) = \log x + \log y"),
    ("abs_epsilon", r"|x - y| < \varepsilon"),
    ("norm_triangle", r"\|x + y\| \leq \|x\| + \|y\|"),
    ("hat_letters", r"\hat{x} + \hat{\theta}"),
    ("widehat", r"\widehat{xyz}"),
    ("vectors", r"\vec{v} \cdot \vec{w} = 0"),
    ("bold_vectors", r"\mathbf{x} \cdot \mathbf{y} = \mathbf{0}"),
    ("greek_lower", r"\alpha + \beta + \gamma + \delta"),
    ("arrow_map", r"f\colon X \rightarrow Y"),
    ("neg_space", r"\int\!\!\int f"),
    ("text_and", r"x \in A\text{ and }y \notin A"),
    ("spaces_named", r"a\,b\;c\quad d\qquad e"),
];

#[test]
fn the_reader_gives_the_expected_answer_for_development_equations() {
    let truth = load("truth.tsv").unwrap();
    let mut pages = Pages::default();
    let mut wrong = Vec::new();
    for (id, want) in EXPECTED {
        let t = truth.equations.iter().find(|t| t.id == id).unwrap_or_else(|| panic!("no equation {id}"));
        assert!(!is_held_out(t), "{id} is held out");
        match read(&mut pages, t, style_of(t)) {
            Ok(r) if r.latex == want => {}
            Ok(r) => wrong.push(format!("{id}: {} (wanted {want})", r.latex)),
            Err(e) => wrong.push(format!("{id}: {e}")),
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Answers whose layout is plain are confident when the environment is known; what depends on the
/// environment is not, when it is not.
#[test]
fn confidence_follows_what_the_glyphs_settle() {
    let truth = load("truth.tsv").unwrap();
    let mut pages = Pages::default();
    let get = |id: &str| truth.equations.iter().find(|t| t.id == id).unwrap();
    for id in ["sup_squares", "sqrt_cube", "int_power", "hat_letters", "bold_vectors"] {
        let r = read(&mut pages, get(id), None).unwrap();
        assert_eq!(r.confidence, Confidence::Confident, "{id}: {:?}", r.diagnostics);
    }
    // A numerator the size of the text is \frac in a display and \dfrac in text: unknown without the
    // environment, settled with it.
    let alone = read(&mut pages, get("frac_simple"), None).unwrap();
    assert_eq!(alone.confidence, Confidence::Tentative);
    assert!(alone.diagnostics.iter().any(|d| d.severity == Severity::Ambiguous && d.message.contains("environment")), "{:?}", alone.diagnostics);
    let told = read(&mut pages, get("frac_simple"), Some(Style::Display)).unwrap();
    assert_eq!((told.latex.as_str(), told.confidence), (r"\frac{a + b}{c}", Confidence::Confident));
    // And told it is inline, the same glyphs are \dfrac: `\dfrac{p}{q}` is the truth of one equation.
    let dfrac = read(&mut pages, get("dfrac_inline"), Some(Style::Text)).unwrap();
    assert_eq!(dfrac.latex, r"\dfrac{p}{q}");
}

/// What the reader cannot read it does not guess at: stacked material, tables and alignments, braces
/// and stacked symbols have no answer; none of these is answered with something wrong.
#[test]
fn what_is_not_read_is_not_answered() {
    let truth = load("truth.tsv").unwrap();
    let mut pages = Pages::default();
    for id in [
        "pmatrix_two",
        "bmatrix_identity",
        "vmatrix_det",
        "matrix_dots",
        "cases_abs",
        "cases_sign",
        "align_two",
        "align_chain",
        "gather_rows",
        "underbrace",
        "overbrace",
        "overset_def",
        "xrightarrow",
        "stackrel",
        "binom_inline",
        "sum_substack",
        "prescripts",
        "set_builder_middle",
    ] {
        let t = truth.equations.iter().find(|t| t.id == id).unwrap_or_else(|| panic!("no equation {id}"));
        let e = read(&mut pages, t, style_of(t)).expect_err(id);
        assert!(!e.is_empty(), "{id}");
    }
}

/// Without TeX: over every development equation the reader's answers are close to the source (the
/// token edit distance counts spellings such as `\to` and `\rightarrow` as differences, so this is a
/// loose check; the typeset comparison is the real one), and the answers it is confident of are closer.
#[test]
fn answers_are_close_to_the_source_without_tex() {
    let truth = load("truth.tsv").unwrap();
    let mut pages = Pages::default();
    let (mut n, mut sum, mut confident_n, mut confident_sum) = (0usize, 0usize, 0usize, 0usize);
    for t in truth.equations.iter().filter(|t| !is_held_out(t) && t.context == "alone") {
        if let Ok(r) = read(&mut pages, t, style_of(t)) {
            let d = edit_distance(&t.source, &r.latex).unwrap();
            n += 1;
            sum += d;
            if r.confidence == Confidence::Confident {
                confident_n += 1;
                confident_sum += d;
            }
        }
    }
    assert!(n >= 90, "{n} answers");
    assert!((sum as f64 / n as f64) < 9.0, "mean distance {}", sum as f64 / n as f64);
    assert!(confident_n >= 70, "{confident_n} confident answers");
    assert!((confident_sum as f64 / confident_n as f64) < 3.5, "confident mean distance {}", confident_sum as f64 / confident_n as f64);
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

/// With TeX: every answer in `EXPECTED` typesets as the source does.
#[test]
#[ignore = "needs TeX Live: cargo xtask math-oracle test"]
fn tex_confirms_the_expected_answers_typeset_like_the_source() {
    let Some(tex) = tex_or_skip() else { return };
    let truth = load("truth.tsv").unwrap();
    let mut pages = Pages::default();
    let mut pairs = Vec::new();
    for (id, _) in EXPECTED {
        let t = truth.equations.iter().find(|t| t.id == id).unwrap();
        pairs.push((t, read(&mut pages, t, style_of(t)).ok().map(|r| r.latex)));
    }
    let list: Vec<(&EquationTruth, Option<&str>)> = pairs.iter().map(|(t, a)| (*t, a.as_deref())).collect();
    for ((t, _), v) in list.iter().zip(tex.typeset_equal_all(&list).unwrap()) {
        assert_eq!(v, Verdict::Equal, "{}: {NEEDS_TEX}", t.id);
    }
}

fn score(tex: &Tex, truths: &[&EquationTruth], style: bool) -> (Report, Report) {
    let mut pages = Pages::default();
    let answers: Vec<Option<Recovered>> = truths.iter().map(|t| read(&mut pages, t, if style { style_of(t) } else { None }).ok()).collect();
    let owned: Vec<EquationTruth> = truths.iter().map(|t| (*t).clone()).collect();
    let all = outcomes(&owned, answers.iter().map(|a| a.as_ref().map(|r| r.latex.clone())).collect(), Some(tex)).unwrap();
    let confident = outcomes(
        &owned,
        answers.iter().map(|a| a.as_ref().filter(|r| r.confidence == Confidence::Confident).map(|r| r.latex.clone())).collect(),
        Some(tex),
    )
    .unwrap();
    (Report::new(&all), Report::new(&confident))
}

/// With TeX: the scores. Development equations: what the reader was worked on; held-out equations:
/// what it was not. Per category, a floor on the share typeset-equal; overall, floors on the share and
/// on the precision of the confident answers.
#[test]
#[ignore = "needs TeX Live: cargo xtask math-oracle test"]
fn tex_holds_the_reader_to_its_scores() {
    let Some(tex) = tex_or_skip() else { return };
    let truth = load("truth.tsv").unwrap();
    let dev: Vec<&EquationTruth> = truth.equations.iter().filter(|t| !is_held_out(t)).collect();
    let held: Vec<&EquationTruth> = truth.equations.iter().filter(|t| is_held_out(t)).collect();
    // Without the environment, then with it.
    for (style, dev_floor, held_floor) in [(false, 0.70, 0.78), (true, 0.70, 0.78)] {
        let (all, confident) = score(&tex, &dev, style);
        let rate = all.total.equal_rate().unwrap();
        assert!(rate >= dev_floor, "development equations typeset-equal: {rate:.3} < {dev_floor} (style given: {style}); {NEEDS_TEX}");
        // Wrong with confidence is the worst outcome: the confident answers are almost all right.
        let precision = confident.total.precision().unwrap();
        assert!(precision >= 0.98, "development confident precision {precision:.3} (style given: {style})");
        // The categories the reader is built for, each held to a floor.
        for (category, floor) in [
            ("scripts", 0.80),
            ("radicals", 0.85),
            ("operators", 0.85),
            ("integrals", 0.85),
            ("delimiters", 0.80),
            ("accents", 0.85),
            ("alphabets", 0.85),
            ("arrows", 0.85),
            ("greek", 0.80),
            ("dots_primes", 0.85),
            ("text", 0.75),
        ] {
            let t = all.by_category.get(category).unwrap_or_else(|| panic!("no category {category}"));
            let rate = t.equal_rate().unwrap();
            assert!(rate >= floor, "{category}: {rate:.3} < {floor} (style given: {style})");
        }
        let (all, confident) = score(&tex, &held, style);
        let rate = all.total.equal_rate().unwrap();
        assert!(rate >= held_floor, "held-out equations typeset-equal: {rate:.3} < {held_floor} (style given: {style})");
        let precision = confident.total.precision().unwrap();
        assert!(precision >= 0.93, "held-out confident precision {precision:.3} (style given: {style})");
    }
}
