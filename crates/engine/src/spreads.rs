//! Organize ▸ Split two-page spreads: a scanned page that holds two book pages side by side
//! becomes two pages, each cropped to one half (see `pdfcraft_organize::split_pages_in_two`).
//!
//! This module decides which pages to split and where: the visible page as displayed (its crop
//! box, `/Rotate` and `/UserUnit`), a fixed fraction of its width or a gutter found in a small
//! raster of it, and the margin each half keeps away from the split line. The halves are then
//! worked out in the page's own user space, which is all the organize layer sees.

use pdfcraft_organize::{PageSplit, SplitAxis};
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};

use crate::{DocId, EditError, Session};

/// Pixels per point for the gutter search: about 100 dpi.
const GUTTER_SCALE: f64 = 100.0 / 72.0;

/// The longest side of a gutter-search raster, in pixels. A larger page renders smaller, so one
/// raster never takes more than about 16 MB.
const GUTTER_MAX_SIDE: f64 = 2000.0;

/// The part of the width the gutter is looked for in: the middle 40%.
const GUTTER_WINDOW: (f64, f64) = (0.3, 0.7);

/// Below this confidence no clear gutter was found and the split goes in the centre.
pub const MIN_GUTTER_CONFIDENCE: f64 = 0.6;

/// The least and most a fixed split position may be, as a fraction of the width.
pub const SPLIT_FRACTION_RANGE: (f64, f64) = (0.1, 0.9);

/// The default width ÷ height above which a page counts as a spread.
pub const DEFAULT_LANDSCAPE_RATIO: f64 = 1.2;

/// The largest margin each half may keep from the split line, in points.
pub const MAX_SPLIT_MARGIN: f64 = 1000.0;

/// Which half comes first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReadingOrder {
    /// The left half, then the right (most scripts).
    #[default]
    LeftToRight,
    /// The right half, then the left (Arabic, Hebrew, and Japanese or Chinese books set vertically).
    RightToLeft,
}

/// Where each page is split.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SplitPosition {
    /// Look for the gutter in a small raster of each page.
    Auto,
    /// This fraction of the displayed width from the displayed left edge (0.5 = the centre).
    Fixed(f64),
}

/// What Split two-page spreads is asked to do.
#[derive(Clone, Debug, PartialEq)]
pub struct SpreadOptions {
    /// The pages to consider, 0-based; `None` means every page.
    pub pages: Option<Vec<usize>>,
    /// Only pages whose visible width ÷ height is above `min_ratio`.
    pub landscape_only: bool,
    pub min_ratio: f64,
    pub position: SplitPosition,
    /// Points each half keeps away from the split line.
    pub margin: f64,
    pub order: ReadingOrder,
}

impl Default for SpreadOptions {
    fn default() -> Self {
        Self {
            pages: None,
            landscape_only: true,
            min_ratio: DEFAULT_LANDSCAPE_RATIO,
            position: SplitPosition::Auto,
            margin: 0.0,
            order: ReadingOrder::LeftToRight,
        }
    }
}

/// How a page's split position was chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitSource {
    /// The fixed fraction asked for.
    Fixed,
    /// A gutter found in the page's raster.
    Gutter,
    /// No clear gutter was found: the centre.
    Centre,
}

/// Where one page will be split.
#[derive(Clone, Debug, PartialEq)]
pub struct SpreadPlan {
    /// 0-based.
    pub page: usize,
    /// The split line as a fraction of the displayed width from the displayed left edge.
    pub fraction: f64,
    /// 0 to 1: how clearly the gutter stood out (1 for a fixed position).
    pub confidence: f64,
    pub source: SplitSource,
    /// Why the search fell back to the centre, when it did.
    pub note: Option<String>,
}

/// What the gutter search found in one raster.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gutter {
    /// The emptiest column, as a fraction of the width.
    pub fraction: f64,
    pub confidence: f64,
    /// Whether it stood out clearly; otherwise `fraction` is the centre.
    pub found: bool,
}

impl Gutter {
    const CENTRE: Gutter = Gutter { fraction: 0.5, confidence: 0.0, found: false };
}

