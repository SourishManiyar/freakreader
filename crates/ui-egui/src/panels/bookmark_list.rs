//! The Bookmarks panel's list editor: the whole outline as rows, edited together and applied as
//! one undo step (Edit bookmarks), alongside the panel's bookmark-by-bookmark editing.
//!
//! Every bookmark is a row: its title and page are text fields, its nesting is its indent.
//! Tab and Shift+Tab indent and outdent the row with the rows under it, Enter adds a row below,
//! Alt+↑/↓ moves the row among its siblings, ↑/↓ step between rows, and Backspace in an empty
//! title deletes the row. Rows can also be dragged by their handle. A page is typed as its
//! number or its label ("iv"); the selected row's position on its page is set below the list
//! (points from the top, or the current view). A plain-text outline pasted into a title, or into
//! Paste outline…, becomes rows. Create Table of Contents opens the editor with what it found,
//! for review.
//!
//! Rows that stand for an existing bookmark keep everything the editor doesn't show (colour,
//! style, a view such as Fit width, an action), and a destination the row leaves alone is not
//! rewritten. Nothing changes in the document until Apply; Cancel discards the list.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;

use egui::{Key, Modifiers, Sense, Stroke, vec2};
use pdfcraft_engine::toc::{PageRef, TocError, TocProposal, TocSource};
use pdfcraft_engine::{OutlineRow, RowTarget};
use pdfcraft_render::DocInfo;

use super::Tokens;
use crate::{icons, theme};

/// Deepest level a row may have (0 is the top), as deep as the outline can be listed.
const MAX_LEVEL: usize = 32;
/// Editor undo steps kept.
const MAX_UNDO: usize = 100;
/// Height of a row.
const ROW_H: f32 = 30.0;
/// Indent per level.
const INDENT: f32 = 14.0;

/// A row to insert: its level (from 0), title, page, left and top on the page (user space), and the page
/// number a table of contents printed for it.
pub type NewRow = (usize, String, Option<usize>, Option<f64>, Option<f64>, Option<String>);

/// What the row's page was when the editor opened, so a destination left alone is kept.
#[derive(Clone, Debug, PartialEq)]
pub struct Original {
    pub title: String,
    pub page_text: String,
    pub left: Option<f64>,
    pub top: Option<f64>,
    /// The destination's type when it is not a plain position (`FitH`, `Fit`…), or an action
    /// going elsewhere: kept unless the page or position is changed.
    pub kept_view: Option<String>,
}

/// One bookmark of the list.
#[derive(Clone, Debug, PartialEq)]
pub struct EditRow {
    /// For egui ids that follow the row when it moves.
    pub key: u64,
    pub level: usize,
    pub title: String,
    /// What was typed: a page number (1-based) or a page label; empty for nowhere.
    pub page_text: String,
    /// Where on the page, in user space; `None` keeps the reader's position that way.
    pub left: Option<f64>,
    pub top: Option<f64>,
    pub origin: Option<pdfcraft_cos::ObjRef>,
    pub original: Option<Original>,
    pub element: Option<pdfcraft_cos::ObjRef>,
    /// The page number a table of contents printed, and whether the title was found on the
    /// page it was mapped to.
    pub printed: Option<(String, bool)>,
}

impl EditRow {
    fn target_changed(&self) -> bool {
        self.original.as_ref().is_none_or(|o| o.page_text != self.page_text || o.top != self.top || o.left != self.left)
    }
}

/// Where the editor came from, for its banner and the undo step's name.
#[derive(Clone, Debug, PartialEq)]
pub enum Origin {
    /// Edit bookmarks as a list.
    Outline,
    /// Create Table of Contents: what it found and where.
    Toc { source: TocSource, found: usize, unverified: usize, unplaced: usize, existing: usize },
}

/// The list editor's state for one document.
#[derive(Clone, Debug)]
pub struct BookmarkList {
    pub doc: pdfcraft_engine::DocId,
    pub rows: Vec<EditRow>,
    pub selected: usize,
    pub origin: Origin,
    undo: Vec<Vec<EditRow>>,
    redo: Vec<Vec<EditRow>>,
    next_key: u64,
    /// The row whose title takes focus next frame.
    focus: Option<u64>,
    /// Paste outline…: the text, and whether its numbers are printed page numbers.
    pub paste: Option<(String, bool)>,
    /// The last refusal (a row that can't be written, or the engine's error).
    pub message: Option<String>,
    /// The row being dragged, and where it would go.
    drag: Option<(usize, usize)>,
    /// The position field of the selected row, as typed.
    position_text: Option<(u64, String)>,
    /// The field being typed in (row key, title or page): one undo step per run of typing.
    typing: Option<(u64, bool)>,
    /// The row whose title or page field had the keyboard last frame.
    focused_row: Option<u64>,
    /// The document's edit generation when the list was read: a later edit or undo makes the
    /// list stale.
    pub generation: u64,
}

/// What the panel asks the app to do with the editor.
#[derive(Clone, Debug, PartialEq)]
pub enum ListAction {
    Apply,
    Cancel,
    /// Read the outline again, dropping the list's changes.
    Reload,
    /// Insert the pasted outline (its text and whether its numbers are printed page numbers).
    PasteOutline(String, bool),
}

/// Rows from the document's bookmarks, each keeping its bookmark.
pub fn rows_of(bookmarks: Vec<pdfcraft_engine::OutlineBookmark>, info: &DocInfo, next_key: &mut u64) -> Vec<EditRow> {
    bookmarks
        .into_iter()
        .map(|b| {
            let (left, top, kept_view) = match &b.view {
                Some((kind, values)) if kind.as_slice() == b"XYZ" => (values.first().copied().flatten(), values.get(1).copied().flatten(), None),
                Some((kind, _)) => (None, None, Some(String::from_utf8_lossy(kind).into_owned())),
                None if b.page.is_none() => (None, None, Some(String::new())),
                None => (None, None, None),
            };
            let page_text = b.page.map(|p| page_text(info, p)).unwrap_or_default();
            *next_key += 1;
            EditRow {
                key: *next_key,
                level: b.path.len().saturating_sub(1),
                original: Some(Original { title: b.title.clone(), page_text: page_text.clone(), left, top, kept_view }),
                title: b.title,
                page_text,
                left,
                top,
                origin: Some(b.obj),
                element: None,
                printed: None,
            }
        })
        .collect()
}

/// How a page is typed in the list: its number.
fn page_text(_info: &DocInfo, page: usize) -> String {
    page.saturating_add(1).to_string()
}

/// The 0-based page `text` names: a page number, else a page label. `Ok(None)` for nowhere.
pub fn parse_page(info: &DocInfo, text: &str) -> Result<Option<usize>, String> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(None);
    }
    let count = info.pages.len();
    if t.chars().all(|c| c.is_ascii_digit()) {
        return match t.parse::<usize>() {
            Ok(n) if n >= 1 && n <= count => Ok(Some(n - 1)),
            _ => Err(crate::i18n::fmt(tl!("There is no page {n}: this document has {count} pages."), &[("n", t), ("count", &count.to_string())])),
        };
    }
    info.pages
        .iter()
        .position(|p| p.label.eq_ignore_ascii_case(t))
        .map(Some)
        .ok_or_else(|| crate::i18n::fmt(tl!("No page is labelled “{label}”."), &[("label", t)]))
}

