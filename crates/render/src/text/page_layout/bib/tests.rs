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
        self.pages
            .iter()
            .enumerate()
            .map(|(page, lines)| PageWords { page, lines: lines.clone(), leading: Some(12.0), diagnostics: Vec::new() })
            .collect()
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
fn a_numbered_list_gives_entries_fields_and_citations() {
    let b = numeric_doc().analyse();
    let region = b.region.as_ref().expect("a list");
    assert_eq!(region.title.as_deref(), Some("References"));
    assert_eq!(b.entries.len(), 2, "{:#?}", b.entries);
    let (e1, e2) = (&b.entries[0], &b.entries[1]);
    assert_eq!((e1.label.as_deref(), e1.label_style, e1.printed_label().as_deref()), (Some("1"), LabelStyle::Bracket, Some("[1]")));
    assert_eq!(e1.lines.len(), 2, "the indented line goes on the entry");
    let f = &e1.fields;
    assert_eq!(f.authors.iter().map(|a| a.family.as_str()).collect::<Vec<_>>(), ["Smith", "Jones"]);
    assert_eq!(f.authors[0].given, "A.");
    let v = |x: &Option<Field>| x.as_ref().map(|x| x.value.clone());
    assert_eq!(v(&f.title).as_deref(), Some("On widgets"));
    assert_eq!(v(&f.venue).as_deref(), Some("J. Wid"));
    assert_eq!(v(&f.volume).as_deref(), Some("3"));
    assert_eq!(v(&f.number).as_deref(), Some("2"));
    assert_eq!(v(&f.pages).as_deref(), Some("1–20"));
    assert_eq!(v(&f.year).as_deref(), Some("2001"));
    assert_eq!(v(&f.doi).as_deref(), Some("10.1234/abc.5"));
    assert_eq!(v(&f.mr).as_deref(), Some("1234567"));
    assert!(f.arxiv.is_none() && f.url.is_none(), "nothing invented");
    let g = &e2.fields;
    assert_eq!(g.authors.first().map(|a| a.family.as_str()), Some("van der Kay"));
    assert_eq!((v(&g.title).as_deref(), v(&g.venue).as_deref(), v(&g.year).as_deref()), (Some("Big book"), Some("Pub"), Some("2019")));
    assert_eq!(v(&g.arxiv).as_deref(), Some("2101.01234"));
    // Citations: [1]; [1, 2]; [2, Thm. 3.1]; [1–2]; [1-2]; [2] after "in"; not the math, not
    // "Section [2]", and [7] is no entry of the list.
    let texts: Vec<(&str, Option<usize>)> = b.citations.iter().map(|c| (c.text.as_str(), c.target)).collect();
    assert_eq!(
        texts,
        [
            ("1", Some(0)),
            ("1", Some(0)),
            ("2", Some(1)),
            ("2", Some(1)),
            ("1", Some(0)),
            ("2", Some(1)),
            ("1", Some(0)),
            ("2", Some(1)),
            ("2", Some(1))
        ],
        "{:#?}",
        b.citations
    );
    let loc = &b.citations[3];
    assert_eq!(loc.locator.as_deref(), Some("Thm. 3.1"));
    assert_eq!(loc.pieces.iter().map(|p| p.glyphs.len()).sum::<usize>(), 1, "the label alone is clickable");
    assert!(b.citations.iter().all(|c| c.confidence >= 0.7 && c.unresolved.is_none()));
    // A group shares its number; two brackets do not.
    assert_eq!(b.citations[1].group, b.citations[2].group);
    assert_ne!(b.citations[0].group, b.citations[1].group);
}

#[test]
fn an_author_year_list_resolves_by_name_and_year() {
    let mut d = Doc::new();
    d.text("As Smith and Jones (2020a) show, (Smith and Jones, 2020b; Kay et al., 2019) agree.");
    d.text("In 2020 nothing is cited, nor by Smith and Jones (2020), which two entries share.");
    d.text("Okafor (2018) has no entry, and (Okafor, 2017a,b) cites two.");
    d.heading("Bibliography");
    let entry = |d: &mut Doc, head: &str, title: &str, rest: &str| {
        let mut w = roman(head);
        w.extend(roman(title));
        let mut tail: Vec<(&str, u8)> = rest.split(' ').map(|x| (x, I)).collect();
        if let Some(last) = tail.last_mut() {
            last.1 = R;
        }
        w.extend(tail);
        d.line(BlockKind::Paragraph, true, 72.0, &w);
        d.line(BlockKind::Paragraph, false, 84.0, &roman("12:1–20."));
    };
    entry(&mut d, "Kay, C., Lo, D., and Ma, E. (2019).", "Big things.", "J. Wid., 3");
    entry(&mut d, "Okafor, F. (2017a).", "One.", "J. Wid., 3");
    entry(&mut d, "Okafor, F. (2017b).", "Two.", "J. Wid., 3");
    entry(&mut d, "Smith, A. and Jones, B. (2020a).", "On widgets.", "J. Wid., 3");
    entry(&mut d, "Smith, A. and Jones, B. (2020b).", "On gadgets.", "J. Wid., 3");
    let b = d.analyse();
    assert_eq!(b.entries.len(), 5, "{:#?}", b.entries.iter().map(|e| &e.text).collect::<Vec<_>>());
    assert!(b.entries.iter().all(|e| e.label.is_none() && e.label_style == LabelStyle::None));
    let first = &b.entries[0].fields;
    assert_eq!(first.authors.iter().map(|a| a.family.as_str()).collect::<Vec<_>>(), ["Kay", "Lo", "Ma"]);
    assert_eq!(first.year.as_ref().map(|y| y.value.as_str()), Some("2019"));
    assert_eq!(b.entries[3].fields.year.as_ref().map(|y| y.value.as_str()), Some("2020a"));
    let got: Vec<(&str, Option<usize>)> = b.citations.iter().map(|c| (c.text.as_str(), c.target)).collect();
    assert_eq!(
        got,
        [
            ("Smith and Jones 2020a", Some(3)),
            ("Smith and Jones 2020b", Some(4)),
            ("Kay et al. 2019", Some(0)),
            ("Smith and Jones 2020", None),
            ("Okafor 2018", None),
            ("Okafor 2017a", Some(1)),
            ("Okafor 2017b", Some(2)),
        ],
        "{:#?}",
        b.citations
    );
    assert_eq!(b.citations[3].unresolved, Some(CiteUnresolved::Ambiguous), "a wrong link is worse than none");
    assert_eq!(b.citations[4].unresolved, Some(CiteUnresolved::NoEntry));
    assert!(b.diagnostics.iter().any(|d| d.code == "citation_ambiguous") && b.diagnostics.iter().any(|d| d.code == "citation_unresolved"));
    // `Smith and Jones` and `2020a` are clickable; `(` and `As` are not.
    assert_eq!(b.citations[0].pieces.iter().map(|p| p.glyphs.len()).sum::<usize>(), "Smithand".len() + "Jones".len() + "2020a".len());
}

