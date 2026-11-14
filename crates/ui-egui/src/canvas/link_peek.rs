//! The reference preview: with the Select tool, a right-click on a link to a place in this
//! document (a theorem, an equation, a cited work) shows that place in a small window by the link,
//! so the reader can look without leaving the page they are on. Another right-click closes it, as
//! does Escape (`app/keys.rs`); a click on the preview goes there.
//!
//! What is shown is the engine's band of the target page ([`pdfcraft_engine::link_preview`]),
//! drawn by a render thread of its own ([`RenderPool::companion`]) with the page view's settings,
//! so the page view's queue, which it replaces every frame, is left alone. The preview closes when
//! the document changes (its pages may be gone), when a dialog comes over the pages, and when its
//! link's page leaves the screen.

use egui::emath::GuiRounding as _;
use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, Vec2, pos2, vec2};
use pdfcraft_engine::link_preview::{LinkPreview, link_preview};
use pdfcraft_render::{DestView, DocInfo, LinkTarget, RenderPool, RenderRequest, Tile};

use super::DocView;
use crate::theme::{self, Tokens};

/// The tallest the picture is, as a share of the pages' height on screen at 100 %: a tall `/FitR`
/// rectangle, or a big zoom, shows smaller.
const MAX_HEIGHT_SHARE: f32 = 0.5;
/// The tallest it is at any size, as a share of the pages' height.
const MAX_HEIGHT_SHARE_CAP: f32 = 0.8;
/// Preferences ▸ Documents and view ▸ Reference preview size, in percent: what it offers.
pub const PREVIEW_SIZES: [u16; 5] = [50, 75, 100, 125, 150];
/// The narrowest the picture is, in points, however far the page is zoomed out.
const MIN_WIDTH: f32 = 240.0;
/// The space between the link and the window, and between the window and the edge of the pages.
const GAP: f32 = 6.0;
const EDGE: f32 = 12.0;
/// The window's padding, and the space between its caption and the picture.
const PAD: f32 = 8.0;
const CAPTION_GAP: f32 = 6.0;
/// The longest side of the rendered picture, in device pixels (the renderer's tile limit).
const MAX_SIDE: f32 = 4096.0;

/// A preview on screen: the link it belongs to, what it shows, and the picture once drawn.
pub struct LinkPeek {
    /// The link: its page and its index in that page's `/Annots` (0-based).
    source: (usize, usize),
    /// Where the link is on its page, in user space, to place the window by it.
    link_rect: [f32; 4],
    /// The part of the target page shown.
    preview: LinkPreview,
    /// Where following the link goes, for a click on the preview.
    dest: DestView,
    /// The target page's label, for the caption.
    label: String,
    /// The document's edit generation the preview was made from.
    generation: u64,
    pool: RenderPool,
    /// The scale last asked for, as its request tag.
    asked: Option<u64>,
    texture: Option<egui::TextureHandle>,
    /// The renderer could not draw the area.
    failed: bool,
}

impl std::fmt::Debug for LinkPeek {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkPeek").field("source", &self.source).field("preview", &self.preview).field("drawn", &self.texture.is_some()).finish()
    }
}

impl LinkPeek {
    /// The preview of the link at `/Annots` index `index` on `page`, when it goes to a page of
    /// this document.
    pub(crate) fn open(doc: &pdfcraft_engine::Document, page: usize, index: usize) -> Option<Self> {
        let preview = link_preview(&doc.info, page, index)?;
        let link = doc.info.links.iter().find(|l| l.page == page && l.index == index)?;
        let LinkTarget::Page(_, dest) = link.target else { return None };
        let label = doc.info.pages.get(preview.page)?.label.clone();
        Some(Self {
            source: (page, index),
            link_rect: link.rect,
            preview,
            dest,
            label,
            generation: doc.edit_generation(),
            pool: doc.renderer.companion(),
            asked: None,
            texture: None,
            failed: false,
        })
    }

    /// The link the preview belongs to: its page and its index in that page's `/Annots`
    /// (0-based).
    pub fn source(&self) -> (usize, usize) {
        self.source
    }

    /// The part of the document shown.
    pub fn preview(&self) -> LinkPreview {
        self.preview
    }

