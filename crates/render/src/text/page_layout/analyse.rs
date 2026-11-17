//! The per-page analysis behind [`super::analyse_page`].
//!
//! 1. *Atoms*: visible, finite, left-to-right glyphs with their font's style (mathematics, bold,
//!    small caps, from the font name).
//! 2. *Frames*: rules that touch, or table rules of one width stacked, make a float's region; the
//!    glyphs inside are the float's.
//! 3. *Fragments*: glyphs in content order on one baseline with word-sized gaps; a smaller glyph on
//!    a shifted baseline beside them (a script, a footnote mark) stays in.
//! 4. *Gutters*: x ranges that almost no fragment crosses while both sides carry text: the
//!    whitespace alleys between columns. Fragments (and frames) that cross one span the columns and
//!    cut the page into zones; between them each zone has one column per alley plus one.
//! 5. *Lines*: fragments of one column on one baseline; small ones riding on a bigger line's band
//!    (inline fractions, limits) join it.
//! 6. *Blocks*: consecutive lines of a column, cut at a gap, an indented first line, a short last
//!    line, a change of size or weight, a list label, a footnote rule, or between text and display.
//! 7. *Kinds*, then *reading order*: zones top to bottom, columns left to right, blocks top to
//!    bottom (side by side ones left to right).

use super::{
    Block, BlockKind, Column, Diagnostic, HeadingInfo, Line, MAX_BLOCK_TEXT_BYTES, MAX_COLUMNS, MAX_FRAME_RULES, PageContext, PageLayout, TextStyle,
    cut,
};
use crate::text::math::MathPage;
use std::collections::BTreeMap;

/// A glyph the analysis uses.
#[derive(Clone, Copy, Debug)]
pub(super) struct Atom {
    pub(super) g: u32,
    pub(super) x0: f32,
    pub(super) x1: f32,
    pub(super) y0: f32,
    pub(super) y1: f32,
    pub(super) base: f32,
    pub(super) size: f32,
    pub(super) math: bool,
    pub(super) bold: bool,
    pub(super) caps: bool,
}

#[derive(Clone, Debug)]
struct Frag {
    atoms: Vec<usize>,
    rect: [f32; 4],
    base: f32,
    size: f32,
}

#[derive(Clone, Debug)]
struct RawLine {
    atoms: Vec<usize>,
    rect: [f32; 4],
    base: f32,
    size: f32,
}

#[derive(Clone, Debug)]
struct Region {
    rect: [f32; 4],
    rules: Vec<u32>,
    atoms: Vec<usize>,
}

/// A block before it is built: its lines (indices into the zone/column's line list) and where.
#[derive(Clone, Debug)]
struct Raw {
    zone: usize,
    col: usize,
    lines: Vec<RawLine>,
    rules: Vec<u32>,
    display: bool,
    footnote: bool,
    float: bool,
    label: bool,
}

fn union(a: &mut [f32; 4], b: &[f32; 4]) {
    a[0] = a[0].min(b[0]);
    a[1] = a[1].min(b[1]);
    a[2] = a[2].max(b[2]);
    a[3] = a[3].max(b[3]);
}

const EMPTY: [f32; 4] = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];

/// Font style from the font's name: (mathematics, bold, small caps).
fn font_style(name: Option<&str>) -> (bool, bool, bool) {
    let n = name.unwrap_or("").to_ascii_uppercase();
    const MATH: [&str; 16] =
        ["CMMI", "CMSY", "CMEX", "CMBSY", "MSAM", "MSBM", "EUFM", "EUFB", "EURM", "EURB", "EUSM", "EUSB", "EUEX", "RSFS", "WASY", "STMARY"];
    let math = MATH.iter().any(|m| n.starts_with(m)) || n.contains("MATH");
    let bold = (n.starts_with("CMB") && !n.starts_with("CMBSY")) || n.contains("BOLD") || n.contains("-BD") || n.contains("BX");
    let caps = n.starts_with("CMCSC") || n.contains("SMALLCAP") || n.ends_with("-SC");
    (math, bold, caps)
}

fn finite(v: &[f32]) -> bool {
    v.iter().all(|x| x.is_finite())
}

fn median(v: &mut [f32]) -> Option<f32> {
    v.sort_by(f32::total_cmp);
    v.get(v.len() / 2).copied()
}

/// Most float regions looked for on one page; each is matched against every glyph.
const MAX_REGIONS: usize = 64;

/// The page's zones: bands of material spanning the column gutters, and the stretches between them.
struct Zones {
    gutters: Vec<(f32, f32)>,
    /// `[y0, y1]` of each band, top to bottom.
    bands: Vec<[f32; 2]>,
}

impl Zones {
    fn new(gutters: Vec<(f32, f32)>, frags: &[Frag], regions: &[Region]) -> Zones {
        let mut z = Zones { gutters, bands: Vec::new() };
        let mut bands: Vec<[f32; 2]> =
            frags.iter().map(|f| f.rect).chain(regions.iter().map(|r| r.rect)).filter(|r| z.crosses(r)).map(|r| [r[1], r[3]]).collect();
        bands.sort_by(|a, b| a[0].total_cmp(&b[0]));
        for b in bands {
            match z.bands.last_mut() {
                Some(m) if b[0] <= m[1] + 2.0 => m[1] = m[1].max(b[1]),
                _ => z.bands.push(b),
            }
        }
        z
    }

    /// Whether `r` crosses a gutter.
    fn crosses(&self, r: &[f32; 4]) -> bool {
        self.gutters.iter().any(|g| r[0] < g.0 - 0.5 && r[2] > g.1 + 0.5)
    }

    /// `(zone, column)` of a box: zone `2k + 1` is band `k`, zone `2k` the stretch above it (`2n`
    /// below the last).
    fn place(&self, r: &[f32; 4]) -> (usize, usize) {
        let cy = (r[1] + r[3]) / 2.0;
        if let Some(k) = self.bands.iter().position(|b| cy >= b[0] - 1.0 && cy <= b[1] + 1.0) {
            return (2 * k + 1, 0);
        }
        let k = self.bands.iter().position(|b| cy < b[0]).unwrap_or(self.bands.len());
        let col = if self.crosses(r) { 0 } else { self.gutters.iter().filter(|g| (r[0] + r[2]) / 2.0 > g.0).count() };
        (2 * k, col)
    }

    /// Columns side by side in a zone.
    fn columns(&self, zone: usize) -> usize {
        if zone % 2 == 1 { 1 } else { self.gutters.len().saturating_add(1) }
    }
}

