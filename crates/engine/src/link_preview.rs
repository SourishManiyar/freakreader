//! Where an in-document link goes, as a part of its target page to look at without leaving the
//! page being read: the reference preview a pointer resting on a cross-reference (or a
//! right-click on it) shows: a theorem, an equation, a cited work.
//!
//! The part shown is first a band the width of the page that starts just above the destination's
//! position and runs down from it, or the destination's own rectangle for `/FitR`. A destination
//! with no position on its page (`/Fit`, `/FitV`, a bare page) shows the top of the page. Once
//! the target page's glyphs are read ([`MathPage`]), [`LinkPreview::fit`] narrows the band to
//! the block the destination names ([`RefKind`], from its hyperref name): an equation's lines, a
//! statement, one bibliography entry, a heading and its first lines, a float with its caption.
//! Everything is in the target page's view space: points from its top-left as displayed, y down,
//! after `/Rotate` ([`PageInfo::user_to_view`]), which is how a renderer [`pdfcraft_render::Tile`]
//! is placed.

mod block;
mod names;

pub use names::{MAX_REF_NUMBER_BYTES, RefKind, RefName, ref_name};
use pdfcraft_render::text::math::MathPage;
use pdfcraft_render::{DestView, DocInfo, LinkTarget, PageInfo};
/// How far above the destination's position the band starts, in points: a destination usually
/// marks a line's baseline or its top, and the line it names should show whole.
pub const PREVIEW_ABOVE: f32 = 14.0;
/// The band's height as a share of the page's displayed width: a few lines of body text on a
/// portrait page, enough for a theorem's statement or an equation and its neighbours.
pub const PREVIEW_HEIGHT_SHARE: f32 = 0.4;
/// The least height the band has, in points, on a very wide and short page.
pub const PREVIEW_MIN_HEIGHT: f32 = 120.0;
/// Room left around a `/FitR` rectangle, in points.
pub const PREVIEW_FIT_MARGIN: f32 = 12.0;
/// The smallest `/FitR` rectangle shown as itself, in points a side; a smaller one shows the band
/// from where it is.
pub const PREVIEW_FIT_MIN: f32 = 4.0;

/// The part of the document an in-document link points at.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkPreview {
    /// The target page, 0-based.
    pub page: usize,
    /// The area of the target page to show, in its view space (`[x0, y0, x1, y1]`, points from
    /// the top-left as displayed, `x0 < x1`, `y0 < y1`), always inside the page.
    pub area: [f32; 4],
    /// Where on the target page the destination is.
    pub view: DestView,
    /// The named destination the link goes through, when it has one (`equation.4.7`).
    pub name: Option<String>,
    /// What the name says the destination is.
    pub reference: RefName,
    /// `area` is the block the destination names, found from the page's glyphs, rather than the
    /// band below it.
    pub fitted: bool,
}

impl LinkPreview {
    /// Whether the target page's glyphs can narrow the area to a block: a destination with a
    /// position on its page whose name says what it is. A `/FitR` rectangle shows itself.
    pub fn wants_glyphs(&self) -> bool {
        !self.fitted
            && !matches!(self.reference.kind, RefKind::Page | RefKind::Unknown)
            && matches!(self.view, DestView::Xyz { top: Some(_), .. } | DestView::FitH { top: Some(_) })
    }

    /// Narrow the area to the block the destination names on `page` (the target page's
    /// [`PageInfo`]), as `math` (its glyphs) shows it. `false`, and the area left as it is, when
    /// the page does not show one there (no text near the destination, a scan, text that does
    /// not run across).
    pub fn fit(&mut self, page: &PageInfo, math: &MathPage) -> bool {
        if !self.wants_glyphs() {
            return false;
        }
        match block_area(page, self.view, self.reference.kind, math) {
            Some(area) => {
                self.area = area;
                self.fitted = true;
                true
            }
            None => false,
        }
    }

    /// What the preview shows, in English, for automation: "Equation 4.7 · page 12" when the
    /// name gives a kind with a number (`page_label` is the target page's label), else
    /// "Page 12". A statement's or a citation's first words say what it is, on the picture.
    pub fn label(&self, page_label: &str) -> String {
        match (self.reference.kind_word(), self.reference.number.as_deref()) {
            (Some(word), Some(n)) => format!("{word} {n} · page {page_label}"),
            _ => format!("Page {page_label}"),
        }
    }
}

