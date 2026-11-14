//! Editing the whole outline as a list (ISO 32000-2 §12.3.3): every bookmark is a row with a
//! nesting level, a title and a destination, and [`rewrite_outline`] writes a list back as the
//! document's outline in one go.
//!
//! A row that stands for an existing bookmark names its object ([`OutlineRow::origin`]); that
//! object is reused, so what the list doesn't show is kept: colour, style, a destination or
//! action the row leaves alone ([`RowTarget::Keep`]), structure links (`/SE`) and unknown keys.
//! Only the values that change are rewritten, and the links between items are rebuilt from the
//! list. Bookmarks the list leaves out are unlinked, as [`crate::delete_bookmark`] does.
//! Everything is checked before anything is written.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString};

use crate::outline::{LIST_DEPTH, container, go_to_at, items, put, recount, relink};
use crate::{MAX_IMPORT_BOOKMARKS, OutlineError, walk};

/// Most rows one list edit writes.
pub const MAX_ROWS: usize = MAX_IMPORT_BOOKMARKS;

/// Longest title a row may have, in characters.
pub const MAX_ROW_TITLE_CHARS: usize = 4096;

/// Where a row's bookmark goes.
#[derive(Clone, Debug, PartialEq)]
pub enum RowTarget {
    /// Leave an existing bookmark's destination or action as it is (a new row goes nowhere).
    Keep,
    /// Nowhere: the destination and action are removed.
    Nowhere,
    /// The 0-based `page`, at the user-space point (`left`, `top`) as `/XYZ left top null`; a
    /// coordinate left out keeps the reader's position that way.
    Page { page: usize, left: Option<f64>, top: Option<f64> },
}

/// One bookmark of an outline written as a list.
#[derive(Clone, Debug, PartialEq)]
pub struct OutlineRow {
    /// 0 for the top level; a row is at most one level deeper than the row before it.
    pub level: usize,
    pub title: String,
    pub target: RowTarget,
    /// The existing bookmark this row stands for (its data is kept), or `None` for a new one.
    pub origin: Option<ObjRef>,
    /// Shown expanded when it has children; `None` keeps an existing bookmark's state, and opens
    /// a new one or one that had no children before.
    pub open: Option<bool>,
    /// The structure element a new row stands for (`/SE`), as bookmarks made from tags have.
    pub element: Option<ObjRef>,
}

impl OutlineRow {
    /// A new row going to `page` (0-based) at `top` (user space), or to the page.
    pub fn new(level: usize, title: impl Into<String>, page: Option<usize>, top: Option<f64>) -> Self {
        let target = page.map_or(RowTarget::Nowhere, |page| RowTarget::Page { page, left: None, top });
        Self { level, title: title.into(), target, origin: None, open: None, element: None }
    }
}

/// The document's outline as rows that [`rewrite_outline`] writes back unchanged: every bookmark
/// in listing order, each naming its object and keeping its destination. Fails when bookmarks
/// are nested deeper than the listing reaches, since a list could not keep them.
pub fn outline_rows(doc: &Document) -> Result<Vec<crate::Bookmark>, OutlineError> {
    let listed = crate::bookmarks(doc);
    if items(doc).len() > listed.len() {
        return Err(OutlineError::Unlisted(LIST_DEPTH.saturating_add(1)));
    }
    if listed.len() > MAX_ROWS {
        return Err(OutlineError::TooManyBookmarks(MAX_ROWS));
    }
    Ok(listed)
}

fn row_error(i: usize, why: impl Into<String>) -> OutlineError {
    OutlineError::Row(i.saturating_add(1), why.into())
}

/// Check every row against the document before anything is written.
fn check(rows: &[OutlineRow], pages: usize, listed: &HashMap<ObjRef, crate::Bookmark>) -> Result<(), OutlineError> {
    if rows.len() > MAX_ROWS {
        return Err(OutlineError::TooManyBookmarks(MAX_ROWS));
    }
    let mut origins = HashSet::new();
    let mut previous: Option<usize> = None;
    for (i, row) in rows.iter().enumerate() {
        let deepest = previous.map_or(0, |p| p.saturating_add(1));
        if row.level > deepest {
            let why = match previous {
                None => "the first bookmark must be at the top level".to_string(),
                Some(_) => "a bookmark can be at most one level deeper than the one above it".to_string(),
            };
            return Err(row_error(i, why));
        }
        if row.level > LIST_DEPTH {
            return Err(OutlineError::TooDeep(LIST_DEPTH.saturating_add(1)));
        }
        previous = Some(row.level);
        let title = row.title.trim();
        let old = row.origin.and_then(|o| listed.get(&o));
        // An existing bookmark whose title is already empty may stay so: files have them.
        if title.is_empty() && old.is_none_or(|b| !b.title.is_empty()) {
            return Err(row_error(i, "the title must not be empty"));
        }
        if row.title.chars().count() > MAX_ROW_TITLE_CHARS {
            return Err(row_error(i, format!("the title is longer than {MAX_ROW_TITLE_CHARS} characters")));
        }
        if let Some(origin) = row.origin {
            if old.is_none() {
                return Err(row_error(i, "the bookmark it stands for is no longer in the outline"));
            }
            if !origins.insert(origin) {
                return Err(row_error(i, "another row already stands for the same bookmark"));
            }
        }
        if let RowTarget::Page { page, left, top } = &row.target {
            if *page >= pages {
                let why = format!("page {} is past the last page ({pages})", page.saturating_add(1));
                return Err(row_error(i, why));
            }
            if ![left, top].iter().copied().flatten().all(|v| v.is_finite()) {
                return Err(row_error(i, "a position must be a finite number"));
            }
        }
    }
    Ok(())
}

