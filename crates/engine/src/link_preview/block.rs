//! The block a destination names, found from what its page draws ([`MathPage`]): the lines of
//! an equation, a theorem's statement, one bibliography entry, a heading and its first lines, a
//! float and its caption.
//!
//! Typeset text is read as **bands**: runs of ink, top to bottom, with no vertical gap inside
//! (a line of text, with its sub- and superscripts and the limits of a big operator). Within the
//! destination's column the gaps between bands, and where each band starts across, tell lines
//! of a paragraph (small gaps), displays (indented from the column's left edge), entries with a
//! hanging indent and the space around floats apart. The measures below are TeX's defaults for
//! 10–12 pt article-like classes, measured on the test papers in
//! `crates/render/tests/data/hyperref`; a page they don't fit (no text, a scan, text not running
//! left to right) gives no block, and the caller shows its fixed band instead.

use pdfcraft_render::text::math::MathPage;

use super::names::RefKind;

/// Vertical slack within which two pieces of ink join one band, in points.
const BAND_JOIN: f32 = 0.3;
/// How far below the destination its block may start, in points: a destination marks the top
/// of what it names, a few points above its first line.
const MAX_LEAD: f32 = 36.0;
/// A band at least this far right of the column's left edge, in points, is a display: wider
/// than any paragraph indent (15–18 pt in the standard classes).
const DISPLAY_INDENT: f32 = 20.0;
/// The largest gap between the rows of one display (`align`'s, a big operator's limits).
const DISPLAY_ROW_GAP: f32 = 9.0;
/// The largest gap between a display and the text of its paragraph, above or below it.
const DISPLAY_GAP: f32 = 16.0;
/// The gap between lines of a paragraph is at most this many times the column's typical one,
/// within [`LINE_GAP_MIN`, `LINE_GAP_MAX`].
const LINE_GAP_FACTOR: f32 = 1.8;
const LINE_GAP_MIN: f32 = 3.0;
const LINE_GAP_MAX: f32 = 6.0;
/// The line gap assumed when the column has no two full lines to measure.
const LINE_GAP_DEFAULT: f32 = 4.5;
/// A band at least this share of the column's width is a full line of body text.
const FULL_LINE_SHARE: f32 = 0.6;
/// Lines shown under a heading.
const SECTION_LINES: usize = 3;
/// The largest gap between a heading and its first lines, in points.
const SECTION_GAP: f32 = 30.0;
/// The largest gap between the rows of a table or the parts of a figure.
const FLOAT_ROW_GAP: f32 = 8.0;
/// The tallest block, as a share of the page's height; a figure may be taller.
const MAX_BLOCK_SHARE: f32 = 0.45;
const MAX_FLOAT_SHARE: f32 = 0.6;
/// Room left around the block, in points.
const PAD_X: f32 = 6.0;
const PAD_Y: f32 = 4.0;
/// A gutter between two columns is at least this wide, in points.
const MIN_GUTTER: f32 = 6.0;
/// The widest page the column search looks across, in points (a 200-inch page): a hostile page
/// size must not size its histogram.
const MAX_SCAN_WIDTH: f32 = 14_400.0;

/// A run of ink with no vertical gap in it, in one column.
#[derive(Clone, Copy, Debug)]
struct Band {
    y0: f32,
    y1: f32,
    x0: f32,
    x1: f32,
    /// The font of its leftmost glyph, when it has a glyph.
    lead_font: Option<u32>,
    /// The font most of its glyphs use.
    main_font: Option<u32>,
}

/// One piece of ink: a glyph's ink box (with its font) or a rule's.
#[derive(Clone, Copy, Debug)]
struct Ink {
    rect: [f32; 4],
    font: Option<u32>,
}

