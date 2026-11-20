//! Organize ▸ Split two-page spreads: scanned pages that hold two book pages side by side become
//! two pages each (see `pdfcraft_engine::spreads`).
//!
//! The pages, only-landscape pages, the split position (found automatically or a fixed fraction),
//! a margin from the split line and the reading order. A preview draws the split line on the first
//! page that would be split and the pages either side of it.

use std::collections::HashMap;

use egui::{Align, Color32, CornerRadius, Layout, Rect, Stroke, pos2, vec2};
use pdfcraft_engine::spreads::{
    DEFAULT_LANDSCAPE_RATIO, MAX_SPLIT_MARGIN, ReadingOrder, SPLIT_FRACTION_RANGE, SplitPosition, SplitSource, SpreadOptions, SpreadPlan,
};
use pdfcraft_engine::{DocId, Edit};

use crate::pageboxes::{Range, Unit};
use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

/// The longest side of a preview page, in points on screen.
const PREVIEW_SIDE: f32 = 150.0;

/// Dialog state. The margin is kept in points.
#[derive(Clone, Debug, PartialEq)]
pub struct SpreadsDraft {
    pub range: Range,
    pub landscape_only: bool,
    /// Find each page's gutter; otherwise split at `fraction`.
    pub auto: bool,
    pub fraction: f64,
    pub margin: f64,
    pub unit: Unit,
    pub order: ReadingOrder,
    /// The draft was opened (the unit is seeded from Preferences once).
    pub seeded: bool,
}

impl Default for SpreadsDraft {
    fn default() -> Self {
        Self {
            range: Range::All,
            landscape_only: true,
            auto: true,
            fraction: 0.5,
            margin: 0.0,
            unit: Unit::Points,
            order: ReadingOrder::LeftToRight,
            seeded: false,
        }
    }
}

impl SpreadsDraft {
    /// What the dialog asks for. `current` is the current page and `count` the page count.
    pub fn options(&self, current: usize, count: usize) -> SpreadOptions {
        let pages = match self.range {
            Range::All => None,
            Range::Current => Some(vec![current.min(count.saturating_sub(1))]),
            Range::Pages(a, b) => Some((a.max(1) - 1..b.min(count)).collect()),
        };
        SpreadOptions {
            pages,
            landscape_only: self.landscape_only,
            min_ratio: DEFAULT_LANDSCAPE_RATIO,
            position: if self.auto { SplitPosition::Auto } else { SplitPosition::Fixed(self.fraction) },
            margin: self.margin,
            order: self.order,
        }
    }
}

/// What the preview shows, worked out again only when the options or the document change.
#[derive(Default)]
pub struct SpreadsPreview {
    key: Option<(DocId, u64, SpreadOptions)>,
    /// The pages shown, each with its plan when it would be split.
    pages: Vec<(usize, Option<SpreadPlan>)>,
    error: Option<String>,
    /// Page textures for one document generation.
    textures: HashMap<usize, egui::TextureHandle>,
    textures_for: Option<(DocId, u64)>,
}

fn texture(app: &PdfCraftApp, ctx: &egui::Context, id: DocId, page: usize) -> Option<egui::TextureHandle> {
    let doc = app.session.get(id)?;
    let p = doc.info.pages.get(page)?;
    let scale = PREVIEW_SIDE * ctx.pixels_per_point() / p.width.max(p.height).max(1.0);
    // Shown on screen only, like the viewer (AGENTS.md §1.4); nothing is saved from it.
    let config = pdfcraft_render::RenderConfig { password: doc.password.as_deref().map(std::sync::Arc::from), ..Default::default() }.on_screen();
    let mut r = pdfcraft_render::PageRenderer::new(doc.display.clone(), config);
    let out = r.render(pdfcraft_render::RenderRequest { page, kind: pdfcraft_render::RequestKind::Pixels, tile: None, scale, tag: 0, preview: true });
    if out.error.is_some() || out.width == 0 || out.height == 0 {
        return None;
    }
    let image = crate::canvas::texture_image([out.width as usize, out.height as usize], out.rgba);
    Some(ctx.load_texture(format!("spreads-preview-{page}"), image, egui::TextureOptions::LINEAR))
}