pub(super) fn run(page: &MathPage, ctx: PageContext) -> PageLayout {
    let mut diags: Vec<Diagnostic> = Vec::new();
    let atoms = collect_atoms(page, &mut diags);
    let (width, height) = page_size(&atoms, ctx);
    let mut layout = PageLayout { page: ctx.index, size: [width, height], ..Default::default() };
    if atoms.is_empty() {
        note(&mut diags, "no_text", "the page has no visible text to lay out (a scan or a picture?)".into());
        layout.diagnostics = diags;
        return layout;
    }
    let body = body_size(&atoms);
    layout.body_size = Some(body);

    let rules: Vec<(u32, [f32; 4], bool)> = page
        .rules
        .iter()
        .enumerate()
        .filter_map(|(i, r)| Some((u32::try_from(i).ok()?, r.rect, r.horizontal)).filter(|(_, rect, _)| finite(rect)))
        .collect();
    let many_rules = rules.len() > MAX_FRAME_RULES;
    let mut regions = if many_rules {
        note(
            &mut diags,
            "too_many_rules",
            format!("{} rules on the page: float frames and rule owners are not looked for past {MAX_FRAME_RULES}", rules.len()),
        );
        Vec::new()
    } else {
        frames(&rules, body)
    };
    if regions.len() > MAX_REGIONS {
        note(&mut diags, "too_many_rules", format!("{} framed regions: only the first {MAX_REGIONS} are taken for floats", regions.len()));
        regions.truncate(MAX_REGIONS);
    }
    let in_region = assign_regions(&mut regions, &atoms);
    let framed: std::collections::HashSet<u32> = regions.iter().flat_map(|r| r.rules.iter().copied()).collect();
    let free_rules: Vec<(u32, [f32; 4], bool)> =
        if many_rules { Vec::new() } else { rules.iter().filter(|(i, _, _)| !framed.contains(i)).copied().collect() };

    let frags = fragments(&atoms, &in_region);
    let (gutters, undecided) = gutters(&frags, &atoms, body);
    if let Some(at) = undecided {
        note(
            &mut diags,
            "columns_undecided",
            format!("could not decide the column count: a possible gutter near x = {at:.0} is crossed by too much text; read as one column"),
        );
    }
    let zones = Zones::new(gutters, &frags, &regions);
    let lead = leading(&frags, body);
    let cx = Ctx { atoms: &atoms, page, body, leading: lead, height };
    let (mut raws, col_rects) = column_blocks(&cx, &frags, &zones, &free_rules);
    for r in &regions {
        let (zone, col) = zones.place(&r.rect);
        raws.push(Raw {
            zone,
            col,
            lines: region_lines(r, &atoms),
            rules: r.rules.clone(),
            display: false,
            footnote: false,
            float: true,
            label: false,
        });
    }
    attach_rules(&mut raws, &free_rules, &atoms);
    let order = reading_order(&raws, &atoms);
    let (columns, zone_ids) = columns(&raws, &col_rects, &zones, &atoms);
    let zone_index = |z: usize| zone_ids.iter().position(|&x| x == z).unwrap_or(0);
    layout.columns = columns;
    let col_index = |raw: &Raw| layout.columns.iter().position(|c| c.zone == zone_index(raw.zone) && c.index == raw.col);
    let mut blocks: Vec<Block> = Vec::with_capacity(raws.len());
    let mut in_order: Vec<&Raw> = Vec::with_capacity(raws.len());
    for raw in order.iter().filter_map(|&o| raws.get(o)) {
        let spans = zones.columns(raw.zone) == 1 && !zones.gutters.is_empty();
        blocks.push(build_block(raw, page, &atoms, if spans { None } else { col_index(raw) }));
        in_order.push(raw);
    }
    let area = text_area(&frags).unwrap_or([0.0, 0.0, width, height]);
    classify(&mut blocks, &in_order, &cx, ctx.index, [width, height], area, &mut diags);
    for (k, b) in blocks.iter_mut().enumerate() {
        b.order = k;
    }
    layout.reading_order = (0..blocks.len()).collect();
    layout.blocks = blocks;
    layout.diagnostics = diags;
    layout
}

/// Each column's box, by `(zone, column)`.
type ColumnBoxes = BTreeMap<(usize, usize), [f32; 4]>;

/// What the later phases share.
struct Ctx<'a> {
    atoms: &'a [Atom],
    page: &'a MathPage,
    body: f32,
    leading: f32,
    height: f32,
}

fn note(diags: &mut Vec<Diagnostic>, code: &'static str, message: String) {
    diags.push(Diagnostic { code, message, block: None });
}

/// The glyphs the analysis uses, and diagnostics for those it leaves out.
fn collect_atoms(page: &MathPage, diags: &mut Vec<Diagnostic>) -> Vec<Atom> {
    if page.truncated {
        note(diags, "truncated", "the page's content was cut at the renderer's limits: what the cut part drew is missing".into());
    }
    if page.capped {
        note(diags, "capped", "the page draws more glyphs or rules than are read: the rest are left out".into());
    }
    let styles: Vec<(bool, bool, bool)> = page.fonts.iter().map(|f| font_style(f.name.as_deref())).collect();
    let (mut invisible, mut other_dir, mut bad) = (0usize, 0usize, 0usize);
    let mut atoms: Vec<Atom> = Vec::with_capacity(page.glyphs.len());
    for (i, g) in page.glyphs.iter().enumerate() {
        let Ok(gi) = u32::try_from(i) else { break };
        let [dx, dy] = g.direction;
        let [x0, ey0, x1, ey1] = g.rect;
        if g.invisible {
            invisible += 1;
        } else if !(dx.is_finite() && dy.is_finite() && finite(&g.rect) && finite(&g.baseline[0]) && g.size.is_finite() && g.size > 0.0)
            || x1 < x0
            || ey1 < ey0
        {
            bad += 1;
        } else if (dx - 1.0).abs() > 0.05 || dy.abs() > 0.05 {
            other_dir += 1;
        } else {
            let (y0, y1) = g.ink.filter(|r| finite(r) && r[2] >= r[0] && r[3] >= r[1]).map_or((ey0, ey1), |r| (r[1], r[3]));
            let (math, bold, caps) = styles.get(g.font as usize).copied().unwrap_or((false, false, false));
            atoms.push(Atom { g: gi, x0, x1, y0, y1, base: g.baseline[0][1], size: g.size, math: math || g.unmapped, bold, caps });
        }
    }
    if invisible > 0 {
        note(diags, "invisible_text", format!("{invisible} invisible glyphs (an OCR layer) left out"));
    }
    if other_dir > 0 {
        note(diags, "other_direction", format!("{other_dir} glyphs that do not run left to right left out"));
    }
    if bad > 0 {
        note(diags, "bad_glyphs", format!("{bad} glyphs with non-finite or empty geometry left out"));
    }
    atoms
}

/// Each region takes the glyphs whose centre it holds; returns each atom's region (`usize::MAX`:
/// none).
fn assign_regions(regions: &mut [Region], atoms: &[Atom]) -> Vec<usize> {
    let mut in_region = vec![usize::MAX; atoms.len()];
    for (ri, r) in regions.iter_mut().enumerate() {
        for (ai, a) in atoms.iter().enumerate() {
            let (cx, cy) = ((a.x0 + a.x1) / 2.0, (a.y0 + a.y1) / 2.0);
            let inside = cx >= r.rect[0] - 1.0 && cx <= r.rect[2] + 1.0 && cy >= r.rect[1] - 1.0 && cy <= r.rect[3] + 1.0;
            if let Some(slot) = in_region.get_mut(ai).filter(|s| **s == usize::MAX && inside) {
                *slot = ri;
                r.atoms.push(ai);
            }
        }
    }
    in_region
}