/// The area of the page (view space, `[x0, y0, x1, y1]`) that shows the block of kind `kind`
/// whose destination is at `(x, y)` (view points; `x` unknown when the destination pins only
/// its height). `size` is the page's displayed size. `None` when the page has no block to show
/// there: no text near the destination, text that does not run left to right, a kind with no
/// block (a page, an unknown name).
pub(super) fn block_area(math: &MathPage, size: [f32; 2], x: Option<f32>, y: f32, kind: RefKind) -> Option<[f32; 4]> {
    let [w, h] = size;
    if !(w.is_finite() && h.is_finite() && w >= 1.0 && h >= 1.0 && y.is_finite()) || matches!(kind, RefKind::Page | RefKind::Unknown) {
        return None;
    }
    let inks = inks(math)?;
    let (cx0, cx1) = column(&inks, w, x.filter(|x| x.is_finite()));
    let in_column: Vec<Ink> = inks.into_iter().filter(|i| (cx0..=cx1).contains(&((i.rect[0] + i.rect[2]) / 2.0))).collect();
    let bands = bands(in_column);
    let layout = Layout::of(&bands)?;
    let first = bands.iter().position(|b| b.y1 > y + 0.5)?;
    let lead = bands.get(first)?.y0 - y;
    if lead > MAX_LEAD && kind != RefKind::Figure {
        return None;
    }
    let (top, bottom) = match kind {
        RefKind::Equation => (layout.display_start(&bands, first), layout.display_end(&bands, first)),
        RefKind::TheoremLike => (first, layout.statement_end(&bands, first)),
        RefKind::Citation | RefKind::Item => (first, layout.entry_end(&bands, first)),
        RefKind::Section => (first, section_end(&bands, first)),
        RefKind::Figure | RefKind::Table => return layout.float(&bands, first, kind, h).map(|(y0, y1)| layout.area(y0, y1, size)),
        RefKind::Page | RefKind::Unknown => return None,
    };
    let (y0, mut y1) = (bands.get(top)?.y0, bands.get(bottom)?.y1);
    // A block taller than the cap ends at the last band that fits (the first always shows).
    let cap = h * MAX_BLOCK_SHARE;
    if y1 - y0 > cap {
        y1 = bands.get(top..=bottom)?.iter().map(|b| b.y1).filter(|b1| b1 - y0 <= cap).fold(bands.get(top)?.y1, f32::max);
    }
    Some(layout.area(y0, y1, size))
}

/// The page's ink, when its text runs left to right: every visible glyph's ink box (its em box
/// when it has no outline) and every rule, with finite coordinates.
fn inks(math: &MathPage) -> Option<Vec<Ink>> {
    let finite = |r: &[f32; 4]| r.iter().all(|v| v.is_finite()) && r[0] <= r[2] && r[1] <= r[3];
    let mut across = 0usize;
    let mut out = Vec::with_capacity(math.glyphs.len().saturating_add(math.rules.len()));
    for g in math.glyphs.iter().filter(|g| !g.invisible) {
        let Some(rect) = g.ink.filter(finite) else { continue };
        let [dx, dy] = g.direction;
        if dx.is_finite() && dy.is_finite() && dx > 0.0 && dy.abs() < dx * 0.2 {
            across = across.saturating_add(1);
        }
        out.push(Ink { rect, font: Some(g.font) });
    }
    // Rotated or vertical text is not read: the bands would cut across its lines.
    if out.is_empty() || across.saturating_mul(2) < out.len() {
        return None;
    }
    out.extend(math.rules.iter().filter(|r| finite(&r.rect)).map(|r| Ink { rect: r.rect, font: None }));
    Some(out)
}

