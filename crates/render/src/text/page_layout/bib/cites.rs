//! Citations in the text.

use super::{Citation, Entry, Region};
use crate::text::page_layout::Diagnostic;
use crate::text::page_layout::refs::PageWords;

pub(super) fn find(_words: &[PageWords], _region: &Region, _entries: &[Entry], _diagnostics: &mut Vec<Diagnostic>) -> Vec<Citation> {
    Vec::new()
}
