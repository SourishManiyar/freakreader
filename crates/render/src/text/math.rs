//! Glyph-level page data for reading mathematics.
//!
//! [`TextGlyph`](super::TextGlyph) keeps a Unicode string and an em box, which is all prose needs.
//! Typeset mathematics is read by symbol: Computer Modern's CMMI, CMSY and CMEX name their
//! symbols by character code, extensible delimiters and radical pieces have no useful Unicode,
//! and sub- and superscripts, fraction bars and big operators are told apart by the ink a glyph
//! really covers, its size and its baseline. [`GlyphDetail`] records that for every glyph drawn,
//! and [`Rule`] records the thin filled rectangles and lines that fraction bars, radical
//! overlines and table lines are made of. Both come in content order, beside the text layout
//! (which is built for prose reading order and stays as it is), in one [`MathPage`].
//!
//! Unlike the text layout this keeps glyphs that have no Unicode (flagged
//! [`unmapped`](GlyphDetail::unmapped)) and takes no account of `/ActualText`, exclusions or
//! reading order: it reports what the content stream draws. A page that draws more than the caps
//! below allow is cut and says so ([`MathPage::capped`]).

use super::device::{MAX_GLYPH_EXTENT, ligature_letters, place};
use super::{
    Affine, BezPath, BlendMode, ClipPath, Context, Device, Glyph, GlyphDrawMode, HashMap, Image, InterpreterCache, InterpreterSettings,
    InterpreterWarning, Paint, PathDrawMode, Pdf, Rect, SoftMask, TransformExt, interpret_page,
};
use hayro::hayro_interpret::font::FontProgramKind;
use kurbo::{Point, Shape};

/// Most glyphs recorded for one page. A dense page of prose has a few tens of thousands; a hostile
/// content stream can draw millions from a few bytes.
pub const MAX_MATH_GLYPHS: usize = 100_000;
/// Most rules recorded for one page.
pub const MAX_MATH_RULES: usize = 20_000;
/// Most distinct fonts recorded for one page.
pub const MAX_MATH_FONTS: usize = 2_048;
/// Most Type 3 glyphs whose procedure is run to measure its ink on one page. Past it a Type 3
/// glyph has no [`ink`](GlyphDetail::ink).
pub const MAX_TYPE3_INK: usize = 20_000;
/// Longest glyph or font name kept, in bytes (cut at a character boundary).
pub const MAX_NAME_BYTES: usize = 128;
/// Longest Unicode text kept for one glyph, in bytes (cut at a character boundary).
pub const MAX_TEXT_BYTES: usize = 64;
/// The thickest line or rectangle, in view-space points, that is a rule rather than a shape.
pub const MAX_RULE_THICKNESS: f64 = 4.0;
/// A rule is at least this many times as long as it is thick.
const MIN_RULE_ASPECT: f64 = 2.0;
/// A filled rectangle thinner than this draws nothing worth a rule (view-space points).
const MIN_RULE_THICKNESS: f64 = 1e-3;
/// How far a corner may sit from its place in an axis-aligned rectangle (view-space points).
const CORNER_TOLERANCE: f64 = 0.01;

/// The caps on what one page records. [`Caps::default`] holds the `MAX_MATH_*` constants; tests
/// lower them to reach a cap without drawing a hundred thousand glyphs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Caps {
    pub(crate) glyphs: usize,
    pub(crate) rules: usize,
    pub(crate) fonts: usize,
    pub(crate) type3_ink: usize,
}

impl Default for Caps {
    fn default() -> Self {
        Caps { glyphs: MAX_MATH_GLYPHS, rules: MAX_MATH_RULES, fonts: MAX_MATH_FONTS, type3_ink: MAX_TYPE3_INK }
    }
}