/// The column `x` is in, as `(left, right)` in view points: the page split at a gutter between
/// two columns of text when it has one and `x` is known, else the whole width.
fn column(inks: &[Ink], width: f32, x: Option<f32>) -> (f32, f32) {
    let whole = (f32::NEG_INFINITY, f32::INFINITY);
    let Some(x) = x else { return whole };
    let bins = width.min(MAX_SCAN_WIDTH).ceil().max(1.0) as usize;
    // How many glyphs cover each point across, summed from where each starts and ends.
    let mut edges = vec![0i64; bins + 1];
    let glyphs = inks.iter().filter(|i| i.font.is_some());
    let mut count = 0usize;
    for i in glyphs {
        let a = (i.rect[0].max(0.0) as usize).min(bins);
        let b = (i.rect[2].max(0.0).ceil() as usize).min(bins);
        if a < b {
            if let Some(e) = edges.get_mut(a) {
                *e += 1;
            }
            if let Some(e) = edges.get_mut(b) {
                *e -= 1;
            }
            count = count.saturating_add(1);
        }
    }
    let mut cover = Vec::with_capacity(bins);
    let mut running = 0i64;
    for e in edges.iter().take(bins) {
        running += e;
        cover.push(running);
    }
    // Where glyphs end and start across, to count those left and right of a point.
    let mut ends: Vec<f32> = inks.iter().filter(|i| i.font.is_some()).map(|i| i.rect[2]).collect();
    let mut starts: Vec<f32> = inks.iter().filter(|i| i.font.is_some()).map(|i| i.rect[0]).collect();
    ends.sort_by(f32::total_cmp);
    starts.sort_by(f32::total_cmp);
    // A gutter: points in the middle half of the page that almost no glyph covers (a title or a
    // full-width float may cross it), with text on both sides of it.
    let quiet = (count / 200).max(1) as i64;
    let (from, to) = (bins / 4, bins.saturating_mul(3) / 4);
    let mut best: Option<(usize, usize)> = None;
    let mut start = None;
    for (p, c) in cover.iter().enumerate().take(to + 1).skip(from) {
        match (start, *c <= quiet && p < to) {
            (None, true) => start = Some(p),
            (Some(s), false) => {
                let mid = (s + p) / 2;
                let off_centre = |run: (usize, usize)| ((run.0 + run.1) / 2).abs_diff(bins / 2);
                if (p - s) as f32 >= MIN_GUTTER && best.is_none_or(|b| off_centre((s, p)) < off_centre(b)) {
                    let left = ends.partition_point(|e| *e <= mid as f32);
                    let right = starts.len().saturating_sub(starts.partition_point(|x| *x < mid as f32));
                    if left.saturating_mul(10) >= count && right.saturating_mul(10) >= count {
                        best = Some((s, p));
                    }
                }
                start = None;
            }
            _ => {}
        }
    }
    match best {
        Some((s, e)) => {
            let mid = (s + e) as f32 / 2.0;
            if x < mid { (f32::NEG_INFINITY, mid) } else { (mid, f32::INFINITY) }
        }
        None => whole,
    }
}

/// The ink joined into bands, top to bottom.
fn bands(mut inks: Vec<Ink>) -> Vec<Band> {
    inks.sort_by(|a, b| a.rect[1].total_cmp(&b.rect[1]));
    let mut out: Vec<(Band, Vec<Ink>)> = Vec::new();
    for ink in inks {
        let [x0, y0, x1, y1] = ink.rect;
        match out.last_mut() {
            Some((band, members)) if y0 <= band.y1 + BAND_JOIN => {
                band.y1 = band.y1.max(y1);
                band.x0 = band.x0.min(x0);
                band.x1 = band.x1.max(x1);
                members.push(ink);
            }
            _ => out.push((Band { y0, y1, x0, x1, lead_font: None, main_font: None }, vec![ink])),
        }
    }
    out.into_iter()
        .map(|(mut band, members)| {
            band.lead_font = members.iter().filter(|i| i.font.is_some()).min_by(|a, b| a.rect[0].total_cmp(&b.rect[0])).and_then(|i| i.font);
            band.main_font = most_common(members.iter().filter_map(|i| i.font));
            band
        })
        .collect()
}

/// The value that occurs most often (the smallest of a tie).
fn most_common(values: impl Iterator<Item = u32>) -> Option<u32> {
    let mut counts = std::collections::BTreeMap::new();
    for v in values {
        *counts.entry(v).or_insert(0usize) += 1;
    }
    counts.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).map(|(v, _)| v)
}

/// The column's measures: its text's left and right edges, and the largest gap between lines
/// of one paragraph.
struct Layout {
    left: f32,
    right: f32,
    line_gap: f32,
}

