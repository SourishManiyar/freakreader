//! Split two-page spreads: one page that holds two book pages side by side becomes two pages,
//! each showing one half through its crop box (ISO 32000-2 §14.11.2).
//!
//! Nothing on the page is redrawn or re-encoded. The page object itself becomes the first page
//! in reading order, so every bookmark, link, named destination and `/OpenAction` that went to the
//! spread now goes to its first half. The second page is a new page object that shares the
//! spread's content streams and resources (the scan image stays once in the file). Its crop box
//! shows the other half; the media box is untouched on both, so the hidden half is still in the
//! file behind each crop box, as after Crop pages.
//!
//! Annotations go to the half that holds their `/Rect`: those wholly on the second half move to
//! the new page (their `/P` follows), those across the split line stay on the first page and a
//! copy goes on the second (a form widget becomes one more widget of the same field). A popup
//! goes with its markup annotation. What the new page does not carry is reported, never dropped
//! in silence: an annotation without a usable `/Rect`, a widget across the line that is its own
//! field, and the page's article beads (`/B`), structure (`/StructParents`), thumbnail (`/Thumb`)
//! and application data (`/PieceInfo`), which stay with the first page.
//!
//! The caller works out the geometry (see `pdfcraft_engine`): which half comes first, where the
//! split line is in user space, and both crop boxes. Here the document is only changed once every
//! requested page has been checked, so a bad page leaves it as it was.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::{INHERITABLE, OrganizeError, check, rebuild, walk};

/// The most pages one call splits. A larger request is refused up front rather than growing the
/// page tree without bound.
pub const MAX_SPLIT_PAGES: usize = 10_000;

/// The most annotations looked at on one page. Past it the page is refused: sorting them would
/// cost unbounded work, and leaving some out would drop them.
pub const MAX_PAGE_ANNOTS: usize = 100_000;

/// The axis of default user space the split line crosses: `X` for a vertical line `x = at`
/// (an unrotated or upside-down page), `Y` for a horizontal one `y = at` (a page turned 90° or
/// 270°, which the viewer shows upright).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitAxis {
    X,
    Y,
}

/// One page to split, all in its own default user space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageSplit {
    /// The page, 0-based, as numbered before the split.
    pub page: usize,
    /// The crop box of the first page in reading order (the page object that stays).
    pub first: [f64; 4],
    /// The crop box of the new, second page.
    pub second: [f64; 4],
    pub axis: SplitAxis,
    /// Where the split line is on `axis`.
    pub at: f64,
    /// True when the first page's half is the one on the low side of the line (smaller x or y).
    pub first_low: bool,
}

/// What became of one split page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SplitPage {
    /// The page, 0-based, as numbered before the split.
    pub page: usize,
    /// Its first half, 0-based, as numbered after the split; the second half follows it.
    pub first: usize,
    /// Annotations moved to the second half.
    pub moved: usize,
    /// Annotations across the split line, kept on the first half with a copy on the second.
    pub on_both: usize,
    /// What the second half does not carry, said in a sentence each (it stays with the first).
    pub first_only: Vec<String>,
}

/// Which half an annotation goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    First,
    Second,
    Both,
}

/// Page keys the second half does not take, with what they hold, for the report.
const FIRST_ONLY_KEYS: [(&[u8], &str); 4] = [
    (b"B", "article threads"),
    (b"StructParents", "tagged structure"),
    (b"Thumb", "the embedded thumbnail"),
    (b"PieceInfo", "application data (/PieceInfo)"),
];