#[cfg(test)]
thread_local! {
    /// Makes `draw_glyph` panic on this thread when the page has this many glyphs recorded, to show
    /// that a panic inside the interpreter comes back as an error.
    pub(super) static PANIC_AT_GLYPH: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// What a font's program is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontKind {
    /// An embedded Type 1 program (`/FontFile`): what pdfTeX embeds for Computer Modern.
    Type1,
    /// An embedded bare CFF program (`/FontFile3`).
    Cff,
    /// An embedded TrueType or OpenType font, as XeTeX and LuaTeX embed OpenType math fonts.
    OpenType,
    /// Not embedded: a standard face, or the substitute for a missing font.
    Standard,
    /// A Type 3 font: its glyphs are drawn by content streams (dvips bitmap fonts).
    Type3,
}

impl FontKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FontKind::Type1 => "type1",
            FontKind::Cff => "cff",
            FontKind::OpenType => "opentype",
            FontKind::Standard => "standard",
            FontKind::Type3 => "type3",
        }
    }
}

/// One font used on the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MathFont {
    /// The font dictionary's `/BaseFont` as written, subset prefix included (`ABCDEF+CMMI10`).
    /// For a Type 0 font, the descendant's. A Type 3 font has none; its `/Name` stands in.
    pub base_font: Option<String>,
    /// `base_font` without the six-letter subset prefix (`CMMI10`, `STIXTwoMath-Regular`): the
    /// name that says which symbol table applies.
    pub name: Option<String>,
    pub kind: FontKind,
    /// Size in bytes of the embedded font program; `None` when there is none to read.
    pub program_bytes: Option<usize>,
    /// Identifies the font within the document (hayro's cache key); glyphs of one font share it.
    pub key: u128,
}

/// One glyph drawn on the page. All geometry is in page view space: points, origin at the top-left
/// of the displayed page, rotation applied, y down.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphDetail {
    /// The character code the content stream showed: a byte for a simple font (CMMI's `0x0B` is
    /// α), the CMap's code for a Type 0 font.
    pub code: u32,
    /// The font, as an index into [`MathPage::fonts`].
    pub font: u32,
    /// The glyph's index in the embedded font program; `None` for a Type 3 glyph.
    pub glyph_id: Option<u32>,
    /// The glyph's name as the font's encoding gives it for the code (`alpha`, `summationtext`,
    /// `radicalbig`); for a CFF program with no entry for the code, its charset's name. `None`
    /// when the font names no glyph for it (a Type 0 font has none).
    pub name: Option<String>,
    /// The glyph's Unicode text as the text layer reads it (`ToUnicode`, the glyph name, naming
    /// conventions); `None` when there is none. Text that is only control characters counts as none.
    pub text: Option<String>,
    /// The glyph has no Unicode text. Math fonts leave many symbols so (CMEX's extensible
    /// delimiter pieces, radical pieces); the text layer drops them, this keeps them.
    pub unmapped: bool,
    /// The font size in view-space points: the length of one em, from the transform that places
    /// the glyph (text matrix, font size and page transform together).
    pub size: f32,
    /// The em box as the text layer boxes the glyph (`TextGlyph::rect`): `[x0, y0, x1, y1]`.
    pub rect: [f32; 4],
    /// The box the glyph's ink really covers, `[x0, y0, x1, y1]`: the exact bounds of its outline
    /// (of what its procedure paints, for a Type 3 glyph). `None` for a glyph that draws nothing
    /// (a space) and for a Type 3 glyph past [`MAX_TYPE3_INK`].
    pub ink: Option<[f32; 4]>,
    /// The glyph's baseline: from its origin to the end of its advance, along the glyph's own
    /// horizontal axis as drawn.
    pub baseline: [[f32; 2]; 2],
    /// Unit vector of the writing direction (`[1, 0]`: left to right).
    pub direction: [f32; 2],
    /// The glyph is drawn in text rendering mode 3 (invisible): an OCR layer, not typesetting.
    pub invisible: bool,
}

/// A rule: a thin filled rectangle or a single straight line, axis-aligned on the page. Fraction
/// bars, radical overlines, table and array lines are drawn so, not as glyphs.
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    /// The ink, `[x0, y0, x1, y1]` in view space (a stroke is widened by half its width each side).
    pub rect: [f32; 4],
    /// The rule's thickness: its thinner side.
    pub thickness: f32,
    /// The rule runs left to right (its box is at least as wide as tall).
    pub horizontal: bool,
    /// Drawn with a stroke (`m l S`) rather than a fill (`re f`).
    pub stroked: bool,
}

