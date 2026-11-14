//! A page's text pieces put back into rows in reading order: columns first (a two-column table of
//! contents or paper is read down the left column, then the right), then rows down each column,
//! and the pieces of a row left to right. A wide gap inside a row is kept as [`GAP`], so a page
//! number set far from its title still reads as one.

use super::parse::GAP;

/// Most pieces of text read from one page.
pub(crate) const MAX_SEGS_PER_PAGE: usize = 20_000;
/// Longest row kept, in characters.
const MAX_ROW_CHARS: usize = 1000;
/// Columns are split at most this many times on a page (up to four columns).
const MAX_SPLIT_DEPTH: usize = 2;
/// Finest grid the gutter search uses, in bins across the text.
const MAX_BINS: usize = 2000;

/// A piece of text as the page draws it, in user space (y up).
#[derive(Clone, Debug, PartialEq)]
pub struct Seg {
    pub text: String,
    pub x0: f64,
    pub x1: f64,
    pub top: f64,
    pub bottom: f64,
    pub size: f64,
    pub bold: bool,
}

/// One row of text in reading order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Row {
    pub(crate) text: String,
    pub(crate) x0: f64,
    pub(crate) x1: f64,
    pub(crate) top: f64,
    pub(crate) bottom: f64,
    /// The size most of its text is set in.
    pub(crate) size: f64,
    /// Most of its text is bold.
    pub(crate) bold: bool,
    /// Which column it is in, counted from the left (0 when the page has one).
    pub(crate) column: usize,
}

fn usable(s: &Seg) -> bool {
    [s.x0, s.x1, s.top, s.bottom, s.size].iter().all(|v| v.is_finite())
        && s.size > 0.0
        && !s.text.trim().is_empty()
        && s.x1 >= s.x0
        && s.top >= s.bottom
}

/// The page's rows in reading order.
pub(crate) fn rows(segs: &[Seg]) -> Vec<Row> {
    let segs: Vec<&Seg> = segs.iter().filter(|s| usable(s)).take(MAX_SEGS_PER_PAGE).collect();
    let mut columns: Vec<Vec<&Seg>> = Vec::new();
    split_columns(segs, 0, &mut columns);
    let mut out = Vec::new();
    for (column, segs) in columns.into_iter().enumerate() {
        for mut row in rows_in_column(segs) {
            row.column = column;
            out.push(row);
        }
    }
    out
}

/// Split `segs` at a vertical gutter, left part first, recursing into each side.
fn split_columns<'a>(segs: Vec<&'a Seg>, depth: usize, out: &mut Vec<Vec<&'a Seg>>) {
    match (depth < MAX_SPLIT_DEPTH).then(|| gutter(&segs)).flatten() {
        Some(x) => {
            // A piece crossing the gutter (a centred heading over both columns) goes with the side
            // its middle is on.
            let (left, right): (Vec<&Seg>, Vec<&Seg>) = segs.into_iter().partition(|s| (s.x0 + s.x1) / 2.0 < x);
            split_columns(left, depth + 1, out);
            split_columns(right, depth + 1, out);
        }
        None => out.push(segs),
    }
}

