//! The outline as a flat list of rows, and Create Table of Contents: `bookmark_rows`,
//! `bookmark_edit_rows` and `bookmark_create_toc`.
//!
//! A row is `level` (1 for the top level), `title`, `page` (1-based) and optionally `top` and
//! `left` (points from the top-left of the displayed page, as `bookmark_add` takes them). In
//! `bookmark_edit_rows` a row may name the existing bookmark it stands for with `from` (its
//! 1-based path in `bookmark_rows`), which keeps that bookmark's colour, style, actions and
//! anything else the row doesn't change; then a `page` left out keeps its destination. The list
//! given is the whole new outline, written as one undo step.
use std::collections::HashMap;

use crate::json::destination;
use crate::{Args, Automation, Result, ToolError, failed};
use pdfcraft_engine::toc::{self, PageRef, TocSource};
use pdfcraft_engine::{Edit, OutlineRow, RowTarget};
use serde_json::{Map, Value, json};

/// The keys a row may have.
const ROW_KEYS: [&str; 6] = ["level", "title", "page", "top", "left", "from"];
/// Longest pasted outline read, in bytes.
const MAX_TEXT_BYTES: usize = 4 << 20;
/// Farthest a position may lie off its page, in points.
const MAX_POSITION: f64 = 1_000_000.0;

fn one_based(path: &[usize]) -> Vec<usize> {
    path.iter().map(|i| i.saturating_add(1)).collect()
}

fn bad(row: usize, what: &str) -> ToolError {
    ToolError::InvalidArgs(format!("row {}: {what}", row.saturating_add(1)))
}

impl Automation {
    pub(crate) fn bookmark_rows(&self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let rows = doc.outline_rows().map_err(failed)?;
        let pages = &doc.info.pages;
        let list: Vec<Value> = rows
            .iter()
            .map(|b| {
                let mut v =
                    json!({ "level": b.path.len(), "path": one_based(&b.path), "title": b.title, "page": b.page.map(|p| p.saturating_add(1)) });
                if let (Some((kind, numbers)), Some(info)) = (&b.view, b.page.and_then(|p| pages.get(p))) {
                    let numbers: Vec<Option<f32>> = numbers.iter().map(|n| n.map(|n| n as f32)).collect();
                    let view = pdfcraft_render::DestView::from_operands(Some(kind), &numbers);
                    if let Some(d) = destination(&view, info) {
                        v["destination"] = d;
                    }
                }
                if let Some(c) = b.color {
                    v["color"] = json!(c);
                }
                if b.bold {
                    v["bold"] = json!(true);
                }
                if b.italic {
                    v["italic"] = json!(true);
                }
                v
            })
            .collect();
        Ok(json!({ "rows": list }))
    }

    pub(crate) fn bookmark_edit_rows(&mut self, a: &Args) -> Result<Value> {
        let keep = a.opt_bool("keep")?.unwrap_or(false);
        let printed = a.opt_bool("printed")?.unwrap_or(false);
        let (rows, unplaced) = match (a.get("rows"), a.opt_str("text")?) {
            (Some(list), None) => {
                if keep || printed {
                    return Err(ToolError::InvalidArgs("keep and printed go with text: rows are the whole outline".into()));
                }
                (self.rows_of(a, list)?, Vec::new())
            }
            (None, Some(text)) => self.rows_of_text(a, text, keep, printed)?,
            _ => return Err(ToolError::InvalidArgs("give either rows or text".into())),
        };
        let count = rows.len();
        let mut out = self.apply(a, Edit::EditBookmarks { rows, toc: false })?;
        out["rows"] = json!(count);
        out["unplaced"] = json!(unplaced);
        Ok(out)
    }

