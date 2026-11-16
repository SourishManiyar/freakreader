//! Link tools: list, add, edit and delete links; create links from URLs in the text; remove
//! all links. Rectangles are points from the top-left of the displayed page.

use pdfcraft_engine::{
    Edit, LinkAction, LinkHighlight, LinkStyle, LinkZoom, read_remote_document, remote_document_path_is_absolute, resolve_remote_document_path,
};
use pdfcraft_render::{DestView, LinkTarget, RemoteDestination};
use serde_json::{Value, json};
use std::path::Path;

use crate::comments::parse_color;
use crate::{Args, Automation, Result, ToolError, failed};

/// Where `destination` puts `doc`, a document just opened from a link or with open parameters:
/// whether it applies, the destination as reported, and a warning when it doesn't. A named
/// destination is looked up in the document, as the app does.
pub(crate) fn destination_report(doc: &pdfcraft_engine::Document, destination: RemoteDestination) -> (bool, Value, Option<&'static str>) {
    let (name, page, view) = match destination {
        RemoteDestination::Default => return (true, json!({ "kind": "default" }), None),
        RemoteDestination::Page { page, view } => (None, page, view),
        RemoteDestination::Named(name) => match pdfcraft_render::named_destination(doc.bytes.clone(), doc.password.as_deref(), &name) {
            Some((page, view)) => (Some(name), page, view),
            None => return (false, json!({ "kind": "named", "name": name }), Some("The PDF opened, but the named destination was not applied.")),
        },
    };
    let mut value = json!({ "kind": "page", "page": page.saturating_add(1), "view": dest_view_value(view) });
    if let Some(name) = name {
        value["kind"] = json!("named");
        value["name"] = json!(name);
    }
    if page < doc.info.pages.len() { (true, value, None) } else { (false, value, Some("The PDF opened, but the page destination is out of range.")) }
}

fn bad(m: impl Into<String>) -> ToolError {
    ToolError::InvalidArgs(m.into())
}