/// What one page draws, for reading its mathematics.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MathPage {
    /// The fonts the glyphs use.
    pub fonts: Vec<MathFont>,
    /// Every glyph drawn within the page's crop box, in content order.
    pub glyphs: Vec<GlyphDetail>,
    /// Every rule drawn within the crop box, in content order.
    pub rules: Vec<Rule>,
    /// The page's content was cut at the renderer's limits (#623): what the cut part drew is missing.
    pub truncated: bool,
    /// More glyphs, rules or fonts were drawn than the caps allow ([`MAX_MATH_GLYPHS`],
    /// [`MAX_MATH_RULES`], [`MAX_MATH_FONTS`]); the rest are left out.
    pub capped: bool,
}

/// `s` cut to at most `max` bytes, at a character boundary.
fn capped_string(s: &str, max: usize) -> String {
    s[..s.floor_char_boundary(max.min(s.len()))].to_string()
}

/// `name` without the six-capital-letter subset prefix (`ABCDEF+CMMI10`).
fn without_subset_prefix(name: &str) -> &str {
    match name.split_once('+') {
        Some((prefix, rest)) if prefix.len() == 6 && prefix.bytes().all(|b| b.is_ascii_uppercase()) => rest,
        _ => name,
    }
}

fn finite_rect(r: Rect) -> Option<[f32; 4]> {
    let out = [r.x0 as f32, r.y0 as f32, r.x1 as f32, r.y1 as f32];
    out.iter().all(|v| v.is_finite()).then_some(out)
}

/// The exact bounds of `path` drawn with `transform`, or `None` for a path that covers no area and
/// no length.
fn exact_box(path: &BezPath, transform: Affine) -> Option<Rect> {
    if path.elements().is_empty() {
        return None;
    }
    let b = (transform * path.clone()).bounding_box();
    let finite = b.x0.is_finite() && b.y0.is_finite() && b.x1.is_finite() && b.y1.is_finite();
    (finite && (b.width() > 0.0 || b.height() > 0.0)).then_some(b)
}

/// Measures the ink of a Type 3 glyph by running its procedure: the union of what it paints.
#[derive(Default)]
struct InkDevice {
    ink: Option<Rect>,
}

impl InkDevice {
    fn add(&mut self, b: Rect) {
        self.ink = Some(self.ink.map_or(b, |i| i.union(b)));
    }
}

impl<'a> Device<'a> for InkDevice {
    fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _: BlendMode) {}
    fn draw_path(&mut self, path: &BezPath, transform: Affine, _: &Paint<'a>, _: &PathDrawMode) {
        if let Some(b) = exact_box(path, transform) {
            self.add(b);
        }
    }
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_glyph(&mut self, _: &Glyph<'a>, _: Affine, _: Affine, _: &Paint<'a>, _: &GlyphDrawMode) {}
    fn draw_image(&mut self, image: Image<'a, '_>, transform: Affine) {
        // The transform places the picture's pixels, not a unit square.
        let (w, h) = (f64::from(image.width()), f64::from(image.height()));
        let corners = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)].map(|(x, y)| transform * Point::new(x, y));
        if corners.iter().all(|p| p.is_finite()) {
            let [a, b, c, d] = corners;
            self.add(Rect::from_points(a, c).union_pt(b).union_pt(d));
        }
    }
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

/// Records every glyph and rule a page draws.
struct MathDevice {
    /// Visible page bounds in view space (`intersected_crop_box` after `initial_transform`).
    page: Rect,
    page_out: MathPage,
    font_index: HashMap<(u128, bool), u32>,
    caps: Caps,
    /// Type 3 procedures run for their ink so far.
    type3_ink: usize,
}

