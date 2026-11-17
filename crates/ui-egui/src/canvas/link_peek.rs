//! The reference preview: with the Select tool, resting the pointer on a link to a place in this
//! document (a theorem, an equation, a cited work) for [`HOVER_DELAY`], or a right-click on it,
//! shows that place in a small window by the link, so the reader can look without leaving the
//! page they are on. A preview the pointer opened closes [`HOVER_GRACE`] after the pointer leaves
//! the link and the window, so the pointer can cross to the window; one a right-click opened
//! stays. Another right-click closes it, as does Escape (`app/keys.rs`); a click on the preview
//! goes there. Preferences ▸ Documents and view turns the pointer's previews off.
//!
//! A mention the text makes without a link ("by Lemma 2.1", "see (3.4)", "Section 3") previews the
//! same way when the document's reference graph ([`super::ref_graph`]) resolved it with at least
//! [`PREVIEW_CONFIDENCE`]: the window shows the named block's own extent and says "inferred" with
//! its title. Such a mention is only read where the pointer is; it never takes the pointer, a
//! press, the cursor or a text selection from the page. Preferences turn these previews off on
//! their own.
//!
//! What is shown is the block the destination names ([`pdfcraft_engine::link_preview`]): the
//! target page's glyphs are read first, then the block is drawn, both by a render thread of its
//! own ([`RenderPool::companion`]) with the page view's settings, so the page view's queue,
//! which it replaces every frame, is left alone. Each request replaces the one before. The
//! window shows once the block is known (or [`GLYPHS_PATIENCE`] has passed, and the band below
//! the destination shows). The preview closes when the document changes (its pages may be
//! gone), when a dialog comes over the pages, and when its link's page leaves the screen; the
//! pointer's also while the pages scroll, zoom or are dragged.

use egui::emath::GuiRounding as _;
use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, Vec2, pos2, vec2};
use pdfcraft_engine::link_preview::{LinkPreview, RefKind, RefName, link_preview};
use pdfcraft_engine::references::{BlockKind as RefBlockKind, PREVIEW_CONFIDENCE, ReferenceGraph, Source as RefSource};
use pdfcraft_render::{DestView, DocInfo, LinkTarget, PageInfo, RenderPool, RenderRequest, RequestKind, Tile};

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
/// How long the pointer rests on a link before its preview opens, in seconds: long enough that
/// moving across a page of references opens none, as a tooltip waits.
pub const HOVER_DELAY: f64 = 0.4;
/// How long a preview the pointer opened stays after the pointer leaves its link and its window,
/// in seconds: time to cross the gap between them.
pub const HOVER_GRACE: f64 = 0.3;
/// How long the window waits for the target page's glyphs before it shows the band below the
/// destination instead, in seconds.
pub const GLYPHS_PATIENCE: f64 = 0.5;
/// The tag of the glyph request: a picture's tag is its scale in thousandths, never this.
const GLYPHS_TAG: u64 = u64::MAX;

/// What a preview belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PeekSource {
    /// A link annotation to a place in this document: its page and its `/Annots` index (0-based).
    Link { page: usize, index: usize },
    /// A mention the text makes without a link ("by Lemma 2.1"): its page and its id in the
    /// document's reference graph ([`super::ref_graph`]).
    Mention { page: usize, id: usize },
}

impl PeekSource {
    /// The page it is on.
    pub fn page(self) -> usize {
        match self {
            PeekSource::Link { page, .. } | PeekSource::Mention { page, .. } => page,
        }
    }
}

/// A link to a place in this document, or a mention of one, under the pointer, and where it is on
/// screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pointed {
    pub source: PeekSource,
    pub on_screen: Rect,
}

/// How much room the picture of an inferred mention's block leaves around it, in points.
const BLOCK_MARGIN: f32 = 6.0;

/// Where a preview's link or mention is on its page, to place the window by it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Anchor {
    /// A link's rectangle, in user space.
    User([f32; 4]),
    /// A mention's box, in view space.
    View([f32; 4]),
}

/// The pointer's previews: the link it rests on and since when, and a link whose preview was
/// closed while the pointer is on it (which does not open again until the pointer leaves it).
#[derive(Clone, Copy, Debug, Default)]
pub struct LinkHover {
    resting: Option<(Pointed, f64)>,
    dismissed: Option<PeekSource>,
}

