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
use std::path::{Path, PathBuf};
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
                // A face that may not be committed lives outside the data directory, by absolute path.
                let path = if Path::new(doc).is_absolute() { PathBuf::from(format!("{doc}.pdf")) } else { data_dir().join(format!("{doc}.pdf")) };
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

// ---- faces ------------------------------------------------------------------------------------------

use pdfcraft_oracle::tex::faces::{Face, FaceTex};
use pdfcraft_oracle::tex::{Env, Verdict};

/// Where the faces that may be committed are.
pub fn committed_faces_dir() -> PathBuf {
    data_dir().join("faces")
}

/// Where `cargo xtask math-oracle faces` writes the faces that may not be committed.
pub fn local_faces_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/math-oracle-faces")
}

/// The stem (`doc`) of a face's files, or `None` when they have not been generated here.
pub fn face_doc(face: &Face) -> Option<String> {
    if face.committed {
        let doc = format!("faces/{}", face.id);
        return data_dir().join(format!("{doc}.tsv")).exists().then_some(doc);
    }
    let stem = local_faces_dir().join(face.id);
    let tsv = stem.with_extension("tsv");
    tsv.exists().then(|| stem.to_string_lossy().into_owned())
}

/// PDF view-space bp in sp, rounded.
fn bp_sp(bp: f32) -> i64 {
    (f64::from(bp) / tex::BP_PER_PT * tex::SP_PER_PT).round() as i64
}

/// A length in bp as sp, rounded to a hundredth of a point first: sizes and rule thicknesses come out
/// of float transforms, and must compare exactly.
fn bp_sp_rounded(bp: f32) -> i64 {
    let pt = (f64::from(bp) / tex::BP_PER_PT * 100.0).round() / 100.0;
    (pt * tex::SP_PER_PT).round() as i64
}

/// A font's PDF name without subset prefix and XeTeX's `-Identity-H` suffix.
pub fn plain_font_name(page: &MathPage, font: u32) -> String {
    let name = page.fonts.get(font as usize).and_then(|f| f.name.clone()).unwrap_or_default();
    name.strip_suffix("-Identity-H").map(str::to_owned).unwrap_or(name)
}

/// Every visible glyph and rule of a page as oracle items (sp, y down), for [`tex::same_typeset`].
pub fn page_items(page: &MathPage) -> Vec<Item> {
    let mut out = Vec::new();
    for g in page.glyphs.iter().filter(|g| !g.invisible) {
        out.push(Item {
            mark: Mark::Glyph { font: plain_font_name(page, g.font), size: bp_sp_rounded(g.size), code: g.code },
            h: bp_sp(g.baseline[0][0]),
            v: bp_sp(g.baseline[0][1]),
        });
    }
    for r in &page.rules {
        let [x0, y0, x1, y1] = r.rect;
        out.push(Item { mark: Mark::Rule { width: bp_sp_rounded(x1 - x0), height: bp_sp_rounded(y1 - y0) }, h: bp_sp(x0), v: bp_sp(y1) });
    }
    out
}

/// The box of everything drawn on a page, widened by half a point: a face's page holds one equation.
pub fn page_box(page: &MathPage) -> Option<[f64; 4]> {
    let boxes = page.glyphs.iter().filter(|g| !g.invisible).flat_map(|g| [Some(g.rect), g.ink]).flatten().chain(page.rules.iter().map(|r| r.rect));
    let b = boxes.reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])])?;
    Some([f64::from(b[0]) - 0.5, f64::from(b[1]) - 0.5, f64::from(b[2]) + 0.5, f64::from(b[3]) + 0.5])
}