/// Find the gutter in a grey raster (`width × height`, row-major, 0 black to 255 white).
///
/// Each column's ink is how many of its pixels differ from the one above by a clear step, so
/// text and line art count and a smooth shadow down the spine does not. The profile is smoothed
/// over about 1% of the width, and the emptiest column in the middle 40% wins; among columns as
/// empty as it, the middle of the empty run nearest the centre. Confidence is how much emptier
/// that column is than the page's average column. The top and bottom 5% of rows are left out,
/// where scanner edges and running heads cross the spine.
pub fn find_gutter(grey: &[u8], width: usize, height: usize) -> Gutter {
    const STEP: u8 = 40;
    if width < 20 || height < 20 || grey.len() < width.saturating_mul(height) {
        return Gutter::CENTRE;
    }
    let (top, bottom) = (height / 20 + 1, height - height / 20);
    let mut ink = vec![0f64; width];
    for y in top..bottom {
        let (Some(row), Some(above)) = (grey.get(y * width..(y + 1) * width), grey.get((y - 1) * width..y * width)) else { continue };
        for ((count, a), b) in ink.iter_mut().zip(row).zip(above) {
            if a.abs_diff(*b) >= STEP {
                *count += 1.0;
            }
        }
    }
    // Box smoothing over 2r+1 columns, with prefix sums.
    let r = (width / 100).max(1);
    let mut prefix = Vec::with_capacity(width + 1);
    prefix.push(0.0);
    for v in &ink {
        prefix.push(prefix.last().copied().unwrap_or(0.0) + v);
    }
    let smooth: Vec<f64> = (0..width)
        .map(|x| {
            let (a, b) = (x.saturating_sub(r), (x + r + 1).min(width));
            let sum = prefix.get(b).copied().unwrap_or(0.0) - prefix.get(a).copied().unwrap_or(0.0);
            sum / (b - a) as f64
        })
        .collect();
    let mean = smooth.iter().sum::<f64>() / width as f64;
    if mean < 0.05 {
        // A blank page: nothing to go by.
        return Gutter::CENTRE;
    }
    let lo = ((width as f64 * GUTTER_WINDOW.0).floor() as usize).min(width - 1);
    let hi = ((width as f64 * GUTTER_WINDOW.1).ceil() as usize).clamp(lo + 1, width);
    let Some(window) = smooth.get(lo..hi) else { return Gutter::CENTRE };
    let min = window.iter().copied().fold(f64::INFINITY, f64::min);
    let tie = min + 0.02 * mean + 1e-9;
    let centre = width as f64 / 2.0;
    let distance = |x: usize| (x as f64 + 0.5 - centre).abs();
    let Some(best) = (lo..hi).filter(|x| smooth.get(*x).is_some_and(|v| *v <= tie)).min_by(|a, b| distance(*a).total_cmp(&distance(*b))) else {
        return Gutter::CENTRE;
    };
    let (mut a, mut b) = (best, best);
    while a > lo && smooth.get(a - 1).is_some_and(|v| *v <= tie) {
        a -= 1;
    }
    while b + 1 < hi && smooth.get(b + 1).is_some_and(|v| *v <= tie) {
        b += 1;
    }
    let fraction = ((a + b) as f64 / 2.0 + 0.5) / width as f64;
    let confidence = (1.0 - min / mean).clamp(0.0, 1.0);
    // An empty run across the whole window gives no position: the middle could be anywhere in it.
    let whole_window = a == lo && b + 1 == hi;
    if confidence < MIN_GUTTER_CONFIDENCE || whole_window {
        return Gutter { confidence: if whole_window { 0.0 } else { confidence }, ..Gutter::CENTRE };
    }
    Gutter { fraction, confidence, found: true }
}

/// Render page `page` (0-based) small and look for its gutter. `size` is its displayed size in points.
pub fn gutter_of_page(renderer: &mut PageRenderer, page: usize, size: (f64, f64)) -> Result<Gutter, String> {
    let (w, h) = size;
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
        return Err("the page has no visible area".into());
    }
    let scale = GUTTER_SCALE.min(GUTTER_MAX_SIDE / w.max(h));
    let out = renderer.render(RenderRequest { page, kind: RequestKind::Pixels, tile: None, scale: scale as f32, tag: 0, preview: false });
    if let Some(e) = out.error {
        return Err(e);
    }
    let (width, height) = (out.width as usize, out.height as usize);
    let pixels: &[u8] = &out.rgba;
    if width == 0 || height == 0 || pixels.len() < width.saturating_mul(height).saturating_mul(4) {
        return Err("the page rendered empty".into());
    }
    // The page clears to white and is opaque, so premultiplied colour is the colour.
    let grey: Vec<u8> = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .map(|[r, g, b, _]| ((u32::from(*r) * 299 + u32::from(*g) * 587 + u32::from(*b) * 114) / 1000) as u8)
        .collect();
    Ok(find_gutter(&grey, width, height))
}