impl BookmarkList {
    pub fn new(doc: pdfcraft_engine::DocId, rows: Vec<EditRow>, next_key: u64) -> Self {
        Self {
            doc,
            rows,
            selected: 0,
            origin: Origin::Outline,
            undo: Vec::new(),
            redo: Vec::new(),
            next_key,
            focus: None,
            paste: None,
            message: None,
            drag: None,
            position_text: None,
            typing: None,
            focused_row: None,
            generation: 0,
        }
    }

    /// Add what Create Table of Contents found after the rows already here, for review.
    pub fn add_toc(&mut self, proposal: &TocProposal, info: &DocInfo) {
        let existing = self.rows.len();
        let first_new = self.rows.len();
        for e in &proposal.entries {
            let key = self.key();
            self.rows.push(EditRow {
                key,
                level: e.level,
                title: e.title.clone(),
                page_text: e.page.map(|p| page_text(info, p)).unwrap_or_default(),
                left: e.left,
                top: e.top,
                origin: None,
                original: None,
                element: e.element,
                printed: e.printed.clone().map(|p| (p, e.verified)),
            });
        }
        normalize(&mut self.rows);
        self.selected = first_new.min(self.rows.len().saturating_sub(1));
        self.origin = Origin::Toc {
            source: proposal.source.clone(),
            found: proposal.entries.len(),
            unverified: proposal.unverified(),
            unplaced: proposal.unplaced(),
            existing,
        };
    }

    fn key(&mut self) -> u64 {
        self.next_key = self.next_key.wrapping_add(1);
        self.next_key
    }

    /// Remember the rows before a change, for the editor's own Undo.
    fn checkpoint(&mut self) {
        self.undo.push(self.rows.clone());
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.message = None;
        self.typing = None;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) {
        if let Some(rows) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.rows, rows));
            self.selected = self.selected.min(self.rows.len().saturating_sub(1));
        }
    }

    pub fn redo(&mut self) {
        if let Some(rows) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.rows, rows));
            self.selected = self.selected.min(self.rows.len().saturating_sub(1));
        }
    }

    /// The rows from `i` through the last row nested under it.
    pub fn block(&self, i: usize) -> std::ops::Range<usize> {
        block(&self.rows, i)
    }

    pub fn can_indent(&self, i: usize) -> bool {
        i > 0 && self.rows.get(i).zip(self.rows.get(i - 1)).is_some_and(|(r, above)| r.level <= above.level && r.level < MAX_LEVEL)
    }

    pub fn can_outdent(&self, i: usize) -> bool {
        self.rows.get(i).is_some_and(|r| r.level > 0)
    }

    /// Nest the row (and the rows under it) one level deeper, under the row above.
    pub fn indent(&mut self, i: usize) {
        if !self.can_indent(i) {
            return;
        }
        self.checkpoint();
        for j in self.block(i) {
            if let Some(r) = self.rows.get_mut(j) {
                r.level = (r.level + 1).min(MAX_LEVEL);
            }
        }
        normalize(&mut self.rows);
    }

    /// Bring the row (and the rows under it) one level out, in place: rows after it that were
    /// its siblings become its children, as in a text editor's list.
    pub fn outdent(&mut self, i: usize) {
        if !self.can_outdent(i) {
            return;
        }
        self.checkpoint();
        for j in self.block(i) {
            if let Some(r) = self.rows.get_mut(j) {
                r.level = r.level.saturating_sub(1);
            }
        }
        normalize(&mut self.rows);
    }

    /// The sibling before or after row `i`: the start of its block.
    fn sibling(&self, i: usize, after: bool) -> Option<usize> {
        let level = self.rows.get(i)?.level;
        if after {
            let j = self.block(i).end;
            self.rows.get(j).filter(|r| r.level == level).map(|_| j)
        } else {
            let mut j = i;
            while j > 0 {
                j -= 1;
                let r = self.rows.get(j)?;
                if r.level < level {
                    return None;
                }
                if r.level == level {
                    return Some(j);
                }
            }
            None
        }
    }

    pub fn can_move(&self, i: usize, down: bool) -> bool {
        self.sibling(i, down).is_some()
    }

    /// Swap the row (with the rows under it) with its previous or next sibling.
    pub fn move_row(&mut self, i: usize, down: bool) {
        let Some(other) = self.sibling(i, down) else { return };
        self.checkpoint();
        let (first, second) = if down { (i, other) } else { (other, i) };
        let a = self.block(first);
        let b = self.block(second);
        let moved: Vec<EditRow> = self.rows.drain(a.start..b.end).collect();
        let (left, right) = moved.split_at(a.len());
        let mut reordered = right.to_vec();
        reordered.extend_from_slice(left);
        let new_index = if down { a.start + b.len() } else { a.start };
        self.rows.splice(a.start..a.start, reordered);
        self.selected = new_index;
    }

    /// Move the row (with the rows under it) to before row `to` (the end appends), nested as
    /// deep as the row above lets it.
    pub fn move_block_to(&mut self, i: usize, to: usize) {
        let b = self.block(i);
        if to >= b.start && to <= b.end {
            return;
        }
        self.checkpoint();
        let moved: Vec<EditRow> = self.rows.drain(b.clone()).collect();
        let at = if to > b.end { to - b.len() } else { to };
        let at = at.min(self.rows.len());
        let above = at.checked_sub(1).and_then(|k| self.rows.get(k)).map(|r| r.level);
        let base = moved.first().map_or(0, |r| r.level);
        let level = base.min(above.map_or(0, |l| l + 1));
        let moved: Vec<EditRow> = moved
            .into_iter()
            .map(|mut r| {
                r.level = r.level - base + level;
                r
            })
            .collect();
        self.rows.splice(at..at, moved);
        normalize(&mut self.rows);
        self.selected = at;
    }

    /// Delete the row and the rows under it.
    pub fn delete(&mut self, i: usize) {
        if i >= self.rows.len() {
            return;
        }
        self.checkpoint();
        let b = self.block(i);
        self.rows.drain(b);
        normalize(&mut self.rows);
        self.selected = i.min(self.rows.len().saturating_sub(1));
    }

    /// Add an empty row after row `i` and the rows under it, at its level (or the first row).
    pub fn add_after(&mut self, i: Option<usize>, page: Option<usize>, top: Option<f64>) -> usize {
        self.checkpoint();
        let (at, level) = match i.filter(|i| *i < self.rows.len()) {
            Some(i) => (self.block(i).end, self.rows.get(i).map_or(0, |r| r.level)),
            None => (self.rows.len(), 0),
        };
        let key = self.key();
        let row = EditRow {
            key,
            level,
            title: String::new(),
            page_text: page.map(|p| (p + 1).to_string()).unwrap_or_default(),
            left: None,
            top,
            origin: None,
            original: None,
            element: None,
            printed: None,
        };
        self.rows.insert(at, row);
        normalize(&mut self.rows);
        self.selected = at;
        self.focus = Some(key);
        at
    }

    /// Insert rows (levels counted from 0) after row `i` and its children, nested at its level.
    pub fn insert_rows(&mut self, i: Option<usize>, new: Vec<NewRow>) {
        if new.is_empty() {
            return;
        }
        self.checkpoint();
        let (at, base) = match i.filter(|i| *i < self.rows.len()) {
            Some(i) => (self.block(i).end, self.rows.get(i).map_or(0, |r| r.level)),
            None => (self.rows.len(), 0),
        };
        let mut rows = Vec::with_capacity(new.len());
        for (level, title, page, left, top, printed) in new {
            let key = self.key();
            rows.push(EditRow {
                key,
                level: (base + level).min(MAX_LEVEL),
                title,
                page_text: page.map(|p| (p + 1).to_string()).unwrap_or_default(),
                left,
                top,
                origin: None,
                original: None,
                element: None,
                printed: printed.map(|p| (p, page.is_some())),
            });
        }
        self.rows.splice(at..at, rows);
        normalize(&mut self.rows);
        self.selected = at;
    }

    /// Drop the rows that stand for the document's existing bookmarks (after Create Table of
    /// Contents, to replace them).
    pub fn remove_existing(&mut self) {
        if !self.rows.iter().any(|r| r.origin.is_some()) {
            return;
        }
        self.checkpoint();
        self.rows.retain(|r| r.origin.is_none());
        normalize(&mut self.rows);
        self.selected = 0;
        if let Origin::Toc { existing, .. } = &mut self.origin {
            *existing = 0;
        }
    }

    /// Why each row can't be written (index, reason), in order.
    pub fn problems(&self, info: &DocInfo) -> Vec<(usize, String)> {
        let mut out = Vec::new();
        for (i, r) in self.rows.iter().enumerate() {
            let unchanged_empty = r.original.as_ref().is_some_and(|o| o.title.trim().is_empty() && r.title.trim().is_empty());
            if r.title.trim().is_empty() && !unchanged_empty {
                out.push((i, tl!("The title is empty.").to_string()));
            } else if let Err(e) = parse_page(info, &r.page_text)
                && r.target_changed()
            {
                out.push((i, e));
            }
        }
        out
    }

    /// The rows as the engine writes them, or the first row that can't be written.
    pub fn outline_rows(&self, info: &DocInfo) -> Result<Vec<OutlineRow>, (usize, String)> {
        if let Some(p) = self.problems(info).into_iter().next() {
            return Err(p);
        }
        let mut out = Vec::with_capacity(self.rows.len());
        for (i, r) in self.rows.iter().enumerate() {
            let target = if r.origin.is_some() && !r.target_changed() {
                RowTarget::Keep
            } else {
                match parse_page(info, &r.page_text).map_err(|e| (i, e))? {
                    None => RowTarget::Nowhere,
                    Some(page) => RowTarget::Page { page, left: r.left, top: r.top },
                }
            };
            let title = if r.title.trim().is_empty() { r.original.as_ref().map_or_else(String::new, |o| o.title.clone()) } else { r.title.clone() };
            out.push(OutlineRow { level: r.level, title, target, origin: r.origin, open: None, element: r.element });
        }
        Ok(out)
    }

    /// Whether the list differs from what the document has.
    pub fn changed(&self) -> bool {
        !matches!(self.origin, Origin::Outline) || !self.undo.is_empty()
    }
}