impl MathDevice {
    /// The index of `glyph`'s font in `fonts`, adding it; `None` past [`MAX_MATH_FONTS`].
    fn font_of(&mut self, glyph: &Glyph<'_>) -> Option<u32> {
        let type3 = matches!(glyph, Glyph::Type3(_));
        let key = (glyph.font_cache_key(), type3);
        if let Some(&i) = self.font_index.get(&key) {
            return Some(i);
        }
        let Some(index) = u32::try_from(self.page_out.fonts.len()).ok().filter(|_| self.page_out.fonts.len() < self.caps.fonts) else {
            self.page_out.capped = true;
            return None;
        };
        let (kind, program_bytes) = match glyph.font_program() {
            None => (FontKind::Type3, None),
            Some(program) => {
                let kind = match program.kind {
                    FontProgramKind::Type1 => FontKind::Type1,
                    FontProgramKind::Cff => FontKind::Cff,
                    FontProgramKind::OpenType => FontKind::OpenType,
                    FontProgramKind::Standard => FontKind::Standard,
                };
                (kind, program.data.map(|d| d.as_ref().as_ref().len()))
            }
        };
        let base_font = glyph.base_font().map(|n| capped_string(n, MAX_NAME_BYTES));
        let name = base_font.as_deref().map(without_subset_prefix).map(str::to_string);
        self.page_out.fonts.push(MathFont { base_font, name, kind, program_bytes, key: key.0 });
        self.font_index.insert(key, index);
        Some(index)
    }

    /// The exact ink of a Type 3 glyph, by running its procedure; `None` past the budget.
    fn type3_ink<'a>(&mut self, glyph: &Glyph<'a>, transform: Affine, glyph_transform: Affine, paint: &Paint<'a>) -> Option<Rect> {
        let Glyph::Type3(g) = glyph else { return None };
        if self.type3_ink >= self.caps.type3_ink {
            return None;
        }
        self.type3_ink += 1;
        let mut dev = InkDevice::default();
        g.interpret(&mut dev, transform, glyph_transform, paint);
        dev.ink
    }

    fn push_rule(&mut self, rect: Rect, thickness: f64, stroked: bool) {
        // Wholly outside the crop box: not on the page.
        if rect.x0.max(self.page.x0) > rect.x1.min(self.page.x1) || rect.y0.max(self.page.y0) > rect.y1.min(self.page.y1) {
            return;
        }
        let Some(rect_f) = finite_rect(rect) else { return };
        if self.page_out.rules.len() >= self.caps.rules {
            self.page_out.capped = true;
            return;
        }
        self.page_out.rules.push(Rule { rect: rect_f, thickness: thickness as f32, horizontal: rect.width() >= rect.height(), stroked });
    }

    /// Finish one subpath of a path drawn with `mode`: a rule if it is one.
    fn subpath(&mut self, points: &[Point], closed: bool, transform: Affine, mode: &PathDrawMode) {
        // A filled `re` comes back to its start by itself; so may an explicit fourth line.
        let mut points = points;
        if let [first, .., last] = points
            && points.len() == 5
            && (first.x - last.x).abs() < f64::EPSILON
            && (first.y - last.y).abs() < f64::EPSILON
        {
            points = points.get(..4).unwrap_or(points);
        }
        let view: Vec<Point> = points.iter().map(|p| transform * *p).collect();
        if view.iter().any(|p| !p.is_finite()) {
            return;
        }
        match mode {
            PathDrawMode::Fill(_) => {
                let [a, b, c, d] = view[..] else { return };
                let bounds = Rect::from_points(a, c).union_pt(b).union_pt(d);
                let (w, h) = (bounds.width(), bounds.height());
                let (thickness, length) = (w.min(h), w.max(h));
                // The four points are the four distinct corners of their bounding box.
                let corner = |p: &Point| {
                    let x = if (p.x - bounds.x0).abs() <= CORNER_TOLERANCE {
                        0
                    } else if (p.x - bounds.x1).abs() <= CORNER_TOLERANCE {
                        1
                    } else {
                        return None;
                    };
                    let y = if (p.y - bounds.y0).abs() <= CORNER_TOLERANCE {
                        0
                    } else if (p.y - bounds.y1).abs() <= CORNER_TOLERANCE {
                        1
                    } else {
                        return None;
                    };
                    Some(x + 2 * y)
                };
                let mut seen = [false; 4];
                for p in &view {
                    match corner(p) {
                        Some(k) => seen[k] = true,
                        None => return,
                    }
                }
                let thin = (MIN_RULE_THICKNESS..=MAX_RULE_THICKNESS).contains(&thickness) && length >= MIN_RULE_ASPECT * thickness;
                if seen.iter().all(|s| *s) && thin {
                    self.push_rule(bounds, thickness, false);
                }
            }
            PathDrawMode::Stroke(props) => {
                let [a, b] = view[..] else { return };
                if closed || !props.dash_array.is_empty() || !props.line_width.is_finite() || props.line_width < 0.0 {
                    return;
                }
                let (dx, dy) = ((b.x - a.x).abs(), (b.y - a.y).abs());
                let length = dx.max(dy);
                // Axis-aligned, and long enough to be a line: a point stroked draws a dot, if anything.
                if dx.min(dy) > CORNER_TOLERANCE || length < MIN_RULE_THICKNESS {
                    return;
                }
                // The line's width is in user space; one uniform scale stands for the transform.
                let thickness = f64::from(props.line_width) * transform.determinant().abs().sqrt();
                if !thickness.is_finite() || thickness > MAX_RULE_THICKNESS || length < MIN_RULE_ASPECT * thickness {
                    return;
                }
                let line = Rect::from_points(a, b);
                let bounds = if dx >= dy {
                    Rect::new(line.x0, line.y0 - thickness / 2.0, line.x1, line.y1 + thickness / 2.0)
                } else {
                    Rect::new(line.x0 - thickness / 2.0, line.y0, line.x1 + thickness / 2.0, line.y1)
                };
                self.push_rule(bounds, thickness, true);
            }
        }
    }
}