/// Bring the preview up to date with `options`.
fn refresh(app: &mut PdfCraftApp, ctx: &egui::Context, id: DocId, current: usize, options: &SpreadOptions) {
    let Some(generation) = app.session.get(id).map(|d| d.edit_generation()) else { return };
    let count = app.session.get(id).map_or(0, |d| d.info.pages.len());
    let key = (id, generation, options.clone());
    if app.spreads_preview.key.as_ref() != Some(&key) {
        let (pages, error) = match app.session.spread_pages(id, options) {
            Ok(chosen) => {
                let first = chosen.first().copied().unwrap_or(current);
                let shown: Vec<usize> =
                    [first.checked_sub(1), Some(first), first.checked_add(1)].into_iter().flatten().filter(|p| *p < count).collect();
                match app.session.plan_spreads_of(id, options, &shown) {
                    Ok(plans) => (shown.iter().map(|p| (*p, plans.iter().find(|x| x.page == *p).cloned())).collect(), None),
                    Err(e) => (Vec::new(), Some(e.to_string())),
                }
            }
            Err(e) => (Vec::new(), Some(e.to_string())),
        };
        let p = &mut app.spreads_preview;
        p.key = Some(key);
        p.pages = pages;
        p.error = error;
    }
    if app.spreads_preview.textures_for != Some((id, generation)) {
        app.spreads_preview.textures.clear();
        app.spreads_preview.textures_for = Some((id, generation));
    }
    let missing: Vec<usize> = app.spreads_preview.pages.iter().map(|(p, _)| *p).filter(|p| !app.spreads_preview.textures.contains_key(p)).collect();
    for page in missing {
        if let Some(t) = texture(app, ctx, id, page) {
            app.spreads_preview.textures.insert(page, t);
        }
    }
}

/// Draw the dialog body; returns (apply, cancel).
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> (bool, bool) {
    let Some((i, id)) = app.active_ids() else { return (false, true) };
    let current = app.views[i].current;
    let count = app.session.get(id).map_or(0, |d| d.info.pages.len()).max(1);
    if !app.spreads_draft.seeded {
        app.spreads_draft.unit = app.units;
        app.spreads_draft.seeded = true;
    }
    ui.label(egui::RichText::new(tl!("Split Two-Page Spreads")).font(theme::semibold(18.0)));
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(tl!(
            "Each page that holds two book pages side by side becomes two pages, each cropped to one half. The other half of each page stays in the file behind the crop box; it is hidden, not removed, as with Crop pages."
        ))
        .color(t.text_muted),
    );
    ui.add_space(8.0);
    let d = &mut app.spreads_draft;
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_max_width(360.0);
            egui::Grid::new("spreads-grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label(tl!("Pages"));
                ui.vertical(|ui| {
                    widgets::radio_value(ui, &mut d.range, Range::All, tl!("All"));
                    widgets::radio_value(
                        ui,
                        &mut d.range,
                        Range::Current,
                        crate::i18n::fmt(tl!("Current page ({n})"), &[("n", &(current + 1).to_string())]),
                    );
                    let (mut a, mut b) = match d.range {
                        Range::Pages(a, b) => (a, b),
                        _ => (1, count),
                    };
                    ui.horizontal(|ui| {
                        let on = matches!(d.range, Range::Pages(..));
                        if ui.add(widgets::radio(on, tl!("From"))).clicked() {
                            d.range = Range::Pages(a, b);
                        }
                        let ra = ui.add(egui::DragValue::new(&mut a).range(1..=count));
                        ui.label(tl!("to"));
                        let rb = ui.add(egui::DragValue::new(&mut b).range(1..=count));
                        if ra.changed() || rb.changed() {
                            d.range = Range::Pages(a.min(b), b.max(a));
                        }
                    });
                    ui.add(widgets::checkbox(&mut d.landscape_only, tl!("Only landscape pages (wider than 1.2 × their height)")));
                });
                ui.end_row();
                ui.label(tl!("Split at"));
                ui.vertical(|ui| {
                    widgets::radio_value(ui, &mut d.auto, true, tl!("The gutter, found on each page"));
                    widgets::radio_value(ui, &mut d.auto, false, tl!("A fixed position"));
                    ui.add_enabled_ui(!d.auto, |ui| {
                        let mut percent = d.fraction * 100.0;
                        let (lo, hi) = (SPLIT_FRACTION_RANGE.0 * 100.0, SPLIT_FRACTION_RANGE.1 * 100.0);
                        let r = ui.add(egui::Slider::new(&mut percent, lo..=hi).suffix("%").max_decimals(1).text(tl!("of the width from the left")));
                        if r.changed() {
                            d.fraction = (percent / 100.0).clamp(SPLIT_FRACTION_RANGE.0, SPLIT_FRACTION_RANGE.1);
                        }
                    });
                });
                ui.end_row();
                let l = ui.label(tl!("Margin"));
                ui.horizontal(|ui| {
                    let k = d.unit.per_point();
                    let mut v = d.margin * k;
                    let r = ui
                        .add(
                            egui::DragValue::new(&mut v)
                                .speed(d.unit.speed())
                                .range(0.0..=MAX_SPLIT_MARGIN * k)
                                .max_decimals(3)
                                .suffix(format!(" {}", d.unit.label())),
                        )
                        .labelled_by(l.id);
                    if r.changed() {
                        d.margin = (v / k).clamp(0.0, MAX_SPLIT_MARGIN);
                    }
                    ui.label(egui::RichText::new(tl!("kept from the split line on each half")).color(t.text_faint));
                });
                ui.end_row();
                ui.label(tl!("Reading order"));
                ui.vertical(|ui| {
                    widgets::radio_value(ui, &mut d.order, ReadingOrder::LeftToRight, tl!("Left to right (left half first)"));
                    widgets::radio_value(ui, &mut d.order, ReadingOrder::RightToLeft, tl!("Right to left (right half first)"));
                });
                ui.end_row();
            });
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(tl!(
                    "Bookmarks, links and page labels that went to a split page go to its first half. Comments and fields go to the half they are on; one across the split is on both."
                ))
                .color(t.text_faint)
                .size(11.5),
            );
        });
    });
    let options = app.spreads_draft.options(current, count);
    let ctx = ui.ctx().clone();
    refresh(app, &ctx, id, current, &options);
    ui.add_space(10.0);
    preview(ui, app, t, id);
    ui.add_space(12.0);
    let (mut apply, mut cancel) = (false, false);
    let can = app.spreads_preview.error.is_none();
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if ui.add_enabled_ui(can, |ui| widgets::pill_button(ui, tl!("Split pages"), true)).inner.clicked() {
            apply = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            cancel = true;
        }
    });
    (apply, cancel)
}