/// The rows from `i` through the last row nested under it.
fn block(rows: &[EditRow], i: usize) -> std::ops::Range<usize> {
    let Some(level) = rows.get(i).map(|r| r.level) else { return i..i };
    let mut end = i + 1;
    while rows.get(end).is_some_and(|r| r.level > level) {
        end += 1;
    }
    i..end
}

/// The first row at the top level, and each row at most one level deeper than the row above.
fn normalize(rows: &mut [EditRow]) {
    let mut previous: Option<usize> = None;
    for r in rows.iter_mut() {
        r.level = r.level.min(previous.map_or(0, |p| p + 1)).min(MAX_LEVEL);
        previous = Some(r.level);
    }
}

/// What the list editor needs from the panel.
pub struct ListCtx<'a> {
    pub info: &'a DocInfo,
    /// The page and user-space position at the top of the window (or the text selection).
    pub here: (usize, Option<f64>, Option<f64>),
    pub editable: bool,
    /// The document's edit generation now.
    pub generation: u64,
}

/// Draw the list editor; returns what the app should do.
pub fn show(ui: &mut egui::Ui, t: &Tokens, editor: &mut BookmarkList, cx: &ListCtx<'_>) -> Option<ListAction> {
    let mut action = None;
    let info = cx.info;
    editor.selected = editor.selected.min(editor.rows.len().saturating_sub(1));
    banner(ui, t, editor);
    if cx.generation != editor.generation {
        ui.label(
            egui::RichText::new(tl!("The document changed since this list was read. Apply writes this list over it; Reload reads it again."))
                .font(theme::regular(11.5))
                .color(egui::Color32::from_rgb(0xd3, 0x8b, 0x00)),
        );
    }
    toolbar(ui, editor, cx);
    if let Some((text, printed)) = editor.paste.as_mut() {
        let mut close = false;
        ui.label(
            egui::RichText::new(tl!("Paste or type an outline: one bookmark per line, indented to nest, with its page at the end."))
                .font(theme::regular(11.5))
                .color(t.text_muted),
        );
        ui.add(crate::widgets::block(text).desired_rows(5).desired_width(ui.available_width()).id_salt("bookmark-paste"));
        ui.add(crate::widgets::checkbox(printed, tl!("The numbers are printed page numbers")));
        ui.horizontal(|ui| {
            if ui.button(tl!("Insert")).clicked() {
                action = Some(ListAction::PasteOutline(text.clone(), *printed));
                close = true;
            }
            if ui.button(tl!("Cancel")).clicked() {
                close = true;
            }
        });
        if close {
            editor.paste = None;
        }
        ui.separator();
    }
    // A multi-line paste into a title is an outline.
    let any_focus = editor.focused_row.take().is_some_and(|k| ui.memory(|m| m.has_focus(title_id(k)) || m.has_focus(page_id(k))));
    if any_focus {
        let pasted = ui.input_mut(|i| {
            let at = i.events.iter().position(|e| matches!(e, egui::Event::Paste(s) if s.trim_end().contains('\n')));
            at.map(|k| i.events.remove(k))
        });
        if let Some(egui::Event::Paste(s)) = pasted {
            action = Some(ListAction::PasteOutline(s, false));
        }
    }
    let footer_h = 150.0;
    let list_h = (ui.available_height() - footer_h).max(ROW_H * 3.0);
    let problems = editor.problems(info);
    let width = ui.available_width();
    let mut row_rects: Vec<(usize, egui::Rect)> = Vec::new();
    let mut op: Option<Op> = None;
    if editor.rows.is_empty() {
        ui.label(egui::RichText::new(tl!("No bookmarks. Add one with +, or paste an outline.")).color(t.text_faint));
    }
    egui::ScrollArea::vertical().id_salt("bookmark-list").auto_shrink([false, false]).max_height(list_h).show_rows(
        ui,
        ROW_H,
        editor.rows.len(),
        |ui, range| {
            for i in range {
                let Some(row) = editor.rows.get(i).cloned() else { continue };
                let problem = problems.iter().find(|(k, _)| *k == i).map(|(_, m)| m.clone());
                let rect = row_ui(ui, t, editor, i, &row, problem.as_deref(), width, cx.editable, &mut op);
                row_rects.push((i, rect));
            }
            // Where a dragged row would land: a line between rows.
            if let Some((from, _)) = editor.drag
                && let Some(pointer) = ui.ctx().pointer_interact_pos()
            {
                let to = row_rects.iter().find(|(_, r)| pointer.y < r.center().y).map_or(editor.rows.len(), |(i, _)| *i);
                editor.drag = Some((from, to));
                let y = row_rects.iter().find(|(i, _)| *i == to).map_or_else(|| row_rects.last().map_or(0.0, |(_, r)| r.bottom()), |(_, r)| r.top());
                let x = ui.min_rect().left();
                ui.painter().hline(x..=x + width, y, Stroke::new(2.0, t.accent));
            }
        },
    );
    // A drag ends when the button is let go, wherever the row it started on has scrolled to.
    if let Some((from, to)) = editor.drag
        && !ui.input(|i| i.pointer.primary_down())
    {
        editor.drag = None;
        op = Some(Op::MoveTo(from, to));
    }
    if let Some(op) = op {
        apply_op(editor, op, cx);
    }
    inspector(ui, t, editor, cx);
    ui.add_space(4.0);
    if let Some(m) = &editor.message {
        ui.label(egui::RichText::new(m).font(theme::regular(11.5)).color(egui::Color32::from_rgb(0xc0, 0x39, 0x2b)));
    }
    if !problems.is_empty() {
        let n = problems.len() as u64;
        let text = crate::i18n::trn(crate::i18n::current(), n, "{n} row needs fixing before Apply.", "{n} rows need fixing before Apply.");
        ui.label(egui::RichText::new(text).font(theme::regular(11.5)).color(egui::Color32::from_rgb(0xc0, 0x39, 0x2b)));
        // The selected row's problem, else the first one, in words.
        if let Some((i, why)) = problems.iter().find(|(i, _)| *i == editor.selected).or(problems.first()) {
            let s = crate::i18n::fmt(tl!("Row {n}: {why}"), &[("n", &(i + 1).to_string()), ("why", why)]);
            if ui.link(egui::RichText::new(s).font(theme::regular(11.5))).clicked() {
                editor.selected = *i;
                editor.focus = editor.rows.get(*i).map(|r| r.key);
            }
        }
    }
    ui.horizontal(|ui| {
        let apply = ui.add_enabled(problems.is_empty() && cx.editable, egui::Button::new(tl!("Apply")));
        if apply.clicked() {
            action = Some(ListAction::Apply);
        }
        if ui.button(tl!("Cancel")).clicked() {
            action = Some(ListAction::Cancel);
        }
        if ui.button(tl!("Reload")).on_hover_text(tl!("Read the bookmarks from the document again, dropping the changes made here")).clicked() {
            action = Some(ListAction::Reload);
        }
    });
    action
}

