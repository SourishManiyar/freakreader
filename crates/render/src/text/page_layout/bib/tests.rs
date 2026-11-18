//! Unit tests of the bibliography analysis on synthetic pages.

use super::*;
use crate::text::page_layout::refs::{FlowLine, GLYPH_BOLD, GLYPH_ITALIC, GLYPH_MATH, PageWords, Word};
use crate::text::page_layout::{BlockKind, DocumentStructure};
use std::time::Instant;

const R: u8 = 0;
const I: u8 = GLYPH_ITALIC;
const B: u8 = GLYPH_BOLD;
const M: u8 = GLYPH_MATH;

/// Builds pages of lines of words; glyph ids count up across each page.
struct Doc {
    pages: Vec<Vec<FlowLine>>,
    next: u32,
    y: f32,
    block: usize,
}

impl Doc {
    fn new() -> Self {
        Doc { pages: vec![Vec::new()], next: 0, y: 100.0, block: 0 }
    }

    fn page(&mut self) -> &mut Self {
        self.pages.push(Vec::new());
        self.next = 0;
        self.y = 100.0;
        self
    }

    /// A line at `x` of `words` (`(text, flags)`), a block of its own when `first`.
    fn line(&mut self, kind: BlockKind, first: bool, x: f32, words: &[(&str, u8)]) -> &mut Self {
        if first {
            self.block += 1;
        }
        let mut cx = x;
        let mut out = Vec::new();
        for (text, flags) in words {
            let mut w = Word { rect: [cx, self.y - 8.0, cx, self.y + 2.0], ..Default::default() };
            for (i, _) in text.char_indices() {
                w.starts.push(i);
                w.glyphs.push(self.next);
                w.spans.push([cx, cx + 5.0]);
                w.flags.push(*flags);
                self.next += 1;
                cx += 5.0;
            }
            w.text = (*text).to_owned();
            w.rect[2] = cx;
            out.push(w);
            cx += 3.0;
        }
        let page = self.pages.len() - 1;
        let line = FlowLine {
            page,
            block: self.block,
            kind,
            first,
            rect: [x, self.y - 8.0, cx, self.y + 2.0],
            baseline: self.y,
            size: 10.0,
            column: [72.0, 540.0],
            words: out,
            mark: None,
        };
        if let Some(p) = self.pages.last_mut() {
            p.push(line);
        }
        self.y += 12.0;
        self
    }

    fn text(&mut self, text: &str) -> &mut Self {
        let words: Vec<(&str, u8)> = text.split(' ').map(|w| (w, R)).collect();
        self.line(BlockKind::Paragraph, true, 72.0, &words)
    }

    fn heading(&mut self, text: &str) -> &mut Self {
        let words: Vec<(&str, u8)> = text.split(' ').map(|w| (w, B)).collect();
        self.y += 8.0;
        self.line(BlockKind::Heading, true, 72.0, &words)
    }

    fn words(&self) -> Vec<PageWords> {
        self.pages.iter().enumerate().map(|(page, lines)| PageWords { page, lines: lines.clone(), leading: Some(12.0), diagnostics: Vec::new() }).collect()
    }

    fn analyse(&self) -> Bibliography {
        analyse_bibliography(&[], &self.words(), &DocumentStructure::default())
    }
}

fn roman(s: &str) -> Vec<(&str, u8)> {
    s.split(' ').map(|w| (w, R)).collect()
}

/// A numbered list in the amsplain manner (initials, italic title, bold volume) with a hanging
/// indent, and text citing it.
fn numeric_doc() -> Doc {
    let mut d = Doc::new();
    d.text("We use [1] and [1, 2] and [2, Thm. 3.1] here.");
    d.text("Ranges [1–2] and [1-2] count, and Theorem 3.1 in [2] too.");
    d.line(BlockKind::Paragraph, true, 72.0, &[("On", R), ("[1,", M), ("2]", M), ("nothing", R), ("is", R), ("cited.", R)]);
    d.text("The proof is in Section [2], and [7] is no entry.");
    d.heading("References");
    let mut e1 = roman("[1] A. Smith and B. Jones,");
    e1.extend([("On", I), ("widgets,", I)]);
    e1.extend(roman("J. Wid."));
    d.line(BlockKind::Paragraph, true, 72.0, &e1);
    let mut c1 = vec![("3", B)];
    c1.extend(roman("(2001), no. 2, 1–20, doi:10.1234/abc.5. MR 1234567."));
    d.line(BlockKind::Paragraph, false, 90.0, &c1);
    let mut e2 = roman("[2] C. van der Kay,");
    e2.extend([("Big", I), ("book,", I)]);
    e2.extend(roman("Pub, Lowtown, 2019, arXiv:2101.01234."));
    d.line(BlockKind::Paragraph, true, 72.0, &e2);
    d
}