impl<'a> Device<'a> for MathDevice {
    fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _: BlendMode) {}
    fn draw_path(&mut self, path: &BezPath, transform: Affine, _: &Paint<'a>, mode: &PathDrawMode) {
        // Each subpath is judged alone: `re` may repeat before one `f`. A curve, or more points
        // than a rectangle has, make a subpath no rule (it is read to its end all the same).
        let mut points: Vec<Point> = Vec::with_capacity(5);
        let (mut closed, mut bad) = (false, false);
        let flush = |dev: &mut Self, points: &mut Vec<Point>, closed: &mut bool, bad: &mut bool| {
            if !*bad && !points.is_empty() {
                dev.subpath(points, *closed, transform, mode);
            }
            points.clear();
            (*closed, *bad) = (false, false);
        };
        for el in path.elements() {
            match *el {
                kurbo::PathEl::MoveTo(p) => {
                    flush(self, &mut points, &mut closed, &mut bad);
                    points.push(p);
                }
                kurbo::PathEl::LineTo(p) => {
                    if points.len() >= 5 {
                        bad = true;
                    } else {
                        points.push(p);
                    }
                }
                kurbo::PathEl::ClosePath => closed = true,
                kurbo::PathEl::QuadTo(..) | kurbo::PathEl::CurveTo(..) => bad = true,
            }
        }
        flush(self, &mut points, &mut closed, &mut bad);
    }
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_glyph(&mut self, glyph: &Glyph<'a>, transform: Affine, glyph_transform: Affine, paint: &Paint<'a>, mode: &GlyphDrawMode) {
        let t = transform * glyph_transform;
        let placed = place(glyph, t);
        let b = placed.bounds;
        // The text layer's own tests: a glyph with a damaged transform, a box too big to be text, or
        // wholly outside the crop box is not on the page.
        let outside_crop = b.x0.max(self.page.x0) >= b.x1.min(self.page.x1) || b.y0.max(self.page.y0) >= b.y1.min(self.page.y1);
        if !(b.x0.is_finite() && b.y0.is_finite() && b.x1.is_finite() && b.y1.is_finite())
            || b.width() > MAX_GLYPH_EXTENT
            || b.height() > MAX_GLYPH_EXTENT
            || outside_crop
        {
            return;
        }
        if self.page_out.glyphs.len() >= self.caps.glyphs {
            self.page_out.capped = true;
            return;
        }
        #[cfg(test)]
        if PANIC_AT_GLYPH.with(|p| p.get()) == Some(self.page_out.glyphs.len()) {
            panic!("injected panic in the math device");
        }
        let c = t.as_coeffs();
        let size = (c[2].hypot(c[3]) * 1000.0) as f32;
        let length = placed.dx.hypot(placed.dy);
        if !(size.is_finite() && length.is_finite() && length > 0.0) {
            return;
        }
        let Some(font) = self.font_of(glyph) else { return };
        let Some(rect) = finite_rect(b) else { return };
        let text = glyph.as_unicode().map(|u| match u {
            hayro::hayro_interpret::hayro_cmap::BfString::Char(c) => c.to_string(),
            hayro::hayro_interpret::hayro_cmap::BfString::String(s) => s,
        });
        // A control character is no symbol: text of only those is none, as the text layer drops it.
        let text = text.filter(|s| !s.chars().all(char::is_control)).map(|s| capped_string(&ligature_letters(s), MAX_TEXT_BYTES));
        let ink = match glyph {
            Glyph::Outline(g) => exact_box(&g.outline(), t),
            Glyph::Type3(_) => self.type3_ink(glyph, transform, glyph_transform, paint),
        }
        .and_then(finite_rect);
        let (origin, end) = (t * Point::new(0.0, 0.0), t * Point::new(placed.advance, 0.0));
        let baseline = [[origin.x as f32, origin.y as f32], [end.x as f32, end.y as f32]];
        if !baseline.iter().flatten().all(|v| v.is_finite()) {
            return;
        }
        self.page_out.glyphs.push(GlyphDetail {
            code: glyph.char_code(),
            font,
            glyph_id: match glyph {
                Glyph::Outline(g) => Some(g.glyph_id().to_u32()),
                Glyph::Type3(_) => None,
            },
            name: glyph.glyph_name().map(|n| capped_string(&n, MAX_NAME_BYTES)),
            unmapped: text.is_none(),
            text,
            size,
            rect,
            ink,
            baseline,
            direction: [(placed.dx / length) as f32, (placed.dy / length) as f32],
            invisible: matches!(mode, GlyphDrawMode::Invisible),
        });
    }
    fn draw_image(&mut self, _: Image<'a, '_>, _: Affine) {}
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