fn title_id(key: u64) -> egui::Id {
    egui::Id::unique(("bookmark-list-title", key))
}

fn page_id(key: u64) -> egui::Id {
    egui::Id::unique(("bookmark-list-page", key))
}

/// The banner over the list: what Create Table of Contents found.
fn banner(ui: &mut egui::Ui, t: &Tokens, editor: &mut BookmarkList) {
    let Origin::Toc { source, found, unverified, unplaced, existing } = editor.origin.clone() else {
        ui.label(
            egui::RichText::new(tl!("Editing bookmarks as a list. Changes apply together as one step."))
                .font(theme::regular(11.5))
                .color(t.text_muted),
        );
        return;
    };
    let n = found as u64;
    let lang = crate::i18n::current();
    let what = match &source {
        TocSource::Contents { pages } => {
            let pages = match (pages.first(), pages.last()) {
                (Some(a), Some(b)) if a != b => format!("{}–{}", a + 1, b + 1),
                (Some(a), _) => (a + 1).to_string(),
                _ => String::new(),
            };
            crate::i18n::fmt(
                &crate::i18n::trn(
                    lang,
                    n,
                    "Found {n} entry in the table of contents on page {pages}.",
                    "Found {n} entries in the table of contents on pages {pages}.",
                ),
                &[("pages", &pages)],
            )
        }
        TocSource::Structure => {
            crate::i18n::trn(lang, n, "No table of contents: found {n} tagged heading.", "No table of contents: found {n} tagged headings.")
        }
        TocSource::Headings => crate::i18n::trn(
            lang,
            n,
            "No table of contents or tags: found {n} heading by its look.",
            "No table of contents or tags: found {n} headings by their look.",
        ),
    };
    ui.label(egui::RichText::new(what).font(theme::medium(12.0)).color(t.text));
    if unverified > 0 {
        let s = crate::i18n::trn(
            lang,
            unverified as u64,
            "{n} title was not found on its page (marked !): check it.",
            "{n} titles were not found on their pages (marked !): check them.",
        );
        ui.label(egui::RichText::new(s).font(theme::regular(11.5)).color(t.text_muted));
    }
    if unplaced > 0 {
        let s = crate::i18n::trn(
            lang,
            unplaced as u64,
            "{n} entry has no page: type one or leave it going nowhere.",
            "{n} entries have no page: type one or leave them going nowhere.",
        );
        ui.label(egui::RichText::new(s).font(theme::regular(11.5)).color(t.text_muted));
    }
    ui.label(egui::RichText::new(tl!("Review them, then Apply. Nothing changes until then.")).font(theme::regular(11.5)).color(t.text_muted));
    if existing > 0 {
        let label = crate::i18n::trn(lang, existing as u64, "Replace the existing bookmark", "Replace the {n} existing bookmarks");
        if ui.button(label).clicked() {
            editor.remove_existing();
        }
    }
    ui.add_space(4.0);
}

fn toolbar(ui: &mut egui::Ui, editor: &mut BookmarkList, cx: &ListCtx<'_>) {
    let i = editor.selected;
    let has = i < editor.rows.len();
    let mut op = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if icons::button(ui, "plus", 26.0, false, tl!("Add a bookmark below (Enter)")).clicked() {
            op = Some(Op::Add(has.then_some(i)));
        }
        let enabled = |ui: &mut egui::Ui, on: bool, icon: &str, tip: &str| {
            ui.add_enabled_ui(on, |ui| icons::button(ui, icon, 26.0, false, tip).clicked()).inner
        };
        if enabled(ui, has, "trash-2", tl!("Delete the bookmark and those under it")) {
            op = Some(Op::Delete(i));
        }
        if enabled(ui, editor.can_outdent(i), "chevron-left", tl!("Outdent (Shift+Tab)")) {
            op = Some(Op::Outdent(i));
        }
        if enabled(ui, editor.can_indent(i), "chevron-right", tl!("Indent (Tab)")) {
            op = Some(Op::Indent(i));
        }
        if enabled(ui, editor.can_move(i, false), "chevron-up", tl!("Move up (Alt+↑)")) {
            op = Some(Op::Move(i, false));
        }
        if enabled(ui, editor.can_move(i, true), "chevron-down", tl!("Move down (Alt+↓)")) {
            op = Some(Op::Move(i, true));
        }
        if icons::button(ui, "clipboard-paste", 26.0, editor.paste.is_some(), tl!("Paste outline…")).clicked() {
            editor.paste = match editor.paste {
                Some(_) => None,
                None => Some((String::new(), false)),
            };
        }
        if enabled(ui, editor.can_undo(), "undo-2", tl!("Undo in the list")) {
            op = Some(Op::Undo);
        }
        if enabled(ui, editor.can_redo(), "redo-2", tl!("Redo in the list")) {
            op = Some(Op::Redo);
        }
    });
    if let Some(op) = op {
        apply_op(editor, op, cx);
    }
}

