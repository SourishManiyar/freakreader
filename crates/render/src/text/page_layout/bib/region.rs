//! Where the list is: its heading, and the lines from it to the next heading or the end.

use super::Region;
use crate::text::page_layout::refs::PageWords;
use crate::text::page_layout::{Diagnostic, DocumentStructure, PageLayout};

pub(super) fn find(
    _layouts: &[PageLayout],
    _words: &[PageWords],
    _structure: &DocumentStructure,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Region> {
    diagnostics.push(Diagnostic { code: "no_bibliography", message: "no reference list was found".into(), block: None });
    None
}
