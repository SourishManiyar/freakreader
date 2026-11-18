//! Citations in the text (read in a later change).

use super::{Citation, Entry, Region};
use crate::text::page_layout::Diagnostic;
use crate::text::page_layout::refs::{LineRef, PageWords};
use std::collections::HashSet;

/// The citations of the text outside the list: none read yet.
pub(super) fn find(_words: &[PageWords], _region: &Region, _list: &HashSet<(LineRef, usize)>, _entries: &[Entry], _diagnostics: &mut Vec<Diagnostic>) -> Vec<Citation> {
    Vec::new()
}