/// Split each of `splits`' pages in two, the first half in place of the page and the second right
/// after it. Page labels move with the pages after them (a label range starts as many pages later
/// as pages were added before it), so numbering runs on through the new pages.
pub fn split_pages_in_two(doc: &mut Document, splits: &[PageSplit]) -> Result<Vec<SplitPage>, OrganizeError> {
    if splits.is_empty() {
        return Err(OrganizeError::Invalid("choose at least one page to split".into()));
    }
    if splits.len() > MAX_SPLIT_PAGES {
        return Err(OrganizeError::Invalid(format!("at most {MAX_SPLIT_PAGES} pages can be split at once; {} were chosen", splits.len())));
    }
    let all = walk(doc)?;
    let indices: Vec<usize> = splits.iter().map(|s| s.page).collect();
    check(&indices, all.len())?;
    let mut by_page: HashMap<usize, &PageSplit> = HashMap::with_capacity(splits.len());
    for s in splits {
        if by_page.insert(s.page, s).is_some() {
            return Err(OrganizeError::Invalid(format!("page {} is listed twice", s.page.saturating_add(1))));
        }
        validate(s)?;
    }
    // Every page is read and planned before anything is written, so one bad page changes nothing.
    let mut plans = Vec::with_capacity(splits.len());
    for (i, (obj, inherited)) in all.iter().enumerate() {
        if let Some(s) = by_page.get(&i) {
            plans.push(plan(doc, i, *obj, inherited, s)?);
        }
    }
    let mut order: Vec<(ObjRef, Dict)> = Vec::with_capacity(all.len().saturating_add(plans.len()));
    let mut out = Vec::with_capacity(plans.len());
    let mut plans = plans.into_iter().peekable();
    for (i, (obj, inherited)) in all.iter().enumerate() {
        order.push((*obj, inherited.clone()));
        if plans.peek().is_some_and(|p| p.page == i)
            && let Some(p) = plans.next()
        {
            let first = order.len().saturating_sub(1);
            let new = apply(doc, *obj, p.clone())?;
            order.push((new, Dict::new()));
            out.push(SplitPage { page: i, first, moved: p.moved, on_both: p.on_both, first_only: p.first_only });
        }
    }
    rebuild(doc, &order)?;
    let mut after = indices;
    after.sort_unstable();
    crate::labels::insert_after_pages(doc, &after)?;
    Ok(out)
}

/// A normalised `[x0, y0, x1, y1]` of four finite numbers.
fn normalised(r: [f64; 4]) -> Option<[f64; 4]> {
    r.iter().all(|v| v.is_finite()).then(|| [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])])
}

fn validate(s: &PageSplit) -> Result<(), OrganizeError> {
    let n = s.page.saturating_add(1);
    if !s.at.is_finite() {
        return Err(OrganizeError::InvalidBox(format!("page {n}: the split position is not a number")));
    }
    for r in [s.first, s.second] {
        let Some(r) = normalised(r) else {
            return Err(OrganizeError::InvalidBox(format!("page {n}: a half's crop box is not a number")));
        };
        let (w, h) = (r[2] - r[0], r[3] - r[1]);
        if !(w.is_finite() && h.is_finite()) || w < 1.0 || h < 1.0 {
            return Err(OrganizeError::InvalidBox(format!("page {n}: a half would be smaller than a point; use a smaller margin")));
        }
    }
    Ok(())
}

/// The new page and what goes onto each half, worked out without changing the document.
#[derive(Clone)]
struct Plan {
    page: usize,
    /// The second page's dictionary, without `/Annots`.
    second: Dict,
    first_crop: Object,
    /// The first page's `/Annots` after the split, when it changes.
    keep: Option<Vec<Object>>,
    /// Entries for the second page: moved ones as they are, copies to be made.
    give: Vec<Give>,
    moved: usize,
    on_both: usize,
    first_only: Vec<String>,
}

#[derive(Clone)]
enum Give {
    /// An indirect annotation that moves; its `/P` (when it has one) is rewritten.
    Move(ObjRef),
    /// A direct annotation dictionary that moves, or a copy of one across the line.
    Inline(Dict),
    /// A copy of an indirect annotation across the line (`/P` rewritten, `/StructParent` left
    /// off), with a copy of its popup when it has one.
    Copy { dict: Dict, popup: Option<Dict>, popup_listed: bool },
    /// One more widget of the field `parent`.
    Widget { dict: Dict, parent: ObjRef },
}

fn rect_obj(r: [f64; 4]) -> Object {
    Object::Array(r.iter().map(|v| Object::Real(*v)).collect())
}

/// An annotation's `/Rect`, normalised; `None` when it isn't four finite numbers.
fn annot_rect(doc: &Document, d: &Dict) -> Option<[f64; 4]> {
    let r = doc.resolve(d.get(b"Rect")?);
    let v: Vec<f64> = r.as_array()?.iter().take(5).filter_map(|x| doc.resolve(x).as_f64()).collect();
    normalised(<[f64; 4]>::try_from(v).ok()?)
}

