//! Across pages: running heads, footers and page numbers from what repeats at the same place, and
//! the section tree from the headings.

use super::{BlockKind, Diagnostic, PageLayout};
use std::collections::HashMap;

/// How far a running element may move between pages and still be the same one, in points.
const SAME_PLACE: f32 = 6.0;
/// Longest section title kept, in bytes.
const MAX_TITLE_BYTES: usize = 256;

/// One heading of the section tree.
#[derive(Clone, Debug, PartialEq)]
pub struct SectionNode {
    /// Page index (from 0) and the heading's index in that page's [`PageLayout::blocks`].
    pub page: usize,
    pub block: usize,
    pub level: u8,
    pub number: Option<String>,
    /// The heading's text without its number.
    pub title: String,
    /// Index of the parent in [`DocumentStructure::sections`].
    pub parent: Option<usize>,
    pub appendix: bool,
    /// The level comes from the heading's look, not its number.
    pub level_guessed: bool,
}

/// What the pages say together.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DocumentStructure {
    /// In reading order.
    pub sections: Vec<SectionNode>,
    pub diagnostics: Vec<Diagnostic>,
}

fn band(b: &super::Block, height: f32) -> Option<bool> {
    let cy = (b.rect[1] + b.rect[3]) / 2.0;
    if cy < 0.5 * height {
        Some(true)
    } else if cy > 0.5 * height {
        Some(false)
    } else {
        None
    }
}

/// A block taken for page furniture by its place on its page.
struct Cand {
    page: usize,
    block: usize,
    /// In the top half.
    top: bool,
    rect: [f32; 4],
    size: f32,
    numeric: bool,
    /// The page's width (for odd and even pages that mirror each other).
    width: f32,
}

impl Cand {
    /// The same element on another page: same band, height and size, both numbers or both not,
    /// aligned left, right or centre, or mirrored.
    fn same_place(&self, b: &Cand) -> bool {
        let (ra, rb) = (self.rect, b.rect);
        let mirrored = (ra[0] - (b.width - rb[2])).abs() < SAME_PLACE;
        let aligned =
            (ra[0] - rb[0]).abs() < SAME_PLACE || (ra[2] - rb[2]).abs() < SAME_PLACE || ((ra[0] + ra[2]) - (rb[0] + rb[2])).abs() < 2.0 * SAME_PLACE;
        b.page != self.page
            && b.top == self.top
            && ((ra[1] + ra[3]) / 2.0 - (rb[1] + rb[3]) / 2.0).abs() < SAME_PLACE
            && (self.size - b.size).abs() < 0.6
            && self.numeric == b.numeric
            && (aligned || mirrored)
    }
}

fn numeric(t: &str) -> bool {
    let t = t.trim();
    !t.is_empty() && t.len() <= 6 && (t.chars().all(|c| c.is_ascii_digit()) || t.chars().all(|c| "ivxlcdmIVXLCDM".contains(c)))
}

