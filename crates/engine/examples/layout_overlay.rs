//! Draws what the page layout analysis found onto a rendered page, for a person to look at: each
//! block's box in a colour for its kind, labelled with its place in the reading order and its kind
//! (T title, AB abstract, H1… heading and level, P paragraph, EQ display, FL float, CA caption, FN
//! footnote, RH running header, RF running footer, PN page number, LI list item, O other), and the
//! column boxes in grey. Not a shipping UI: an inspection aid.
//!
//! Usage: `layout_overlay IN.pdf PAGE OUT.png [SCALE]` (PAGE from 1, SCALE device pixels per point,
//! default 2). Running heads are confirmed against up to the first 30 pages.

use std::error::Error;
use std::sync::Arc;

use pdfcraft_render::text::page_layout::{self, BlockKind, PageContext, PageLayout};
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RequestKind};

/// A 3 × 5 bitmap face for the labels, drawn for this example: each row's three low bits, left
/// to right.
fn glyph(c: char) -> Option<[u8; 5]> {
    Some(match c {
        '0' => [7, 5, 5, 5, 7],
        '1' => [2, 6, 2, 2, 7],
        '2' => [7, 1, 7, 4, 7],
        '3' => [7, 1, 7, 1, 7],
        '4' => [5, 5, 7, 1, 1],
        '5' => [7, 4, 7, 1, 7],
        '6' => [7, 4, 7, 5, 7],
        '7' => [7, 1, 1, 2, 2],
        '8' => [7, 5, 7, 5, 7],
        '9' => [7, 5, 7, 1, 7],
        'A' => [2, 5, 7, 5, 5],
        'B' => [6, 5, 6, 5, 6],
        'C' => [7, 4, 4, 4, 7],
        'E' => [7, 4, 6, 4, 7],
        'F' => [7, 4, 6, 4, 4],
        'H' => [5, 5, 7, 5, 5],
        'I' => [7, 2, 2, 2, 7],
        'L' => [4, 4, 4, 4, 7],
        'N' => [5, 7, 7, 7, 5],
        'O' => [7, 5, 5, 5, 7],
        'P' => [7, 5, 7, 4, 4],
        'Q' => [7, 5, 5, 7, 1],
        'R' => [6, 5, 6, 5, 5],
        'T' => [7, 2, 2, 2, 2],
        ' ' => [0; 5],
        _ => return None,
    })
}

fn code(b: &page_layout::Block) -> String {
    match b.kind {
        BlockKind::Title => "T".into(),
        BlockKind::Abstract => "AB".into(),
        BlockKind::Heading => format!("H{}", b.heading.as_ref().map_or(0, |h| h.level)),
        BlockKind::Paragraph => "P".into(),
        BlockKind::DisplayEquation => "EQ".into(),
        BlockKind::Float => "FL".into(),
        BlockKind::Caption => "CA".into(),
        BlockKind::Footnote => "FN".into(),
        BlockKind::RunningHeader => "RH".into(),
        BlockKind::RunningFooter => "RF".into(),
        BlockKind::PageNumber => "PN".into(),
        BlockKind::ListItem => "LI".into(),
        BlockKind::Other => "O".into(),
    }
}

fn colour(k: BlockKind) -> [u8; 3] {
    match k {
        BlockKind::Title | BlockKind::Heading => [200, 30, 30],
        BlockKind::Abstract => [150, 60, 160],
        BlockKind::Paragraph => [30, 90, 200],
        BlockKind::DisplayEquation => [0, 150, 70],
        BlockKind::Float | BlockKind::Caption => [220, 120, 0],
        BlockKind::Footnote => [120, 90, 40],
        BlockKind::RunningHeader | BlockKind::RunningFooter | BlockKind::PageNumber => [0, 150, 160],
        BlockKind::ListItem => [180, 0, 120],
        BlockKind::Other => [90, 90, 90],
    }
}

struct Canvas {
    w: usize,
    h: usize,
    rgba: Vec<u8>,
}

impl Canvas {
    fn dot(&mut self, x: i64, y: i64, c: [u8; 3]) {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return;
        }
        let at = (y as usize * self.w + x as usize) * 4;
        if let Some(px) = self.rgba.get_mut(at..at + 4) {
            px.copy_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }

