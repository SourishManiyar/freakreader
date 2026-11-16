//! The plug-in point for recovering LaTeX from typeset mathematics.
//!
//! No recoverer exists yet. The instrument that will score one does: the math oracle
//! (`crates/render/tests/data/math/oracle`, `cargo xtask math-oracle report`) hands an
//! implementation each equation's page and region and checks the answer by typesetting it with TeX.

use super::MathPage;

/// Recovers the LaTeX of the mathematics drawn in a region of a page.
pub trait LatexRecoverer {
    /// LaTeX for what `page` draws in `region` (`[x0, y0, x1, y1]` in view space, as
    /// [`MathPage`]'s geometry), or `None` when nothing is recognised.
    ///
    /// The answer is the body of the formula as it appears inside its environment: no `$…$` or
    /// `\[…\]`, no `\begin{align}`; rows separated by `\\` and alignment points marked `&` when the
    /// region holds an aligned display. An equation number beside a display is not part of it.
    fn recover(&self, page: &MathPage, region: [f32; 4]) -> Option<String>;
}