/// Every glyph and rule of one page, in content order. Panics inside the interpreter are the
/// caller's to catch.
pub(crate) fn extract_math_page(pdf: &Pdf, page: usize, settings: &InterpreterSettings) -> Result<MathPage, String> {
    extract_math_page_with(pdf, page, settings, Caps::default())
}

pub(crate) fn extract_math_page_with(pdf: &Pdf, page: usize, settings: &InterpreterSettings, caps: Caps) -> Result<MathPage, String> {
    let pages = pdf.pages();
    let p = pages.get(page).ok_or("the page could not be read")?;
    let (w, h) = p.render_dimensions();
    let cache = InterpreterCache::new();
    // The interpreter says when the content was cut at its limits (#623).
    let cut = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let settings = {
        let flag = cut.clone();
        let inner = settings.warning_sink.clone();
        InterpreterSettings {
            warning_sink: std::sync::Arc::new(move |warning| {
                if matches!(warning, InterpreterWarning::ContentTruncated) {
                    flag.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                inner(warning);
            }),
            ..settings.clone()
        }
    };
    let initial = p.initial_transform(true).to_kurbo();
    let c = p.intersected_crop_box();
    let mut ctx = Context::new(initial, Rect::new(0.0, 0.0, f64::from(w), f64::from(h)), &cache, p.xref(), settings);
    let mut dev = MathDevice {
        page: initial.transform_rect_bbox(Rect::new(c.x0, c.y0, c.x1, c.y1)),
        page_out: MathPage::default(),
        font_index: HashMap::new(),
        caps,
        type3_ink: 0,
    };
    interpret_page(p, &mut ctx, &mut dev);
    let mut out = dev.page_out;
    out.truncated = cut.load(std::sync::atomic::Ordering::Relaxed);
    Ok(out)
}

pub mod ast;
pub mod layout;
pub mod recover;
pub mod symbols;
pub mod unicode;

#[cfg(test)]
mod tests;