    fn rect(&mut self, r: [f32; 4], scale: f32, c: [u8; 3], width: i64) {
        let [x0, y0, x1, y1] = r.map(|v| (v * scale).round() as i64);
        for t in 0..width {
            for x in x0..=x1 {
                self.dot(x, y0 - t, c);
                self.dot(x, y1 + t, c);
            }
            for y in y0..=y1 {
                self.dot(x0 - t, y, c);
                self.dot(x1 + t, y, c);
            }
        }
    }

    fn label(&mut self, text: &str, x: i64, y: i64, c: [u8; 3]) {
        let px = 2;
        let width = text.chars().count() as i64 * 4 * px + px;
        for yy in y..y + 7 * px {
            for xx in x..x + width {
                self.dot(xx, yy, [255, 255, 255]);
            }
        }
        for (k, ch) in text.chars().enumerate() {
            let Some(rows) = glyph(ch) else { continue };
            for (row, bits) in rows.iter().enumerate() {
                for col in 0..3 {
                    if bits & (4 >> col) != 0 {
                        for dy in 0..px {
                            for dx in 0..px {
                                self.dot(x + px + (k as i64 * 4 + col) * px + dx, y + px + row as i64 * px + dy, c);
                            }
                        }
                    }
                }
            }
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, page, out, rest @ ..] = &args[..] else {
        return Err("usage: layout_overlay IN.pdf PAGE OUT.png [SCALE]".into());
    };
    let page: usize = page.parse::<usize>()?.checked_sub(1).ok_or("pages count from 1")?;
    let scale: f32 = rest.first().map_or(Ok(2.0), |s| s.parse())?;
    let bytes = Arc::new(std::fs::read(input)?);
    let info = pdfcraft_render::inspect(bytes.clone(), None)?;
    let count = info.pages.len().min(30).max(page + 1);
    if page >= info.pages.len() {
        return Err(format!("the document has {} pages", info.pages.len()).into());
    }
    let mut renderer = PageRenderer::new(bytes, RenderConfig::default());
    let mut layouts: Vec<PageLayout> = (0..count)
        .map(|p| {
            let size = info.pages.get(p).map_or((0.0, 0.0), |i| (i.width, i.height));
            let ctx = PageContext { index: p, width: size.0, height: size.1 };
            renderer.glyph_details(p).map(|m| page_layout::analyse_page(&m, ctx)).unwrap_or_default()
        })
        .collect();
    page_layout::analyse_document(&mut layouts);
    let layout = layouts.get(page).ok_or("no layout")?;
    let shot = renderer.render(RenderRequest { page, kind: RequestKind::Pixels, tile: None, scale, tag: 0, preview: false });
    if let Some(e) = shot.error {
        return Err(e.into());
    }
    let mut canvas = Canvas { w: shot.width as usize, h: shot.height as usize, rgba: shot.rgba.as_ref().to_vec() };
    for c in &layout.columns {
        canvas.rect(c.rect, scale, [190, 190, 190], 1);
    }
    for (place, &i) in layout.reading_order.iter().enumerate() {
        let Some(b) = layout.blocks.get(i) else { continue };
        let c = colour(b.kind);
        canvas.rect([b.rect[0] - 1.0, b.rect[1] - 1.0, b.rect[2] + 1.0, b.rect[3] + 1.0], scale, c, 2);
        let x = ((b.rect[0] - 1.0) * scale) as i64;
        let y = ((b.rect[1] - 1.0) * scale) as i64 - 14;
        canvas.label(&format!("{place} {}", code(b)), x, y.max(0), c);
    }
    let png = pdfcraft_engine::export::encode_png(shot.width, shot.height, &canvas.rgba)?;
    std::fs::write(out, png)?;
    let kinds: Vec<String> = layout.reading_order.iter().filter_map(|&i| layout.blocks.get(i)).map(code).collect();
    println!("page {}: {} blocks in reading order: {}", page + 1, kinds.len(), kinds.join(" "));
    for d in &layout.diagnostics {
        println!("  {}: {}", d.code, d.message);
    }
    Ok(())
}