/// A change to the list, applied after the rows are drawn.
#[derive(Clone, Debug, PartialEq)]
enum Op {
    Add(Option<usize>),
    Delete(usize),
    Indent(usize),
    Outdent(usize),
    Move(usize, bool),
    MoveTo(usize, usize),
    Select(usize),
    Focus(usize),
    Undo,
    Redo,
    /// Typing changed a row's title or page.
    Typed(usize, String, String),
}

fn apply_op(editor: &mut BookmarkList, op: Op, cx: &ListCtx<'_>) {
    match op {
        Op::Add(i) => {
            let (page, _, top) = cx.here;
            let page = i.and_then(|i| editor.rows.get(i)).and_then(|r| parse_page(cx.info, &r.page_text).ok().flatten()).or(Some(page));
            let top = if page == Some(cx.here.0) { top } else { None };
            editor.add_after(i, page, top);
        }
        Op::Delete(i) => editor.delete(i),
        Op::Indent(i) => editor.indent(i),
        Op::Outdent(i) => editor.outdent(i),
        Op::Move(i, down) => editor.move_row(i, down),
        Op::MoveTo(i, to) => editor.move_block_to(i, to),
        Op::Select(i) => editor.selected = i,
        Op::Focus(i) => {
            editor.selected = i;
            editor.focus = editor.rows.get(i).map(|r| r.key);
        }
        Op::Undo => editor.undo(),
        Op::Redo => editor.redo(),
        Op::Typed(i, title, page) => {
            let Some((key, title_changed)) = editor.rows.get(i).map(|r| (r.key, r.title != title)) else { return };
            // A run of typing in one field is one undo step.
            let field = (key, title_changed);
            if editor.typing != Some(field) {
                editor.checkpoint();
                editor.typing = Some(field);
            }
            if let Some(r) = editor.rows.get_mut(i) {
                r.title = title;
                r.page_text = page;
            }
        }
    }
}

/// One row: drag handle, indent, title field, page field. Returns its rect.
#[allow(clippy::too_many_arguments)]
fn row_ui(
    ui: &mut egui::Ui,
    t: &Tokens,
    editor: &mut BookmarkList,
    i: usize,
    row: &EditRow,
    problem: Option<&str>,
    width: f32,
    editable: bool,
    op: &mut Option<Op>,
) -> egui::Rect {
    let selected = i == editor.selected;
    let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::hover());
    if selected {
        ui.painter().rect_filled(rect, 4.0, t.selected);
    }
    let mut child =
        ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(2.0, 1.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let ui = &mut child;
    ui.spacing_mut().item_spacing.x = 4.0;
    // The drag handle.
    let (handle, handle_resp) = ui.allocate_exact_size(vec2(12.0, ROW_H - 4.0), Sense::click_and_drag());
    for k in 0..3 {
        let y = handle.center().y - 5.0 + k as f32 * 5.0;
        ui.painter().hline(handle.left() + 2.0..=handle.right() - 2.0, y, Stroke::new(1.2, t.text_faint));
    }
    let handle_resp = handle_resp.on_hover_cursor(egui::CursorIcon::Grab).on_hover_text(tl!("Drag to move"));
    if handle_resp.drag_started() && editable {
        editor.drag = Some((i, i));
    }
    if handle_resp.clicked() {
        *op = Some(Op::Select(i));
    }
    ui.add_space(row.level.min(12) as f32 * INDENT);
    let page_w = 46.0;
    let marker_w = if row.printed.is_some() { 14.0 } else { 0.0 };
    let title_w = (ui.available_width() - page_w - marker_w - 8.0).max(40.0);
    let mut title = row.title.clone();
    let mut page = row.page_text.clone();
    let filter = egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: false };
    let title_edit = crate::widgets::line(&mut title).desired_width(title_w).id(title_id(row.key)).event_filter(filter).hint_text(tl!("Title"));
    let title_resp = ui.add_enabled(editable, title_edit);
    title_resp.widget_info(|| {
        let mut info = egui::WidgetInfo::text_edit(editable, &row.title, &title, "");
        info.label = Some(tl!("Bookmark title").to_string());
        info
    });
    // An assistive "click" (or the control channel's) puts the caret in the field, as a pointer
    // press does.
    if editor.focus == Some(row.key) || (title_resp.clicked() && !title_resp.has_focus()) {
        title_resp.request_focus();
        editor.focus = None;
    }
    if let Some((text, verified)) = &row.printed {
        let (r, resp) = ui.allocate_exact_size(vec2(marker_w, ROW_H - 6.0), Sense::hover());
        if !verified {
            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, "!", theme::semibold(13.0), egui::Color32::from_rgb(0xd3, 0x8b, 0x00));
            resp.on_hover_text(crate::i18n::fmt(
                tl!("Printed page {page}: the title was not found on the page it was mapped to."),
                &[("page", text)],
            ));
        } else {
            resp.on_hover_text(crate::i18n::fmt(tl!("Printed page {page}: the title was found on this page."), &[("page", text)]));
        }
    }
    let page_resp =
        ui.add_enabled(editable, crate::widgets::line(&mut page).desired_width(page_w).id(page_id(row.key)).event_filter(filter).hint_text("—"));
    if page_resp.clicked() && !page_resp.has_focus() {
        page_resp.request_focus();
    }
    page_resp.widget_info(|| {
        let mut info = egui::WidgetInfo::text_edit(editable, &row.page_text, &page, "");
        info.label = Some(tl!("Bookmark page").to_string());
        info
    });
    let bad_page = problem.is_some() && !row.title.trim().is_empty();
    for (resp, bad) in [(&title_resp, problem.is_some() && row.title.trim().is_empty()), (&page_resp, bad_page)] {
        if bad {
            ui.painter().rect_stroke(resp.rect, 4.0, Stroke::new(1.5, egui::Color32::from_rgb(0xc0, 0x39, 0x2b)), egui::StrokeKind::Inside);
        }
    }
    if let Some(p) = problem {
        title_resp.clone().on_hover_text(p);
    }
    if title != row.title || page != row.page_text {
        *op = Some(Op::Typed(i, title, page.clone()));
    }
    let focused = title_resp.has_focus() || page_resp.has_focus();
    if focused {
        editor.focused_row = Some(row.key);
    }
    if title_resp.gained_focus() || page_resp.gained_focus() {
        *op = op.take().or(Some(Op::Select(i)));
    }
    if focused && op.is_none() && editable {
        let input = |k: Key, m: Modifiers| ui.input_mut(|inp| inp.consume_key(m, k));
        if input(Key::Tab, Modifiers::SHIFT) {
            *op = Some(Op::Outdent(i));
        } else if input(Key::Tab, Modifiers::NONE) {
            *op = Some(Op::Indent(i));
        } else if input(Key::ArrowUp, Modifiers::ALT) {
            *op = Some(Op::Move(i, false));
        } else if input(Key::ArrowDown, Modifiers::ALT) {
            *op = Some(Op::Move(i, true));
        } else if input(Key::ArrowUp, Modifiers::NONE) && i > 0 {
            *op = Some(Op::Focus(i - 1));
        } else if input(Key::ArrowDown, Modifiers::NONE) && i + 1 < editor.rows.len() {
            *op = Some(Op::Focus(i + 1));
        } else if title_resp.has_focus() && row.title.is_empty() && input(Key::Backspace, Modifiers::NONE) {
            *op = Some(Op::Delete(i));
            editor.focus = i.checked_sub(1).and_then(|k| editor.rows.get(k)).map(|r| r.key);
        }
    }
    // Enter ends the field: a new row below.
    if (title_resp.lost_focus() || page_resp.lost_focus()) && ui.input(|inp| inp.key_pressed(Key::Enter)) && op.is_none() && editable {
        *op = Some(Op::Add(Some(i)));
    }
    rect
}

