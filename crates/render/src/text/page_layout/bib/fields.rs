//! The fields of an entry (read in a later change).

use super::lines::BibLine;
use super::{Entry, Fields};

/// Lower-case words that belong to a family name (`van der Berg`, `de la Cruz`).
pub(super) const PARTICLES: [&str; 12] = ["van", "von", "der", "den", "de", "del", "della", "di", "da", "du", "la", "le"];

/// The fields of entry `e`: none read yet.
pub(super) fn parse(_lines: &[BibLine<'_>], _members: &[usize], _e: &Entry) -> Fields {
    Fields::default()
}