impl Layout {
    fn of(bands: &[Band]) -> Option<Self> {
        if bands.is_empty() {
            return None;
        }
        // The left edge most lines start at: body text outnumbers headings and displays.
        let mut starts = std::collections::BTreeMap::new();
        for b in bands {
            *starts.entry(b.x0.clamp(-1e6, 1e6) as i64).or_insert(0usize) += 1;
        }
        let mode = starts.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))).map(|(x, _)| *x as f32)?;
        let left = bands.iter().map(|b| b.x0).filter(|x0| (x0 - mode).abs() < 1.0).fold(mode + 1.0, f32::min);
        let right = bands.iter().map(|b| b.x1).fold(f32::NEG_INFINITY, f32::max);
        let width = (right - left).max(1.0);
        let full = |b: &Band| b.x1 - b.x0 >= width * FULL_LINE_SHARE;
        let mut gaps: Vec<f32> = bands.windows(2).filter(|p| full(&p[0]) && full(&p[1])).map(|p| p[1].y0 - p[0].y1).filter(|g| *g >= 0.0).collect();
        gaps.sort_by(f32::total_cmp);
        let line_gap = gaps.get(gaps.len() / 2).map_or(LINE_GAP_DEFAULT, |g| (g * LINE_GAP_FACTOR).clamp(LINE_GAP_MIN, LINE_GAP_MAX));
        Some(Layout { left, right, line_gap })
    }

    fn display(&self, b: &Band) -> bool {
        b.x0 >= self.left + DISPLAY_INDENT
    }

    /// A full line of body text: as wide as most of the column, from its left edge (a plot's
    /// axes or a wide display start further in).
    fn full(&self, b: &Band) -> bool {
        b.x1 - b.x0 >= (self.right - self.left).max(1.0) * FULL_LINE_SHARE && !self.display(b)
    }

    /// The first row of the display whose row `first` is: the rows above it that are indented as
    /// displays are and close to it, so an `align` shows whole whichever row is named.
    fn display_start(&self, bands: &[Band], first: usize) -> usize {
        let mut start = first;
        while let (Some(row), Some(below)) = (start.checked_sub(1).and_then(|i| bands.get(i)), bands.get(start)) {
            if !(self.display(row) && below.y0 - row.y1 <= DISPLAY_ROW_GAP) {
                break;
            }
            start -= 1;
        }
        start
    }

    /// The last row of the display that starts at `first`: the rows below it that are indented
    /// as displays are and close to it (an `align`'s rows, a big operator's limits).
    fn display_end(&self, bands: &[Band], first: usize) -> usize {
        let mut last = first;
        for (i, pair) in bands.windows(2).enumerate().skip(first) {
            if !(self.display(&pair[1]) && pair[1].y0 - pair[0].y1 <= DISPLAY_ROW_GAP) {
                break;
            }
            last = i + 1;
        }
        last
    }

    /// The last band of the statement that starts at `first`: its lines, the displays in it, and
    /// its text after a display (set in the statement's own font). It ends at a wider gap, or at
    /// a line after a display that starts in another font (the next statement's heading).
    fn statement_end(&self, bands: &[Band], first: usize) -> usize {
        let mut last = first;
        let mut fonts = std::collections::BTreeMap::<u32, usize>::new();
        let mut after_display = bands.get(first).is_some_and(|b| self.display(b));
        if !after_display && let Some(f) = bands.get(first).and_then(|b| b.main_font) {
            *fonts.entry(f).or_default() += 1;
        }
        for (i, pair) in bands.windows(2).enumerate().skip(first) {
            let (b, gap) = (&pair[1], pair[1].y0 - pair[0].y1);
            let body = fonts.iter().max_by(|a, c| a.1.cmp(c.1).then(c.0.cmp(a.0))).map(|(f, _)| *f);
            let display = self.display(b);
            let goes_on = gap <= self.line_gap
                || (display && gap <= DISPLAY_GAP)
                || (after_display && !display && gap <= DISPLAY_GAP && body.is_some() && b.lead_font == body);
            if !goes_on {
                break;
            }
            if !display && let Some(f) = b.main_font {
                *fonts.entry(f).or_default() += 1;
            }
            after_display = display;
            last = i + 1;
        }
        last
    }

    /// The last line of the entry that starts at `first` (a bibliography entry, a list item):
    /// the lines below it that are indented past where it starts and close to it.
    fn entry_end(&self, bands: &[Band], first: usize) -> usize {
        let Some(start) = bands.get(first).map(|b| b.x0) else { return first };
        let mut last = first;
        for (i, pair) in bands.windows(2).enumerate().skip(first) {
            if !(pair[1].x0 > start + 1.5 && pair[1].y0 - pair[0].y1 <= self.line_gap) {
                break;
            }
            last = i + 1;
        }
        last
    }

    /// The top and bottom (view points) of the float whose caption starts at `first`: a table's
    /// rows below its caption, a figure's picture above it, else whichever side has something.
    fn float(&self, bands: &[Band], first: usize, kind: RefKind, height: f32) -> Option<(f32, f32)> {
        // The caption's lines.
        let mut caption_end = first;
        while let (Some(a), Some(b)) = (bands.get(caption_end), bands.get(caption_end + 1)) {
            if b.y0 - a.y1 > self.line_gap {
                break;
            }
            caption_end += 1;
        }
        let (top, bottom) = (bands.get(first)?.y0, bands.get(caption_end)?.y1);
        let cap = height * MAX_FLOAT_SHARE;
        let below = || {
            let mut end = caption_end;
            while let (Some(a), Some(b)) = (bands.get(end), bands.get(end + 1)) {
                if b.y0 - a.y1 > FLOAT_ROW_GAP || b.y1 - top > cap {
                    break;
                }
                end += 1;
            }
            (end > caption_end).then(|| bands.get(end).map(|b| b.y1)).flatten()
        };
        let above = || self.picture_top(bands, first, bottom - cap).filter(|t| top - *t >= 2.0 * PAD_Y);
        let found = if kind == RefKind::Table {
            below().map(|b| (top, b)).or_else(|| above().map(|t| (t, bottom)))
        } else {
            above().map(|t| (t, bottom)).or_else(|| below().map(|b| (top, b)))
        };
        Some(found.unwrap_or((top, bottom)))
    }

    /// The top of a figure's picture, whose caption starts at band `caption`: just below the text
    /// above it (the last line of the paragraph that holds the first full line found going up),
    /// or the top of the highest ink above when there is no text, no higher than `limit`.
    fn picture_top(&self, bands: &[Band], caption: usize, limit: f32) -> Option<f32> {
        let above = bands.get(..caption)?;
        if above.is_empty() {
            return None;
        }
        let mut top = None;
        for (j, b) in above.iter().enumerate().rev() {
            if self.full(b) {
                // The paragraph that holds this line ends a few lines further down, perhaps with a
                // short last line.
                let mut end = j;
                while let (Some(a), Some(n)) = (above.get(end), above.get(end + 1)) {
                    if n.y0 - a.y1 > self.line_gap {
                        break;
                    }
                    end += 1;
                }
                return above.get(end).map(|b| b.y1 + PAD_Y).filter(|t| *t >= limit).or(top);
            }
            if b.y0 < limit {
                break;
            }
            top = Some(b.y0);
        }
        top
    }

    /// The area from `y0` to `y1` across the column, with room around it, on the page.
    fn area(&self, y0: f32, y1: f32, [w, h]: [f32; 2]) -> [f32; 4] {
        let x0 = (self.left - PAD_X).clamp(0.0, w - 1.0);
        let x1 = (self.right + PAD_X).clamp(x0 + 1.0, w);
        let y0 = (y0 - PAD_Y).clamp(0.0, h - 1.0);
        let y1 = (y1 + PAD_Y).clamp(y0 + 1.0, h);
        [x0, y0, x1, y1]
    }
}