/// The pages `options` chooses, 0-based: the range (every page when none is given), less those
/// that aren't wider than `min_ratio` when only landscape pages are wanted. A page taller than it
/// is wide is split only when the range names it and only-landscape is off.
pub fn chosen_pages(doc: &pdfcraft_cos::Document, options: &SpreadOptions) -> Result<Vec<usize>, EditError> {
    check_options(options)?;
    let pages = pdfcraft_model::pages(doc);
    let n = pages.len();
    let range: Vec<usize> = match &options.pages {
        Some(p) => {
            if let Some(bad) = p.iter().find(|i| **i >= n) {
                return Err(EditError::Invalid(format!("page {} is out of range: the document has {n} pages", bad.saturating_add(1))));
            }
            let mut p = p.clone();
            p.sort_unstable();
            p.dedup();
            p
        }
        None => (0..n).collect(),
    };
    let explicit = options.pages.is_some();
    Ok(range
        .into_iter()
        .filter(|i| {
            let Some(page) = pages.get(*i) else { return false };
            let (w, h) = page.display_size(doc);
            let ratio = if h > 0.0 { w / h } else { 0.0 };
            if options.landscape_only { ratio > options.min_ratio } else { explicit || ratio >= 1.0 }
        })
        .collect())
}

fn check_options(o: &SpreadOptions) -> Result<(), EditError> {
    if !(o.min_ratio.is_finite() && (1.0..=10.0).contains(&o.min_ratio)) {
        return Err(EditError::Invalid("the landscape ratio must be between 1 and 10".into()));
    }
    if !(o.margin.is_finite() && (0.0..=MAX_SPLIT_MARGIN).contains(&o.margin)) {
        return Err(EditError::Invalid(format!("the margin must be between 0 and {MAX_SPLIT_MARGIN} points")));
    }
    if let SplitPosition::Fixed(f) = o.position
        && !(f.is_finite() && (SPLIT_FRACTION_RANGE.0..=SPLIT_FRACTION_RANGE.1).contains(&f))
    {
        return Err(EditError::Invalid(format!(
            "the split position must be a fraction of the width between {} and {}",
            SPLIT_FRACTION_RANGE.0, SPLIT_FRACTION_RANGE.1
        )));
    }
    Ok(())
}

fn split_of(crop: [f64; 4], rotation: i64, unit: f64, page: usize, fraction: f64, margin: f64, order: ReadingOrder) -> Result<PageSplit, EditError> {
    let n = page.saturating_add(1);
    if !(fraction.is_finite() && fraction > 0.0 && fraction < 1.0) {
        return Err(EditError::Invalid(format!("page {n}: the split position must be inside the page")));
    }
    let unit = if unit.is_finite() && unit >= 1.0 { unit } else { 1.0 };
    let m = margin / unit;
    let [x0, y0, x1, y1] = crop;
    let (w, h) = (x1 - x0, y1 - y0);
    // The displayed left edge is low x (0°), high x (180°), low y (90°) or high y (270°).
    let (axis, at, left_low) = match rotation {
        90 => (SplitAxis::Y, y0 + fraction * h, true),
        180 => (SplitAxis::X, x1 - fraction * w, false),
        270 => (SplitAxis::Y, y1 - fraction * h, false),
        _ => (SplitAxis::X, x0 + fraction * w, true),
    };
    let (low, high) = match axis {
        SplitAxis::X => ([x0, y0, at - m, y1], [at + m, y0, x1, y1]),
        SplitAxis::Y => ([x0, y0, x1, at - m], [x0, at + m, x1, y1]),
    };
    let first_low = left_low == (order == ReadingOrder::LeftToRight);
    let (first, second) = if first_low { (low, high) } else { (high, low) };
    let ok = |r: &[f64; 4]| r.iter().all(|v| v.is_finite()) && r[2] - r[0] >= 1.0 && r[3] - r[1] >= 1.0;
    if !ok(&first) || !ok(&second) {
        return Err(EditError::Invalid(format!("page {n}: the margin leaves a half smaller than a point; use a smaller margin")));
    }
    Ok(PageSplit { page, first, second, axis, at, first_low })
}

impl Session {
    /// Where Split two-page spreads would split each page `options` chooses, without changing
    /// anything. The automatic position renders each page at about 100 dpi.
    pub fn plan_spreads(&self, id: DocId, options: &SpreadOptions) -> Result<Vec<SpreadPlan>, EditError> {
        self.plan_spreads_among(id, options, None)
    }