impl RefName {
    /// The English word the preview's caption names a kind with, for the kinds whose number it
    /// shows: equation, section, figure, table, item. A statement, a citation and a page say what
    /// they are on the picture itself.
    pub fn kind_word(&self) -> Option<&'static str> {
        match self.kind {
            RefKind::Equation => Some("Equation"),
            RefKind::Section => Some("Section"),
            RefKind::Figure => Some("Figure"),
            RefKind::Table => Some("Table"),
            RefKind::Item => Some("Item"),
            RefKind::TheoremLike | RefKind::Citation | RefKind::Page | RefKind::Unknown => None,
        }
    }
}

/// The block of kind `kind` that a destination `view` names on `page`, as `math` (the page's
/// glyphs) shows it: the area to preview, in view space, inside the page. `None` when the page
/// shows none there (see [`LinkPreview::fit`]), or the destination has no position.
pub fn block_area(page: &PageInfo, view: DestView, kind: RefKind, math: &MathPage) -> Option<[f32; 4]> {
    let (left, top) = match view {
        DestView::Xyz { left, top, .. } => (left, top),
        DestView::FitH { top } => (None, top),
        DestView::Top | DestView::Fit | DestView::FitV { .. } | DestView::FitR { .. } => return None,
    };
    let (w, h) = (page.width, page.height);
    let [fx, fy] = page.dest_fraction(left, top, 0);
    block::block_area(math, [w, h], fx.map(|f| f * w), fy? * h, kind)
}

/// The area of `page` to show for a destination `view` on it.
pub fn preview_area(page: &PageInfo, view: DestView) -> [f32; 4] {
    let (w, h) = (page.width.max(1.0), page.height.max(1.0));
    // A band the width of the page from `top` (view points) down, kept on the page.
    let band = |top: f32| {
        let height = (w * PREVIEW_HEIGHT_SHARE).max(PREVIEW_MIN_HEIGHT).min(h);
        let y0 = (top - PREVIEW_ABOVE).clamp(0.0, h - height);
        [0.0, y0, w, y0 + height]
    };
    // A destination's vertical position as displayed: on a page turned a quarter its user `top`
    // decides the displayed x instead, and the band then starts at the top of the page.
    let top_of = |left: Option<f32>, top: Option<f32>| page.dest_fraction(left, top, 0)[1].map_or(0.0, |fy| fy * h);
    match view {
        DestView::Xyz { left, top, .. } => band(top_of(left, top)),
        DestView::FitH { top } => band(top_of(None, top)),
        DestView::FitR { rect } => {
            let a = page.user_to_view(rect[0], rect[1]);
            let b = page.user_to_view(rect[2], rect[3]);
            let m = PREVIEW_FIT_MARGIN;
            let (x0, y0) = ((a[0].min(b[0])).max(0.0), (a[1].min(b[1])).max(0.0));
            let (x1, y1) = ((a[0].max(b[0])).min(w), (a[1].max(b[1])).min(h));
            // A rectangle too thin to read (or off the page) shows the band from its top instead.
            if x1 - x0 >= PREVIEW_FIT_MIN && y1 - y0 >= PREVIEW_FIT_MIN {
                [(x0 - m).max(0.0), (y0 - m).max(0.0), (x1 + m).min(w), (y1 + m).min(h)]
            } else {
                band(if y0.is_finite() { y0.min(h) } else { 0.0 })
            }
        }
        DestView::Top | DestView::Fit | DestView::FitV { .. } => band(0.0),
    }
}

/// What the link at `/Annots` index `index` on `page` (both 0-based) points at, when it goes to
/// a page of this document; `None` for a link elsewhere (a web address, another file, a layer
/// change), one whose target page doesn't exist, or no such link. The area is the band; see
/// [`LinkPreview::fit`].
pub fn link_preview(info: &DocInfo, page: usize, index: usize) -> Option<LinkPreview> {
    let link = info.links.iter().find(|l| l.page == page && l.index == index)?;
    let LinkTarget::Page(target, view) = link.target else { return None };
    dest_preview(info, target, view, link.dest_name.as_deref())
}