impl LinkHover {
    /// The preview of `source` was closed: it does not open again while the pointer stays on it.
    pub fn dismiss(&mut self, source: PeekSource) {
        self.resting = None;
        self.dismissed = Some(source);
    }
}

/// What opened a preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// The pointer rested on the link: the preview closes when the pointer goes.
    Hover,
    /// A right-click: the preview stays until closed.
    Click,
}

/// A preview on screen: the link it belongs to, what it shows, and the picture once drawn.
pub struct LinkPeek {
    /// The link or the mention.
    source: PeekSource,
    /// Where it is on its page, to place the window by it.
    anchor: Anchor,
    /// For a mention the text makes without a link: the title of the block it names
    /// (`Lemma 2.1`), which the caption gives with "inferred".
    inferred: Option<String>,
    /// The part of the target page shown.
    preview: LinkPreview,
    /// Where following the link goes, for a click on the preview.
    dest: DestView,
    /// The target page's label, for the caption.
    label: String,
    /// The target page, for fitting the area to its glyphs.
    target_page: PageInfo,
    /// What opened the preview.
    trigger: Trigger,
    /// When the target page's glyphs were asked for (`ctx` time, seconds).
    glyphs_asked: Option<f64>,
    /// The area is final: fitted to the block, or the band once the glyphs failed or were slow.
    settled: bool,
    /// Where the link was on screen when the pointer opened the preview.
    opened_at: Option<Rect>,
    /// When the pointer left the link and the window, while it stays away.
    left_at: Option<f64>,
    /// The pointer was over the window last frame.
    pointer_inside: bool,
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
    pub(crate) fn open(doc: &pdfcraft_engine::Document, page: usize, index: usize, trigger: Trigger) -> Option<Self> {
        let preview = link_preview(&doc.info, page, index)?;
        let link = doc.info.links.iter().find(|l| l.page == page && l.index == index)?;
        let LinkTarget::Page(_, dest) = link.target else { return None };
        let target_page = doc.info.pages.get(preview.page)?.clone();
        Some(Self {
            source: PeekSource::Link { page, index },
            anchor: Anchor::User(link.rect),
            inferred: None,
            preview,
            dest,
            label: target_page.label.clone(),
            target_page,
            trigger,
            glyphs_asked: None,
            settled: false,
            opened_at: None,
            left_at: None,
            pointer_inside: false,
            generation: doc.edit_generation(),
            pool: doc.renderer.companion(),
            asked: None,
            texture: None,
            failed: false,
        })
    }

    /// The preview of the mention `id` of `graph` on `page`: a mention the text makes without a
    /// link, resolved with at least [`PREVIEW_CONFIDENCE`]. It shows the block the mention names,
    /// its own extent on its first page, and a click there goes to it.
    pub(crate) fn open_mention(doc: &pdfcraft_engine::Document, graph: &ReferenceGraph, page: usize, id: usize, trigger: Trigger) -> Option<Self> {
        let m = graph.mentions.get(id).filter(|m| m.source == RefSource::Inferred && m.confidence >= PREVIEW_CONFIDENCE && m.page() == page)?;
        let block = graph.block(m.target?)?;
        let target_page = doc.info.pages.get(block.page())?.clone();
        let (w, h) = (target_page.width.max(1.0), target_page.height.max(1.0));
        let [x0, y0, x1, y1] = block.first_rect();
        if !([x0, y0, x1, y1].iter().all(|v| v.is_finite()) && x1 > x0 && y1 > y0) {
            return None;
        }
        let area = [(x0 - BLOCK_MARGIN).max(0.0), (y0 - BLOCK_MARGIN).max(0.0), (x1 + BLOCK_MARGIN).min(w), (y1 + BLOCK_MARGIN).min(h)];
        if area[2] <= area[0] || area[3] <= area[1] {
            return None;
        }
        // A click goes to the block's top, as a link to it would.
        let [left, top] = target_page.view_to_user(area[0], area[1]);
        let dest = DestView::Xyz { left: Some(left), top: Some(top), zoom: None };
        let kind = match block.kind {
            RefBlockKind::Equation => RefKind::Equation,
            RefBlockKind::Heading => RefKind::Section,
            RefBlockKind::Float if block.label == "Table" => RefKind::Table,
            RefBlockKind::Float => RefKind::Figure,
            RefBlockKind::Statement | RefBlockKind::Proof => RefKind::TheoremLike,
        };
        let preview =
            LinkPreview { page: block.page(), area, view: dest, name: None, reference: RefName { kind, number: block.number.clone() }, fitted: true };
        let on_page = m.extent.iter().find(|e| e.page == page)?.rect;
        Some(Self {
            source: PeekSource::Mention { page, id },
            anchor: Anchor::View(on_page),
            inferred: Some(block.title()),
            preview,
            dest,
            label: target_page.label.clone(),
            target_page,
            trigger,
            glyphs_asked: None,
            settled: false,
            opened_at: None,
            left_at: None,
            pointer_inside: false,
            generation: doc.edit_generation(),
            pool: doc.renderer.companion(),
            asked: None,
            texture: None,
            failed: false,
        })
    }