    pub(crate) fn bookmark_create_toc(&mut self, a: &Args) -> Result<Value> {
        let apply = a.opt_bool("apply")?.unwrap_or(false);
        let keep = a.opt_bool("keep")?.unwrap_or(false);
        if keep && !apply {
            return Err(ToolError::InvalidArgs("keep goes with apply".into()));
        }
        let doc = self.doc(a)?;
        let proposal = doc.create_toc().map_err(failed)?;
        let pages = &doc.info.pages;
        let entries: Vec<Value> = proposal
            .entries
            .iter()
            .map(|e| {
                let top = match (e.top, e.page.and_then(|p| pages.get(p))) {
                    (Some(top), Some(info)) => {
                        info.dest_fraction(None, Some(top as f32), 0)[1].map(|f| f64::from((f * info.height * 100.0).round() / 100.0))
                    }
                    _ => None,
                };
                json!({
                    "level": e.level.saturating_add(1),
                    "title": e.title,
                    "page": e.page.map(|p| p.saturating_add(1)),
                    "top": top,
                    "printed": e.printed,
                    "verified": e.verified,
                })
            })
            .collect();
        let (source, toc_pages) = match &proposal.source {
            TocSource::Contents { pages } => ("contents", pages.iter().map(|p| p.saturating_add(1)).collect()),
            TocSource::Structure => ("structure", Vec::new()),
            TocSource::Headings => ("headings", Vec::new()),
        };
        let found = json!({
            "source": source,
            "toc_pages": toc_pages,
            "entries": entries,
            "unverified": proposal.unverified(),
            "unplaced": proposal.unplaced(),
        });
        if !apply {
            return Ok(found);
        }
        let mut rows = if keep { self.existing_rows(a)? } else { Vec::new() };
        rows.extend(proposal.rows());
        let mut out = self.apply(a, Edit::EditBookmarks { rows, toc: true })?;
        if let (Some(o), Value::Object(f)) = (out.as_object_mut(), found) {
            o.extend(f);
        }
        Ok(out)
    }

    /// The outline's rows as they are, each keeping its bookmark.
    fn existing_rows(&self, a: &Args) -> Result<Vec<OutlineRow>> {
        let doc = self.doc(a)?;
        let rows = doc.outline_rows().map_err(failed)?;
        Ok(rows
            .into_iter()
            .map(|b| OutlineRow {
                level: b.path.len().saturating_sub(1),
                title: b.title,
                target: RowTarget::Keep,
                origin: Some(b.obj),
                open: None,
                element: None,
            })
            .collect())
    }

    /// Rows from JSON, `from` paths resolved against the outline as it is now.
    fn rows_of(&self, a: &Args, list: &Value) -> Result<Vec<OutlineRow>> {
        let items = list.as_array().ok_or_else(|| ToolError::InvalidArgs("rows must be a list of rows".into()))?;
        if items.len() > pdfcraft_engine::MAX_OUTLINE_ROWS {
            return Err(ToolError::InvalidArgs(format!("at most {} rows", pdfcraft_engine::MAX_OUTLINE_ROWS)));
        }
        let doc = self.doc(a)?;
        let pages = &doc.info.pages;
        // Only read when a row names an existing bookmark.
        let mut by_path: Option<HashMap<Vec<usize>, pdfcraft_cos::ObjRef>> = None;
        let mut out = Vec::with_capacity(items.len());
        for (i, item) in items.iter().enumerate() {
            let o: &Map<String, Value> = item.as_object().ok_or_else(|| bad(i, "must be an object"))?;
            if let Some(k) = o.keys().find(|k| !ROW_KEYS.contains(&k.as_str())) {
                return Err(bad(i, &format!("unknown key {k:?} (a row has {})", ROW_KEYS.join(", "))));
            }
            let level = o.get("level").and_then(Value::as_u64).filter(|l| *l >= 1).ok_or_else(|| bad(i, "level must be 1 or more"))?;
            let level = usize::try_from(level - 1).map_err(|_| bad(i, "level is too deep"))?;
            let title = o.get("title").and_then(Value::as_str).ok_or_else(|| bad(i, "needs a title"))?.to_string();
            let origin = match o.get("from").filter(|v| !v.is_null()) {
                None => None,
                Some(v) => {
                    let path: Vec<usize> = v
                        .as_array()
                        .and_then(|p| p.iter().map(|x| x.as_u64().filter(|n| *n >= 1).and_then(|n| usize::try_from(n - 1).ok())).collect())
                        .filter(|p: &Vec<usize>| !p.is_empty())
                        .ok_or_else(|| bad(i, "from must be a bookmark's 1-based path, as bookmark_rows lists it"))?;
                    let map = match &mut by_path {
                        Some(m) => m,
                        None => by_path.insert(doc.outline_rows().map_err(failed)?.into_iter().map(|b| (b.path, b.obj)).collect()),
                    };
                    Some(*map.get(&path).ok_or_else(|| bad(i, &format!("there is no bookmark at {:?}", one_based(&path))))?)
                }
            };
            let coordinate = |key: &str| -> Result<Option<f32>> {
                match o.get(key).filter(|v| !v.is_null()) {
                    None => Ok(None),
                    Some(v) => v
                        .as_f64()
                        .filter(|v| v.is_finite() && v.abs() <= MAX_POSITION)
                        .map(|v| Some(v as f32))
                        .ok_or_else(|| bad(i, &format!("{key} must be a number of points from -{MAX_POSITION} to {MAX_POSITION}"))),
                }
            };
            let (top, left) = (coordinate("top")?, coordinate("left")?);
            let target = match o.get("page") {
                None if origin.is_some() => RowTarget::Keep,
                None | Some(Value::Null) => RowTarget::Nowhere,
                Some(p) => {
                    let page =
                        p.as_u64().filter(|p| *p >= 1).and_then(|p| usize::try_from(p - 1).ok()).ok_or_else(|| bad(i, "page must be 1 or more"))?;
                    let info =
                        pages.get(page).ok_or_else(|| bad(i, &format!("page {} is past the last page ({})", page.saturating_add(1), pages.len())))?;
                    let [left, top] = info.xyz_for_view(left, top);
                    RowTarget::Page { page, left: left.map(f64::from), top: top.map(f64::from) }
                }
            };
            if !matches!(target, RowTarget::Page { .. }) && (top.is_some() || left.is_some()) {
                return Err(bad(i, "top and left go with a page"));
            }
            out.push(OutlineRow { level, title, target, origin, open: None, element: None });
        }
        Ok(out)
    }