impl Automation {
    fn view_rect(&self, a: &Args, page: usize) -> Result<Option<[f64; 4]>> {
        let Some(v) = a.get("rect") else { return Ok(None) };
        let r: Vec<f64> = v.as_array().map(|x| x.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
        let r = <[f64; 4]>::try_from(r).map_err(|_| bad("rect must be 4 numbers"))?;
        let p = &self.doc(a)?.info.pages[page];
        let (u0, u1) = (p.view_to_user(r[0] as f32, r[1] as f32), p.view_to_user(r[2] as f32, r[3] as f32));
        Ok(Some([u0[0].min(u1[0]) as f64, u0[1].min(u1[1]) as f64, u0[0].max(u1[0]) as f64, u0[1].max(u1[1]) as f64]))
    }

    /// The action `url`, `to_page` and `zoom` ask for, if any. `current` is the link's action now
    /// (link_edit): a new page keeps the link's zoom, and a zoom alone keeps its page.
    fn link_action(&self, a: &Args, current: Option<&LinkAction>) -> Result<Option<LinkAction>> {
        let zoom = link_zoom(a)?;
        let (current_page, current_zoom) = match current {
            Some(LinkAction::Page { page, zoom }) => (Some(*page), Some(*zoom)),
            _ => (None, None),
        };
        match (a.opt_str("url")?, a.opt_int("to_page")?) {
            (Some(_), Some(_)) => Err(bad("pass url or to_page, not both")),
            (Some(_), None) if zoom.is_some() => Err(bad("zoom goes with to_page, not url")),
            (Some(u), None) => Ok(Some(LinkAction::Uri(u.to_string()))),
            (None, Some(p)) => {
                let n = self.doc(a)?.info.pages.len() as i64;
                if p < 1 || p > n {
                    return Err(bad(format!("to_page must be 1–{n}")));
                }
                Ok(Some(LinkAction::Page { page: p as usize - 1, zoom: zoom.or(current_zoom).unwrap_or(LinkZoom::FitPage) }))
            }
            (None, None) => match (zoom, current_page) {
                (None, _) => Ok(None),
                (Some(zoom), Some(page)) => Ok(Some(LinkAction::Page { page, zoom })),
                (Some(_), None) => Err(bad("zoom needs to_page: the link doesn't go to a page")),
            },
        }
    }

    fn link_style(&self, a: &Args, base: LinkStyle) -> Result<LinkStyle> {
        let mut s = base;
        if let Some(v) = a.opt_bool("visible")? {
            s.visible = v;
        }
        if let Some(c) = a.opt_str("color")? {
            s.color = parse_color(c)?;
        }
        if let Some(w) = a.opt_num("width")? {
            s.width = w.clamp(1.0, 3.0);
        }
        if let Some(h) = a.opt_str("highlight")? {
            s.highlight = match h {
                "none" => LinkHighlight::None,
                "invert" => LinkHighlight::Invert,
                "outline" => LinkHighlight::Outline,
                "inset" => LinkHighlight::Inset,
                o => return Err(bad(format!("unknown highlight {o:?}"))),
            };
        }
        Ok(s)
    }

    pub(crate) fn link_list(&self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let viewed: std::collections::HashMap<(usize, usize), &pdfcraft_render::Link> =
            doc.info.links.iter().map(|t| ((t.page, t.index), t)).collect();
        let items: Vec<Value> = doc
            .links()
            .iter()
            .map(|l| {
                // The link's page may be one the viewer could not read (#2660): rect unchanged then.
                let rect = crate::view_rect(&doc.info, l.page, l.rect.map(|x| x as f32));
                let mut v = json!({ "page": l.page + 1, "index": l.index + 1, "rect": rect, "visible": l.style.visible });
                let link = viewed.get(&(l.page, l.index));
                // Where the link is active when that isn't its whole rect: its /QuadPoints
                // (ISO 32000-2 Table 176), as the bounding box of each quadrilateral.
                if let Some(link) = link.filter(|k| !k.quads.is_empty()) {
                    let areas: Vec<Value> = link
                        .quads
                        .iter()
                        .map(|q| {
                            let (xs, ys) = ([q[0], q[2], q[4], q[6]], [q[1], q[3], q[5], q[7]]);
                            let fold = |v: [f32; 4], f: fn(f32, f32) -> f32, start: f32| v.into_iter().fold(start, f);
                            let box_ = [
                                fold(xs, f32::min, f32::MAX),
                                fold(ys, f32::min, f32::MAX),
                                fold(xs, f32::max, f32::MIN),
                                fold(ys, f32::max, f32::MIN),
                            ];
                            json!(crate::view_rect(&doc.info, l.page, box_))
                        })
                        .collect();
                    v["active_areas"] = json!(areas);
                }
                // Where an in-document link goes, as the viewer resolves it: named destinations
                // included, with the position on the page.
                let target = link.and_then(|t| match &t.target {
                    LinkTarget::Page(page, view) => Some((*page, view)),
                    _ => None,
                });
                match (&l.action, target) {
                    (LinkAction::Uri(u), _) => {
                        v["url"] = json!(u);
                        // A relative address, resolved against the catalog's /URI /Base
                        // (ISO 32000-2 Table 211): what the viewer offers to open. That is the
                        // address inspection kept, which stays raw once its text budget is spent;
                        // resolve afresh only for a link inspection did not list.
                        let opened = match link.map(|t| &t.target) {
                            Some(LinkTarget::Uri(kept)) => Some(kept.clone()),
                            Some(_) => None,
                            None => pdfcraft_render::resolve_uri(doc.info.uri_base.as_deref(), u),
                        };
                        if let Some(opened) = opened.filter(|o| o != u) {
                            v["resolved_url"] = json!(opened);
                        }
                    }
                    (action, Some((page, view))) => {
                        v["to_page"] = json!(page + 1);
                        if let Some(d) = doc.info.pages.get(page).and_then(|p| crate::destination(view, p)) {
                            v["destination"] = d;
                        }
                        match action {
                            LinkAction::Page { zoom, .. } => v["zoom"] = zoom_value(*zoom),
                            LinkAction::Other(_) => v["named"] = json!(true),
                            LinkAction::Uri(_) => {}
                        }
                        // The named destination it goes through (hyperref's `equation.4.7`), as
                        // the viewer resolved it.
                        if let Some(name) = link.and_then(|t| t.dest_name.as_deref()) {
                            v["named"] = json!(true);
                            v["dest_name"] = json!(name);
                            let reference = pdfcraft_engine::link_preview::ref_name(name);
                            v["dest_kind"] = json!(reference.kind.as_str());
                            if let Some(n) = reference.number {
                                v["dest_number"] = json!(n);
                            }
                        }
                    }
                    (LinkAction::Page { page, zoom }, None) => {
                        v["to_page"] = json!(page + 1);
                        v["zoom"] = zoom_value(*zoom);
                    }
                    (LinkAction::Other(o), None) => v["other"] = json!(o),
                }
                v
            })
            .collect();
        let mut out = json!({ "count": items.len(), "links": items });
        // Addresses written as plain page text, which the viewer makes clickable (#4207). Reading
        // the pages' text costs more than the list above, so only on request.
        if a.opt_bool("text_links")?.unwrap_or(false) {
            let id = doc.id;
            // One share of the pages at a time (`MAX_TEXT_LINK_PAGES_PER_SCAN`), from the 1-based
            // `text_links_from`; `text_links_next_page` says where the next share starts.
            let pages = doc.info.pages.len().max(1);
            let from = match a.opt_int("text_links_from")? {
                None => 0,
                Some(p) => usize::try_from(p)
                    .ok()
                    .filter(|p| (1..=pages).contains(p))
                    .map(|p| p - 1)
                    .ok_or_else(|| bad(format!("text_links_from must be a page from 1 to {pages}")))?,
            };
            let scan = self.session.text_links(id, from);
            if let Some(next) = scan.next_page {
                out["text_links_next_page"] = json!(next.saturating_add(1));
            }
            let found: Vec<Value> = scan
                .links
                .into_iter()
                .map(|(page, l)| {
                    let mut v = json!({ "page": page + 1, "rects": l.rects, "url": l.uri });
                    // What the viewer does on a click: the same check every document link gets.
                    if let Err(e) = pdfcraft_engine::links::document_url(&l.uri) {
                        v["blocked"] = json!(e.to_string());
                    }
                    v
                })
                .collect();
            out["text_links"] = json!(found);
        }
        Ok(out)
    }

    pub(crate) fn link_open(&mut self, a: &Args) -> Result<Value> {
        let page_number = a.int("page")?;
        let index_number = a.int("index")?;
        let page = usize::try_from(page_number.checked_sub(1).filter(|n| *n >= 0).ok_or_else(|| bad("page must be 1 or more"))?)
            .map_err(|_| bad("page is out of range"))?;
        let index = usize::try_from(index_number.checked_sub(1).filter(|n| *n >= 0).ok_or_else(|| bad("index must be 1 or more"))?)
            .map_err(|_| bad("index is out of range"))?;
        let (source_path, file, destination, new_window) = {
            let source = self.doc(a)?;
            if page >= source.info.pages.len() {
                return Err(bad(format!("page {page_number} is out of range: the document has {} pages", source.info.pages.len())));
            }
            let link = source
                .info
                .links
                .iter()
                .find(|link| link.page == page && link.index == index)
                .ok_or_else(|| failed(format!("page {page_number} has no link {index_number} (see link_list)")))?;
            let LinkTarget::RemoteGoTo { file, destination, new_window, .. } = &link.target else {
                return Err(bad("the selected link is not a supported local PDF GoToR link"));
            };
            (source.path.clone(), file.clone(), destination.clone(), *new_window)
        };
        let path = resolve_remote_document_path(source_path.as_deref().map(Path::new), &file).map_err(failed)?;
        let allow_absolute = a.opt_bool("allow_absolute")?.unwrap_or(false);
        if remote_document_path_is_absolute(&file) && !allow_absolute {
            return Err(failed(
                "this GoToR link names an absolute local path; review the target with link_list and pass allow_absolute: true to open it",
            ));
        }
        let path = self.resolve(&path.to_string_lossy(), false)?;
        let bytes = read_remote_document(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let id =
            self.session.open(name, Some(path.to_string_lossy().into_owned()), std::sync::Arc::new(bytes), a.opt_str("password")?).map_err(failed)?;
        let opened = self.session.get(id).ok_or_else(|| failed("the linked document vanished"))?;
        let (destination_applied, destination_value, warning) = destination_report(opened, destination);
        let mut display_path = path.to_string_lossy().into_owned();
        if let Some(root) = &self.root
            && let Ok(relative) = path.strip_prefix(root)
        {
            display_path = relative.to_string_lossy().into_owned();
        }
        Ok(json!({
            "document": crate::summary(opened),
            "path": display_path,
            "destination_applied": destination_applied,
            "destination": destination_value,
            "warning": warning,
            "new_window_hint": new_window,
        }))
    }

    pub(crate) fn link_add(&mut self, a: &Args) -> Result<Value> {
        let page = self.page(a)?;
        let rect = self.view_rect(a, page)?.ok_or_else(|| bad("link_add needs rect"))?;
        let action = self.link_action(a, None)?.ok_or_else(|| bad("pass url or to_page"))?;
        let style = self.link_style(a, LinkStyle::default())?;
        self.apply(a, Edit::AddLink { page, rect, action, style })
    }

    /// The link `page` and `index` name: its page, index in `/Annots`, appearance and action.
    fn link_index(&self, a: &Args) -> Result<(usize, usize, LinkStyle, LinkAction)> {
        let page = self.page(a)?;
        let index = a.int("index")?;
        let doc = self.doc(a)?;
        let l = doc
            .links()
            .iter()
            .find(|l| l.page == page && l.index as i64 + 1 == index)
            .ok_or_else(|| failed(format!("page {} has no link {index} (see link_list)", page + 1)))?;
        Ok((page, l.index, l.style, l.action.clone()))
    }

    pub(crate) fn link_edit(&mut self, a: &Args) -> Result<Value> {
        let (page, index, style, current) = self.link_index(a)?;
        let rect = self.view_rect(a, page)?;
        let action = self.link_action(a, Some(&current))?;
        let new_style = self.link_style(a, style)?;
        self.apply(a, Edit::SetLink { page, index, rect, action, style: (new_style != style).then_some(new_style) })
    }

    pub(crate) fn link_delete(&mut self, a: &Args) -> Result<Value> {
        let (page, index, ..) = self.link_index(a)?;
        self.apply(a, Edit::DeleteLink { page, index })
    }

    pub(crate) fn links_from_urls(&mut self, a: &Args) -> Result<Value> {
        let id = self.doc(a)?.id;
        let found = self.session.find_urls(id);
        if found.is_empty() {
            return Err(failed("no unlinked web addresses were found in the text"));
        }
        let urls: Vec<String> = found.iter().map(|f| f.2.clone()).collect();
        let mut out = self.apply(a, Edit::AddLinks { links: found, style: LinkStyle::default() })?;
        out["created"] = json!(urls);
        Ok(out)
    }

    pub(crate) fn links_remove(&mut self, a: &Args) -> Result<Value> {
        let pages = match a.opt_ints("pages")? {
            Some(_) => Some(self.pages(a, "pages")?),
            None => None,
        };
        let before = match &pages {
            Some(ps) => self.doc(a)?.links().iter().filter(|l| ps.contains(&l.page)).count(),
            None => self.doc(a)?.links().len(),
        };
        let mut out = self.apply(a, Edit::RemoveLinks { pages })?;
        out["removed"] = json!(before);
        Ok(out)
    }
}

/// The zoom a `zoom` argument names: fit_page, fit_width, fit_height, fit_visible, actual
/// (100%), inherit (keep the reader's zoom), or a percentage, kept within 1–6400.
fn link_zoom(a: &Args) -> Result<Option<LinkZoom>> {
    match a.get("zoom") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(p)) => match p.as_f64() {
            Some(p) if p.is_finite() && p > 0.0 => Ok(Some(LinkZoom::Percent(p.clamp(1.0, 6400.0)))),
            _ => Err(bad("zoom must be a positive percentage")),
        },
        Some(Value::String(z)) => Ok(Some(match z.as_str() {
            "fit_page" => LinkZoom::FitPage,
            "fit_width" => LinkZoom::FitWidth,
            "fit_height" => LinkZoom::FitHeight,
            "fit_visible" => LinkZoom::FitVisible,
            "actual" => LinkZoom::Percent(100.0),
            "inherit" => LinkZoom::Inherit,
            o => return Err(bad(format!("unknown zoom {o:?}"))),
        })),
        Some(_) => Err(bad("zoom is a name or a percentage")),
    }
}