    /// What the preview belongs to: a link, or a mention the text makes without one.
    pub fn source(&self) -> PeekSource {
        self.source
    }

    /// For a mention the text makes without a link, the title of the block it names.
    pub fn inferred(&self) -> Option<&str> {
        self.inferred.as_deref()
    }

    /// The part of the document shown.
    pub fn preview(&self) -> &LinkPreview {
        &self.preview
    }

    /// Whether the picture has come back from the renderer.
    pub fn drawn(&self) -> bool {
        self.texture.is_some()
    }

    /// Whether the renderer could not draw the area.
    pub fn failed(&self) -> bool {
        self.failed
    }

    /// What opened the preview.
    pub fn trigger(&self) -> Trigger {
        self.trigger
    }

    /// Whether the area is final (the block, or the band), so the window shows.
    pub fn settled(&self) -> bool {
        self.settled
    }

    /// What the window's caption says: the kind and number the destination's name gives with the
    /// target page's label ("Equation 4.7 · page 12"), else where it goes. A statement's or a
    /// citation's first words say what it is, on the picture itself.
    pub fn caption(&self) -> String {
        let p = self.label.as_str();
        if let Some(t) = &self.inferred {
            return crate::i18n::fmt(tl!("{t} · page {p} · inferred"), &[("t", t), ("p", p)]);
        }
        let template = match (self.preview.reference.kind, self.preview.reference.number.as_deref()) {
            (_, None) => None,
            (RefKind::Equation, Some(_)) => Some(tl!("Equation {n} · page {p}")),
            (RefKind::Section, Some(_)) => Some(tl!("Section {n} · page {p}")),
            (RefKind::Figure, Some(_)) => Some(tl!("Figure {n} · page {p}")),
            (RefKind::Table, Some(_)) => Some(tl!("Table {n} · page {p}")),
            (RefKind::Item, Some(_)) => Some(tl!("Item {n} · page {p}")),
            (RefKind::TheoremLike | RefKind::Citation | RefKind::Page | RefKind::Unknown, Some(_)) => None,
        };
        match (template, self.preview.reference.number.as_deref()) {
            (Some(template), Some(n)) => crate::i18n::fmt(template, &[("n", n), ("p", p)]),
            _ => crate::i18n::fmt(tl!("Go to page {p}"), &[("p", p)]),
        }
    }

    /// Settle the area, at `now` (seconds): ask for the target page's glyphs once, fit the area
    /// to the block when they come back, and give up on them after [`GLYPHS_PATIENCE`]. `true`
    /// once the area is final.
    fn settle(&mut self, now: f64) -> bool {
        if self.settled {
            return true;
        }
        let Some(asked) = self.glyphs_asked else {
            if !self.preview.wants_glyphs() {
                self.settled = true;
                return true;
            }
            let request = RenderRequest { page: self.preview.page, kind: RequestKind::Glyphs, tag: GLYPHS_TAG, ..Default::default() };
            self.pool.set_queue(vec![request]);
            self.glyphs_asked = Some(now);
            return false;
        };
        while let Some(out) = self.pool.try_recv() {
            if out.request.tag != GLYPHS_TAG {
                continue;
            }
            // Glyphs that could not be read leave the band.
            if let Some(math) = out.glyphs {
                self.preview.fit(&self.target_page, &math);
            }
            self.settled = true;
        }
        // A clock that does not read as a number gives up at once.
        let waited = now - asked;
        if !self.settled && (waited >= GLYPHS_PATIENCE || waited.is_nan()) {
            self.settled = true;
        }
        self.settled
    }