/// The selected row's page, position and what the table of contents printed for it.
fn inspector(ui: &mut egui::Ui, t: &Tokens, editor: &mut BookmarkList, cx: &ListCtx<'_>) {
    ui.separator();
    let i = editor.selected;
    let Some(row) = editor.rows.get(i).cloned() else { return };
    let info = cx.info;
    let page = parse_page(info, &row.page_text).ok().flatten();
    let small = |s: String| egui::RichText::new(s).font(theme::regular(11.5)).color(t.text_muted);
    match page.and_then(|p| info.pages.get(p).map(|pi| (p, pi))) {
        Some((p, pi)) => {
            let mut s = crate::i18n::fmt(tl!("Page {n} of {count}"), &[("n", &(p + 1).to_string()), ("count", &info.pages.len().to_string())]);
            if pi.label != (p + 1).to_string() {
                s.push_str(&crate::i18n::fmt(tl!(", labelled “{label}”"), &[("label", &pi.label)]));
            }
            ui.label(small(s));
        }
        None => {
            ui.label(small(tl!("Goes nowhere: type a page number or label.").to_string()));
        }
    }
    if let Some(kept) = row.original.as_ref().and_then(|o| o.kept_view.clone()).filter(|_| !row.target_changed()) {
        let s = if kept.is_empty() {
            tl!("Keeps its action (it goes somewhere other than a page here).").to_string()
        } else {
            crate::i18n::fmt(tl!("Keeps its {view} view until its page or position is changed."), &[("view", &kept)])
        };
        ui.label(small(s));
    }
    if let Some((printed, verified)) = &row.printed {
        let s = if *verified {
            crate::i18n::fmt(tl!("Printed page {page}; the title was found there."), &[("page", printed)])
        } else {
            crate::i18n::fmt(tl!("Printed page {page}; the title was not found on the page: check it."), &[("page", printed)])
        };
        ui.label(small(s));
    }
    // Position: points from the top of the displayed page.
    let shown = page.and_then(|p| info.pages.get(p)).and_then(|pi| {
        let [_, y] = pi.dest_fraction(row.left.map(|v| v as f32), row.top.map(|v| v as f32), 0);
        y.map(|y| (y * pi.height).round())
    });
    let mut text = match &editor.position_text {
        Some((key, text)) if *key == row.key => text.clone(),
        _ => shown.map(|y| format!("{y}")).unwrap_or_default(),
    };
    ui.horizontal(|ui| {
        ui.label(small(tl!("From the top").to_string()));
        let resp = ui.add_enabled(page.is_some() && cx.editable, crate::widgets::line(&mut text).desired_width(56.0).hint_text(tl!("top")));
        resp.widget_info(|| {
            let mut info = egui::WidgetInfo::text_edit(true, "", &text, "");
            info.label = Some(tl!("Points from the top of the page").to_string());
            info
        });
        ui.label(small("pt".into()));
        if resp.changed() {
            editor.position_text = Some((row.key, text.clone()));
        }
        if resp.lost_focus() {
            let value = text.trim().replace(',', ".");
            let parsed = if value.is_empty() { Some(None) } else { value.parse::<f32>().ok().filter(|v| v.is_finite() && v.abs() < 1e6).map(Some) };
            match (parsed, page.and_then(|p| info.pages.get(p))) {
                (Some(y), Some(pi)) => {
                    let [left, top] = pi.xyz_for_view(None, y);
                    editor.checkpoint();
                    if let Some(r) = editor.rows.get_mut(i) {
                        r.left = left.map(f64::from);
                        r.top = top.map(f64::from);
                    }
                }
                _ => editor.message = Some(tl!("The position is a number of points from the top of the page.").to_string()),
            }
            editor.position_text = None;
        }
        if ui
            .add_enabled(cx.editable, egui::Button::new(tl!("Use current view")))
            .on_hover_text(tl!("Go to the page and place shown at the top of the window now"))
            .clicked()
        {
            let (p, left, top) = cx.here;
            editor.checkpoint();
            if let Some(r) = editor.rows.get_mut(i) {
                r.page_text = (p + 1).to_string();
                r.left = left;
                r.top = top;
            }
            editor.position_text = None;
        }
    });
}

/// Create table of contents running on a thread of its own for one document. Dropping it tells
/// the thread to stop at its next page.
pub struct TocRun {
    /// The document's edit generation when it was asked: a result for another is dropped.
    generation: u64,
    cancel: Arc<AtomicBool>,
    pages_read: Arc<AtomicUsize>,
    /// `None` when it stopped before finishing.
    receiver: mpsc::Receiver<Option<Result<TocProposal, TocError>>>,
}

impl TocRun {
    /// Pages read so far (a page may be read more than once).
    pub fn pages_read(&self) -> usize {
        self.pages_read.load(Ordering::Relaxed)
    }
}

impl Drop for TocRun {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl crate::PdfCraftApp {
    /// Open the Bookmarks panel's list editor on document `id`'s outline (or show the one
    /// already open for it).
    pub fn open_bookmark_list(&mut self, id: pdfcraft_engine::DocId) {
        self.right = Some(crate::RightPanel::Bookmarks);
        if self.bookmark_lists.contains_key(&id) {
            return;
        }
        if let Some(list) = self.read_bookmark_list(id) {
            self.bookmark_lists.insert(id, list);
        }
    }

    /// A list editor holding document `id`'s outline as it is, or `None` with the reason shown.
    fn read_bookmark_list(&mut self, id: pdfcraft_engine::DocId) -> Option<BookmarkList> {
        let doc = self.session.get(id)?;
        if !(doc.allows_assembly() && doc.read_only_reason.is_none()) {
            self.notify_tr("This document's bookmarks can't be changed.");
            return None;
        }
        match doc.outline_rows() {
            Ok(bookmarks) => {
                let mut key = 0;
                let rows = rows_of(bookmarks, &doc.info, &mut key);
                let mut list = BookmarkList::new(id, rows, key);
                list.generation = doc.edit_generation();
                Some(list)
            }
            Err(e) => {
                self.notify_error(e);
                None
            }
        }
    }

