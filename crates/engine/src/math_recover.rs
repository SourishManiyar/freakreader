//! Recovering LaTeX from typeset mathematics: the engine's entry point for
//! `pdfcraft_render::text::math::layout`, which the `text_recover_latex` tool and Edit ▸ Copy as
//! LaTeX call.
//!
//! Read-only and one region of one page at a time: the page's glyphs and rules are read without
//! rasterizing it ([`PageRenderer::glyph_details`]), then the layout reader turns the ones inside the
//! region into a formula. Whether the region could be read is the caller's answer, not an error of
//! this call: a region the reader does not handle (a matrix, a glyph of a font it does not know) comes
//! back as [`RecoverError`] with what was met.
//!
//! The two steps are also offered apart ([`page_glyphs`], [`recover_in_page`]) so that a caller
//! reading several regions of one page (the desktop app, one marquee after another) reads the page
//! once.

use pdfcraft_render::PageRenderer;
use pdfcraft_render::text::math::MathPage;
use pdfcraft_render::text::math::recover::{RecoverError, RecoverOptions, Recovered, recover_latex};

use crate::export::ExportSource;

/// What the recovered LaTeX body is wrapped in when it is copied: nothing (the default, so it can
/// be pasted inside whatever the user is writing), inline math `$…$`, or display math `\[…\]`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Wrapper {
    #[default]
    None,
    Inline,
    Display,
}

impl Wrapper {
    /// Every wrapper, in the order a choice offers them.
    pub const ALL: [Wrapper; 3] = [Wrapper::None, Wrapper::Inline, Wrapper::Display];

    /// The name the tools, the control channel and the saved preferences use.
    pub fn name(self) -> &'static str {
        match self {
            Wrapper::None => "none",
            Wrapper::Inline => "inline",
            Wrapper::Display => "display",
        }
    }

    /// The wrapper called `name` (see [`Wrapper::name`]).
    pub fn from_name(name: &str) -> Option<Wrapper> {
        Wrapper::ALL.into_iter().find(|w| w.name() == name)
    }

    /// `body` wrapped: `x^{2}`, `$x^{2}$` or `\[x^{2}\]`. The body is not changed.
    pub fn wrap(self, body: &str) -> String {
        match self {
            Wrapper::None => body.to_string(),
            Wrapper::Inline => format!("${body}$"),
            Wrapper::Display => format!("\\[{body}\\]"),
        }
    }
}

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
    let glyphs = page_glyphs(source, page)?;
    recover_in_page(&glyphs, region, options)
}

/// Every glyph and rule of `page` (0-based) of the snapshot, for [`recover_in_page`]. `Err` is a page
/// that does not exist or cannot be read, or a panic caught on the way.
pub fn page_glyphs(source: ExportSource, page: usize) -> Result<MathPage, String> {
    crate::guard(|| {
        if page >= source.pages {
            return Err(format!("page {} does not exist: the document has {} pages", page.saturating_add(1), source.pages));
        }
        let mut renderer = PageRenderer::new(source.bytes, source.config);
        renderer.glyph_details(page).map_err(|e| format!("page {}: {e}", page.saturating_add(1)))
    })?
}

/// The formula drawn in `region` of a page read with [`page_glyphs`]: the reader's answer, or `Err`
/// for a panic caught on the way.
pub fn recover_in_page(glyphs: &MathPage, region: [f32; 4], options: RecoverOptions) -> Result<Result<Recovered, RecoverError>, String> {
    crate::guard(|| recover_latex(glyphs, region, &options))
}

#[cfg(test)]
mod tests {
    use super::Wrapper;

    #[test]
    fn a_wrapper_wraps_the_body_unchanged_and_round_trips_by_name() {
        let body = r"\frac{a}{b} + \$";
        assert_eq!(Wrapper::None.wrap(body), body);
        assert_eq!(Wrapper::Inline.wrap(body), r"$\frac{a}{b} + \$$");
        assert_eq!(Wrapper::Display.wrap(body), r"\[\frac{a}{b} + \$\]");
        for w in Wrapper::ALL {
            assert_eq!(Wrapper::from_name(w.name()), Some(w));
        }
        assert_eq!(Wrapper::from_name("dollars"), None);
        assert_eq!(Wrapper::default(), Wrapper::None);
    }
}
