//! Glyph-level page data for reading mathematics: character codes, font and glyph names, exact ink
//! boxes, baselines and rules, kept beside (not instead of) the text layer. Read-only, one page at a
//! time, without rasterizing it. The record is `pdfcraft_render::text::math`'s; this is the engine's
//! entry point for it, which the `text_glyph_details` tool calls.

use std::collections::HashSet;

use pdfcraft_render::{PageRenderer, text::math::MathPage};

use crate::export::ExportSource;

/// The glyphs and rules of each of `pages` (0-based, in the order asked, each once) of the current
/// document snapshot, which includes unsaved edits. A page that cannot be read is an error naming it:
/// never an empty page standing in for one that failed, never a partial list.
pub fn glyph_details(source: ExportSource, pages: &[usize]) -> Result<Vec<(usize, MathPage)>, String> {
    crate::guard(|| glyph_details_inner(source, pages))?
}

fn glyph_details_inner(source: ExportSource, pages: &[usize]) -> Result<Vec<(usize, MathPage)>, String> {
    if let Some(page) = pages.iter().find(|&&page| page >= source.pages) {
        return Err(format!("page {} does not exist: the document has {} pages", page.saturating_add(1), source.pages));
    }
    let mut renderer = PageRenderer::new(source.bytes, source.config);
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(pages.len());
    for &page in pages {
        if seen.insert(page) {
            let details = renderer.glyph_details(page).map_err(|e| format!("page {}: {e}", page.saturating_add(1)))?;
            out.push((page, details));
        }
    }
    Ok(out)
}