    /// Create table of contents: find the document's own table of contents (else its tags, else
    /// its headings) off the UI thread, showing progress in the Bookmarks panel; what was found
    /// opens in the list editor for review, after the existing bookmarks ([`Self::poll_toc_runs`]).
    /// Inline (tests, the web build) it is found before this returns.
    pub fn create_toc(&mut self, id: pdfcraft_engine::DocId) {
        self.right = Some(crate::RightPanel::Bookmarks);
        if self.toc_runs.contains_key(&id) {
            return; // already running: its progress shows in the panel
        }
        let Some(doc) = self.session.get(id) else { return };
        let job = match doc.toc_job() {
            Ok(job) => job,
            Err(e) => {
                self.notify_error(e);
                return;
            }
        };
        let (sender, receiver) = mpsc::channel();
        let run = TocRun {
            generation: doc.edit_generation(),
            cancel: Arc::new(AtomicBool::new(false)),
            pages_read: Arc::new(AtomicUsize::new(0)),
            receiver,
        };
        let (cancel, pages_read, hold, ctx) = (run.cancel.clone(), run.pages_read.clone(), self.toc_hold.take(), self.ctx.clone());
        let work = move || {
            if let Some(hold) = hold {
                let _ = hold.recv();
            }
            let found = job.run(&|| !cancel.load(Ordering::Relaxed), &pages_read);
            let _ = sender.send(found);
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        if !self.run_inline {
            match std::thread::Builder::new().name("pdfcraft-create-toc".into()).spawn(work) {
                Ok(_) => {
                    self.toc_runs.insert(id, run);
                }
                Err(e) => self.notify_error(e),
            }
            return;
        }
        work();
        self.toc_runs.insert(id, run);
        self.poll_toc_runs();
    }

    /// Stop Create table of contents on document `id`, if it is running: nothing it found is
    /// shown.
    pub fn stop_create_toc(&mut self, id: pdfcraft_engine::DocId) {
        // Dropping the run tells its thread to stop at its next page.
        self.toc_runs.remove(&id);
    }

    /// Show what each finished Create table of contents found. A result for a document that
    /// closed, or changed since it was asked, is dropped.
    pub(crate) fn poll_toc_runs(&mut self) {
        if self.toc_runs.is_empty() {
            return;
        }
        let ids: Vec<pdfcraft_engine::DocId> = self.toc_runs.keys().copied().collect();
        for id in ids {
            let outcome = match self.toc_runs.get(&id).map(|run| run.receiver.try_recv()) {
                None | Some(Err(mpsc::TryRecvError::Empty)) => continue,
                Some(Ok(found)) => found.map(|found| found.map_err(|e: TocError| e.to_string())),
                Some(Err(mpsc::TryRecvError::Disconnected)) => Some(Err(tl!("Create table of contents stopped unexpectedly.").to_string())),
            };
            let Some(run) = self.toc_runs.remove(&id) else { continue };
            let Some(doc) = self.session.get(id) else { continue };
            if run.cancel.load(Ordering::Relaxed) {
                continue;
            }
            if doc.edit_generation() != run.generation {
                self.notify_tr("The document changed while its table of contents was being made, so nothing was added. Try again.");
                continue;
            }
            match outcome {
                None => {}
                Some(Err(why)) => self.notify_error(why),
                Some(Ok(proposal)) => self.show_toc(id, &proposal),
            }
        }
    }

    /// Open `proposal` in document `id`'s list editor for review, after the existing bookmarks
    /// (and after the changes of a list already open).
    fn show_toc(&mut self, id: pdfcraft_engine::DocId, proposal: &TocProposal) {
        let list = match self.bookmark_lists.remove(&id) {
            Some(list) => Some(list),
            None => self.read_bookmark_list(id),
        };
        let Some(mut list) = list else { return };
        let Some(doc) = self.session.get(id) else { return };
        list.checkpoint();
        list.add_toc(proposal, &doc.info);
        self.bookmark_lists.insert(id, list);
        self.right = Some(crate::RightPanel::Bookmarks);
    }

    /// Carry out what the list editor asked for.
    pub fn bookmark_list_action(&mut self, id: pdfcraft_engine::DocId, action: ListAction) {
        match action {
            ListAction::Cancel => {
                self.bookmark_lists.remove(&id);
            }
            ListAction::Reload => {
                if let Some(list) = self.read_bookmark_list(id) {
                    self.bookmark_lists.insert(id, list);
                }
            }
            ListAction::Apply => {
                // Borrowed, not copied: the description holds every page, and this is per click.
                let Some(doc) = self.session.get(id) else { return };
                let Some(list) = self.bookmark_lists.get_mut(&id) else { return };
                let rows = match list.outline_rows(&doc.info) {
                    Ok(rows) => rows,
                    Err((i, why)) => {
                        list.selected = i;
                        list.message = Some(crate::i18n::fmt(tl!("Row {n}: {why}"), &[("n", &(i + 1).to_string()), ("why", &why)]));
                        return;
                    }
                };
                let toc = matches!(list.origin, Origin::Toc { .. });
                if self.active_ids().map(|(_, active)| active) != Some(id) {
                    return;
                }
                match self.session.get(id).map(|d| pdfcraft_organize_check(d, &rows)) {
                    Some(Err(why)) => {
                        if let Some(list) = self.bookmark_lists.get_mut(&id) {
                            list.message = Some(why);
                        }
                    }
                    _ => {
                        if self.apply_edit(pdfcraft_engine::Edit::EditBookmarks { rows, toc }) {
                            self.bookmark_lists.remove(&id);
                        } else if let Some(list) = self.bookmark_lists.get_mut(&id) {
                            list.message = Some(tl!("The bookmarks were not changed.").to_string());
                        }
                    }
                }
            }
            ListAction::PasteOutline(text, printed) => {
                let Some(doc) = self.session.get(id) else { return };
                let entries = pdfcraft_engine::toc::parse_outline_text(&text);
                if entries.is_empty() {
                    return;
                }
                let info = &doc.info;
                let placed: Vec<(Option<usize>, Option<f64>, Option<f64>)> = if printed {
                    let wanted: Vec<(String, Option<PageRef>)> = entries.iter().map(|e| (e.title.clone(), e.page.clone())).collect();
                    doc.place_printed(&wanted).into_iter().map(|(p, left, top, _)| (p, left, top)).collect()
                } else {
                    entries
                        .iter()
                        .map(|e| {
                            let page = match &e.page {
                                None => None,
                                Some(PageRef::Arabic(n)) => usize::try_from(*n).ok().and_then(|n| n.checked_sub(1)).filter(|p| *p < info.pages.len()),
                                Some(other) => info.pages.iter().position(|p| p.label.eq_ignore_ascii_case(&other.printed())),
                            };
                            (page, None, None)
                        })
                        .collect()
                };
                let missing = entries.iter().zip(&placed).filter(|(e, (p, _, _))| e.page.is_some() && p.is_none()).count();
                let rows: Vec<_> = entries
                    .into_iter()
                    .zip(placed)
                    .map(|(e, (page, left, top))| {
                        let printed_page = if printed { e.page.as_ref().map(PageRef::printed) } else { None };
                        (e.level, e.title, page, left, top, printed_page)
                    })
                    .collect();
                let Some(list) = self.bookmark_lists.get_mut(&id) else { return };
                let at = (list.selected < list.rows.len()).then_some(list.selected);
                list.insert_rows(at, rows);
                if missing > 0 {
                    list.message = Some(crate::i18n::trn(
                        crate::i18n::current(),
                        missing as u64,
                        "{n} pasted line names a page this document doesn't have; it goes nowhere.",
                        "{n} pasted lines name pages this document doesn't have; they go nowhere.",
                    ));
                }
            }
        }
    }
}

/// Rows the engine would refuse, said before the edit is tried (the engine checks again).
fn pdfcraft_organize_check(doc: &pdfcraft_engine::Document, rows: &[OutlineRow]) -> Result<(), String> {
    if rows.len() > pdfcraft_engine::MAX_OUTLINE_ROWS {
        return Err(crate::i18n::fmt(tl!("At most {n} bookmarks can be written at once."), &[("n", &pdfcraft_engine::MAX_OUTLINE_ROWS.to_string())]));
    }
    if !(doc.allows_assembly() && doc.read_only_reason.is_none()) {
        return Err(tl!("This document's bookmarks can't be changed.").to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(levels: &[usize]) -> BookmarkList {
        let mut key = 0;
        let rows = levels
            .iter()
            .enumerate()
            .map(|(i, l)| {
                key += 1;
                EditRow {
                    key,
                    level: *l,
                    title: format!("R{i}"),
                    page_text: String::new(),
                    left: None,
                    top: None,
                    origin: None,
                    original: None,
                    element: None,
                    printed: None,
                }
            })
            .collect();
        BookmarkList::new(pdfcraft_engine::DocId(1), rows, key)
    }

    fn shape(e: &BookmarkList) -> Vec<(String, usize)> {
        e.rows.iter().map(|r| (r.title.clone(), r.level)).collect()
    }

    fn s(v: &[(&str, usize)]) -> Vec<(String, usize)> {
        v.iter().map(|(t, l)| (t.to_string(), *l)).collect()
    }

    #[test]
    fn indent_and_outdent_carry_the_rows_under_a_row() {
        let mut e = rows(&[0, 0, 1, 0]);
        assert!(!e.can_indent(0), "the first row has nothing to nest under");
        e.indent(1);
        assert_eq!(shape(&e), s(&[("R0", 0), ("R1", 1), ("R2", 2), ("R3", 0)]));
        assert!(!e.can_indent(1), "already one deeper than the row above");
        e.outdent(1);
        assert_eq!(shape(&e), s(&[("R0", 0), ("R1", 0), ("R2", 1), ("R3", 0)]));
        // Outdenting in place: a later sibling becomes its child.
        let mut e = rows(&[0, 1, 1]);
        e.outdent(1);
        assert_eq!(shape(&e), s(&[("R0", 0), ("R1", 0), ("R2", 1)]));
        e.undo();
        assert_eq!(shape(&e), s(&[("R0", 0), ("R1", 1), ("R2", 1)]));
        e.redo();
        assert_eq!(shape(&e), s(&[("R0", 0), ("R1", 0), ("R2", 1)]));
    }

    #[test]
    fn moving_swaps_whole_blocks_among_siblings_only() {
        let mut e = rows(&[0, 1, 0, 1, 1]);
        assert!(!e.can_move(0, false));
        e.move_row(0, true);
        assert_eq!(shape(&e), s(&[("R2", 0), ("R3", 1), ("R4", 1), ("R0", 0), ("R1", 1)]));
        assert_eq!(e.selected, 3);
        assert!(!e.can_move(1, false), "the first child has no sibling above");
        e.move_row(2, false);
        assert_eq!(shape(&e), s(&[("R2", 0), ("R4", 1), ("R3", 1), ("R0", 0), ("R1", 1)]));
    }

    #[test]
    fn dragging_a_block_fits_it_under_the_row_above() {
        let mut e = rows(&[0, 1, 2, 0]);
        e.move_block_to(1, 4);
        assert_eq!(shape(&e), s(&[("R0", 0), ("R3", 0), ("R1", 1), ("R2", 2)]));
        e.move_block_to(3, 0);
        assert_eq!(shape(&e), s(&[("R2", 0), ("R0", 0), ("R3", 0), ("R1", 1)]));
        let before = shape(&e);
        e.move_block_to(1, 1);
        assert_eq!(shape(&e), before, "onto itself changes nothing");
    }

    #[test]
    fn delete_add_and_insert_keep_the_nesting_valid() {
        let mut e = rows(&[0, 1, 1, 0]);
        e.delete(0);
        assert_eq!(shape(&e), s(&[("R3", 0)]));
        e.add_after(Some(0), Some(2), None);
        assert_eq!(e.rows.len(), 2);
        assert_eq!((e.rows[1].level, e.rows[1].page_text.as_str()), (0, "3"));
        e.insert_rows(Some(0), vec![(1, "Deep".into(), None, None, None, None), (0, "Flat".into(), Some(0), None, None, Some("iv".into()))]);
        assert_eq!(shape(&e), s(&[("R3", 0), ("Deep", 1), ("Flat", 0), ("", 0)]));
        let mut empty = rows(&[]);
        empty.delete(0);
        empty.indent(0);
        empty.move_row(0, true);
        empty.move_block_to(0, 3);
        assert!(empty.rows.is_empty());
    }
}

#[cfg(test)]
mod apply_cost {
    use crate::PdfCraftApp;

    /// A document of `pages` empty pages, each with its own page object.
    fn pdf(pages: usize) -> Vec<u8> {
        let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", i + 3)).collect();
        let mut out = format!(
            "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [{}] /Count {pages} /MediaBox [0 0 300 400] >> endobj\n",
            kids.join(" ")
        );
        for i in 0..pages {
            out.push_str(&format!("{} 0 obj << /Type /Page /Parent 2 0 R >> endobj\n", i + 3));
        }
        out.push_str("trailer << /Root 1 0 R >>\n%%EOF");
        out.into_bytes()
    }

    /// Apply on a list with a row that needs fixing (an empty title), best of five rounds, on a
    /// document of `pages` pages: everything Apply does before the edit.
    fn held_apply(pages: usize) -> std::time::Duration {
        let mut app = PdfCraftApp::new();
        app.open_bytes("pages.pdf", None, pdf(pages)).expect("opens");
        let id = app.active_ids().expect("active").1;
        app.open_bookmark_list(id);
        let list = app.bookmark_lists.get_mut(&id).expect("list editor");
        list.add_after(None, Some(0), None);
        list.rows[0].title.clear();
        let mut best = std::time::Duration::MAX;
        for _ in 0..5 {
            let start = std::time::Instant::now();
            for _ in 0..20 {
                app.bookmark_list_action(id, super::ListAction::Apply);
            }
            best = best.min(start.elapsed() / 20);
        }
        assert!(app.bookmark_lists.get(&id).and_then(|l| l.message.as_ref()).is_some(), "Apply was held back");
        best
    }

    /// Apply reads the document's pages where they are: what it does before the edit costs the
    /// same on a long document (it copied the whole document description each click).
    #[test]
    fn apply_costs_the_same_on_a_long_document() {
        let small = held_apply(10);
        let large = held_apply(20_000);
        eprintln!("list Apply before the edit: 10 pages {small:?}, 20000 pages {large:?}");
        assert!(large < small * 2 + std::time::Duration::from_micros(50), "10 pages {small:?}, 20000 pages {large:?}");
    }
}