    /// A watch on the preview's render thread, which tells when it has exited (it does once the
    /// preview is dropped: closed, its document closed, or the app gone).
    pub fn render_watch(&self) -> pdfcraft_render::PoolWatch {
        self.pool.watch()
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
            // A glyph request given up on comes back late: not a picture.
            if out.request.tag != tag || out.request.kind != RequestKind::Pixels {
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
/// under it that goes somewhere in this document, or the mention the text makes there without a
/// link (Select tool). An open preview closes, wherever the right-click is; otherwise the found
/// one's preview opens. Returns whether the right-click was the preview's: the page's menu stays
/// shut then.
pub(super) fn right_click(view: &mut DocView, doc: &pdfcraft_engine::Document, found: Option<PeekSource>) -> bool {
    // A right-click on the link whose preview the pointer opened keeps it open.
    if let Some(peek) = view.link_peek.as_mut().filter(|p| p.trigger == Trigger::Hover && Some(p.source) == found) {
        peek.trigger = Trigger::Click;
        return true;
    }
    if let Some(peek) = view.link_peek.take() {
        view.link_hover.dismiss(peek.source);
        return true;
    }
    let Some(found) = found else { return false };
    view.link_peek = open(view, doc, found, Trigger::Click);
    view.link_peek.is_some()
}

/// The preview of `source`: a link's, or a mention's from the document's reference graph.
fn open(view: &DocView, doc: &pdfcraft_engine::Document, source: PeekSource, trigger: Trigger) -> Option<LinkPeek> {
    match source {
        PeekSource::Link { page, index } => LinkPeek::open(doc, page, index, trigger),
        PeekSource::Mention { page, id } => {
            let graph = view.refs.graph(doc.edit_generation())?.clone();
            LinkPeek::open_mention(doc, &graph, page, id, trigger)
        }
    }
}

/// What the pointer does to the previews this frame. `pointed`: the link to a place in this
/// document under it, when a page found one (Select tool, no button down). `allowed`: the
/// preference is on and nothing stops a preview (no dialog, menu or palette over the pages, not
/// full screen, no text field holding the keys, the pages not scrolling). `pressed`: a button is
/// down. `now`: `ctx` time in seconds.
///
/// After the pointer rests on one link for [`HOVER_DELAY`], with the link still where it was
/// on screen (a scroll or a zoom starts the wait again), its preview opens. A preview the pointer
/// opened closes when previews are not allowed, when a button goes down outside it, when its
/// link moves on screen, and [`HOVER_GRACE`] after the pointer has left both the link and the
/// window.
pub(super) fn hover(
    ctx: &egui::Context,
    view: &mut DocView,
    doc: &pdfcraft_engine::Document,
    pointed: Option<Pointed>,
    allowed: bool,
    pressed: bool,
    now: f64,
) {
    let inside = view.link_peek.as_ref().is_some_and(|p| p.pointer_inside);
    // Over the window, a link under it is not pointed at.
    let pointed = pointed.filter(|_| !inside && allowed && !pressed);
    if view.link_hover.dismissed.is_some_and(|d| pointed.is_none_or(|p| p.source != d)) {
        view.link_hover.dismissed = None;
    }
    let pointer_peek = view.link_peek.as_ref().is_some_and(|p| p.trigger == Trigger::Hover);
    if pointer_peek {
        let close = view.link_peek.as_mut().is_some_and(|peek| {
            let on_link = pointed.is_some_and(|p| p.source == peek.source);
            let moved = pointed.filter(|_| on_link).zip(peek.opened_at).is_some_and(|(p, at)| p.on_screen != at);
            if !allowed || (pressed && !peek.pointer_inside) || moved {
                return true;
            }
            if on_link || peek.pointer_inside {
                peek.left_at = None;
                return false;
            }
            let left = *peek.left_at.get_or_insert(now);
            if now - left >= HOVER_GRACE {
                return true;
            }
            ctx.request_repaint_after(std::time::Duration::from_secs_f64((HOVER_GRACE - (now - left)).max(0.0)));
            false
        });
        if close {
            view.link_peek = None;
        }
    }
    let Some(p) = pointed else {
        view.link_hover.resting = None;
        return;
    };
    let shown = view.link_peek.as_ref().is_some_and(|peek| peek.source == p.source);
    if shown || view.link_hover.dismissed == Some(p.source) {
        view.link_hover.resting = None;
        return;
    }
    let since = match view.link_hover.resting {
        Some((resting, since)) if resting == p => since,
        _ => now,
    };
    let rested = now - since;
    if rested >= HOVER_DELAY {
        view.link_hover.resting = None;
        if let Some(mut peek) = open(view, doc, p.source, Trigger::Hover) {
            peek.opened_at = Some(p.on_screen);
            view.link_peek = Some(peek);
        }
    } else {
        view.link_hover.resting = Some((p, since));
        ctx.request_repaint_after(std::time::Duration::from_secs_f64((HOVER_DELAY - rested).max(0.0)));
    }
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
    let page = peek.source.page();
    let xform = info.pages.get(page).and(view.screen_xforms.iter().find(|(p, _)| *p == page).map(|(_, xf)| *xf));
    let link = xform.map(|xf| match peek.anchor {
        Anchor::User(r) => xf.user_rect(info, page, r),
        Anchor::View(r) => xf.view_rect(r),
    });
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
    // Nothing shows until the area is known: the window would change size under the pointer.
    let now = ctx.input(|i| i.time);
    if !peek.settle(now) {
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
        return;
    }
    let t = Tokens::get(&ctx);
    let room = vec2(viewport.width() - 2.0 * (EDGE + PAD), viewport.height() * (MAX_HEIGHT_SHARE * size).min(MAX_HEIGHT_SHARE_CAP));
    let ppp = ctx.pixels_per_point();
    let picture = snap(peek.picture_size(preview_zoom(xform.map(|xf| xf.zoom()), size), room), ppp);
    if peek.render(&ctx, picture, ppp) {
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }
    let caption = peek.caption();
    let caption_font = theme::medium(12.0);
    let caption_h = ctx.fonts_mut(|f| f.row_height(&caption_font));
    let size = picture + vec2(2.0 * PAD, 2.0 * PAD + caption_h + CAPTION_GAP);
    // On whole pixels, so the picture's texels fall on the screen's.
    let at = place(link, size, viewport).round_to_pixels(ppp);
    let mut go = false;
    let mut close = false;
    let mut inside = false;
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
        // The window's own response: whether the pointer is on it, so a preview the pointer
        // opened stays while it is.
        inside = shown.response.contains_pointer() || shown.inner.contains_pointer();
    });
    peek.pointer_inside = inside;
    if go {
        let (target, dest) = (peek.preview.page, peek.dest);
        view.link_peek = None;
        view.go_to_dest(target, dest, info);
    } else if close {
        let source = peek.source;
        view.link_peek = None;
        view.link_hover.dismiss(source);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peek_of(area: [f32; 4]) -> LinkPeek {
        let pool = RenderPool::new_inline(std::sync::Arc::new(b"not a pdf".to_vec()), Default::default());
        LinkPeek {
            source: PeekSource::Link { page: 0, index: 0 },
            anchor: Anchor::User([0.0; 4]),
            inferred: None,
            preview: LinkPreview {
                page: 1,
                area,
                view: DestView::Top,
                name: None,
                reference: pdfcraft_engine::link_preview::ref_name(""),
                fitted: false,
            },
            dest: DestView::Top,
            label: "2".into(),
            target_page: PageInfo { width: 612.0, height: 792.0, label: "2".into(), crop: [0.0, 0.0, 612.0, 792.0], rotation: 0 },
            trigger: Trigger::Click,
            glyphs_asked: None,
            settled: false,
            opened_at: None,
            left_at: None,
            pointer_inside: false,
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