/// How `link_list` reports a link's zoom: the names `link_zoom` reads, a percentage as a number.
fn zoom_value(zoom: LinkZoom) -> Value {
    match zoom {
        LinkZoom::FitPage => json!("fit_page"),
        LinkZoom::FitWidth => json!("fit_width"),
        LinkZoom::FitHeight => json!("fit_height"),
        LinkZoom::FitVisible => json!("fit_visible"),
        LinkZoom::Inherit => json!("inherit"),
        LinkZoom::Percent(p) => json!(p),
    }
}

fn dest_view_value(view: DestView) -> Value {
    match view {
        DestView::Top => json!({ "kind": "top" }),
        DestView::Xyz { left, top, zoom } => json!({ "kind": "xyz", "left": left, "top": top, "zoom": zoom }),
        DestView::Fit => json!({ "kind": "fit" }),
        DestView::FitH { top } => json!({ "kind": "fit_h", "top": top }),
        DestView::FitV { left } => json!({ "kind": "fit_v", "left": left }),
        DestView::FitR { rect } => json!({ "kind": "fit_r", "rect": rect }),
    }
}

/// Most named destinations `dest_list` returns in one call.
const MAX_DEST_LIST_LIMIT: usize = 10_000;
/// How many it returns when the call doesn't say.
const DEFAULT_DEST_LIST_LIMIT: usize = 1_000;

