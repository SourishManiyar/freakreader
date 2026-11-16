//! The layout reader on the math oracle's faces: the equation list typeset in fonts the symbol table
//! does not know, read by the reader's Unicode path (`tests/data/math/oracle/README.md`, "Faces").
//!
//! The committed faces (OFL-1.1 fonts only) are read everywhere. The `#[ignore]`d tests run TeX
//! (`cargo xtask math-oracle test`) and also score the faces generated locally under
//! `target/math-oracle-faces/` (`cargo xtask math-oracle faces`), which may not be committed; a face not
//! generated here is skipped with a notice. Floors are where the reader stood when it was last worked
//! on: raise them with it, never lower one to make a number pass.

use crate::math_oracle::harness::{FaceIdentity, Pages, answers, face_outcomes, load_face, plain_font_name, region_of};
use pdfcraft_oracle::tex::faces::{FACES, Face, FaceTex, face};
use pdfcraft_oracle::tex::{EquationTruth, Mutation, Report, Verdict};
use pdfcraft_render::text::math::MathPage;
use pdfcraft_render::text::math::recover::{Confidence, LatexRecoverer, RecoverOptions, Style, recover_latex};

fn style_of(t: &EquationTruth) -> Style {
    if t.env.as_str() == "inline" { Style::Text } else { Style::Display }
}

fn committed(id: &str) -> &'static Face {
    let f = face(id).unwrap_or_else(|| panic!("no face {id}"));
    assert!(f.committed, "{id} is not committed");
    f
}

/// Every committed face is there, holds the equations it says, and embeds only the fonts it may: the
/// licence check of what is committed.
#[test]
fn committed_faces_embed_only_their_open_fonts() {
    let mut pages = Pages::default();
    for f in FACES.iter().filter(|f| f.committed) {
        let truths = load_face(f, &mut pages).unwrap().unwrap_or_else(|| panic!("committed face {} is missing", f.id));
        assert!(truths.len() >= 160, "{}: {} equations", f.id, truths.len());
        for t in &truths {
            let page = pages.get(&t.doc, t.page).unwrap();
            assert!(!page.glyphs.is_empty(), "{} {}", f.id, t.id);
            for font in 0..page.fonts.len() {
                let name = plain_font_name(page, font as u32);
                assert!(f.fonts.iter().any(|p| name.starts_with(p)), "{}: page {} embeds {name}, not one of {:?}", f.id, t.page, f.fonts);
            }
        }
    }
}

/// Equations of the STIX Two face (a development face) with the answer the reader gives, the
/// environment known: the Unicode path's mechanisms (math alphanumerics, text-font words, large
/// operators by size, accents by width, spaces without metrics). Each is confident.
const STIX_EXPECTED: [(&str, &str); 14] = [
    ("sup_squares", r"x^{2} + y^{2} = r^{2}"),
    ("frac_simple", r"\frac{a + b}{c}"),
    ("sqrt_cube", r"\sqrt[3]{u + v}"),
    ("sum_gauss", r"\sum_{i = 1}^{n}i = \frac{n(n + 1)}{2}"),
    ("int_power", r"\int_{0}^{1}x^{2}\,dx = \frac{1}{3}"),
    ("greek_lower", r"\alpha + \beta + \gamma + \delta"),
    ("hat_letters", r"\hat{x} + \hat{\theta}"),
    ("widehat", r"\widehat{xyz}"),
    ("blackboard", r"\mathbb{N} \subset \mathbb{Z} \subset \mathbb{Q} \subset \mathbb{R}"),
    ("bold_vectors", r"\mathbf{x} \cdot \mathbf{y} = \mathbf{0}"),
    ("log_product", r"\log (xy) = \log x + \log y"),
    ("sub_index", r"a_{n} + a_{n + 1} = a_{n + 2}"),
    ("lim_inline", r"\lim_{x \rightarrow 0}\frac{\sin x}{x} = 1"),
    ("frac_nested", r"\frac{1}{1 + \frac{1}{x}}"),
];