/// Where a gutter runs between two columns: an empty vertical band in the middle half of the
/// text, at least a line high's width, with several rows of text on each side. Almost nothing
/// may cross it (a heading over both columns).
fn gutter(segs: &[&Seg]) -> Option<f64> {
    if segs.len() < 6 {
        return None;
    }
    let left = segs.iter().map(|s| s.x0).fold(f64::INFINITY, f64::min);
    let right = segs.iter().map(|s| s.x1).fold(f64::NEG_INFINITY, f64::max);
    let width = right - left;
    if !(width.is_finite() && width > 1.0) {
        return None;
    }
    let mut sizes: Vec<f64> = segs.iter().map(|s| s.size).collect();
    sizes.sort_by(f64::total_cmp);
    let size = sizes.get(sizes.len() / 2).copied().unwrap_or(10.0);
    let bins = (width.ceil() as usize).clamp(1, MAX_BINS);
    let bin = width / bins as f64;
    let mut cover = vec![0usize; bins];
    for s in segs {
        // Each bin index is within 0..bins: the offsets are clamped to the text's extent.
        let a = (((s.x0 - left) / bin).floor().max(0.0) as usize).min(bins - 1);
        let b = (((s.x1 - left) / bin).ceil().max(0.0) as usize).min(bins);
        for c in cover.iter_mut().take(b).skip(a) {
            *c = c.saturating_add(1);
        }
    }
    let allowed = (segs.len() / 40).max(1);
    let (from, to) = (bins / 4, bins.saturating_mul(3) / 4);
    let mut best: Option<(usize, usize)> = None;
    let mut run_start = None;
    for i in from..=to {
        let open = i < to && cover.get(i).is_some_and(|c| *c <= allowed);
        match (open, run_start) {
            (true, None) => run_start = Some(i),
            (false, Some(s)) => {
                if best.is_none_or(|(bs, be)| i - s > be - bs) {
                    best = Some((s, i));
                }
                run_start = None;
            }
            _ => {}
        }
    }
    let (s, e) = best?;
    let gap = (e - s) as f64 * bin;
    if gap < (size * 1.2).max(8.0) {
        return None;
    }
    let x = left + (s + e) as f64 / 2.0 * bin;
    // Rows of words on each side: a column of page numbers set far right of their titles is
    // not a column of its own.
    let wordy = |s: &Seg| s.text.chars().filter(|c| c.is_alphabetic()).count() >= 3 && super::parse::roman_value(s.text.trim()).is_none();
    let rows_on = |side: &dyn Fn(&Seg) -> bool| {
        let mut tops: Vec<i64> = segs.iter().filter(|s| side(s) && wordy(s)).map(|s| (s.top / size.max(1.0)).round() as i64).collect();
        tops.sort_unstable();
        tops.dedup();
        tops.len()
    };
    (rows_on(&|s| s.x1 <= x) >= 3 && rows_on(&|s| s.x0 >= x) >= 3).then_some(x)
}

/// The rows of one column: pieces whose heights overlap by half form a row, read left to right.
fn rows_in_column(mut segs: Vec<&Seg>) -> Vec<Row> {
    segs.sort_by(|a, b| b.top.total_cmp(&a.top).then(a.x0.total_cmp(&b.x0)));
    let mut groups: Vec<Vec<&Seg>> = Vec::new();
    for s in segs {
        let joins = groups.last().is_some_and(|g| {
            let top = g.iter().map(|x| x.top).fold(f64::NEG_INFINITY, f64::max);
            let bottom = g.iter().map(|x| x.bottom).fold(f64::INFINITY, f64::min);
            let overlap = s.top.min(top) - s.bottom.max(bottom);
            let height = (s.top - s.bottom).min(top - bottom).max(0.1);
            overlap >= height * 0.5
        });
        match groups.last_mut() {
            Some(g) if joins => g.push(s),
            _ => groups.push(vec![s]),
        }
    }
    groups.into_iter().filter_map(row_of).collect()
}

fn row_of(mut g: Vec<&Seg>) -> Option<Row> {
    g.sort_by(|a, b| a.x0.total_cmp(&b.x0));
    let first = g.first()?;
    let mut text = String::new();
    let mut last_x1 = first.x0;
    let (mut bold_chars, mut chars) = (0usize, 0usize);
    let mut by_size: Vec<(f64, usize)> = Vec::new();
    for s in &g {
        let piece = s.text.trim();
        let gap = s.x0 - last_x1;
        if !text.is_empty() {
            if gap > s.size * 1.5 {
                text.push(GAP);
            } else if gap > s.size * 0.12 && !text.ends_with(' ') {
                text.push(' ');
            }
        }
        text.push_str(piece);
        last_x1 = last_x1.max(s.x1);
        let n = piece.chars().count();
        chars = chars.saturating_add(n);
        if s.bold {
            bold_chars = bold_chars.saturating_add(n);
        }
        let size = (s.size * 2.0).round() / 2.0;
        match by_size.iter_mut().find(|(sz, _)| (*sz - size).abs() < 0.01) {
            Some((_, c)) => *c = c.saturating_add(n),
            None => by_size.push((size, n)),
        }
        if text.len() > MAX_ROW_CHARS.saturating_mul(4) {
            break;
        }
    }
    let text: String = text.chars().take(MAX_ROW_CHARS).collect();
    let size = by_size.iter().max_by_key(|(_, c)| *c).map_or(first.size, |(s, _)| *s);
    Some(Row {
        text,
        x0: g.iter().map(|s| s.x0).fold(f64::INFINITY, f64::min),
        x1: g.iter().map(|s| s.x1).fold(f64::NEG_INFINITY, f64::max),
        top: g.iter().map(|s| s.top).fold(f64::NEG_INFINITY, f64::max),
        bottom: g.iter().map(|s| s.bottom).fold(f64::INFINITY, f64::min),
        size,
        bold: bold_chars.saturating_mul(2) > chars,
        column: 0,
    })
}

