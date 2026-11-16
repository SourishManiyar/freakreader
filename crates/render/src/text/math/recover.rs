//! The plug-in point for recovering LaTeX from typeset mathematics, and the first recoverer.
//!
//! The instrument that scores a recoverer is the math oracle (`crates/render/tests/data/math/oracle`,
//! `cargo xtask math-oracle report`): it hands an implementation each equation's page and region and
//! checks the answer by typesetting it with TeX. [`LayoutRecoverer`] is the layout reader of
//! [`layout`](super::layout) behind that interface.

use super::MathPage;
pub use super::layout::{Confidence, Diagnostic, ErrorKind, RecoverError, RecoverOptions, Recovered, Severity, Style, recover_latex};

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

/// The layout reader as a [`LatexRecoverer`]: its best answer, whether or not it is sure of it.
#[derive(Clone, Copy, Debug, Default)]
pub struct LayoutRecoverer {
    pub options: RecoverOptions,
}

impl LatexRecoverer for LayoutRecoverer {
    fn recover(&self, page: &MathPage, region: [f32; 4]) -> Option<String> {
        recover_latex(page, region, &self.options).ok().map(|r| r.latex)
    }
}

/// The layout reader's answers that it is [`Confident`](Confidence::Confident) of, and nothing else:
/// what a copy-as-LaTeX command should offer without a warning.
#[derive(Clone, Copy, Debug, Default)]
pub struct ConfidentRecoverer {
    pub options: RecoverOptions,
}

impl LatexRecoverer for ConfidentRecoverer {
    fn recover(&self, page: &MathPage, region: [f32; 4]) -> Option<String> {
        recover_latex(page, region, &self.options).ok().filter(|r| r.confidence == Confidence::Confident).map(|r| r.latex)
    }
}