/// A face's equations, as truths whose box and reference come from its PDF.
pub fn load_face(face: &Face, pages: &mut Pages) -> Result<Option<Vec<EquationTruth>>, String> {
    let Some(doc) = face_doc(face) else { return Ok(None) };
    let tsv_path = if face.committed { data_dir().join(format!("{doc}.tsv")) } else { PathBuf::from(format!("{doc}.tsv")) };
    let text = std::fs::read_to_string(&tsv_path).map_err(|e| format!("{}: {e}", tsv_path.display()))?;
    let mut out = Vec::new();
    for line in text.lines().filter(|l| !l.starts_with('#') && !l.is_empty()) {
        let f: Vec<&str> = line.splitn(6, '\t').collect();
        let [page, id, category, tier, env, source] = f[..] else { return Err(format!("{}: a row that does not read: {line}", tsv_path.display())) };
        let page: usize = page.parse().map_err(|_| format!("bad page in {line}"))?;
        let math = pages.get(&doc, page)?;
        let bbox = page_box(math).ok_or_else(|| format!("{}: page {page} draws nothing", face.id))?;
        let items = page_items(math);
        out.push(EquationTruth {
            id: id.to_owned(),
            base: id.to_owned(),
            doc: doc.clone(),
            page,
            category: category.to_owned(),
            tier: tier.parse().map_err(|_| format!("bad tier in {line}"))?,
            env: Env::parse(env).ok_or_else(|| format!("bad env in {line}"))?,
            context: tex::ALONE.to_owned(),
            bbox,
            source: source.to_owned(),
            reference: items.clone(),
            items,
        });
    }
    Ok(Some(out))
}

/// Answers scored in a face: each answer typeset by the face's engine, one per page, and compared with
/// the truth's page item by item.
pub fn face_outcomes<'a>(truths: &'a [EquationTruth], answers: Vec<Option<String>>, tex: Option<&FaceTex>) -> Result<Vec<Outcome<'a>>, String> {
    let verdicts: Vec<Option<Verdict>> = match tex {
        None => vec![None; truths.len()],
        Some(tex) => {
            let entries: Vec<(Env, &str)> = truths.iter().zip(&answers).filter_map(|(t, a)| Some((t.env, a.as_deref()?))).collect();
            let set = tex.typeset(&entries).map_err(|e| e.to_string())?;
            let mut renderers: Vec<PageRenderer> =
                set.pdfs.into_iter().map(|pdf| PageRenderer::new(Arc::new(pdf), RenderConfig::default())).collect();
            let mut placed = set.placed.into_iter();
            let mut out = Vec::with_capacity(truths.len());
            for (t, a) in truths.iter().zip(&answers) {
                if a.is_none() {
                    out.push(Some(Verdict::Different("no answer".to_owned())));
                    continue;
                }
                let v = match placed.next() {
                    Some(Ok((pdf, page))) => match renderers.get_mut(pdf).map(|r| r.glyph_details(page)) {
                        Some(Ok(math)) => tex::same_typeset(&t.reference, &page_items(&math)),
                        Some(Err(e)) => Verdict::Failed(e),
                        None => Verdict::Failed("no PDF".to_owned()),
                    },
                    Some(Err(why)) => Verdict::Failed(why),
                    None => Verdict::Failed("not typeset".to_owned()),
                };
                out.push(Some(v));
            }
            out
        }
    };
    Ok(truths.iter().zip(answers).zip(verdicts).map(|((truth, answer), verdict)| Outcome { truth, answer, verdict }).collect())
}

/// A test recoverer for faces: answers the truth's source of the equation whose region it is handed
/// and whose page draws exactly what the truth's does (two pages can have the same box: `vmatrix` and
/// `Vmatrix` in a font whose bars are as wide).
pub struct FaceIdentity<'a> {
    pub truths: &'a [EquationTruth],
}

impl LatexRecoverer for FaceIdentity<'_> {
    fn recover(&self, page: &MathPage, region: [f32; 4]) -> Option<String> {
        let items = page_items(page);
        self.truths.iter().find(|t| region_of(t) == region && t.reference == items).map(|t| t.source.clone())
    }
}
