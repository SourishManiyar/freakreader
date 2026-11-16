//! The math oracle's harness, shared by its tests (`tests/it/math_oracle.rs`) and its report
//! (`examples/math_oracle.rs`): reading the committed truth, the pages it describes, the test
//! recoverers, and scoring.
// Each of the two users needs part of this.
#![allow(dead_code)]

use pdfcraft_oracle::tex::{self, EquationTruth, Item, Mark, Mutation, Outcome, Tex, TexError, TruthSet};
use pdfcraft_render::text::math::recover::LatexRecoverer;
use pdfcraft_render::text::math::{GlyphDetail, MathPage};
use pdfcraft_render::{PageRenderer, RenderConfig};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// How far a glyph's baseline origin or a rule's edge on the page may sit from where TeX put it, in
/// bp. pdfTeX writes positions with three decimals.
pub const PLACE_TOLERANCE_BP: f64 = 0.02;

pub fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/math/oracle")
}

/// `truth.tsv` or `mutants.tsv`.
pub fn load(name: &str) -> Result<TruthSet, String> {
    let path = data_dir().join(name);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    TruthSet::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The glyph details of the oracle's PDFs, read once per page.
#[derive(Default)]
pub struct Pages {
    renderers: HashMap<String, PageRenderer>,
    cache: HashMap<(String, usize), MathPage>,
}

impl Pages {
    pub fn get(&mut self, doc: &str, page: usize) -> Result<&MathPage, String> {
        let key = (doc.to_owned(), page);
        if !self.cache.contains_key(&key) {
            if !self.renderers.contains_key(doc) {
                let path = data_dir().join(format!("{doc}.pdf"));
                let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                self.renderers.insert(doc.to_owned(), PageRenderer::new(Arc::new(bytes), RenderConfig::default()));
            }
            let renderer = self.renderers.get_mut(doc).ok_or("no renderer")?;
            let details = renderer.glyph_details(page)?;
            self.cache.insert(key.clone(), details);
        }
        self.cache.get(&key).ok_or_else(|| "no page".to_owned())
    }
}

pub fn region_of(t: &EquationTruth) -> [f32; 4] {
    t.bbox.map(|v| v as f32)
}

/// The font name of a glyph without subset prefix, upper case (`CMMI10`).
pub fn font_name(page: &MathPage, g: &GlyphDetail) -> String {
    page.fonts.get(g.font as usize).and_then(|f| f.name.clone()).unwrap_or_default().to_uppercase()
}

/// Whether the page glyph `g` is the glyph `item` of the truth: font, size, code and baseline origin.
pub fn is_glyph(page: &MathPage, g: &GlyphDetail, item: &Item) -> bool {
    let Mark::Glyph { font, size, code } = &item.mark else { return false };
    g.code == *code
        && font_name(page, g) == font.to_uppercase()
        && (f64::from(g.size) - tex::size_bp(*size)).abs() < 1e-3
        && (f64::from(g.baseline[0][0]) - tex::view_bp(item.h)).abs() <= PLACE_TOLERANCE_BP
        && (f64::from(g.baseline[0][1]) - tex::view_bp(item.v)).abs() <= PLACE_TOLERANCE_BP
}

/// The ink box TeX gives the rule `item` (`[width, height]` up from its bottom-left corner), in view
/// space.
pub fn rule_rect(item: &Item) -> Option<[f64; 4]> {
    let Mark::Rule { width, height } = item.mark else { return None };
    let (x0, y1) = (tex::view_bp(item.h), tex::view_bp(item.v));
    Some([x0, y1 - tex::size_bp(height), x0 + tex::size_bp(width), y1])
}

/// The page's glyphs whose baseline origin lies in `region`: what a recoverer handed the region
/// should read.
pub fn glyphs_in(page: &MathPage, region: [f32; 4]) -> Vec<&GlyphDetail> {
    let [x0, y0, x1, y1] = region;
    page.glyphs.iter().filter(|g| (x0..=x1).contains(&g.baseline[0][0]) && (y0..=y1).contains(&g.baseline[0][1])).collect()
}

/// Whether `found` is exactly the glyphs of `t`, one for one.
pub fn same_glyphs(page: &MathPage, t: &EquationTruth, found: &[&GlyphDetail]) -> bool {
    let glyphs: Vec<&Item> = t.items.iter().filter(|i| matches!(i.mark, Mark::Glyph { .. })).collect();
    if glyphs.len() != found.len() {
        return false;
    }
    let mut taken = vec![false; found.len()];
    glyphs.iter().all(|item| match found.iter().enumerate().position(|(k, g)| !taken[k] && is_glyph(page, g, item)) {
        Some(k) => {
            taken[k] = true;
            true
        }
        None => false,
    })
}

/// A test recoverer that answers the truth's own source. It finds the equation by what the page
/// draws in the region (every glyph of it, and nothing else, at TeX's places), so it also proves
/// the region isolates the equation; it must score 100%.
pub struct Identity<'a> {
    pub truths: &'a [EquationTruth],
}

impl LatexRecoverer for Identity<'_> {
    fn recover(&self, page: &MathPage, region: [f32; 4]) -> Option<String> {
        let found = glyphs_in(page, region);
        self.truths.iter().find(|t| region_of(t) == region && same_glyphs(page, t, &found)).map(|t| t.source.clone())
    }
}

/// A test recoverer that answers the source with one deliberate mistake (or the source when the
/// mistake does not apply). The typeset check must reject every equation the mistake applies to.
pub struct Broken<'a> {
    pub identity: Identity<'a>,
    pub mutation: Mutation,
}

impl LatexRecoverer for Broken<'_> {
    fn recover(&self, page: &MathPage, region: [f32; 4]) -> Option<String> {
        let source = self.identity.recover(page, region)?;
        Some(self.mutation.apply(&source).unwrap_or(source))
    }
}

/// The baseline: answers nothing. What the report shows while no real recoverer exists.
pub struct Nothing;

impl LatexRecoverer for Nothing {
    fn recover(&self, _: &MathPage, _: [f32; 4]) -> Option<String> {
        None
    }
}

/// What `recoverer` answers for each equation, handed its page and region.
pub fn answers(recoverer: &dyn LatexRecoverer, truths: &[EquationTruth], pages: &mut Pages) -> Result<Vec<Option<String>>, String> {
    truths.iter().map(|t| Ok(recoverer.recover(pages.get(&t.doc, t.page)?, region_of(t)))).collect()
}

/// The answers scored: edit distance always, the typeset check when `tex` is given.
pub fn outcomes<'a>(truths: &'a [EquationTruth], answers: Vec<Option<String>>, tex: Option<&Tex>) -> Result<Vec<Outcome<'a>>, TexError> {
    let verdicts = match tex {
        Some(tex) => {
            let pairs: Vec<(&EquationTruth, Option<&str>)> = truths.iter().zip(&answers).map(|(t, a)| (t, a.as_deref())).collect();
            tex.typeset_equal_all(&pairs)?.into_iter().map(Some).collect()
        }
        None => vec![None; truths.len()],
    };
    Ok(truths.iter().zip(answers).zip(verdicts).map(|((truth, answer), verdict)| Outcome { truth, answer, verdict }).collect())
}