    /// Whether the picture has come back from the renderer.
    pub fn drawn(&self) -> bool {
        self.texture.is_some()
    }

    /// Whether the renderer could not draw the area.
    pub fn failed(&self) -> bool {
        self.failed
    }

    /// The picture's size on screen, in points: at `zoom` (screen points per page point, the
    /// link's page's), so its text reads at the size of the page's, but no wider than `room`, no
    /// taller than `tallest`, and no narrower than [`MIN_WIDTH`] (or `room`, when less).
    fn picture_size(&self, zoom: f32, room: Vec2) -> Vec2 {
        let [x0, y0, x1, y1] = self.preview.area;
        let (aw, ah) = ((x1 - x0).max(1.0), (y1 - y0).max(1.0));
        let zoom = if zoom.is_finite() && zoom > 0.0 { zoom } else { 1.0 };
        let fit = (room.x.max(1.0) / aw).min(room.y.max(1.0) / ah);
        let least = MIN_WIDTH.min(room.x.max(1.0)) / aw;
        let per_point = zoom.min(fit).max(least);
        vec2(aw * per_point, ah * per_point)
    }

    /// Ask for the picture at `size` points on a screen of `ppp` pixels per point, when not asked
    /// already, and take in what has come back. `true` while it is still being drawn.
    fn render(&mut self, ctx: &egui::Context, size: Vec2, ppp: f32) -> bool {
        let [x0, y0, x1, y1] = self.preview.area;
        let (aw, ah) = ((x1 - x0).max(1.0), (y1 - y0).max(1.0));
        let scale = (size.x * ppp / aw).min(MAX_SIDE / aw).min(MAX_SIDE / ah);
        if !(scale.is_finite() && scale > 0.0) {
            self.failed = true;
            return false;
        }
        // Whole thousandths of a pixel per point: a scale changes only when the window or the
        // screen does.
        let tag = (scale * 1000.0).round() as u64;
        if self.asked != Some(tag) && !self.failed {
            let tile = Tile {
                x: (x0 * scale).floor().max(0.0) as u32,
                y: (y0 * scale).floor().max(0.0) as u32,
                w: (aw * scale).ceil().max(1.0) as u32,
                h: (ah * scale).ceil().max(1.0) as u32,
            };
            self.pool.set_queue(vec![RenderRequest { page: self.preview.page, tile: Some(tile), scale, tag, ..Default::default() }]);
            self.asked = Some(tag);
        }
        while let Some(out) = self.pool.try_recv() {
            if out.request.tag != tag {
                continue;
            }
            if out.error.is_some() || out.width == 0 || out.height == 0 {
                self.failed = true;
            } else {
                let image = super::cache::texture_image([out.width as usize, out.height as usize], out.rgba);
                self.texture = Some(ctx.load_texture("link-preview", image, egui::TextureOptions::LINEAR));
            }
        }
        !self.failed && self.texture.is_none()
    }
}

/// What this frame's right-click on the pages does to the preview. `found`: the link a page found
/// under it that goes somewhere in this document (Select tool), as (page, `/Annots` index). An open
/// preview closes, wherever the right-click is; otherwise the link's preview opens. Returns whether
/// the right-click was the preview's: the page's menu stays shut then.
pub(super) fn right_click(view: &mut DocView, doc: &pdfcraft_engine::Document, found: Option<(usize, usize)>) -> bool {
    if view.link_peek.take().is_some() {
        return true;
    }
    let Some((page, index)) = found else { return false };
    view.link_peek = LinkPeek::open(doc, page, index);
    view.link_peek.is_some()
}

/// The zoom the preview is drawn at: the link's page's (screen points per page point), but never
/// below the viewer's 100 % ([`super::PT`]), where body text is too small to read, times
/// Reference preview size (`size`, a factor). A page zoomed out to see more of it still gets a
/// preview that reads.
fn preview_zoom(page_zoom: Option<f32>, size: f32) -> f32 {
    let page = page_zoom.filter(|z| z.is_finite() && *z > 0.0).unwrap_or(super::PT);
    let size = if size.is_finite() && size > 0.0 { size } else { 1.0 };
    page.max(super::PT) * size
}