/// The blocks of every zone's columns, and each column's box.
fn column_blocks(cx: &Ctx<'_>, frags: &[Frag], zones: &Zones, free_rules: &[(u32, [f32; 4], bool)]) -> (Vec<Raw>, ColumnBoxes) {
    let mut groups: BTreeMap<(usize, usize), Vec<usize>> = BTreeMap::new();
    for (fi, f) in frags.iter().enumerate() {
        groups.entry(zones.place(&f.rect)).or_default().push(fi);
    }
    let mut raws: Vec<Raw> = Vec::new();
    let mut col_rects: BTreeMap<(usize, usize), [f32; 4]> = BTreeMap::new();
    for (&(zone, col), members) in &groups {
        let lines = lines(frags, members, cx.atoms, cx.body);
        let mut rect = EMPTY;
        for l in &lines {
            union(&mut rect, &l.rect);
        }
        col_rects.insert((zone, col), rect);
        // The column's edges from its own long lines: a short last page leaves the right column
        // empty, and its text must not look short against the whole page.
        let mut rights: Vec<f32> = lines.iter().filter(|l| l.atoms.len() >= 8).map(|l| l.rect[2]).collect();
        rights.sort_by(f32::total_cmp);
        let right = rights.get(rights.len() * 4 / 5).copied().unwrap_or(rect[2]);
        let colrect = [rect[0], rect[1], right.max(rect[0] + 1.0), rect[3]];
        // A footnote rule: short, at the column's left, in the lower part, with text under it.
        let footrule = free_rules
            .iter()
            .filter(|(_, r, h)| {
                let len = r[2] - r[0];
                *h && len >= 15.0
                    && len <= 0.6 * (colrect[2] - colrect[0])
                    && (r[0] - colrect[0]).abs() <= 2.0
                    && r[1] > cx.height * 0.4
                    && r[1] >= rect[1]
            })
            .map(|(_, r, _)| r[1])
            .fold(None, |acc: Option<f32>, y| Some(acc.map_or(y, |a| a.max(y))))
            .filter(|y| lines.iter().any(|l| l.rect[1] > *y));
        raws.extend(blocks(zone, col, lines, colrect, cx, footrule));
    }
    (raws, col_rects)
}

/// Free rules join the block whose box holds them (a fraction bar, a radical's overline).
fn attach_rules(raws: &mut [Raw], free_rules: &[(u32, [f32; 4], bool)], atoms: &[Atom]) {
    let boxes: Vec<[f32; 4]> = raws.iter().map(|r| raw_rect(r, atoms)).collect();
    for (ri, rr, _) in free_rules {
        let (cx, cy) = ((rr[0] + rr[2]) / 2.0, (rr[1] + rr[3]) / 2.0);
        let at = boxes.iter().position(|b| cx >= b[0] - 2.0 && cx <= b[2] + 2.0 && cy >= b[1] - 2.0 && cy <= b[3] + 2.0);
        if let Some(raw) = at.and_then(|k| raws.get_mut(k)) {
            raw.rules.push(*ri);
        }
    }
}

/// Zone, column, top, then left to right along a row (first baselines within 2 pt).
fn reading_order(raws: &[Raw], atoms: &[Atom]) -> Vec<usize> {
    let keys: Vec<(usize, usize, f32, f32)> = raws
        .iter()
        .map(|r| {
            let b = raw_rect(r, atoms);
            (r.zone, r.col, r.lines.first().map_or(b[1], |l| l.base), b[0])
        })
        .collect();
    let key = |k: usize| keys.get(k).copied().unwrap_or((usize::MAX, usize::MAX, f32::MAX, f32::MAX));
    let mut order: Vec<usize> = (0..raws.len()).collect();
    order.sort_by(|&a, &b| {
        let (ka, kb) = (key(a), key(b));
        (ka.0, ka.1).cmp(&(kb.0, kb.1)).then(ka.2.total_cmp(&kb.2))
    });
    let mut i = 0;
    while i < order.len() {
        let mut j = i + 1;
        while let (Some(&a), Some(&b)) = (order.get(i), order.get(j)) {
            let (ka, kb) = (key(a), key(b));
            if (ka.0, ka.1) != (kb.0, kb.1) || (kb.2 - ka.2).abs() > 2.0 {
                break;
            }
            j += 1;
        }
        if let Some(row) = order.get_mut(i..j) {
            row.sort_by(|&a, &b| key(a).3.total_cmp(&key(b).3));
        }
        i = j;
    }
    order
}

/// The page's columns, zone by zone, and the zone numbering they use.
fn columns(raws: &[Raw], col_rects: &ColumnBoxes, zones: &Zones, atoms: &[Atom]) -> (Vec<Column>, Vec<usize>) {
    let mut zone_ids: Vec<usize> = raws.iter().map(|r| r.zone).collect();
    zone_ids.sort_unstable();
    zone_ids.dedup();
    let index = |z: usize| zone_ids.iter().position(|&x| x == z).unwrap_or(0);
    let mut rects: BTreeMap<(usize, usize), [f32; 4]> = BTreeMap::new();
    for r in raws {
        let rect = rects.entry((r.zone, r.col)).or_insert_with(|| col_rects.get(&(r.zone, r.col)).copied().unwrap_or(EMPTY));
        union(rect, &raw_rect(r, atoms));
    }
    let columns = rects
        .iter()
        .map(|(&(zone, col), &rect)| {
            let used = rects.keys().filter(|k| k.0 == zone).map(|k| k.1 + 1).max().unwrap_or(1);
            Column { rect: if finite(&rect) { rect } else { [0.0; 4] }, zone: index(zone), index: col, of: used.max(zones.columns(zone)) }
        })
        .collect();
    (columns, zone_ids)
}

fn page_size(atoms: &[Atom], ctx: PageContext) -> (f32, f32) {
    if ctx.width.is_finite() && ctx.height.is_finite() && ctx.width > 0.0 && ctx.height > 0.0 {
        return (ctx.width, ctx.height);
    }
    let w = atoms.iter().map(|a| a.x1).fold(0.0f32, f32::max);
    let h = atoms.iter().map(|a| a.y1).fold(0.0f32, f32::max);
    (w.max(1.0), h.max(1.0))
}

/// The most common glyph size, to a tenth of a point.
fn body_size(atoms: &[Atom]) -> f32 {
    let mut counts: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();
    for a in atoms {
        *counts.entry((a.size * 10.0).round() as i64).or_default() += 1;
    }
    let best = counts.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).map_or(10.0, |(k, _)| k as f32 / 10.0);
    if best.is_finite() && best > 0.0 { best } else { 10.0 }
}

/// Float frames: clusters of at least two rules that touch, or table rules of one width stacked.
fn frames(rules: &[(u32, [f32; 4], bool)], body: f32) -> Vec<Region> {
    let n = rules.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], mut i: usize) -> usize {
        let mut guard = 0;
        while let Some(&up) = p.get(i) {
            if up == i || guard > 64 {
                break;
            }
            let grand = p.get(up).copied().unwrap_or(up);
            if let Some(slot) = p.get_mut(i) {
                *slot = grand;
            }
            i = up;
            guard += 1;
        }
        i
    }
    let long = 4.0 * body;
    for i in 0..n {
        for j in i.saturating_add(1)..n {
            let (Some((_, a, ah)), Some((_, b, bh))) = (rules.get(i), rules.get(j)) else { continue };
            let touch = a[0] <= b[2] + 1.0 && b[0] <= a[2] + 1.0 && a[1] <= b[3] + 1.0 && b[1] <= a[3] + 1.0;
            let table =
                *ah && *bh && (a[0] - b[0]).abs() < 1.0 && (a[2] - b[2]).abs() < 1.0 && a[2] - a[0] >= long && (a[1] - b[1]).abs() < 12.0 * body;
            if touch || table {
                let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                if ri != rj
                    && let Some(slot) = parent.get_mut(ri)
                {
                    *slot = rj;
                }
            }
        }
    }
    let mut clusters: std::collections::BTreeMap<usize, Region> = std::collections::BTreeMap::new();
    for i in 0..n {
        let root = find(&mut parent, i);
        let Some((ri, rect, _)) = rules.get(i) else { continue };
        let c = clusters.entry(root).or_insert(Region { rect: EMPTY, rules: Vec::new(), atoms: Vec::new() });
        union(&mut c.rect, rect);
        c.rules.push(*ri);
    }
    clusters.into_values().filter(|c| c.rules.len() >= 2 && c.rect[2] - c.rect[0] >= long && c.rect[3] - c.rect[1] >= 0.8 * body).collect()
}

