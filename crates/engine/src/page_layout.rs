//! Page layout analysis of a document: the engine's entry point for
//! `pdfcraft_render::text::page_layout`, which the `page_layout` and `doc_sections` tools call.
//!
//! Read-only. Each page's glyphs and rules are read without rasterizing it
//! ([`PageRenderer::glyph_details`]) and analysed; running heads and page numbers are then confirmed
//! across pages by what repeats. A page that cannot be read gives an empty layout that says why,
//! never an error: only a page that does not exist, or a panic caught on the way, is one.

use pdfcraft_render::PageRenderer;
use pdfcraft_render::text::page_layout::{self, Diagnostic, DocumentStructure, PageContext, PageLayout};

use crate::export::ExportSource;

/// Most pages laid out in one call of [`page_layouts`].
pub const MAX_LAYOUT_PAGES: usize = 200;
/// Most pages read for [`document_structure`]; past it the rest are left out and it says so.
pub const MAX_STRUCTURE_PAGES: usize = 2_000;
/// Running heads are confirmed by repetition: [`page_layouts`] reads neighbouring pages until it has
/// at least this many (or the whole document).
pub const CONTEXT_PAGES: usize = 5;

fn layout_of(renderer: &mut PageRenderer, source: &ExportSource, page: usize) -> PageLayout {
    let (width, height) = source.sizes.get(page).copied().unwrap_or((0.0, 0.0));
    let ctx = PageContext { index: page, width, height };
    match renderer.glyph_details(page) {
        Ok(math) => page_layout::analyse_page(&math, ctx),
        Err(e) => PageLayout {
            page,
            size: [width, height],
            diagnostics: vec![Diagnostic { code: "unreadable", message: format!("the page's content could not be read: {e}"), block: None }],
            ..Default::default()
        },
    }
}

/// The pages to read for `pages`: those, then their nearest neighbours until there are at least
/// [`CONTEXT_PAGES`] (fewer when the document is shorter), in page order.
fn with_context(pages: &[usize], count: usize) -> Vec<usize> {
    let mut all: Vec<usize> = pages.iter().copied().filter(|&p| p < count).collect();
    all.sort_unstable();
    all.dedup();
    let want = CONTEXT_PAGES.min(count);
    let mut step = 1usize;
    while all.len() < want && step < count {
        for &p in pages {
            for q in [p.checked_sub(step), p.checked_add(step).filter(|&q| q < count)].into_iter().flatten() {
                if all.len() < want && !all.contains(&q) {
                    all.push(q);
                }
            }
        }
        step = step.saturating_add(1);
    }
    all.sort_unstable();
    all
}

/// The layouts of `pages` (0-based) of the snapshot, in the order given. Running heads are
/// confirmed against these pages and, when they are fewer than [`CONTEXT_PAGES`], their neighbours.
/// `Err` is a page that does not exist, more than [`MAX_LAYOUT_PAGES`] pages, or a panic caught.
pub fn page_layouts(source: ExportSource, pages: &[usize]) -> Result<Vec<PageLayout>, String> {
    if pages.len() > MAX_LAYOUT_PAGES {
        return Err(format!("at most {MAX_LAYOUT_PAGES} pages at a time; {} were asked for", pages.len()));
    }
    if let Some(p) = pages.iter().find(|&&p| p >= source.pages) {
        return Err(format!("page {} does not exist: the document has {} pages", p.saturating_add(1), source.pages));
    }
    crate::guard(|| {
        let read = with_context(pages, source.pages);
        let mut renderer = PageRenderer::new(source.bytes.clone(), source.config.clone());
        let mut layouts: Vec<PageLayout> = read.iter().map(|&p| layout_of(&mut renderer, &source, p)).collect();
        page_layout::analyse_document(&mut layouts);
        pages.iter().filter_map(|p| layouts.iter().find(|l| l.page == *p).cloned()).collect()
    })
}

/// The section tree of the snapshot, with every page's layout. Reads at most
/// [`MAX_STRUCTURE_PAGES`] pages; the second value says whether pages were left out.
pub fn document_structure(source: ExportSource) -> Result<(Vec<PageLayout>, DocumentStructure, bool), String> {
    crate::guard(|| {
        let n = source.pages.min(MAX_STRUCTURE_PAGES);
        let mut renderer = PageRenderer::new(source.bytes.clone(), source.config.clone());
        let mut layouts: Vec<PageLayout> = (0..n).map(|p| layout_of(&mut renderer, &source, p)).collect();
        let structure = page_layout::analyse_document(&mut layouts);
        (layouts, structure, source.pages > n)
    })
}

#[cfg(test)]
mod tests {
    use super::{CONTEXT_PAGES, with_context};

    #[test]
    fn neighbours_fill_the_context_without_leaving_the_document() {
        assert_eq!(with_context(&[0], 20), (0..CONTEXT_PAGES).collect::<Vec<_>>());
        assert_eq!(with_context(&[10], 20), vec![8, 9, 10, 11, 12]);
        assert_eq!(with_context(&[19], 20), vec![15, 16, 17, 18, 19]);
        assert_eq!(with_context(&[1], 2), vec![0, 1]);
        assert_eq!(with_context(&[3, 9, 12, 15, 18, 1], 20), vec![1, 3, 9, 12, 15, 18]);
        assert_eq!(with_context(&[], 0), Vec::<usize>::new());
        assert_eq!(with_context(&[7, 7], 8), vec![3, 4, 5, 6, 7]);
    }
}