fn side_of(rect: [f64; 4], s: &PageSplit) -> Side {
    let (lo, hi) = match s.axis {
        SplitAxis::X => (rect[0], rect[2]),
        SplitAxis::Y => (rect[1], rect[3]),
    };
    let low = if hi <= s.at {
        true
    } else if lo >= s.at {
        false
    } else {
        return Side::Both;
    };
    if low == s.first_low { Side::First } else { Side::Second }
}

fn plan(doc: &Document, i: usize, obj: ObjRef, inherited: &Dict, s: &PageSplit) -> Result<Plan, OrganizeError> {
    let n = i.saturating_add(1);
    let page = doc.get(obj).as_dict().cloned().ok_or_else(|| OrganizeError::Invalid(format!("page {n} is not a page dictionary")))?;
    let mut first_only = Vec::new();
    let mut second = Dict::new();
    for (k, v) in page.iter() {
        let k = k.as_slice();
        if k == b"Annots" || k == b"CropBox" || k == b"Parent" {
            continue;
        }
        if let Some((_, what)) = FIRST_ONLY_KEYS.iter().find(|(key, _)| *key == k) {
            first_only.push(format!("{what} stay with the first half"));
            continue;
        }
        second.set(k.to_vec(), v.clone());
    }
    for k in INHERITABLE {
        if k != b"CropBox"
            && !second.contains(k)
            && let Some(v) = inherited.get(k)
        {
            second.set(k.to_vec(), v.clone());
        }
    }
    second.set(b"Type".to_vec(), Object::name("Page"));
    second.set(b"CropBox".to_vec(), rect_obj(normalised(s.second).unwrap_or(s.second)));

    let list = page.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
    if list.len() > MAX_PAGE_ANNOTS {
        return Err(OrganizeError::Invalid(format!("page {n} has more than {MAX_PAGE_ANNOTS} annotations; it was not split")));
    }
    let dicts: Vec<Option<Dict>> = list.iter().map(|a| doc.resolve(a).as_dict().cloned()).collect();
    let refs: Vec<Option<ObjRef>> = list.iter().map(Object::as_ref).collect();
    let position: HashMap<ObjRef, usize> = refs.iter().enumerate().filter_map(|(j, r)| r.map(|r| (r, j))).collect();
    let mut no_rect = 0usize;
    let mut sides: Vec<Side> = dicts
        .iter()
        .map(|d| match d.as_ref().and_then(|d| annot_rect(doc, d)) {
            Some(r) => side_of(r, s),
            None => {
                no_rect += 1;
                Side::First
            }
        })
        .collect();
    // A popup goes where its annotation goes. One whose annotation is across the line stays with
    // it on the first half; the annotation's copy brings a copy of the popup.
    let mut copied_popups: HashSet<usize> = HashSet::new();
    for j in 0..dicts.len() {
        let Some(d) = dicts.get(j).and_then(Option::as_ref) else { continue };
        if d.name(b"Subtype") != Some(b"Popup") {
            continue;
        }
        let Some(k) = d.reference(b"Parent").and_then(|p| position.get(&p).copied()) else { continue };
        let parent = sides.get(k).copied();
        if let (Some(parent), Some(side)) = (parent, sides.get_mut(j)) {
            *side = if parent == Side::Both {
                copied_popups.insert(j);
                Side::First
            } else {
                parent
            };
        }
    }
    let mut keep = Vec::with_capacity(list.len());
    let mut give = Vec::new();
    let (mut moved, mut on_both, mut own_fields) = (0usize, 0usize, 0usize);
    for (j, entry) in list.iter().enumerate() {
        let side = sides.get(j).copied().unwrap_or(Side::First);
        let dict = dicts.get(j).cloned().flatten();
        match (side, refs.get(j).copied().flatten(), dict) {
            (Side::First, ..) | (_, _, None) => keep.push(entry.clone()),
            (Side::Second, Some(r), Some(_)) => {
                moved += 1;
                give.push(Give::Move(r));
            }
            (Side::Second, None, Some(d)) => {
                moved += 1;
                give.push(Give::Inline(d));
            }
            (Side::Both, r, Some(d)) => {
                keep.push(entry.clone());
                let mut copy = d.clone();
                copy.remove(b"StructParent");
                if d.name(b"Subtype") == Some(b"Widget") {
                    match d.reference(b"Parent") {
                        Some(parent) => {
                            on_both += 1;
                            give.push(Give::Widget { dict: copy, parent });
                        }
                        None => own_fields += 1,
                    }
                    continue;
                }
                on_both += 1;
                if r.is_none() {
                    give.push(Give::Inline(copy));
                    continue;
                }
                let popup_ref = d.reference(b"Popup");
                let popup = popup_ref.and_then(|p| doc.get(p).as_dict().cloned()).map(|mut p| {
                    p.remove(b"StructParent");
                    p
                });
                let popup_listed = popup_ref.and_then(|p| position.get(&p)).is_some_and(|k| copied_popups.contains(k));
                give.push(Give::Copy { dict: copy, popup, popup_listed });
            }
        }
    }
    if no_rect > 0 {
        first_only.push(if no_rect == 1 {
            "1 annotation without a usable /Rect stays on the first half".to_string()
        } else {
            format!("{no_rect} annotations without a usable /Rect stay on the first half")
        });
    }
    if own_fields > 0 {
        first_only.push(if own_fields == 1 {
            "1 form field across the split, being its own widget, stays on the first half only".to_string()
        } else {
            format!("{own_fields} form fields across the split, each its own widget, stay on the first half only")
        });
    }
    let changed = keep.len() != list.len();
    let first_crop = rect_obj(normalised(s.first).unwrap_or(s.first));
    Ok(Plan { page: i, second, first_crop, keep: changed.then_some(keep), give, moved, on_both, first_only })
}

