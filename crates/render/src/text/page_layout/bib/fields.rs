//! The fields of an entry.

use super::{Entry, Fields};
use crate::text::page_layout::refs::PageWords;

pub(super) fn parse(_words: &[PageWords], _entry: &Entry) -> Fields {
    Fields::default()
}
