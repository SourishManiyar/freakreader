//! The entries of the list.

use super::{Entry, Region};
use crate::text::page_layout::Diagnostic;
use crate::text::page_layout::refs::PageWords;

pub(super) fn split(_words: &[PageWords], _region: &Region, _diagnostics: &mut Vec<Diagnostic>) -> Vec<Entry> {
    Vec::new()
}
