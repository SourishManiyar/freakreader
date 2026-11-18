//! The list's own lines, rebuilt from its words: on a page of two columns of entries the page
//! layout may join a line of the left column with one of the right, or set the right column's
//! labels apart as a column of their own. Per page, the list's words are split at the gutter (the
//! widest gap across them that leaves at least [`SIDE_SHARE`] of their width on each side), then
//! grouped by baseline, column by column, top to bottom.

use super::Region;
use super::region::region_lines;
use crate::text::page_layout::refs::{LineRef, PageWords, Word};

/// A column of entries holds at least this share of the list's width on a page.
const SIDE_SHARE: f32 = 0.3;
/// A gutter is at least this wide, in points.
const MIN_GUTTER: f32 = 4.0;
/// Two words are on one line when their baselines are this close, in ems.
const SAME_LINE: f32 = 0.4;

/// A word of the list: the page line it came from, its index there, and the word.
pub(super) type ListWord<'a> = (LineRef, usize, &'a Word);

/// A line of the list.
pub(super) struct BibLine<'a> {
    pub page: usize,
    /// Left to right.
    pub words: Vec<ListWord<'a>>,
    pub rect: [f32; 4],
    /// The middle of its words' height (a baseline for measuring the space between lines).
    pub baseline: f32,
    pub size: f32,
    /// The left and right edge of its column.
    pub column: [f32; 2],
}

impl<'a> BibLine<'a> {
    pub fn word_list(&self) -> impl Iterator<Item = &'a Word> + '_ {
        self.words.iter().map(|(_, _, w)| *w)
    }
}

/// Where the gutter between two columns of `spans` (each word's left and right edge) is, if any.
fn gutter(spans: &mut [[f32; 2]]) -> Option<f32> {
    spans.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let x0 = spans.first()?[0];
    let x1 = spans.iter().map(|s| s[1]).fold(f32::NEG_INFINITY, f32::max);
    let width = x1 - x0;
    if !(width.is_finite() && width > 0.0) {
        return None;
    }
    // Of the gaps wide enough with enough on each side, the one that halves the text best: an alley
    // between a column's labels and its text is wide too, but off centre.
    let mut best: Option<(f32, f32)> = None;
    let mut reach = x0;
    for s in spans.iter() {
        let gap = s[0] - reach;
        let balance = (reach - x0).min(x1 - s[0]);
        if gap >= MIN_GUTTER && balance >= SIDE_SHARE * width && best.is_none_or(|(b, _)| balance > b) {
            best = Some((balance, (reach + s[0]) / 2.0));
        }
        reach = reach.max(s[1]);
    }
    best.map(|(_, at)| at)
}

/// The list's lines in reading order.
pub(super) fn build<'a>(words: &'a [PageWords], region: &Region) -> Vec<BibLine<'a>> {
    let refs = region_lines(words, region);
    let mut out = Vec::new();
    let mut k = 0;
    while let Some(&(page, _)) = refs.get(k) {
        // The region's lines on this page.
        let on_page: Vec<LineRef> = refs.iter().skip(k).take_while(|r| r.0 == page).copied().collect();
        k += on_page.len().max(1);
        let mut items: Vec<(ListWord<'a>, f32, f32)> = Vec::new();
        let mut column = [f32::INFINITY, f32::NEG_INFINITY];
        for &r in &on_page {
            let Some(l) = words.get(r.0).and_then(|w| w.lines.get(r.1)) else { continue };
            column = [column[0].min(l.column[0]), column[1].max(l.column[1])];
            for (i, w) in l.words.iter().enumerate() {
                // Each word's own height on the page: the layout's line may hold words of several.
                if w.rect.iter().all(|v| v.is_finite()) && !w.glyphs.is_empty() {
                    items.push(((r, i, w), (w.rect[1] + w.rect[3]) / 2.0, l.size));
                }
            }
        }
        let mut spans: Vec<[f32; 2]> = items.iter().map(|((_, _, w), _, _)| [w.rect[0], w.rect[2]]).collect();
        let split = gutter(&mut spans);
        if std::env::var_os("PDFCRAFT_BIB_DEBUG").is_some() {
            eprintln!("page {page}: {} words, gutter {split:?}", spans.len());
        }
        let sides: Vec<Vec<(ListWord<'a>, f32, f32)>> = match split {
            Some(g) => {
                let (left, right): (Vec<_>, Vec<_>) = items.into_iter().partition(|((_, _, w), _, _)| (w.rect[0] + w.rect[2]) / 2.0 < g);
                vec![left, right]
            }
            None => vec![items],
        };
        for mut side in sides {
            if side.is_empty() {
                continue;
            }
            let col = if split.is_some() {
                let a = side.iter().map(|((_, _, w), _, _)| w.rect[0]).fold(f32::INFINITY, f32::min);
                let b = side.iter().map(|((_, _, w), _, _)| w.rect[2]).fold(f32::NEG_INFINITY, f32::max);
                [a, b]
            } else {
                column
            };
            side.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.2.rect[0].total_cmp(&b.0.2.rect[0])));
            let mut line: Option<BibLine<'a>> = None;
            for (item, baseline, size) in side {
                let w = item.2;
                let same = line.as_ref().is_some_and(|l| (baseline - l.baseline).abs() <= SAME_LINE * l.size.max(size).max(1.0));
                if !same && let Some(done) = line.take() {
                    out.push(done);
                }
                match line.as_mut() {
                    Some(l) => {
                        l.rect = [l.rect[0].min(w.rect[0]), l.rect[1].min(w.rect[1]), l.rect[2].max(w.rect[2]), l.rect[3].max(w.rect[3])];
                        l.words.push(item);
                    }
                    None => line = Some(BibLine { page, words: vec![item], rect: w.rect, baseline, size, column: col }),
                }
            }
            if let Some(done) = line {
                out.push(done);
            }
        }
    }
    for l in &mut out {
        l.words.sort_by(|a, b| a.2.rect[0].total_cmp(&b.2.rect[0]));
    }
    out
}