/// Whether going to `page` at (`left`, `top`) is where `old` already goes, so its destination
/// (and a view the list can't show, such as `/FitH`) is kept. A row giving only the page it
/// already goes to keeps it too.
fn same_target(old: &crate::Bookmark, page: usize, left: Option<f64>, top: Option<f64>) -> bool {
    if old.page != Some(page) {
        return false;
    }
    if left.is_none() && top.is_none() {
        return true;
    }
    match &old.view {
        Some((kind, values)) if kind.as_slice() == b"XYZ" => {
            let near = |a: Option<f64>, b: Option<&Option<f64>>| match (a, b.copied().flatten()) {
                (None, None) => true,
                (Some(a), Some(b)) => (a - b).abs() < 0.01,
                _ => false,
            };
            near(left, values.first()) && near(top, values.get(1))
        }
        _ => false,
    }
}

/// Write `rows` as the document's whole outline, as one change of the object graph: rows naming
/// an existing bookmark reuse its object (keeping everything the row doesn't change), new rows
/// get new items, and bookmarks no row names are unlinked. `rows` may be empty, which removes
/// every bookmark. Everything is checked first, so an error leaves the document as it was.
pub fn rewrite_outline(doc: &mut Document, rows: &[OutlineRow]) -> Result<(), OutlineError> {
    let pages: Vec<ObjRef> = walk(doc)?.into_iter().map(|(r, _)| r).collect();
    let listed: HashMap<ObjRef, crate::Bookmark> = outline_rows(doc)?.into_iter().map(|b| (b.obj, b)).collect();
    check(rows, pages.len(), &listed)?;
    let root = container(doc, &[])?;
    // Each row's item.
    let mut objs = Vec::with_capacity(rows.len());
    for row in rows {
        let title = row.title.trim();
        let old = row.origin.and_then(|o| listed.get(&o));
        let obj = match (row.origin, old) {
            (Some(o), Some(old)) => {
                if title != old.title {
                    put(doc, o, b"Title", Some(Object::String(PdfString::text(title))))?;
                }
                o
            }
            _ => {
                let mut d = Dict::new();
                d.set(b"Title".to_vec(), Object::String(PdfString::text(title)));
                if let Some(se) = row.element {
                    d.set(b"SE".to_vec(), Object::Ref(se));
                }
                doc.add(Object::Dict(d))
            }
        };
        match &row.target {
            RowTarget::Keep => {}
            RowTarget::Nowhere => {
                if old.is_some() {
                    put(doc, obj, b"Dest", None)?;
                    put(doc, obj, b"A", None)?;
                }
            }
            RowTarget::Page { page, left, top } => {
                if !old.is_some_and(|b| same_target(b, *page, *left, *top)) {
                    let page_obj = *pages.get(*page).ok_or(crate::OrganizeError::NoSuchPage(*page))?;
                    put(doc, obj, b"Dest", Some(go_to_at(page_obj, *left, *top)))?;
                    // `/Dest` and `/A` must not both be present (§12.3.3, Table 151).
                    put(doc, obj, b"A", None)?;
                }
            }
        }
        objs.push(obj);
    }
    // Children by row: each row's parent is the nearest row above it one level up.
    let mut kids: Vec<Vec<ObjRef>> = vec![Vec::new(); rows.len()];
    let mut top = Vec::new();
    // The row open at each level, outermost first.
    let mut open: Vec<usize> = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        open.truncate(row.level);
        match open.last().and_then(|&p| kids.get_mut(p)) {
            Some(list) => list.extend(objs.get(i).copied()),
            None => top.extend(objs.get(i).copied()),
        }
        open.push(i);
    }
    relink(doc, root, &top)?;
    for (i, row) in rows.iter().enumerate() {
        let (Some(&obj), Some(children)) = (objs.get(i), kids.get(i)) else { continue };
        relink(doc, obj, children)?;
        if children.is_empty() {
            continue;
        }
        // Only the sign of `/Count` is set here; `recount` works out its value. An existing
        // bookmark that had children has a nonzero `/Count` saying whether it was open.
        let count = doc.get(obj).as_dict().and_then(|d| d.int(b"Count")).unwrap_or(0);
        let open_now = match row.open {
            Some(open) => open,
            None if row.origin.is_some() && count != 0 => count > 0,
            None => true,
        };
        if count == 0 || (count > 0) != open_now {
            put(doc, obj, b"Count", Some(Object::Int(if open_now { 1 } else { -1 })))?;
        }
    }
    recount(doc)
}