#[cfg(test)]
pub(crate) fn seg(text: &str, x0: f64, top: f64, size: f64) -> Seg {
    // About half an em per character, as a proportional font sets Latin text.
    let w = text.chars().count() as f64 * size * 0.5;
    Seg { text: text.into(), x0, x1: x0 + w, top, bottom: top - size, size, bold: false }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pieces_of_a_row_join_with_a_gap_mark_when_far_apart() {
        let r = rows(&[seg("Introduction", 72.0, 700.0, 10.0), seg("3", 500.0, 700.0, 10.0), seg("Next", 72.0, 685.0, 10.0)]);
        assert_eq!(r.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), [format!("Introduction{GAP}3").as_str(), "Next"]);
    }

    #[test]
    fn two_columns_are_read_left_column_first() {
        let mut segs = Vec::new();
        for i in 0..8 {
            let top = 700.0 - f64::from(i) * 14.0;
            segs.push(seg(&format!("Left {i} .... {}", i + 1), 50.0, top, 10.0));
            segs.push(seg(&format!("Right {i} .... {}", i + 40), 330.0, top, 10.0));
        }
        let r = rows(&segs);
        assert_eq!(r.len(), 16);
        assert!(r[..8].iter().all(|r| r.text.starts_with("Left") && r.column == 0), "{r:?}");
        assert!(r[8..].iter().all(|r| r.text.starts_with("Right") && r.column == 1));
    }

    #[test]
    fn a_single_column_with_a_ragged_edge_is_not_split() {
        let segs: Vec<Seg> = (0..12)
            .map(|i| {
                let text = if i % 2 == 0 { "A fairly long line of body text that runs on" } else { "Short" };
                seg(text, 72.0, 700.0 - f64::from(i) * 12.0, 10.0)
            })
            .collect();
        assert!(rows(&segs).iter().all(|r| r.column == 0));
    }

    #[test]
    fn page_numbers_set_far_right_are_not_a_column() {
        let segs: Vec<Seg> = (0..8)
            .flat_map(|i| {
                let top = 700.0 - f64::from(i) * 14.0;
                [seg(&format!("Chapter title {i}"), 72.0, top, 10.0), seg(&(i * 7 + 1).to_string(), 520.0, top, 10.0)]
            })
            .collect();
        let r = rows(&segs);
        assert_eq!(r.len(), 8);
        assert!(r.iter().all(|r| r.column == 0 && r.text.contains(GAP)), "{r:?}");
    }

    #[test]
    fn broken_coordinates_are_skipped() {
        let mut bad = seg("x", f64::NAN, 10.0, 10.0);
        bad.x1 = f64::INFINITY;
        let negative = Seg { size: -1.0, ..seg("y", 0.0, 10.0, 10.0) };
        let inverted = Seg { x1: -5.0, ..seg("z", 0.0, 10.0, 10.0) };
        assert!(rows(&[bad, negative, inverted, seg("  ", 0.0, 0.0, 9.0)]).is_empty());
        let huge = Seg { x0: -1e300, x1: 1e300, ..seg("wide", 0.0, 10.0, 10.0) };
        let many: Vec<Seg> = (0..10).map(|i| seg("w", f64::from(i), f64::from(i) * 12.0, 10.0)).chain([huge]).collect();
        assert!(!rows(&many).is_empty());
    }
}