/// What a destination `view` on page `target` (0-based), named `name` when it is a named
/// destination, shows; `None` when the page doesn't exist. The area is the band; see
/// [`LinkPreview::fit`].
pub fn dest_preview(info: &DocInfo, target: usize, view: DestView, name: Option<&str>) -> Option<LinkPreview> {
    let shown = info.pages.get(target)?;
    let reference = name.map_or(RefName { kind: RefKind::Unknown, number: None }, ref_name);
    Some(LinkPreview { page: target, area: preview_area(shown, view), view, name: name.map(str::to_owned), reference, fitted: false })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn letter(rotation: u16) -> PageInfo {
        let (width, height) = if rotation.is_multiple_of(180) { (612.0, 792.0) } else { (792.0, 612.0) };
        PageInfo { width, height, label: "1".into(), crop: [0.0, 0.0, 612.0, 792.0], rotation }
    }

    #[test]
    fn an_xyz_destination_shows_a_band_from_just_above_its_line() {
        let area = preview_area(&letter(0), DestView::Xyz { left: Some(72.0), top: Some(692.0), zoom: None });
        // 692 in user space is 100 points from the top of the page.
        let expected = [0.0, 100.0 - PREVIEW_ABOVE, 612.0, 100.0 - PREVIEW_ABOVE + 612.0 * PREVIEW_HEIGHT_SHARE];
        assert!(area.iter().zip(expected).all(|(a, e)| (a - e).abs() < 1e-3), "{area:?} vs {expected:?}");
    }

    #[test]
    fn a_band_near_the_bottom_stays_on_the_page() {
        let area = preview_area(&letter(0), DestView::FitH { top: Some(10.0) });
        assert_eq!(area[3], 792.0, "{area:?}");
        assert!(area[3] - area[1] > PREVIEW_MIN_HEIGHT);
    }

    #[test]
    fn destinations_without_a_position_show_the_top_of_the_page() {
        for view in [DestView::Top, DestView::Fit, DestView::FitV { left: Some(30.0) }, DestView::Xyz { left: None, top: None, zoom: None }] {
            let area = preview_area(&letter(0), view);
            assert_eq!(area[1], 0.0, "{view:?}");
        }
    }

    #[test]
    fn a_fit_rectangle_shows_itself_with_a_margin() {
        let area = preview_area(&letter(0), DestView::FitR { rect: [100.0, 500.0, 300.0, 600.0] });
        let m = PREVIEW_FIT_MARGIN;
        assert_eq!(area, [100.0 - m, 192.0 - m, 300.0 + m, 292.0 + m]);
        // A sliver of a rectangle reads as a band from where it is.
        let sliver = preview_area(&letter(0), DestView::FitR { rect: [100.0, 500.0, 100.0, 500.0] });
        assert_eq!((sliver[0], sliver[2]), (0.0, 612.0));
    }

    #[test]
    fn a_preview_is_fitted_only_where_a_block_is_found() {
        let info = DocInfo { pages: vec![letter(0)], ..Default::default() };
        let view = DestView::Xyz { left: Some(72.0), top: Some(692.0), zoom: None };
        let mut preview = dest_preview(&info, 0, view, Some("equation.4.7")).expect("page 1");
        assert_eq!((preview.reference.kind, preview.reference.number.as_deref()), (RefKind::Equation, Some("4.7")));
        assert_eq!(preview.label("iv"), "Equation 4.7 · page iv");
        let band = preview.area;
        // A page with no text (a scan): the band stays.
        assert!(!preview.fit(&letter(0), &MathPage::default()));
        assert_eq!((preview.area, preview.fitted), (band, false));
        // A /FitR rectangle shows itself, and an unnamed destination has no block to look for.
        let fit_r = dest_preview(&info, 0, DestView::FitR { rect: [100.0, 500.0, 300.0, 600.0] }, Some("equation.1")).expect("page 1");
        assert!(!fit_r.wants_glyphs());
        assert!(!dest_preview(&info, 0, view, None).expect("page 1").wants_glyphs());
        assert_eq!(dest_preview(&info, 0, view, Some("theorem.2.1")).expect("page 1").label("1"), "Page 1");
        assert!(dest_preview(&info, 1, view, None).is_none(), "no such page");
    }

    #[test]
    fn hostile_numbers_never_leave_the_page() {
        for top in [f32::NAN, f32::INFINITY, -1e30, 1e30] {
            for page in [letter(0), letter(90), PageInfo { width: 0.0, height: 0.0, label: String::new(), crop: [0.0; 4], rotation: 0 }] {
                let area = preview_area(&page, DestView::Xyz { left: Some(top), top: Some(top), zoom: None });
                let (w, h) = (page.width.max(1.0), page.height.max(1.0));
                assert!(area.iter().all(|v| v.is_finite()), "{area:?}");
                assert!(area[0] >= 0.0 && area[1] >= 0.0 && area[2] <= w && area[3] <= h && area[0] < area[2] && area[1] < area[3], "{area:?}");
            }
        }
        let rect = [f32::NAN, 0.0, f32::INFINITY, 1e30];
        let area = preview_area(&letter(0), DestView::FitR { rect });
        assert!(area.iter().all(|v| v.is_finite()) && area[1] < area[3], "{area:?}");
    }
}