/// The last band shown for a heading at `first`: the heading and up to [`SECTION_LINES`] lines
/// under it, while they follow closely.
fn section_end(bands: &[Band], first: usize) -> usize {
    let mut last = first;
    for (i, pair) in bands.windows(2).enumerate().skip(first).take(SECTION_LINES) {
        if pair[1].y0 - pair[0].y1 > SECTION_GAP {
            break;
        }
        last = i + 1;
    }
    last
}

#[cfg(test)]
mod tests {
    use pdfcraft_render::text::math::{GlyphDetail, MathPage};

    use super::*;

    fn glyph(rect: [f32; 4], font: u32, direction: [f32; 2]) -> GlyphDetail {
        GlyphDetail {
            code: 65,
            font,
            glyph_id: None,
            name: None,
            text: Some("A".into()),
            unmapped: false,
            size: 10.0,
            rect,
            ink: Some(rect),
            baseline: [[rect[0], rect[3]], [rect[2], rect[3]]],
            direction,
            invisible: false,
        }
    }

    /// A line of text from `x0` to `x1` with its top at `y`, 8 points tall, in `font`.
    fn line(x0: f32, x1: f32, y: f32, font: u32) -> Vec<GlyphDetail> {
        let n = ((x1 - x0) / 6.0).max(1.0) as usize;
        (0..n).map(|i| glyph([x0 + i as f32 * 6.0, y, x0 + i as f32 * 6.0 + 5.0, y + 8.0], font, [1.0, 0.0])).collect()
    }

    /// Body text in lines 11 points apart from `y`, with a display in the middle.
    fn page() -> MathPage {
        let mut glyphs = Vec::new();
        for k in 0..4 {
            glyphs.extend(line(72.0, 540.0, 100.0 + 11.0 * k as f32, 0));
        }
        glyphs.extend(line(250.0, 400.0, 155.0, 1));
        for k in 0..3 {
            glyphs.extend(line(72.0, 540.0, 175.0 + 11.0 * k as f32, 0));
        }
        MathPage { glyphs, ..Default::default() }
    }