#[test]
fn the_reader_reads_the_stix_face() {
    let mut pages = Pages::default();
    let truths = load_face(committed("stix2"), &mut pages).unwrap().unwrap();
    let mut wrong = Vec::new();
    for (id, want) in STIX_EXPECTED {
        let Some(t) = truths.iter().find(|t| t.id == id) else {
            wrong.push(format!("{id}: not in the face"));
            continue;
        };
        let page = pages.get(&t.doc, t.page).unwrap();
        match recover_latex(page, region_of(t), &RecoverOptions { style: Some(style_of(t)) }) {
            Ok(r) if r.latex == want && r.confidence == Confidence::Confident => {}
            Ok(r) => wrong.push(format!("{id}: {} ({:?}; wanted {want}) {:?}", r.latex, r.confidence, r.diagnostics)),
            Err(e) => wrong.push(format!("{id}: {e}")),
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// What the Unicode path cannot settle is not confident: `\bigl` against `\left`, an italic correction
/// against a space.
#[test]
fn the_unicode_path_doubts_what_it_cannot_settle() {
    let mut pages = Pages::default();
    let truths = load_face(committed("stix2"), &mut pages).unwrap().unwrap();
    for id in ["manual_sizes", "ho_big_sizes", "int_contour", "rel_congruence"] {
        let t = truths.iter().find(|t| t.id == id).unwrap_or_else(|| panic!("{id}"));
        let page = pages.get(&t.doc, t.page).unwrap();
        if let Ok(r) = recover_latex(page, region_of(t), &RecoverOptions { style: Some(style_of(t)) }) {
            assert_eq!(r.confidence, Confidence::Tentative, "{id}: {} {:?}", r.latex, r.diagnostics);
        }
    }
}

/// The reader told the environment, its confident answers only.
struct Confident;

impl Confident {
    fn answer(page: &MathPage, t: &EquationTruth) -> (Option<String>, Option<String>) {
        match recover_latex(page, region_of(t), &RecoverOptions { style: Some(style_of(t)) }) {
            Ok(r) => {
                let confident = (r.confidence == Confidence::Confident).then(|| r.latex.clone());
                (Some(r.latex), confident)
            }
            Err(_) => (None, None),
        }
    }
}

/// A face's TeX, or a notice why it is skipped.
fn face_tex(f: &Face) -> Option<FaceTex> {
    match FaceTex::find(f) {
        Ok(t) => Some(t),
        Err(absent) => {
            assert!(!absent.required(), "{absent}");
            eprintln!("{}: {absent}", f.id);
            None
        }
    }
}

/// Floors per face, the environment told: the typeset-equal share of the best answers, and how many
/// confident answers are given (their precision is held to 0.98 in every face). Set on 2026-11-16, a
/// little under the scores in the oracle README.
const FLOORS: [(&str, f64, usize); 11] = [
    ("stix2", 0.70, 95),
    ("newtx", 0.46, 53),
    ("fira", 0.70, 75),
    ("libertinus", 0.65, 67),
    ("stix2-xe", 0.72, 99),
    ("termes", 0.70, 93),
    ("pagella", 0.71, 101),
    ("bonum", 0.65, 53),
    ("lm-math", 0.70, 78),
    ("mathptmx", 0.52, 38),
    ("txfonts", 0.53, 75),
];

#[test]
#[ignore = "needs TeX Live: cargo xtask math-oracle test"]
fn tex_holds_the_reader_to_its_scores_in_every_face() {
    let mut pages = Pages::default();
    for f in &FACES {
        let Some(truths) = load_face(f, &mut pages).unwrap() else {
            assert!(!f.committed, "committed face {} is missing", f.id);
            eprintln!("face {}: not generated here (cargo xtask math-oracle faces --only {})", f.id, f.id);
            continue;
        };
        let Some(tex) = face_tex(f) else { continue };
        // The check itself: the truth's own source typesets equal, every time.
        let identity = answers(&FaceIdentity { truths: &truths }, &truths, &mut pages).unwrap();
        let scored = face_outcomes(&truths, identity, Some(&tex)).unwrap();
        let bad: Vec<&str> = scored.iter().filter(|o| o.verdict != Some(Verdict::Equal)).map(|o| o.truth.id.as_str()).collect();
        assert!(bad.is_empty(), "{}: the truth's own source does not typeset equal: {bad:?}", f.id);
        let mut best = Vec::new();
        let mut confident = Vec::new();
        for t in &truths {
            let (b, c) = Confident::answer(pages.get(&t.doc, t.page).unwrap(), t);
            best.push(b);
            confident.push(c);
        }
        let all = Report::new(&face_outcomes(&truths, best, Some(&tex)).unwrap());
        let sure = Report::new(&face_outcomes(&truths, confident, Some(&tex)).unwrap());
        let rate = all.total.equal_rate().unwrap_or(0.0);
        let answered = sure.total.answered;
        let precision = sure.total.precision().unwrap_or(1.0);
        eprintln!("face {}: typeset-equal {rate:.3}, {answered} confident, precision {precision:.3}", f.id);
        // Wrong with confidence is the worst outcome, in every face.
        assert!(precision >= 0.98, "{}: confident precision {precision:.3}", f.id);
        if let Some((_, rate_floor, answered_floor)) = FLOORS.iter().find(|(id, ..)| *id == f.id) {
            assert!(rate >= *rate_floor, "{}: typeset-equal {rate:.3} < {rate_floor}", f.id);
            assert!(answered >= *answered_floor, "{}: {answered} confident answers < {answered_floor}", f.id);
        }
    }
}

/// The face check tells every deliberate mistake from the truth, as the main set's check does.
#[test]
#[ignore = "needs TeX Live: cargo xtask math-oracle test"]
fn tex_rejects_every_mutant_in_a_face() {
    let mut pages = Pages::default();
    let f = committed("stix2");
    let truths = load_face(f, &mut pages).unwrap().unwrap();
    let Some(tex) = face_tex(f) else { return };
    for m in Mutation::ALL {
        let identity = FaceIdentity { truths: &truths };
        let mutated: Vec<Option<String>> =
            truths.iter().map(|t| identity.recover(pages.get(&t.doc, t.page).ok()?, region_of(t)).and_then(|s| m.apply(&s))).collect();
        let applied = mutated.iter().filter(|a| a.is_some()).count();
        let scored = face_outcomes(&truths, mutated, Some(&tex)).unwrap();
        let equal: Vec<&str> = scored.iter().filter(|o| o.verdict == Some(Verdict::Equal)).map(|o| o.truth.id.as_str()).collect();
        assert!(applied > 0, "{}: applies nowhere", m.name());
        assert!(equal.is_empty(), "{}: typeset equal to the truth: {equal:?}", m.name());
    }
}