    /// Rows from a pasted outline: all new, after the existing bookmarks with `keep`. Page numbers
    /// are page numbers, or with `printed` the numbers the document prints, mapped as Create Table
    /// of Contents maps them. Also the 1-based lines whose page could not be found.
    fn rows_of_text(&self, a: &Args, text: &str, keep: bool, printed: bool) -> Result<(Vec<OutlineRow>, Vec<usize>)> {
        if text.len() > MAX_TEXT_BYTES {
            return Err(ToolError::InvalidArgs(format!("text must be at most {MAX_TEXT_BYTES} bytes")));
        }
        let entries = toc::parse_outline_text(text);
        if entries.is_empty() {
            return Err(ToolError::InvalidArgs("text has no outline lines".into()));
        }
        let doc = self.doc(a)?;
        let pages = &doc.info.pages;
        let mut unplaced = Vec::new();
        let placed: Vec<(Option<usize>, Option<f64>, Option<f64>)> = if printed {
            let wanted: Vec<(String, Option<PageRef>)> = entries.iter().map(|e| (e.title.clone(), e.page.clone())).collect();
            doc.place_printed(&wanted).into_iter().map(|(p, left, top, _)| (p, left, top)).collect()
        } else {
            let mut out = Vec::with_capacity(entries.len());
            for (i, e) in entries.iter().enumerate() {
                let page = match &e.page {
                    None => None,
                    Some(PageRef::Arabic(n)) => {
                        let p = usize::try_from(*n).unwrap_or(usize::MAX).saturating_sub(1);
                        if p >= pages.len() {
                            return Err(bad(i, &format!("page {n} is past the last page ({})", pages.len())));
                        }
                        Some(p)
                    }
                    // A roman numeral or a label names a page by its label.
                    Some(other) => {
                        let label = other.printed();
                        let p = pages.iter().position(|p| p.label.eq_ignore_ascii_case(&label));
                        Some(p.ok_or_else(|| bad(i, &format!("no page is labelled {label:?}")))?)
                    }
                };
                out.push((page, None, None));
            }
            out
        };
        let mut rows = if keep { self.existing_rows(a)? } else { Vec::new() };
        for (i, (e, (page, left, top))) in entries.into_iter().zip(placed).enumerate() {
            if e.page.is_some() && page.is_none() {
                unplaced.push(i.saturating_add(1));
            }
            let target = match page {
                Some(page) => RowTarget::Page { page, left, top },
                None => RowTarget::Nowhere,
            };
            rows.push(OutlineRow { level: e.level, title: e.title, target, origin: None, open: None, element: None });
        }
        Ok((rows, unplaced))
    }
}