    /// The pages (0-based) Split two-page spreads would split with `options`.
    pub fn spread_pages(&self, id: DocId, options: &SpreadOptions) -> Result<Vec<usize>, EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        let cos = &doc.editor.as_ref().ok_or_else(|| EditError::ReadOnly(doc.read_only_reason.clone().unwrap_or_default()))?.cos;
        chosen_pages(cos, options)
    }

    /// [`Session::plan_spreads`] for those of `only` (0-based) that `options` chooses: a preview
    /// of a few pages without rendering the rest.
    pub fn plan_spreads_of(&self, id: DocId, options: &SpreadOptions, only: &[usize]) -> Result<Vec<SpreadPlan>, EditError> {
        self.plan_spreads_among(id, options, Some(only))
    }

    fn plan_spreads_among(&self, id: DocId, options: &SpreadOptions, only: Option<&[usize]>) -> Result<Vec<SpreadPlan>, EditError> {
        let doc = self.get(id).ok_or(EditError::NoDocument)?;
        let cos = &doc.editor.as_ref().ok_or_else(|| EditError::ReadOnly(doc.read_only_reason.clone().unwrap_or_default()))?.cos;
        let mut chosen = chosen_pages(cos, options)?;
        if let Some(only) = only {
            chosen.retain(|p| only.contains(p));
        }
        if chosen.len() > pdfcraft_organize::MAX_SPLIT_PAGES {
            return Err(EditError::Invalid(format!(
                "at most {} pages can be split at once; {} were chosen: choose a smaller page range",
                pdfcraft_organize::MAX_SPLIT_PAGES,
                chosen.len()
            )));
        }
        let mut renderer = match options.position {
            SplitPosition::Auto => Some(PageRenderer::new(
                doc.display.clone(),
                // True colours and no comments: only the page's own ink says where the gutter is.
                RenderConfig { password: doc.password.as_deref().map(std::sync::Arc::from), hide_comments: true, ..Default::default() },
            )),
            SplitPosition::Fixed(_) => None,
        };
        let pages = pdfcraft_model::pages(cos);
        let mut plans = Vec::with_capacity(chosen.len());
        for i in chosen {
            let plan = match (options.position, renderer.as_mut()) {
                (SplitPosition::Fixed(f), _) => SpreadPlan { page: i, fraction: f, confidence: 1.0, source: SplitSource::Fixed, note: None },
                (SplitPosition::Auto, Some(r)) => {
                    let size = pages.get(i).map_or((0.0, 0.0), |p| p.display_size(cos));
                    match gutter_of_page(r, i, size) {
                        Ok(g) if g.found => {
                            SpreadPlan { page: i, fraction: g.fraction, confidence: g.confidence, source: SplitSource::Gutter, note: None }
                        }
                        Ok(g) => SpreadPlan {
                            page: i,
                            fraction: 0.5,
                            confidence: g.confidence,
                            source: SplitSource::Centre,
                            note: Some("no clear gutter was found; split in the centre".into()),
                        },
                        Err(e) => SpreadPlan {
                            page: i,
                            fraction: 0.5,
                            confidence: 0.0,
                            source: SplitSource::Centre,
                            note: Some(format!("the page could not be rendered to look for the gutter ({e}); split in the centre")),
                        },
                    }
                }
                (SplitPosition::Auto, None) => SpreadPlan { page: i, fraction: 0.5, confidence: 0.0, source: SplitSource::Centre, note: None },
            };
            plans.push(plan);
        }
        Ok(plans)
    }

    /// Split two-page spreads: plan as [`Session::plan_spreads`], then split every planned page
    /// in one undoable [`crate::Edit::SplitSpreads`]. Returns the plans and what became of each
    /// page. Choosing no page is refused.
    pub fn split_spreads(&mut self, id: DocId, options: &SpreadOptions) -> Result<(Vec<SpreadPlan>, Vec<pdfcraft_organize::SplitPage>), EditError> {
        let plans = self.plan_spreads(id, options)?;
        if plans.is_empty() {
            return Err(EditError::Invalid(if options.landscape_only {
                "no page in the range is wider than the landscape ratio; nothing was split (turn off only-landscape pages to split others)".into()
            } else {
                "no page in the range can be split; nothing was split".into()
            }));
        }
        let edit =
            crate::Edit::SplitSpreads { splits: plans.iter().map(|p| (p.page, p.fraction)).collect(), margin: options.margin, order: options.order };
        let report = self.apply_reporting(id, edit)?;
        Ok((plans, report.split.unwrap_or_default()))
    }
}

/// Run [`crate::Edit::SplitSpreads`] on a working copy.
pub(crate) fn run(
    doc: &mut pdfcraft_cos::Document,
    splits: &[(usize, f64)],
    margin: f64,
    order: ReadingOrder,
) -> Result<Vec<pdfcraft_organize::SplitPage>, EditError> {
    if !(margin.is_finite() && (0.0..=MAX_SPLIT_MARGIN).contains(&margin)) {
        return Err(EditError::Invalid(format!("the margin must be between 0 and {MAX_SPLIT_MARGIN} points")));
    }
    let pages = pdfcraft_model::pages(doc);
    let mut cuts = Vec::with_capacity(splits.len());
    for (page, fraction) in splits {
        let p = pages.get(*page).ok_or_else(|| EditError::Invalid(format!("page {} does not exist", page.saturating_add(1))))?;
        cuts.push(split_of(p.crop(doc), p.rotation(doc), p.user_unit(doc), *page, *fraction, margin, order)?);
    }
    let out = pdfcraft_organize::split_pages_in_two(doc, &cuts)?;
    for p in &out {
        for line in &p.first_only {
            doc.note_repair(format!("split page {}: {line}", p.page.saturating_add(1)));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