/// Glyphs in content order on one baseline with word-sized gaps.
fn fragments(atoms: &[Atom], in_region: &[usize]) -> Vec<Frag> {
    let mut out: Vec<Frag> = Vec::new();
    let mut cur: Option<Frag> = None;
    let mut prev: Option<Atom> = None;
    for (i, a) in atoms.iter().enumerate() {
        if in_region.get(i).is_some_and(|&r| r != usize::MAX) {
            continue;
        }
        let continues = match (&cur, prev) {
            (Some(f), Some(p)) => {
                let s = f.size.max(a.size);
                let gap = a.x0 - p.x1;
                let same_base = (a.base - f.base).abs() <= 0.15 * f.size.min(a.size);
                // A smaller glyph raised or lowered beside the line: a script or a mark; or the line's
                // own size again after one.
                let script = (a.size <= 0.9 * f.size || p.size <= 0.9 * f.size || a.size <= 0.9 * p.size)
                    && (a.base - f.base).abs() <= 0.75 * f.size.max(a.size);
                (same_base || script) && gap <= 1.6 * s && gap >= -1.5 * s
            }
            _ => false,
        };
        if continues {
            if let Some(f) = cur.as_mut() {
                f.atoms.push(i);
                union(&mut f.rect, &[a.x0, a.y0, a.x1, a.y1]);
                if a.size > f.size * 1.05 {
                    f.size = a.size;
                    f.base = a.base;
                }
            }
        } else {
            if let Some(f) = cur.take() {
                out.push(f);
            }
            cur = Some(Frag { atoms: vec![i], rect: [a.x0, a.y0, a.x1, a.y1], base: a.base, size: a.size });
        }
        prev = Some(*a);
    }
    if let Some(f) = cur {
        out.push(f);
    }
    // A fragment that starts on a script (a footnote mark) has its line's baseline from its text.
    for f in &mut out {
        if let Some(main) = f.atoms.iter().filter_map(|&i| atoms.get(i)).max_by(|a, b| a.size.total_cmp(&b.size)) {
            f.size = main.size;
            let same: Vec<f32> = f.atoms.iter().filter_map(|&i| atoms.get(i)).filter(|a| (a.size - main.size).abs() < 0.05).map(|a| a.base).collect();
            let mut same = same;
            if let Some(m) = median(&mut same) {
                f.base = m;
            }
        }
    }
    out
}

fn text_area(frags: &[Frag]) -> Option<[f32; 4]> {
    let mut r = EMPTY;
    for f in frags {
        union(&mut r, &f.rect);
    }
    finite(&r).then_some(r)
}

/// Column gutters `(x0, x1)`, left to right, and the x of an alley that was too doubtful to use.
fn gutters(frags: &[Frag], atoms: &[Atom], body: f32) -> (Vec<(f32, f32)>, Option<f32>) {
    let Some(area) = text_area(frags) else { return (Vec::new(), None) };
    let span = area[2] - area[0];
    if span.is_nan() || span <= 4.0 * body || frags.len() < 6 {
        return (Vec::new(), None);
    }
    let n = ((span / 0.5).ceil() as usize).clamp(16, 4096);
    let w = span / n as f32;
    let bin = |x: f32| (((x - area[0]) / w).floor().max(0.0) as usize).min(n - 1);
    let mut diff = vec![0i64; n + 1];
    for f in frags {
        // Only text of some length counts: a lone symbol or number says little about columns.
        if f.atoms.len() < 2 && atoms.len() > 50 {
            continue;
        }
        let (a, b) = (bin(f.rect[0]), bin(f.rect[2]));
        if let Some(d) = diff.get_mut(a) {
            *d += 1;
        }
        if let Some(d) = diff.get_mut(b.saturating_add(1)) {
            *d -= 1;
        }
    }
    let mut cover = Vec::with_capacity(n);
    let mut acc = 0i64;
    for d in diff.iter().take(n) {
        acc += d;
        cover.push(acc.max(0) as f32);
    }
    let total: f32 = cover.iter().sum();
    let mut sorted = cover.clone();
    sorted.sort_by(f32::total_cmp);
    let p75 = sorted.get(sorted.len() * 3 / 4).copied().unwrap_or(0.0);
    if p75 <= 0.0 || total <= 0.0 {
        return (Vec::new(), None);
    }
    let min_width = (0.5 * body).max(3.0);
    // Runs of bins at or under `limit`, as (x0, x1, max cover / p75), that have text on both sides.
    let runs = |limit: f32| {
        let mut out: Vec<(f32, f32, f32)> = Vec::new();
        let mut k = 0;
        while k < n {
            if cover.get(k).copied().unwrap_or(0.0) > limit {
                k += 1;
                continue;
            }
            let start = k;
            while k < n && cover.get(k).copied().unwrap_or(f32::MAX) <= limit {
                k += 1;
            }
            let (x0, x1) = (area[0] + start as f32 * w, area[0] + k as f32 * w);
            let left: f32 = cover.get(..start).unwrap_or(&[]).iter().sum();
            let right: f32 = cover.get(k..).unwrap_or(&[]).iter().sum();
            if x1 - x0 >= min_width && left >= 0.15 * total && right >= 0.15 * total {
                let depth = cover.get(start..k).unwrap_or(&[]).iter().copied().fold(0.0f32, f32::max) / p75;
                out.push((x0, x1, depth));
            }
        }
        out
    };
    let mut found = runs(0.15 * p75);
    // An alley that is almost one, crossed by more than a title and a few captions.
    let doubtful = runs(0.35 * p75).into_iter().find(|d| !found.iter().any(|f| f.0 < d.1 && d.0 < f.1)).map(|d| (d.0 + d.1) / 2.0);
    found.sort_by(|a, b| a.2.total_cmp(&b.2));
    found.truncate(MAX_COLUMNS.saturating_sub(1));
    found.sort_by(|a, b| a.0.total_cmp(&b.0));
    let doubtful = if found.is_empty() { doubtful } else { None };
    (found.into_iter().map(|(a, b, _)| (a, b)).collect(), doubtful)
}

/// The usual distance between baselines of body-size lines, from fragments.
fn leading(frags: &[Frag], body: f32) -> f32 {
    let mut bases: Vec<(f32, f32)> =
        frags.iter().filter(|f| (f.size - body).abs() < 0.1 * body && f.atoms.len() >= 5).map(|f| (f.rect[0], f.base)).collect();
    bases.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut gaps: Vec<f32> =
        bases.windows(2).filter_map(|w| Some(w.get(1)?.1 - w.first()?.1)).filter(|g| *g > 0.9 * body && *g < 2.0 * body).collect();
    median(&mut gaps).unwrap_or(1.2 * body)
}

