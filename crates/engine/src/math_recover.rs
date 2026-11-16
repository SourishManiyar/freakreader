//! Recovering LaTeX from typeset mathematics: the engine's entry point for
//! `pdfcraft_render::text::math::layout`, which the `text_recover_latex` tool calls.
//!
//! Read-only and one region of one page at a time: the page's glyphs and rules are read without
//! rasterizing it ([`PageRenderer::glyph_details`]), then the layout reader turns the ones inside the
//! region into a formula. Whether the region could be read is the caller's answer, not an error of
//! this call: a region the reader does not handle (a matrix, a glyph of a font it does not know) comes
//! back as [`RecoverError`] with what was met.

use pdfcraft_render::PageRenderer;
use pdfcraft_render::text::math::recover::{RecoverError, RecoverOptions, Recovered, recover_latex};

use crate::export::ExportSource;

/// The formula drawn in `region` (`[x0, y0, x1, y1]` in points from the top-left of the displayed
/// `page`, 0-based) of the current document snapshot, which includes unsaved edits.
///
/// `Err` is a page that does not exist or cannot be read, or a panic caught on the way; the inner
/// result is the reader's: the formula with its confidence, or why the region is not answered.
pub fn recover_latex_in(
    source: ExportSource,
    page: usize,
    region: [f32; 4],
    options: RecoverOptions,
) -> Result<Result<Recovered, RecoverError>, String> {
    crate::guard(|| recover_inner(source, page, region, options))?
}

fn recover_inner(source: ExportSource, page: usize, region: [f32; 4], options: RecoverOptions) -> Result<Result<Recovered, RecoverError>, String> {
    if page >= source.pages {
        return Err(format!("page {} does not exist: the document has {} pages", page.saturating_add(1), source.pages));
    }
    let mut renderer = PageRenderer::new(source.bytes, source.config);
    let details = renderer.glyph_details(page).map_err(|e| format!("page {}: {e}", page.saturating_add(1)))?;
    Ok(recover_latex(&details, region, &options))
}