/// Confirms each page's running elements by repetition across `pages`, and builds the section tree
/// from the headings (levels from numbers, else from the look of numbered headings of a known level).
/// Updates the pages' blocks: kinds and confidences of running elements, heading levels.
pub fn analyse_document(pages: &mut [PageLayout]) -> DocumentStructure {
    let mut out = DocumentStructure::default();
    let cands: Vec<Cand> = pages
        .iter()
        .enumerate()
        .flat_map(|(pi, p)| {
            let (w, h) = (p.size[0], p.size[1]);
            p.blocks.iter().enumerate().filter(|(_, b)| b.kind.is_running()).filter_map(move |(bi, b)| {
                Some(Cand { page: pi, block: bi, top: band(b, h)?, rect: b.rect, size: b.style.size, numeric: numeric(&b.text), width: w })
            })
        })
        .collect();
    let page_count = pages.len();
    let confirmed: Vec<bool> = cands.iter().map(|a| cands.iter().any(|b| a.same_place(b))).collect();
    for (i, c) in cands.iter().enumerate() {
        let (pi, bi, top, num) = (c.page, c.block, c.top, c.numeric);
        let ok = confirmed.get(i).copied().unwrap_or(false);
        let Some(p) = pages.get_mut(pi) else { continue };
        // With three pages or more, page furniture repeats: text at the edge that does not is text.
        if !ok && !num && page_count >= 3 {
            if let Some(b) = p.blocks.get_mut(bi) {
                b.kind = BlockKind::Paragraph;
                b.confidence = 0.5;
            }
            p.diagnostics.retain(|d| !(d.code == "running_unconfirmed" && d.block == Some(bi)));
            continue;
        }
        if let Some(b) = p.blocks.get_mut(bi) {
            b.kind = if num {
                BlockKind::PageNumber
            } else if top {
                BlockKind::RunningHeader
            } else {
                BlockKind::RunningFooter
            };
            b.confidence = if ok { 0.95 } else { 0.4 };
        }
        if ok {
            p.diagnostics.retain(|d| !(d.code == "running_unconfirmed" && d.block == Some(bi)));
        } else if let Some(d) = p.diagnostics.iter_mut().find(|d| d.code == "running_unconfirmed" && d.block == Some(bi)) {
            d.message = "taken for page furniture by its place alone: nothing like it repeats on the other pages read".into();
        }
    }

    // Section tree. Styles of numbered headings give levels to unnumbered ones that look the same.
    let key = |b: &super::Block| ((b.style.size * 2.0).round() as i64, b.style.bold, b.style.small_caps);
    let mut style_level: HashMap<(i64, bool, bool), u8> = HashMap::new();
    for b in pages.iter().flat_map(|p| p.blocks.iter()) {
        if let Some(h) = b.heading.as_ref().filter(|h| !h.level_guessed && b.kind == BlockKind::Heading) {
            style_level.entry(key(b)).or_insert(h.level);
        }
    }
    let mut stack: Vec<(u8, usize)> = Vec::new();
    let mut in_appendix = false;
    for (pi, p) in pages.iter_mut().enumerate() {
        let order = p.reading_order.clone();
        for bi in order {
            let Some(b) = p.blocks.get_mut(bi) else { continue };
            if b.kind != BlockKind::Heading {
                continue;
            }
            let k = key(b);
            let Some(h) = b.heading.as_mut() else { continue };
            if h.level_guessed
                && let Some(&level) = style_level.get(&k)
            {
                h.level = level;
            }
            in_appendix |= h.appendix;
            h.appendix |= in_appendix;
            let level = h.level.max(1);
            while stack.last().is_some_and(|(l, _)| *l >= level) {
                stack.pop();
            }
            let mut title = b.text.trim().to_owned();
            if let Some(n) = &h.number {
                title = title
                    .strip_prefix("Appendix ")
                    .unwrap_or(&title)
                    .trim_start()
                    .strip_prefix(n.as_str())
                    .unwrap_or(&title)
                    .trim_start_matches('.')
                    .trim()
                    .to_owned();
            }
            super::cut(&mut title, MAX_TITLE_BYTES);
            let node = SectionNode {
                page: pi,
                block: bi,
                level,
                number: h.number.clone(),
                title,
                parent: stack.last().map(|(_, i)| *i),
                appendix: h.appendix,
                level_guessed: h.level_guessed && !style_level.contains_key(&k),
            };
            if node.level_guessed {
                out.diagnostics.push(Diagnostic {
                    code: "heading_level_guessed",
                    message: format!(
                        "page {}: heading \"{}\" has no number and no numbered heading looks like it: level {level} guessed",
                        pi + 1,
                        node.title
                    ),
                    block: Some(bi),
                });
            }
            stack.push((level, out.sections.len()));
            out.sections.push(node);
        }
    }
    out
}