    const LETTER: [f32; 2] = [612.0, 792.0];

    #[test]
    fn a_display_is_its_own_lines() {
        let area = block_area(&page(), LETTER, Some(250.0), 152.0, RefKind::Equation).expect("a block");
        assert!((area[1] - (155.0 - PAD_Y)).abs() < 0.01 && (area[3] - (163.0 + PAD_Y)).abs() < 0.01, "{area:?}");
        // Across the column, from its text's left edge to its right.
        assert!((area[0] - (72.0 - PAD_X)).abs() < 0.01 && area[2] > 535.0, "{area:?}");
    }

    #[test]
    fn a_page_with_nothing_to_read_has_no_block() {
        let empty = MathPage::default();
        assert_eq!(block_area(&empty, LETTER, None, 100.0, RefKind::TheoremLike), None);
        // Text running down the page.
        let mut down = page();
        for g in &mut down.glyphs {
            g.direction = [0.0, 1.0];
        }
        assert_eq!(block_area(&down, LETTER, None, 98.0, RefKind::TheoremLike), None);
        // A destination far above any text, and kinds with no block.
        assert_eq!(block_area(&page(), LETTER, None, 10.0, RefKind::TheoremLike), None);
        assert_eq!(block_area(&page(), LETTER, None, 98.0, RefKind::Page), None);
        assert_eq!(block_area(&page(), LETTER, None, 98.0, RefKind::Unknown), None);
        // Past the last line.
        assert_eq!(block_area(&page(), LETTER, None, 700.0, RefKind::Citation), None);
    }

    #[test]
    fn hostile_numbers_never_leave_the_page() {
        let mut page = page();
        page.glyphs.push(glyph([f32::NAN, 120.0, 90.0, 128.0], 0, [1.0, 0.0]));
        page.glyphs.push(glyph([-1e30, 130.0, 1e30, 131.0], 0, [1.0, 0.0]));
        page.glyphs.push(glyph([80.0, 140.0, 90.0, 148.0], 0, [f32::NAN, f32::INFINITY]));
        for size in [LETTER, [1e9, 792.0], [612.0, 1e9]] {
            for x in [None, Some(f32::NAN), Some(-5.0), Some(1e30)] {
                for kind in [RefKind::Equation, RefKind::TheoremLike, RefKind::Citation, RefKind::Section, RefKind::Figure, RefKind::Table] {
                    if let Some(a) = block_area(&page, size, x, 98.0, kind) {
                        assert!(a.iter().all(|v| v.is_finite()), "{a:?}");
                        assert!(a[0] >= 0.0 && a[1] >= 0.0 && a[2] <= size[0] && a[3] <= size[1] && a[0] < a[2] && a[1] < a[3], "{a:?}");
                    }
                }
            }
        }
        for size in [[f32::NAN, 792.0], [0.0, 0.0], [612.0, f32::INFINITY]] {
            assert_eq!(block_area(&page, size, None, 98.0, RefKind::TheoremLike), None);
        }
        assert_eq!(block_area(&page, LETTER, None, f32::NAN, RefKind::TheoremLike), None);
    }

    #[test]
    fn a_block_is_capped_at_its_share_of_the_page() {
        // One statement of 80 lines, 10 points apart.
        let glyphs = (0..80).flat_map(|k| line(72.0, 540.0, 20.0 + 9.0 * k as f32, 0)).collect();
        let area = block_area(&MathPage { glyphs, ..Default::default() }, LETTER, None, 18.0, RefKind::TheoremLike).expect("a block");
        assert!(area[3] - area[1] <= 792.0 * MAX_BLOCK_SHARE + 2.0 * PAD_Y + 0.01, "{area:?}");
    }

    #[test]
    fn two_columns_are_told_apart_by_their_gutter() {
        let mut glyphs = Vec::new();
        for k in 0..10 {
            glyphs.extend(line(72.0, 290.0, 100.0 + 11.0 * k as f32, 0));
            glyphs.extend(line(322.0, 540.0, 100.0 + 11.0 * k as f32, 0));
        }
        let math = MathPage { glyphs, ..Default::default() };
        let right = block_area(&math, LETTER, Some(330.0), 98.0, RefKind::Section).expect("a block");
        assert!(right[0] > 300.0, "{right:?}");
        let left = block_area(&math, LETTER, Some(80.0), 98.0, RefKind::Section).expect("a block");
        assert!(left[2] < 310.0, "{left:?}");
    }
}