#[test]
fn no_heading_and_no_numbered_end_is_no_bibliography() {
    let mut d = Doc::new();
    d.text("Lone brackets [1] and [2] cite nothing: there is no list.");
    d.text("[1] A line that starts with a bracket.");
    d.text("More text after it, so no list ends the paper.");
    let b = d.analyse();
    assert!(b.region.is_none() && b.entries.is_empty() && b.citations.is_empty());
    assert!(b.diagnostics.iter().any(|x| x.code == "no_bibliography"));
    // A numbered list that ends the paper, with no heading, is taken, and says so.
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
    assert_eq!((b.entries.len(), b.citations.len()), (3, 2));
    // Out of order, it is not.
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
    assert_eq!(e2.fields.url.as_ref().map(|u| u.value.as_str()), Some("https://example.org/a/b/c.pdf"), "a URL broken over lines stays whole");
    assert_eq!(b.citations.iter().map(|c| c.target).collect::<Vec<_>>(), [Some(2), Some(0)], "the appendix is no part of the list");
}

#[test]
fn hostile_lists_and_citations_give_defined_results() {
    // A heading with nothing after it: no list.
    let mut d = Doc::new();
    d.text("Text [1].");
    d.heading("References");
    let b = d.analyse();
    assert!(b.entries.is_empty() && b.citations.is_empty());
    // A label of 40 digits, a range to 999999999, a year of 99999, ligatures, other digits and dashes.
    let mut d = Doc::new();
    let long = "1".repeat(40);
    let cite_long = format!("[{long}]");
    d.text(&format!("Cite {cite_long} and [1-999999999] and [1—2] and [١] and [2−1]."));
    d.heading("References");
    let lab = format!("[{long}]");
    d.line(BlockKind::Paragraph, true, 72.0, &[(lab.as_str(), R), ("A.", R), ("Smith,", R), ("Oﬃce", I), ("of", I), ("ﬁelds,", I), ("99999.", R)]);
    d.line(BlockKind::Paragraph, true, 72.0, &[("[1]", R), ("\u{FFFD}\u{FFFD},", R), ("ÿ́,", R), ("2001.", R)]);
    d.line(BlockKind::Paragraph, true, 72.0, &[("[2]", R), ("B.", R), ("Kay,", R), ("Two,", R), ("2002.", R)]);
    let b = d.analyse();
    assert_eq!(b.entries.len(), 3);
    assert_eq!(b.entries[0].label.as_deref(), None, "a label longer than any label is none");
    assert!(b.entries[0].fields.year.is_none(), "99999 is no year");
    assert!(b.citations.iter().all(|c| c.pieces.iter().all(|p| p.rect.iter().all(|v| v.is_finite()))));
    let huge: Vec<&Citation> = b.citations.iter().filter(|c| c.text == "999999999").collect();
    assert_eq!(huge.len(), 1, "{:#?}", b.citations);
    assert!(huge[0].target.is_none() && b.citations.iter().filter(|c| c.in_range).count() == 0);
    assert!(b.diagnostics.iter().any(|x| x.code == "range_capped"));
    // A word whose parts disagree in length, and non-finite boxes: no panic, a defined result.
    let mut d = numeric_doc();
    if let Some(w) = d.pages[0][0].words.get_mut(2) {
        w.starts.truncate(1);
        w.spans.clear();
        w.flags.push(0xFF);
        w.rect = [f32::NAN, f32::INFINITY, -1e30, 1e30];
    }
    let b = d.analyse();
    assert_eq!(b.entries.len(), 2);
}

#[test]
fn a_hundred_thousand_citations_and_ten_thousand_entries_stay_linear() {
    let mut d = Doc::new();
    for _ in 0..1_000 {
        let line: Vec<(&str, u8)> = (0..100).map(|_| ("[1]", R)).collect();
        d.line(BlockKind::Paragraph, true, 72.0, &line);
    }
    d.heading("References");
    for n in 1..=10_000 {
        let label = format!("[{n}]");
        let line = [(label.as_str(), R), ("A.", R), ("Smith,", R), ("Title,", I), ("2001.", R)];
        d.line(BlockKind::Paragraph, true, 72.0, &line);
    }
    let started = Instant::now();
    let b = d.analyse();
    let took = started.elapsed();
    assert_eq!(b.entries.len(), 10_000);
    assert_eq!(b.citations.len(), 100_000);
    assert!(b.citations.iter().all(|c| c.target == Some(0)));
    assert!(took.as_secs_f64() < 20.0, "{took:?}");
}