/// The lines of one zone's column.
fn lines(frags: &[Frag], members: &[usize], atoms: &[Atom], body: f32) -> Vec<RawLine> {
    let mut fs: Vec<&Frag> = members.iter().filter_map(|&i| frags.get(i)).collect();
    fs.sort_by(|a, b| a.base.total_cmp(&b.base).then(a.rect[0].total_cmp(&b.rect[0])));
    // Bands of one baseline, then split along it at wide gaps.
    let mut out: Vec<RawLine> = Vec::new();
    let mut i = 0;
    while i < fs.len() {
        let Some(first) = fs.get(i) else { break };
        let mut j = i + 1;
        while let Some(f) = fs.get(j) {
            if (f.base - first.base).abs() <= 0.3 * f.size.min(first.size) { j += 1 } else { break }
        }
        let mut band: Vec<&Frag> = fs.get(i..j).unwrap_or(&[]).to_vec();
        band.sort_by(|a, b| a.rect[0].total_cmp(&b.rect[0]));
        let mut cur: Option<RawLine> = None;
        for f in band {
            let joins = cur.as_ref().is_some_and(|l| f.rect[0] - l.rect[2] <= 3.0 * l.size.max(f.size));
            if joins {
                if let Some(l) = cur.as_mut() {
                    l.atoms.extend_from_slice(&f.atoms);
                    union(&mut l.rect, &f.rect);
                    if f.size > l.size {
                        l.size = f.size;
                        l.base = f.base;
                    }
                }
            } else {
                if let Some(l) = cur.take() {
                    out.push(l);
                }
                cur = Some(RawLine { atoms: f.atoms.clone(), rect: f.rect, base: f.base, size: f.size });
            }
        }
        if let Some(l) = cur {
            out.push(l);
        }
        i = j;
    }
    // Satellites: a small line riding on a bigger one's band joins it.
    out.sort_by(|a, b| a.base.total_cmp(&b.base));
    let mut host: Vec<Option<usize>> = vec![None; out.len()];
    let max_size = out.iter().map(|l| l.size).fold(body, f32::max);
    for s in 0..out.len() {
        let Some(sat) = out.get(s) else { continue };
        let small = sat.size <= 0.85 * body || sat.atoms.len() <= 3;
        if !small {
            continue;
        }
        let lo = out.partition_point(|l| l.base < sat.base - 0.75 * max_size);
        let mut best: Option<(usize, f32)> = None;
        for (h, l) in out.iter().enumerate().skip(lo).take(64) {
            if l.base > sat.base + 1.0 * max_size {
                break;
            }
            if h == s || l.atoms.len() <= sat.atoms.len() || l.size < sat.size || host.get(h).copied().flatten().is_some() {
                continue;
            }
            let reach = sat.base >= l.base - 0.9 * l.size && sat.base <= l.base + 0.65 * l.size;
            let beside = sat.rect[0] >= l.rect[0] - l.size && sat.rect[2] <= l.rect[2] + l.size;
            let small_enough = sat.size <= 0.85 * l.size || (sat.atoms.len() <= 3 && l.atoms.len() >= 6);
            if reach && beside && small_enough {
                let dist = (l.base - sat.base).abs();
                if best.is_none_or(|(_, d)| dist < d) {
                    best = Some((h, dist));
                }
            }
        }
        if let (Some((h, _)), Some(slot)) = (best, host.get_mut(s)) {
            *slot = Some(h);
        }
    }
    let mut keep: Vec<Option<RawLine>> = out.into_iter().map(Some).collect();
    for s in 0..keep.len() {
        let Some(h) = host.get(s).copied().flatten() else { continue };
        let Some(sat) = keep.get_mut(s).and_then(Option::take) else { continue };
        match keep.get_mut(h).and_then(Option::as_mut) {
            Some(l) => {
                l.atoms.extend(sat.atoms);
                union(&mut l.rect, &sat.rect);
            }
            None => {
                if let Some(slot) = keep.get_mut(s) {
                    *slot = Some(sat);
                }
            }
        }
    }
    let mut lines: Vec<RawLine> = keep.into_iter().flatten().collect();
    for l in &mut lines {
        l.atoms.sort_by(|&a, &b| atoms.get(a).map_or(0.0, |x| x.x0).total_cmp(&atoms.get(b).map_or(0.0, |x| x.x0)));
    }
    lines.sort_by(|a, b| a.base.total_cmp(&b.base).then(a.rect[0].total_cmp(&b.rect[0])));
    lines
}

fn region_lines(r: &Region, atoms: &[Atom]) -> Vec<RawLine> {
    if r.atoms.is_empty() {
        return Vec::new();
    }
    let in_region = vec![usize::MAX; atoms.len()];
    let sub: Vec<Atom> = r.atoms.iter().filter_map(|&i| atoms.get(i).copied()).collect();
    let frags = fragments(&sub, &in_region[..sub.len().min(in_region.len())]);
    let members: Vec<usize> = (0..frags.len()).collect();
    let body = sub.first().map_or(10.0, |a| a.size);
    let mut ls = lines(&frags, &members, &sub, body);
    // Back to the page's atom indices.
    for l in &mut ls {
        l.atoms = l.atoms.iter().filter_map(|&k| r.atoms.get(k).copied()).collect();
    }
    ls
}

fn raw_rect(raw: &Raw, _atoms: &[Atom]) -> [f32; 4] {
    let mut r = EMPTY;
    for l in &raw.lines {
        union(&mut r, &l.rect);
    }
    if finite(&r) { r } else { [0.0; 4] }
}

/// Per line: the share of mathematics glyphs, bold and small-caps letters.
fn shares(l: &RawLine, atoms: &[Atom]) -> (f32, f32, f32) {
    let n = l.atoms.len().max(1) as f32;
    let (mut m, mut b, mut c) = (0.0, 0.0, 0.0);
    for a in l.atoms.iter().filter_map(|&i| atoms.get(i)) {
        m += f32::from(u8::from(a.math));
        b += f32::from(u8::from(a.bold));
        c += f32::from(u8::from(a.caps));
    }
    (m / n, b / n, c / n)
}

fn line_text(l: &RawLine, atoms: &[Atom], page: &MathPage) -> String {
    let mut s = String::new();
    let mut prev: Option<&Atom> = None;
    for a in l.atoms.iter().filter_map(|&i| atoms.get(i)) {
        let Some(t) = page.glyphs.get(a.g as usize).and_then(|g| g.text.as_deref()) else { continue };
        if let Some(p) = prev
            && a.x0 - p.x1 > 0.18 * a.size.max(p.size)
            && !s.ends_with(' ')
        {
            s.push(' ');
        }
        s.push_str(t);
        prev = Some(a);
        if s.len() > MAX_BLOCK_TEXT_BYTES {
            break;
        }
    }
    s
}

/// `(3.2)`, `(12)`, `(A.1)`, `(2a)`.
pub(super) fn is_tag(t: &str) -> bool {
    let t = t.trim();
    let Some(inner) = t.strip_prefix('(').and_then(|r| r.strip_suffix(')')) else { return false };
    let inner = inner.trim();
    !inner.is_empty()
        && inner.len() <= 12
        && inner.chars().any(|c| c.is_ascii_digit())
        && inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '\'' || c == '*')
}