/// `size` in points made a whole number of pixels at `ppp` pixels per point, so the picture
/// rendered for it maps one texel to one screen pixel.
fn snap(size: Vec2, ppp: f32) -> Vec2 {
    let ppp = if ppp.is_finite() && ppp > 0.0 { ppp } else { 1.0 };
    (size * ppp).round().max(vec2(1.0, 1.0)) / ppp
}

/// Where the window goes: below the link when it fits under it in `viewport`, else above it, else
/// wherever it is most in view; across, centred on the link and kept inside.
fn place(link: Rect, size: Vec2, viewport: Rect) -> Pos2 {
    let below = link.bottom() + GAP;
    let above = link.top() - GAP - size.y;
    let y = if below + size.y <= viewport.bottom() - EDGE {
        below
    } else if above >= viewport.top() + EDGE {
        above
    } else {
        (viewport.bottom() - EDGE - size.y).max(viewport.top() + EDGE)
    };
    let x = (link.center().x - size.x / 2.0).clamp(viewport.left() + EDGE, (viewport.right() - EDGE - size.x).max(viewport.left() + EDGE));
    pos2(x, y)
}

/// Draw the open preview by its link, or close it when it no longer belongs on screen (see the
/// module notes). `modal`: something is over the pages. A click on the picture goes to the
/// destination and closes the preview; a right-click on the window closes it.
/// `size`: Reference preview size, in percent ([`PREVIEW_SIZES`]; anything else is 100).
pub(super) fn show(ui: &egui::Ui, view: &mut DocView, doc: &pdfcraft_engine::Document, info: &DocInfo, modal: bool, size: u16) {
    let size = if PREVIEW_SIZES.contains(&size) { f32::from(size) / 100.0 } else { 1.0 };
    let Some(peek) = view.link_peek.as_mut() else { return };
    let (page, _) = peek.source;
    let xform = info.pages.get(page).and(view.screen_xforms.iter().find(|(p, _)| *p == page).map(|(_, xf)| *xf));
    let link = xform.map(|xf| xf.user_rect(info, page, peek.link_rect));
    let viewport = view.viewport_screen;
    let (Some(link), false, true) = (link, modal, doc.edit_generation() == peek.generation) else {
        view.link_peek = None;
        return;
    };
    if !viewport.intersects(link) {
        view.link_peek = None;
        return;
    }
    let ctx = ui.ctx().clone();
    let t = Tokens::get(&ctx);
    let room = vec2(viewport.width() - 2.0 * (EDGE + PAD), viewport.height() * (MAX_HEIGHT_SHARE * size).min(MAX_HEIGHT_SHARE_CAP));
    let ppp = ctx.pixels_per_point();
    let picture = snap(peek.picture_size(preview_zoom(xform.map(|xf| xf.zoom()), size), room), ppp);
    if peek.render(&ctx, picture, ppp) {
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }
    let caption = crate::i18n::fmt(tl!("Go to page {p}"), &[("p", &peek.label)]);
    let caption_font = theme::medium(12.0);
    let caption_h = ctx.fonts_mut(|f| f.row_height(&caption_font));
    let size = picture + vec2(2.0 * PAD, 2.0 * PAD + caption_h + CAPTION_GAP);
    // On whole pixels, so the picture's texels fall on the screen's.
    let at = place(link, size, viewport).round_to_pixels(ppp);
    let mut go = false;
    let mut close = false;
    egui::Area::new(egui::Id::unique(("link-preview", view.id.0))).order(egui::Order::Foreground).fixed_pos(at).constrain(false).show(&ctx, |ui| {
        let frame = egui::Frame::new()
            .fill(t.card)
            .stroke(Stroke::new(1.0, t.border))
            .corner_radius(CornerRadius::same(8))
            .shadow(ctx.global_style().visuals.popup_shadow)
            .inner_margin(egui::Margin::same(PAD as i8));
        let shown = frame.show(ui, |ui| {
            ui.set_width(picture.x);
            ui.label(egui::RichText::new(caption.clone()).font(caption_font.clone()).color(t.text_muted));
            ui.add_space(CAPTION_GAP);
            let (rect, resp) = ui.allocate_exact_size(picture, Sense::click());
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, &caption));
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 0.0, Color32::WHITE);
            if let Some(texture) = &peek.texture {
                // At its own pixel size from a whole pixel: one texel per screen pixel, never
                // resampled (which blurs small text). It is the picture's size within a pixel.
                let shown = Rect::from_min_size(rect.min.round_to_pixels(ppp), texture.size_vec2() / ppp);
                painter.image(texture.id(), shown, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            } else {
                let note = if peek.failed { tl!("This part of the page could not be drawn.") } else { tl!("Drawing…") };
                painter.text(rect.center(), Align2::CENTER_CENTER, note, theme::regular(12.0), t.text_muted);
            }
            painter.rect_stroke(rect, 0.0, Stroke::new(1.0, t.divider), egui::StrokeKind::Inside);
            let resp = theme::hand(resp).on_hover_text(tl!("Click to go there; right-click to close"));
            go = resp.clicked();
            resp
        });
        close = shown.response.secondary_clicked() || shown.inner.secondary_clicked();
    });
    if go {
        let (target, dest) = (peek.preview.page, peek.dest);
        view.link_peek = None;
        view.go_to_dest(target, dest, info);
    } else if close {
        view.link_peek = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peek_of(area: [f32; 4]) -> LinkPeek {
        let pool = RenderPool::new_inline(std::sync::Arc::new(b"not a pdf".to_vec()), Default::default());
        LinkPeek {
            source: (0, 0),
            link_rect: [0.0; 4],
            preview: LinkPreview { page: 1, area },
            dest: DestView::Top,
            label: "2".into(),
            generation: 0,
            pool,
            asked: None,
            texture: None,
            failed: false,
        }
    }

    #[test]
    fn the_picture_reads_at_the_pages_zoom_within_the_room_it_has() {
        let peek = peek_of([0.0, 100.0, 612.0, 345.0]);
        let room = vec2(1200.0, 450.0);
        // At the page's zoom while it fits.
        assert_eq!(peek.picture_size(1.5, room), vec2(918.0, 367.5));
        // Narrower than the window: fitted to it.
        let narrow = peek.picture_size(1.5, vec2(600.0, 450.0));
        assert!((narrow.x - 600.0).abs() < 0.01, "{narrow:?}");
        // Zoomed far out: still readable, at its least width.
        assert!((peek.picture_size(0.1, room).x - MIN_WIDTH).abs() < 0.01);
        // Nonsense zoom reads as 100%.
        assert_eq!(peek.picture_size(f32::NAN, room), vec2(612.0, 245.0));
    }

    #[test]
    fn a_zoomed_out_page_still_gets_a_readable_preview_on_whole_pixels() {
        use super::super::PT;
        // Zoomed out to 50 %: the preview is drawn at 100 % (actual size), times its size setting.
        assert_eq!(preview_zoom(Some(0.5 * PT), 1.0), PT);
        assert_eq!(preview_zoom(Some(0.5 * PT), 1.5), 1.5 * PT);
        // Zoomed in, it follows the page.
        assert_eq!(preview_zoom(Some(2.0 * PT), 1.0), 2.0 * PT);
        assert_eq!(preview_zoom(None, f32::NAN), PT);
        // Whole pixels at 1× and 1.5×.
        assert_eq!(snap(vec2(612.4, 244.8), 1.0), vec2(612.0, 245.0));
        let at_150 = snap(vec2(612.4, 244.8), 1.5) * 1.5;
        assert_eq!(at_150, at_150.round());
    }

    #[test]
    fn the_window_goes_below_the_link_then_above_it_and_stays_inside() {
        let viewport = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
        let size = vec2(300.0, 200.0);
        let near_top = Rect::from_min_size(pos2(100.0, 50.0), vec2(40.0, 12.0));
        assert_eq!(place(near_top, size, viewport), pos2(EDGE, 62.0 + GAP));
        let near_bottom = Rect::from_min_size(pos2(700.0, 500.0), vec2(40.0, 12.0));
        assert_eq!(place(near_bottom, size, viewport), pos2(800.0 - EDGE - 300.0, 500.0 - GAP - 200.0));
        // Taller than the window: as much of it in view as fits, from the top.
        let tall = place(near_top, vec2(300.0, 900.0), viewport);
        assert_eq!(tall.y, EDGE);
        // Wider than the window: from its left edge.
        assert_eq!(place(near_top, vec2(1000.0, 100.0), viewport).x, EDGE);
    }
}