impl Automation {
    /// `dest_list`: the document's named destinations (hyperref's `equation.4.7`, `cite.knuth`),
    /// in page order: { count, destinations: [{ name, page, page_label, destination, kind,
    /// number? }], next_from?, left_out, unresolved }. `from` (1-based) and `limit` page through
    /// a long list. Does not modify the document.
    pub(crate) fn dest_list(&self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let from = match a.opt_int("from")? {
            None => 0,
            Some(n) => usize::try_from(n).ok().and_then(|n| n.checked_sub(1)).ok_or_else(|| bad("from must be 1 or more"))?,
        };
        let limit = match a.opt_int("limit")? {
            None => DEFAULT_DEST_LIST_LIMIT,
            Some(n) => usize::try_from(n)
                .ok()
                .filter(|n| (1..=MAX_DEST_LIST_LIMIT).contains(n))
                .ok_or_else(|| bad(format!("limit must be from 1 to {MAX_DEST_LIST_LIMIT}")))?,
        };
        let found = pdfcraft_render::named_destinations(doc.display.clone(), doc.password.as_deref())
            .ok_or_else(|| failed("the document's named destinations could not be read"))?;
        let total = found.list.len();
        let items: Vec<Value> = found
            .list
            .iter()
            .skip(from)
            .take(limit)
            .map(|d| {
                let reference = pdfcraft_engine::link_preview::ref_name(&d.name);
                let page = doc.info.pages.get(d.page);
                let mut v = json!({ "name": d.name, "page": d.page + 1, "kind": reference.kind.as_str() });
                if let Some(page) = page {
                    v["page_label"] = json!(page.label);
                    if let Some(position) = crate::destination(&d.view, page) {
                        v["destination"] = position;
                    }
                }
                if let Some(n) = reference.number {
                    v["number"] = json!(n);
                }
                v
            })
            .collect();
        let mut out = json!({ "count": total, "destinations": items, "left_out": found.left_out, "unresolved": found.unresolved });
        if from.saturating_add(limit) < total {
            out["next_from"] = json!(from + limit + 1);
        }
        Ok(out)
    }