/// Write one plan: the first page's crop box and annotations, then the new page. Returns the new page.
fn apply(doc: &mut Document, obj: ObjRef, p: Plan) -> Result<ObjRef, OrganizeError> {
    let new = doc.add(Object::Null);
    let Plan { second, first_crop, keep, give, .. } = p;
    let mut annots = Vec::with_capacity(give.len());
    for g in give {
        match g {
            Give::Move(r) => {
                if doc.get(r).as_dict().is_some_and(|d| d.contains(b"P")) {
                    doc.update_dict(r, |d| d.set(b"P".to_vec(), Object::Ref(new)))?;
                }
                annots.push(Object::Ref(r));
            }
            Give::Inline(mut d) => {
                if d.contains(b"P") {
                    d.set(b"P".to_vec(), Object::Ref(new));
                }
                annots.push(Object::Dict(d));
            }
            Give::Copy { mut dict, popup, popup_listed } => {
                if dict.contains(b"P") {
                    dict.set(b"P".to_vec(), Object::Ref(new));
                }
                let r = doc.add(Object::Null);
                if let Some(mut popup) = popup {
                    let pr = doc.add(Object::Null);
                    popup.set(b"Parent".to_vec(), Object::Ref(r));
                    if popup.contains(b"P") {
                        popup.set(b"P".to_vec(), Object::Ref(new));
                    }
                    doc.set(pr, Object::Dict(popup));
                    dict.set(b"Popup".to_vec(), Object::Ref(pr));
                    doc.set(r, Object::Dict(dict));
                    annots.push(Object::Ref(r));
                    if popup_listed {
                        annots.push(Object::Ref(pr));
                    }
                } else {
                    doc.set(r, Object::Dict(dict));
                    annots.push(Object::Ref(r));
                }
            }
            Give::Widget { mut dict, parent } => {
                if dict.contains(b"P") {
                    dict.set(b"P".to_vec(), Object::Ref(new));
                }
                let r = doc.add(Object::Dict(dict));
                crate::import::add_kid(doc, parent, r)?;
                annots.push(Object::Ref(r));
            }
        }
    }
    let mut second = second;
    if !annots.is_empty() {
        second.set(b"Annots".to_vec(), Object::Array(annots));
    }
    doc.set(new, Object::Dict(second));
    doc.update_dict(obj, |d| {
        d.set(b"CropBox".to_vec(), first_crop);
        // Written on the page itself: an `/Annots` array kept as its own object might be shared.
        if let Some(keep) = keep {
            d.set(b"Annots".to_vec(), Object::Array(keep));
        }
    })?;
    Ok(new)
}

#[cfg(test)]
mod tests;