#[test]
fn a_numbered_list_is_cut_into_its_entries() {
    let b = numeric_doc().analyse();
    let region = b.region.as_ref().expect("a list");
    assert_eq!(region.title.as_deref(), Some("References"));
    assert_eq!(b.entries.len(), 2, "{:#?}", b.entries);
    let e1 = &b.entries[0];
    assert_eq!((e1.label.as_deref(), e1.label_style, e1.printed_label().as_deref()), (Some("1"), LabelStyle::Bracket, Some("[1]")));
    assert_eq!(e1.lines.len(), 2, "the indented line goes on the entry");
    assert!(e1.text.starts_with("A. Smith and B. Jones"), "{}", e1.text);
}

#[test]
fn no_heading_and_no_numbered_end_is_no_bibliography() {
    let mut d = Doc::new();
    d.text("Lone brackets [1] and [2] cite nothing: there is no list.");
    d.text("[1] A line that starts with a bracket.");
    d.text("More text after it, so no list ends the paper.");
    let b = d.analyse();
    assert!(b.region.is_none() && b.entries.is_empty());
    assert!(b.diagnostics.iter().any(|x| x.code == "no_bibliography"));
    let mut d = Doc::new();
    d.text("We cite [2] and [3].");
    for (n, t) in [("[1]", "A. Smith, One, 2001."), ("[2]", "B. Kay, Two, 2002."), ("[3]", "C. Lo, Three, 2003.")] {
        let mut w = vec![(n, R)];
        w.extend(roman(t));
        d.line(BlockKind::Paragraph, true, 72.0, &w);
    }
    let b = d.analyse();
    assert!(b.region.as_ref().is_some_and(|r| r.heading.is_none() && r.confidence < 0.9));
    assert!(b.diagnostics.iter().any(|x| x.code == "no_heading"));
    assert_eq!(b.entries.len(), 3);
    let mut d = Doc::new();
    for n in ["[1]", "[3]", "[2]"] {
        d.line(BlockKind::Paragraph, true, 72.0, &[(n, R), ("Text.", R)]);
    }
    assert!(d.analyse().region.is_none());
}

#[test]
fn a_list_goes_on_across_pages_and_stops_at_the_next_heading() {
    let mut d = Doc::new();
    d.text("See [3].");
    d.heading("7. REFERENCES");
    d.line(BlockKind::Paragraph, true, 72.0, &roman("[1] A. Smith, A first entry that runs on"));
    d.line(BlockKind::Paragraph, false, 90.0, &roman("to a second line, 2001."));
    d.line(BlockKind::Paragraph, true, 72.0, &roman("[2] B. Kay, An entry broken at the page,"));
    d.page();
    d.line(BlockKind::Paragraph, true, 90.0, &roman("going on here, 2002, https://example.org/a/"));
    d.line(BlockKind::Paragraph, false, 90.0, &[("b/c.pdf.", crate::text::page_layout::refs::GLYPH_MONO)]);
    d.line(BlockKind::Paragraph, true, 72.0, &roman("[3] C. Lo, Three, 2003."));
    d.heading("Appendix A. Proofs");
    d.text("[1] is cited in the appendix too.");
    let b = d.analyse();
    assert_eq!(b.entries.len(), 3, "{:#?}", b.entries.iter().map(|e| &e.text).collect::<Vec<_>>());
    let e2 = &b.entries[1];
    assert_eq!(e2.pieces.iter().map(|p| p.page).collect::<Vec<_>>(), [0, 1], "one entry over the page break");
    assert!(e2.text.ends_with("https://example.org/a/b/c.pdf."), "a URL broken over lines stays whole: {}", e2.text);
}

#[test]
fn ten_thousand_entries_stay_linear() {
    let mut d = Doc::new();
    d.heading("References");
    for n in 1..=10_000 {
        let label = format!("[{n}]");
        let line = [(label.as_str(), R), ("A.", R), ("Smith,", R), ("Title,", I), ("2001.", R)];
        d.line(BlockKind::Paragraph, true, 72.0, &line);
    }
    let started = Instant::now();
    let b = d.analyse();
    assert_eq!(b.entries.len(), 10_000);
    assert!(started.elapsed().as_secs_f64() < 20.0);
}