/// A list label opening a line: a bullet, `1.`, `(a)`, `iii)`.
pub(super) fn starts_with_label(t: &str) -> bool {
    let t = t.trim_start();
    if t.starts_with(['•', '∙', '◦', '‣', '–', '−']) {
        return true;
    }
    let head: String = t.chars().take_while(|c| !c.is_whitespace()).collect();
    let core = head.trim_start_matches('(');
    let Some(body) = core.strip_suffix('.').or_else(|| core.strip_suffix(')')) else { return false };
    !body.is_empty()
        && body.len() <= 4
        && (body.chars().all(|c| c.is_ascii_digit())
            || (body.len() == 1 && body.chars().all(|c| c.is_ascii_lowercase()))
            || body.chars().all(|c| "ivx".contains(c)))
        && t.len() > head.len()
}

/// What a line says about where blocks begin.
struct LineFeat {
    left: f32,
    right: f32,
    display: bool,
    heading_style: bool,
    label: bool,
    raised_start: bool,
    below_rule: bool,
}

/// The atoms of `l` from `range`, as a line of their own.
fn part(l: &RawLine, range: std::ops::Range<usize>) -> RawLine {
    RawLine { atoms: l.atoms.get(range).unwrap_or(&[]).to_vec(), rect: l.rect, base: l.base, size: l.size }
}

fn line_features(l: &RawLine, colrect: [f32; 4], cx: &Ctx<'_>, footrule: Option<f32>) -> LineFeat {
    let atoms = cx.atoms;
    let (cl, cr) = (colrect[0], colrect[2]);
    let cw = (cr - cl).max(1.0);
    let s = l.size;
    let left = l.rect[0] - cl;
    let right = cr - l.rect[2];
    let centered = left > 1.5 * s && ((l.rect[0] + l.rect[2]) / 2.0 - (cl + cr) / 2.0).abs() < (0.06 * cw).max(3.0);
    // An equation number: the line's last piece after a wide gap, at the right margin, or its
    // first one at the left (amsart's `leqno`).
    let line_atoms: Vec<&Atom> = l.atoms.iter().filter_map(|&i| atoms.get(i)).collect();
    let wide = |w: &[&Atom]| matches!(w, [a, b] if b.x0 - a.x1 > 2.0 * s);
    let n = l.atoms.len();
    let tail_start = line_atoms.windows(2).rposition(wide).map_or(0, |k| k + 1);
    let head_end = line_atoms.windows(2).position(wide).map_or(n, |k| k + 1);
    let right_tag = right < 1.5 * s && is_tag(&line_text(&part(l, tail_start..n), atoms, cx.page));
    let left_tag = !right_tag && left < 1.5 * s && is_tag(&line_text(&part(l, 0..head_end), atoms, cx.page));
    let rest = if right_tag {
        part(l, 0..tail_start)
    } else if left_tag {
        part(l, head_end..n)
    } else {
        part(l, 0..n)
    };
    let tagged = right_tag || left_tag;
    let tag_only = tagged && rest.atoms.is_empty();
    let (m, b, c) = shares(if rest.atoms.is_empty() { l } else { &rest }, atoms);
    let display = tag_only
        || (m >= 0.3 && (centered || left >= 2.0 * s) && (right >= 1.0 * s || tagged))
        || (m >= 0.6 && (left >= 0.8 * s || right >= 0.8 * s || tagged));
    let first = l.atoms.first().and_then(|&i| atoms.get(i));
    LineFeat {
        left,
        right,
        display,
        heading_style: (b >= 0.5 || c >= 0.5 || s >= 1.12 * cx.body) && m < 0.5,
        label: !display && starts_with_label(&line_text(l, atoms, cx.page)),
        raised_start: first.is_some_and(|a| a.base < l.base - 0.15 * l.size && a.size < 0.9 * l.size),
        below_rule: footrule.is_some_and(|y| l.rect[1] > y),
    }
}

/// Whether line `l` (features `f`) starts a new block after `last`, whose last line is `pl` (`pf`).
fn starts_block(l: &RawLine, f: &LineFeat, last: &Raw, pl: &RawLine, pf: &LineFeat, cx: &Ctx<'_>) -> bool {
    let s = l.size.max(pl.size);
    let lead = cx.leading * s / cx.body;
    let side_by_side = (l.base - pl.base).abs() < 0.3 * s && (l.rect[0] > pl.rect[2] + 2.0 * s || pl.rect[0] > l.rect[2] + 2.0 * s);
    if f.display != pf.display || f.below_rule != pf.below_rule {
        return true;
    }
    if f.display {
        return l.rect[1] - pl.rect[3] > 1.2 * lead && !side_by_side;
    }
    side_by_side
        || l.base - pl.base > 1.5 * lead
        || l.size.max(pl.size) > 1.12 * l.size.min(pl.size)
        || f.heading_style != pf.heading_style
        || f.label
        // An indented first line; under a list label the text hangs indented.
        || (f.left - pf.left >= 0.5 * l.size && f.left - pf.left <= 6.0 * l.size && !last.label)
        // A short last line, then a line at the margin.
        || (pf.right > 1.5 * pl.size && f.left <= 0.5 * l.size && !last.footnote)
        || (f.below_rule && f.raised_start)
        || (last.label && f.left < pf.left - 0.5 * l.size)
}

fn blocks(zone: usize, col: usize, lines: Vec<RawLine>, colrect: [f32; 4], cx: &Ctx<'_>, footrule: Option<f32>) -> Vec<Raw> {
    let feats: Vec<LineFeat> = lines.iter().map(|l| line_features(l, colrect, cx, footrule)).collect();
    let mut out: Vec<Raw> = Vec::new();
    let mut prev: Option<&LineFeat> = None;
    for (l, f) in lines.into_iter().zip(&feats) {
        let new = match (prev, out.last()) {
            (Some(pf), Some(last)) => last.lines.last().is_none_or(|pl| starts_block(&l, f, last, pl, pf, cx)),
            _ => true,
        };
        if new {
            out.push(Raw {
                zone,
                col,
                lines: Vec::new(),
                rules: Vec::new(),
                display: f.display,
                footnote: f.below_rule,
                float: false,
                label: f.label,
            });
        }
        if let Some(last) = out.last_mut() {
            last.lines.push(l);
        }
        prev = Some(f);
    }
    merge_displays(out, cx.atoms, cx.body, cx.leading)
}

/// Pieces of a display that came apart (a limit under an integral, an alignment row read as text, a
/// number on a line of its own) go back into it: a small or mostly mathematical block within a line
/// of a display, and anything short between two displays that close.
fn merge_displays(blocks: Vec<Raw>, atoms: &[Atom], body: f32, leading: f32) -> Vec<Raw> {
    let mathy = |r: &Raw| {
        if r.footnote || r.label || r.float || r.lines.len() > 3 {
            return false;
        }
        let n: usize = r.lines.iter().map(|l| l.atoms.len()).sum();
        let m: usize = r.lines.iter().flat_map(|l| l.atoms.iter()).filter(|&&i| atoms.get(i).is_some_and(|a| a.math)).count();
        let small = r.lines.iter().all(|l| l.atoms.len() <= 6 || l.size <= 0.85 * body);
        small || m * 5 >= n * 2
    };
    let near = |a: &Raw, b: &Raw| {
        let (ra, rb) = (raw_rect(a, atoms), raw_rect(b, atoms));
        let gap = rb[1] - ra[3];
        gap <= 0.9 * leading && rb[0] < ra[2] && ra[0] < rb[2]
    };
    let mut out: Vec<Raw> = Vec::with_capacity(blocks.len());
    for b in blocks {
        let Some(last) = out.last_mut() else {
            out.push(b);
            continue;
        };
        if last.display && !b.display && mathy(&b) && near(last, &b) {
            last.lines.extend(b.lines);
            continue;
        }
        if b.display && near(last, &b) && (last.display || mathy(last)) {
            last.lines.extend(b.lines);
            last.display = true;
            // A short piece between two displays that are one after all.
            let len = out.len();
            if len >= 2
                && let (Some(prev), Some(cur)) = (out.get(len - 2), out.get(len - 1))
                && prev.display
                && near(prev, cur)
                && let Some(cur) = out.pop()
                && let Some(prev) = out.last_mut()
            {
                prev.lines.extend(cur.lines);
            }
            continue;
        }
        out.push(b);
    }
    for r in &mut out {
        r.lines.sort_by(|a, b| a.base.total_cmp(&b.base).then(a.rect[0].total_cmp(&b.rect[0])));
    }
    out
}