/// The pages shown, each with its split line (and the margin either side of it) when it would be split.
fn preview(ui: &mut egui::Ui, app: &PdfCraftApp, t: &Tokens, id: DocId) {
    let p = &app.spreads_preview;
    if let Some(e) = &p.error {
        ui.label(egui::RichText::new(e).color(t.text));
        return;
    }
    ui.horizontal(|ui| {
        for (page, plan) in &p.pages {
            let Some(info) = app.session.get(id).and_then(|d| d.info.pages.get(*page)) else { continue };
            let (w, h) = (info.width.max(1.0), info.height.max(1.0));
            let s = PREVIEW_SIDE / w.max(h);
            ui.vertical(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(PREVIEW_SIDE + 10.0, PREVIEW_SIDE + 10.0), egui::Sense::hover());
                let rect = Rect::from_center_size(r.center(), vec2(w * s, h * s));
                match p.textures.get(page) {
                    Some(tex) => {
                        ui.painter().image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                    }
                    None => {
                        ui.painter().rect_filled(rect, CornerRadius::ZERO, Color32::WHITE);
                    }
                }
                ui.painter().rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.0, t.border), egui::StrokeKind::Outside);
                let caption = match plan {
                    Some(plan) => {
                        let x = rect.left() + rect.width() * plan.fraction as f32;
                        let m = (app.spreads_draft.margin as f32 * s).min(rect.width() / 2.0);
                        if m > 0.0 {
                            let band = Rect::from_min_max(pos2(x - m, rect.top()), pos2(x + m, rect.bottom()));
                            ui.painter().rect_filled(band, CornerRadius::ZERO, t.accent.gamma_multiply(0.25));
                        }
                        ui.painter().line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(2.0, t.accent));
                        match plan.source {
                            SplitSource::Centre => tl!("No clear gutter: centre").to_string(),
                            SplitSource::Gutter => {
                                crate::i18n::fmt(tl!("Gutter found ({n}% sure)"), &[("n", &format!("{:.0}", plan.confidence * 100.0))])
                            }
                            SplitSource::Fixed => tl!("Fixed position").to_string(),
                        }
                    }
                    None => tl!("Left as it is").to_string(),
                };
                ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Page {label}"), &[("label", &(page + 1).to_string())])).size(11.0));
                ui.label(egui::RichText::new(caption).size(11.0).color(t.text_faint));
            });
        }
    });
}

/// Split the pages the draft chooses, as one undoable edit, and say what happened.
pub(crate) fn apply(app: &mut PdfCraftApp) {
    let Some((i, id)) = app.active_ids() else { return };
    let count = app.session.get(id).map_or(0, |d| d.info.pages.len());
    let options = app.spreads_draft.options(app.views[i].current, count);
    let plans = match app.session.plan_spreads(id, &options) {
        Ok(p) if p.is_empty() => {
            app.notify_tr("No page in the range is a two-page spread; nothing was split");
            return;
        }
        Ok(p) => p,
        Err(e) => {
            app.notify_error(e);
            return;
        }
    };
    let edit = Edit::SplitSpreads { splits: plans.iter().map(|p| (p.page, p.fraction)).collect(), margin: options.margin, order: options.order };
    if app.apply_edit(edit) {
        let centred = plans.iter().filter(|p| p.source == SplitSource::Centre).count();
        let n = plans.len().to_string();
        if centred == 0 {
            app.notify_fmt("Split {n} pages in two", &[("n", &n)]);
        } else {
            app.notify_fmt("Split {n} pages in two; {k} had no clear gutter and were split in the centre", &[("n", &n), ("k", &centred.to_string())]);
        }
    }
}