    /// `link_preview`: what an in-document link (`page` and `index`) or a named destination
    /// (`dest`) points at, as the viewer's reference preview shows it: a PNG of the target page's
    /// area, then { page, label, caption, area, fitted, kind, name?, number?, warnings? }, the
    /// page 1-based and the area in points from its top-left. With `fit` (the default) the area is
    /// the block the destination names, read from the page's glyphs, when the page shows one.
    /// Does not modify the document.
    pub(crate) fn link_preview(&mut self, a: &Args) -> Result<Vec<crate::Content>> {
        use crate::pages::{DEFAULT_DPI, MAX_DPI, rendered_content};
        let id = self.doc(a)?.id;
        let dpi = a.opt_num("dpi")?.unwrap_or(DEFAULT_DPI);
        if !(1.0..=MAX_DPI).contains(&dpi) {
            return Err(ToolError::InvalidArgs(format!("dpi must be between 1 and {MAX_DPI}")));
        }
        let fit = a.opt_bool("fit")?.unwrap_or(true);
        let mut preview = match (a.opt_str("dest")?, a.get("page").is_some() || a.get("index").is_some()) {
            (Some(_), true) => return Err(bad("pass dest, or page and index, not both")),
            (Some(name), false) => {
                let doc = self.doc(a)?;
                let (page, view) = pdfcraft_render::named_destination(doc.display.clone(), doc.password.as_deref(), name)
                    .ok_or_else(|| bad(format!("the document has no named destination {name:?} that goes to one of its pages (see dest_list)")))?;
                pdfcraft_engine::link_preview::dest_preview(&doc.info, page, view, Some(name))
                    .ok_or_else(|| bad(format!("the named destination {name:?} goes to page {}, which the document does not have", page + 1)))?
            }
            (None, _) => {
                let page = self.page(a)?;
                let number = a.int("index")?;
                let index = usize::try_from(number)
                    .ok()
                    .and_then(|n| n.checked_sub(1))
                    .ok_or_else(|| ToolError::InvalidArgs("index must be a positive link index from link_list".into()))?;
                let doc = self.doc(a)?;
                match pdfcraft_engine::link_preview::link_preview(&doc.info, page, index) {
                    Some(preview) => preview,
                    None => {
                        let exists = doc.info.links.iter().any(|l| l.page == page && l.index == index);
                        return Err(ToolError::InvalidArgs(if exists {
                            format!(
                                "link {number} on page {} does not go to a page of this document; only links that link_list gives a to_page have a preview",
                                page + 1
                            )
                        } else {
                            format!("page {} has no link {number} (see link_list)", page + 1)
                        }));
                    }
                }
            }
        };
        let page_info = self.doc(a)?.info.pages.get(preview.page).cloned().ok_or_else(|| failed("the target page is gone"))?;
        let mut warnings = Vec::new();
        if fit && preview.wants_glyphs() {
            match self.renderer(id)?.glyph_details(preview.page) {
                Ok(math) => {
                    preview.fit(&page_info, &math);
                }
                Err(e) => warnings.push(format!("the target page's glyphs could not be read ({e}): the band below the destination is shown")),
            }
        }
        let label = page_info.label.clone();
        let scale = (dpi / 72.0) as f32;
        let [x0, y0, x1, y1] = preview.area;
        let tile = pdfcraft_render::Tile {
            x: (x0 * scale).floor().max(0.0) as u32,
            y: (y0 * scale).floor().max(0.0) as u32,
            w: ((x1 - x0) * scale).ceil().max(1.0) as u32,
            h: ((y1 - y0) * scale).ceil().max(1.0) as u32,
        };
        // The renderer draws at most this many pixels a side in one tile, and cuts a larger one short.
        const MAX_TILE_SIDE: u32 = 4096;
        if tile.w > MAX_TILE_SIDE || tile.h > MAX_TILE_SIDE {
            return Err(ToolError::InvalidArgs(format!(
                "at {dpi} dpi the preview would be {}×{} pixels, more than {MAX_TILE_SIDE} a side: use a lower dpi",
                tile.w, tile.h
            )));
        }
        let request = pdfcraft_render::RenderRequest { page: preview.page, tile: Some(tile), scale, ..Default::default() };
        let out = self.renderer(id)?.render(request);
        if let Some(e) = out.error {
            return Err(failed(format!("page {} could not be rendered: {e}", preview.page + 1)));
        }
        let mut content = rendered_content(&out, preview.page)?;
        let mut place = json!({
            "page": preview.page + 1,
            "label": label,
            "caption": preview.label(&label),
            "area": preview.area,
            "fitted": preview.fitted,
            "kind": preview.reference.kind.as_str(),
        });
        if let Some(name) = &preview.name {
            place["name"] = json!(name);
        }
        if let Some(n) = &preview.reference.number {
            place["number"] = json!(n);
        }
        // One JSON part: the render's warnings, when there are any, join the place.
        match content.last_mut() {
            Some(crate::Content::Json(v)) => {
                if let (Some(v), Value::Object(place)) = (v.as_object_mut(), place) {
                    v.extend(place);
                }
            }
            _ => content.push(crate::Content::Json(place)),
        }
        if !warnings.is_empty()
            && let Some(crate::Content::Json(v)) = content.last_mut()
        {
            let mut all: Vec<Value> = v.get("warnings").and_then(Value::as_array).cloned().unwrap_or_default();
            all.extend(warnings.into_iter().map(Value::from));
            v["warnings"] = json!(all);
        }
        Ok(content)
    }
}