fn build_block(raw: &Raw, page: &MathPage, atoms: &[Atom], column: Option<usize>) -> Block {
    let mut rect = EMPTY;
    let mut lines = Vec::with_capacity(raw.lines.len());
    let mut text = String::new();
    let mut sizes: Vec<f32> = Vec::new();
    let (mut bold, mut caps, mut letters) = (0usize, 0usize, 0usize);
    for l in &raw.lines {
        union(&mut rect, &l.rect);
        lines.push(Line { glyphs: l.atoms.iter().filter_map(|&i| atoms.get(i)).map(|a| a.g).collect(), baseline: l.base, rect: l.rect });
        let t = line_text(l, atoms, page);
        if !text.is_empty() && !t.is_empty() {
            text.push(' ');
        }
        text.push_str(&t);
        for a in l.atoms.iter().filter_map(|&i| atoms.get(i)) {
            sizes.push(a.size);
            if !a.math {
                letters += 1;
                bold += usize::from(a.bold);
                caps += usize::from(a.caps);
            }
        }
    }
    for &r in &raw.rules {
        if let Some(rule) = page.rules.get(r as usize) {
            union(&mut rect, &rule.rect);
        }
    }
    cut(&mut text, MAX_BLOCK_TEXT_BYTES);
    let size = {
        let mut counts: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();
        for s in &sizes {
            *counts.entry((s * 10.0).round() as i64).or_default() += 1;
        }
        counts.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).map_or(0.0, |(k, _)| k as f32 / 10.0)
    };
    let letters = letters.max(1);
    Block {
        kind: if raw.float {
            BlockKind::Float
        } else if raw.display {
            BlockKind::DisplayEquation
        } else if raw.footnote {
            BlockKind::Footnote
        } else {
            BlockKind::Paragraph
        },
        rect: if finite(&rect) { rect } else { [0.0; 4] },
        lines,
        rules: raw.rules.clone(),
        text,
        order: 0,
        column,
        confidence: 0.8,
        style: TextStyle { size, bold: bold * 2 >= letters, small_caps: caps * 2 >= letters },
        heading: None,
        equation_tag: None,
        partner: None,
    }
}

/// The first word gap of a block's first line, in ems of the glyph before it.
fn first_gap(raw: &Raw, atoms: &[Atom]) -> f32 {
    let Some(l) = raw.lines.first() else { return 0.0 };
    let line: Vec<&Atom> = l.atoms.iter().filter_map(|&i| atoms.get(i)).collect();
    line.windows(2)
        .find_map(|w| match w {
            [a, b] if b.x0 - a.x1 > 0.15 * a.size => Some((b.x0 - a.x1) / a.size.max(0.1)),
            _ => None,
        })
        .unwrap_or(0.0)
}

/// The number a heading starts with, and the text after it: `3.2 Title`, `A.1. Title`, `B Title`.
pub(super) fn heading_number(text: &str) -> Option<(String, bool)> {
    let t = text.trim_start();
    let t = t.strip_prefix("Appendix ").unwrap_or(t);
    let head: String = t.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '.').collect();
    let rest = t.get(head.len()..).unwrap_or("");
    let number = head.trim_end_matches('.');
    if number.is_empty() || number.len() > 12 {
        return None;
    }
    let parts: Vec<&str> = number.split('.').collect();
    let first = parts.first()?;
    let lettered = first.len() == 1 && first.chars().all(|c| c.is_ascii_uppercase());
    let numeric = |p: &str| !p.is_empty() && p.len() <= 3 && p.chars().all(|c| c.is_ascii_digit());
    let ok = (numeric(first) || lettered) && parts.iter().skip(1).all(|p| numeric(p));
    // A lettered heading needs a separator: `A Title` but not `Abstract`.
    let separated = rest.starts_with([' ', '\u{a0}']) || head.ends_with('.') || rest.is_empty();
    (ok && separated && !(lettered && rest.is_empty())).then(|| (number.to_owned(), lettered))
}

fn classify(blocks: &mut [Block], raws: &[&Raw], cx: &Ctx<'_>, page_index: usize, size: [f32; 2], area: [f32; 4], diags: &mut Vec<Diagnostic>) {
    mark_running(blocks, cx, page_index, size[1]);
    for (i, b) in blocks.iter_mut().enumerate() {
        kind_of(b, raws.get(i).copied(), cx);
    }
    if page_index == 0 {
        front_matter(blocks, cx.body, size, area);
    }
    link_captions(blocks, cx.leading);
    for (i, b) in blocks.iter().enumerate() {
        notes(b, i, diags);
    }
}

fn is_numeral(t: &str) -> bool {
    let t = t.trim();
    !t.is_empty() && t.len() <= 6 && (t.chars().all(|c| c.is_ascii_digit()) || t.chars().all(|c| "ivxlcdmIVXLCDM".contains(c)))
}

/// Running elements: alone above everything else, or below, in the page's outer quarters.
fn mark_running(blocks: &mut [Block], cx: &Ctx<'_>, page_index: usize, height: f32) {
    let tops: Vec<[f32; 4]> = blocks.iter().map(|b| b.rect).collect();
    for (i, b) in blocks.iter_mut().enumerate() {
        // Page furniture is set no larger than the text, and a first page's bold top line is its title.
        if b.kind == BlockKind::Float || b.lines.len() > 2 || b.style.size > 1.1 * cx.body || (page_index == 0 && b.style.bold) {
            continue;
        }
        let r = b.rect;
        let numeric = is_numeral(&b.text);
        let others = || tops.iter().enumerate().filter(move |(k, _)| *k != i).map(|(_, o)| o);
        // Set apart from everything else by a line's height (a page number may sit closer, under
        // footnotes).
        let gap = if numeric { 0.25 } else { 1.0 } * cx.leading;
        let top = r[3] < 0.25 * height && others().all(|o| o[1] >= r[3] + gap || (o[3] < 0.25 * height && (o[1] - r[1]).abs() < 4.0));
        let bottom = r[1] > 0.75 * height && others().all(|o| o[3] <= r[1] - gap || (o[1] > 0.75 * height && (o[1] - r[1]).abs() < 4.0));
        if !(top || bottom) {
            continue;
        }
        b.kind = if numeric {
            BlockKind::PageNumber
        } else if top {
            BlockKind::RunningHeader
        } else {
            BlockKind::RunningFooter
        };
        b.confidence = 0.5;
    }
}

/// The number of a display: in parentheses at the start (on the left) or at the end.
fn equation_tag(text: &str) -> Option<String> {
    let first = text.trim_start();
    if let Some(end) = first.find(')')
        && let Some(cand) = first.get(..=end)
        && is_tag(cand)
    {
        return Some(cand.to_owned());
    }
    let t = text.trim_end();
    let cand = t.get(t.rfind('(')?..)?;
    is_tag(cand).then(|| cand.to_owned())
}

/// A heading's information, or `None` when the block does not look like one.
fn heading_info(b: &Block, raw: Option<&Raw>, cx: &Ctx<'_>) -> Option<HeadingInfo> {
    let text = b.text.trim_start();
    let few = b.lines.len() <= 3 && b.text.len() <= 160;
    let styled = b.style.bold || b.style.small_caps || b.style.size >= 1.12 * cx.body;
    if !(few && styled && raw.is_some_and(|r| !r.display)) {
        return None;
    }
    // A bare letter is a number only after `Appendix` or with the quad LaTeX sets after a number:
    // neither `A Title` nor the author `A. Writer` is appendix A.
    let number = heading_number(text)
        .filter(|(n, lettered)| !lettered || n.contains('.') || text.starts_with("Appendix") || raw.is_some_and(|r| first_gap(r, cx.atoms) >= 0.6));
    let (level, guessed) = match &number {
        Some((n, _)) => (u8::try_from(n.split('.').count()).unwrap_or(9).min(9), false),
        None if b.style.size >= 1.25 * cx.body => (1, true),
        None if b.style.size >= 1.08 * cx.body => (2, true),
        None => (3, true),
    };
    Some(HeadingInfo {
        level,
        number: number.as_ref().map(|n| n.0.clone()),
        level_guessed: guessed,
        appendix: number.as_ref().is_some_and(|n| n.1) || text.to_lowercase().starts_with("appendix"),
    })
}

/// A block's kind from its own look, and the confidence that goes with it.
fn kind_of(b: &mut Block, raw: Option<&Raw>, cx: &Ctx<'_>) {
    let text = b.text.trim_start();
    let lower = text.to_lowercase();
    match b.kind {
        BlockKind::DisplayEquation => {
            b.equation_tag = equation_tag(&b.text);
            b.confidence = if b.equation_tag.is_some() { 0.9 } else { 0.75 };
            return;
        }
        BlockKind::Float => {
            b.confidence = 0.8;
            return;
        }
        k if k.is_running() => return,
        _ => {}
    }
    let numbered_after = text.split_whitespace().nth(1).and_then(|w| w.chars().next()).is_some_and(|c| c.is_ascii_digit() || c.is_ascii_uppercase());
    if (lower.starts_with("figure") || lower.starts_with("fig.") || lower.starts_with("table")) && numbered_after {
        b.kind = BlockKind::Caption;
        b.confidence = 0.85;
    } else if b.kind == BlockKind::Footnote {
        b.confidence = 0.85;
    } else if let Some(h) = heading_info(b, raw, cx) {
        b.kind = BlockKind::Heading;
        b.confidence = if h.level_guessed { 0.6 } else { 0.85 };
        b.heading = Some(h);
    } else if starts_with_label(text) {
        b.kind = BlockKind::ListItem;
        b.confidence = 0.75;
    } else {
        b.confidence = 0.8;
    }
}

/// Front matter of the first page: the title, then authors and the date, then the abstract.
fn front_matter(blocks: &mut [Block], body: f32, size: [f32; 2], area: [f32; 4]) {
    let acx = (area[0] + area[2]) / 2.0;
    let aw = (area[2] - area[0]).max(1.0);
    let mut seen_title = false;
    let mut in_abstract = false;
    for b in blocks.iter_mut() {
        if b.kind.is_running() {
            continue;
        }
        let lower = b.text.trim_start().to_lowercase();
        let bcx = (b.rect[0] + b.rect[2]) / 2.0;
        let centred = (bcx - acx).abs() < 0.1 * aw && b.rect[2] - b.rect[0] < 0.9 * aw;
        let unnumbered = b.heading.as_ref().is_none_or(|h| h.number.is_none());
        if !seen_title {
            if centred && unnumbered && b.lines.len() <= 4 && (b.style.size >= 1.2 * body || b.style.bold) && b.rect[1] < 0.4 * size[1] {
                b.kind = BlockKind::Title;
                b.heading = None;
                b.confidence = 0.7;
                seen_title = true;
                continue;
            }
            break;
        }
        if lower.starts_with("abstract") {
            b.kind = BlockKind::Abstract;
            b.heading = None;
            b.confidence = 0.8;
            in_abstract = true;
            continue;
        }
        let indented_both = b.rect[0] - area[0] > 1.5 * body && area[2] - b.rect[2] > 1.5 * body;
        if in_abstract && (indented_both || b.style.size < 0.95 * body) && matches!(b.kind, BlockKind::Paragraph | BlockKind::ListItem) {
            b.kind = BlockKind::Abstract;
            b.confidence = 0.7;
            continue;
        }
        if in_abstract {
            break;
        }
        if centred && b.lines.len() <= 4 && matches!(b.kind, BlockKind::Paragraph | BlockKind::Heading | BlockKind::Other) && unnumbered {
            b.kind = BlockKind::Other;
            b.heading = None;
            b.confidence = 0.5;
            continue;
        }
        break;
    }
}

/// Captions and their floats find each other: the nearest float above or below that overlaps.
fn link_captions(blocks: &mut [Block], leading: f32) {
    let floats: Vec<usize> = blocks.iter().enumerate().filter(|(_, b)| b.kind == BlockKind::Float).map(|(i, _)| i).collect();
    let captions: Vec<usize> = blocks.iter().enumerate().filter(|(_, b)| b.kind == BlockKind::Caption).map(|(i, _)| i).collect();
    for &c in &captions {
        let Some(cr) = blocks.get(c).map(|b| b.rect) else { continue };
        let best = floats
            .iter()
            .filter_map(|&f| {
                let fr = blocks.get(f)?.rect;
                let dist = (cr[1] - fr[3]).max(fr[1] - cr[3]).max(0.0);
                let overlap = cr[2].min(fr[2]) - cr[0].max(fr[0]);
                (overlap > 0.0 && dist < 4.0 * leading).then_some((f, dist))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((f, _)) = best {
            if let Some(b) = blocks.get_mut(c) {
                b.partner = Some(f);
            }
            if let Some(b) = blocks.get_mut(f) {
                b.partner = Some(c);
            }
        }
    }
}

/// What the page says it is unsure of about block `i`.
fn notes(b: &Block, i: usize, diags: &mut Vec<Diagnostic>) {
    if b.kind == BlockKind::Float && b.partner.is_none() {
        diags.push(Diagnostic { code: "float_without_caption", message: "a framed region has no caption next to it".into(), block: Some(i) });
    }
    if let Some(h) = b.heading.as_ref().filter(|h| h.level_guessed) {
        diags.push(Diagnostic {
            code: "heading_level_guessed",
            message: format!("heading level {} guessed from its size and weight: it has no number", h.level),
            block: Some(i),
        });
    }
    if b.kind.is_running() {
        diags.push(Diagnostic {
            code: "running_unconfirmed",
            message: "taken for page furniture by its place alone; other pages confirm it".into(),
            block: Some(i),
        });
    }
}
