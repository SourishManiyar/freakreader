//! Unit tests of statement and reference detection on synthetic pages.

use super::mentions::{canonical_statement, number_core};
use super::statements::parse_head;
use super::*;
use crate::text::math::{GlyphDetail, MathFont, MathPage};
use crate::text::page_layout::{Block, Column, DocumentStructure, PageContext, PageLayout, SectionNode, analyse_page};
use std::time::Instant;

const BOLD: Style = Style { bold: true, italic: false, small_caps: false, math: false };
const ITALIC: Style = Style { bold: false, italic: true, small_caps: false, math: false };
const ROMAN: Style = Style { bold: false, italic: false, small_caps: false, math: false };

/// Builds lines of words; glyph ids count up across the page.
struct Page {
    lines: Vec<FlowLine>,
    next: u32,
    y: f32,
}

impl Page {
    fn new() -> Self {
        Page { lines: Vec::new(), next: 0, y: 100.0 }
    }

    /// A line of `words` (`(text, style)`) in block `block` of kind `kind`.
    fn line(&mut self, block: usize, kind: BlockKind, first: bool, words: &[(&str, Style)]) -> &mut Self {
        let mut x = 72.0;
        let mut out = Vec::new();
        for (text, style) in words {
            let mut w = Word { rect: [x, self.y - 8.0, x, self.y + 2.0], style: *style, ..Default::default() };
            for (i, c) in text.char_indices() {
                w.starts.push(i);
                w.glyphs.push(self.next);
                w.spans.push([x, x + 5.0]);
                self.next += 1;
                x += 5.0;
                let _ = c;
            }
            w.text = (*text).to_owned();
            w.rect[2] = x;
            out.push(w);
            x += 3.0;
        }
        self.lines.push(FlowLine {
            page: 0,
            block,
            kind,
            first,
            rect: [72.0, self.y - 8.0, 540.0, self.y + 2.0],
            baseline: self.y,
            size: 10.0,
            column: [72.0, 540.0],
            words: out,
            mark: None,
        });
        self.y += 12.0;
        self
    }

    /// A paragraph of plain words, its first line opening block `block`.
    fn text(&mut self, block: usize, text: &str) -> &mut Self {
        let words: Vec<(&str, Style)> = text.split(' ').map(|w| (w, ROMAN)).collect();
        self.line(block, BlockKind::Paragraph, true, &words)
    }

    fn words(&self) -> Vec<PageWords> {
        vec![PageWords { page: 0, lines: self.lines.clone(), leading: Some(12.0), diagnostics: Vec::new() }]
    }
}

fn layout_with(blocks: Vec<Block>) -> PageLayout {
    let n = blocks.len();
    PageLayout {
        page: 0,
        size: [612.0, 792.0],
        columns: vec![Column { rect: [72.0, 72.0, 540.0, 720.0], zone: 0, index: 0, of: 1 }],
        blocks,
        reading_order: (0..n).collect(),
        ..Default::default()
    }
}

fn block(kind: BlockKind, text: &str, tag: Option<&str>) -> Block {
    Block {
        kind,
        rect: [72.0, 100.0, 540.0, 120.0],
        lines: Vec::new(),
        rules: Vec::new(),
        text: text.to_owned(),
        order: 0,
        column: Some(0),
        confidence: 0.8,
        style: Default::default(),
        heading: None,
        equation_tag: tag.map(str::to_owned),
        partner: None,
    }
}

fn mention_text(m: &Mention) -> String {
    format!("{} {}", m.word, m.number).trim().to_owned()
}

#[test]
fn heads_are_read_from_the_printed_word() {
    let w = |items: &[(&str, Style)]| -> Vec<Word> {
        let mut p = Page::new();
        p.line(0, BlockKind::Paragraph, true, items);
        p.lines.remove(0).words
    };
    let h = parse_head(&w(&[("Lemma", BOLD), ("2.1.", BOLD), ("Every", ITALIC)])).expect("lemma");
    assert_eq!((h.kind.as_str(), h.number.as_deref(), h.words), ("Lemma", Some("2.1"), 2));
    let h = parse_head(&w(&[("Theorem", BOLD), ("3", BOLD), ("(Main", ROMAN), ("theorem).", ROMAN), ("Let", ITALIC)])).expect("named");
    assert_eq!((h.number.as_deref(), h.name.as_deref()), (Some("3"), Some("Main theorem")));
    let h = parse_head(&w(&[("Proof", ITALIC), ("of", ITALIC), ("Lemma", ITALIC), ("2.1.", ITALIC), ("We", ROMAN)])).expect("proof");
    assert!(h.proof && h.name.as_deref() == Some("of Lemma 2.1"));
    let h = parse_head(&w(&[("Main", BOLD), ("Theorem.", BOLD), ("Every", ITALIC)])).expect("unnumbered");
    assert_eq!((h.kind.as_str(), h.number), ("Main Theorem", None));
    // An open set: a word no list knows is a head with a number.
    assert!(parse_head(&w(&[("Hypothesis", BOLD), ("H4.", BOLD), ("x", ROMAN)])).is_none(), "H4 is no number");
    assert_eq!(parse_head(&w(&[("Hypothesis", BOLD), ("4.", BOLD), ("Every", ROMAN)])).map(|h| h.kind), Some("Hypothesis".to_owned()));
    assert!(parse_head(&w(&[("Musing.", BOLD), ("Every", ROMAN)])).is_none(), "an unknown word without a number");
    // Not heads: italic running on into italic, a heading's word, no period, plain text.
    assert!(parse_head(&w(&[("Lemma", ITALIC), ("2.", ITALIC), ("Every", ITALIC)])).is_none());
    assert!(parse_head(&w(&[("Appendix", BOLD), ("A.", BOLD), ("Bounds", BOLD)])).is_none());
    assert!(parse_head(&w(&[("Lemma", BOLD), ("2.1", BOLD), ("says", ROMAN)])).is_none());
    assert!(parse_head(&w(&[("Lemma", ROMAN), ("2.1.", ROMAN), ("Every", ROMAN)])).is_none());
    assert!(parse_head(&[]).is_none());
}

#[test]
fn numbers_and_words_are_read_strictly() {
    assert_eq!(number_core("2.1."), Some("2.1"));
    assert_eq!(number_core("A.4,"), Some("A.4"));
    assert_eq!(number_core("12a)"), Some("12a"));
    assert_eq!(number_core("2.1(ii)"), Some("2.1"));
    assert_eq!(number_core("abc"), None);
    // Digits other than ASCII are no number: a defined result, not a guess.
    assert_eq!(number_core("٢.١"), None);
    assert_eq!(number_core("２"), None);
    // Too long for a number.
    assert_eq!(number_core(&"9".repeat(40)), None);
    assert_eq!(canonical_statement("Lemmas").as_deref(), Some("lemma"));
    assert_eq!(canonical_statement("Thm.").as_deref(), Some("theorem"));
    assert_eq!(canonical_statement("Lemmata").as_deref(), Some("lemma"));
    assert_eq!(canonical_statement("Main Theorem").as_deref(), Some("main theorem"));
    assert_eq!(canonical_statement("Banana"), None);
}

/// Lemma 2.1 and Theorem 2.1 both exist: the word decides, never the number.
fn shared_counter_page() -> (Vec<PageLayout>, Page) {
    let mut p = Page::new();
    p.line(0, BlockKind::Paragraph, true, &[("Lemma", BOLD), ("2.1.", BOLD), ("Every", ITALIC), ("bound", ITALIC), ("holds.", ITALIC)]);
    p.line(1, BlockKind::Paragraph, true, &[("Theorem", BOLD), ("2.1.", BOLD), ("Every", ITALIC), ("map", ITALIC), ("is.", ITALIC)]);
    p.line(2, BlockKind::Paragraph, true, &[("Proof.", ITALIC), ("By", ROMAN), ("Lemma", ROMAN), ("2.1,", ROMAN), ("done.", ROMAN), ("■", ROMAN)]);
    let mark = p.lines[2].words.last().map(|w| EndMark { kind: EndMarkKind::Filled, glyphs: w.glyphs.clone(), rules: Vec::new(), rect: w.rect });
    p.lines[2].mark = mark;
    p.text(3, "See Theorem 2.1 and Lemmas 2.1 and 2.3, Lemmas 2.1–2.3 and Proposition 2.1(ii).");
    p.text(4, "As in Theorem 2.1 in [5] and Lemma 2 of the appendix of [3], Lemma 9.9 is new.");
    p.text(5, "by (3.4) and (3.4), see (7.7), the case (3.4) and Eq. (3.4), §3 and Section 3.");
    let layouts = vec![layout_with(vec![
        block(BlockKind::Paragraph, "", None),
        block(BlockKind::Paragraph, "", None),
        block(BlockKind::Paragraph, "", None),
        block(BlockKind::Paragraph, "", None),
        block(BlockKind::Paragraph, "", None),
        block(BlockKind::Paragraph, "", None),
        block(BlockKind::DisplayEquation, "x = 1 (3.4)", Some("(3.4)")),
        block(BlockKind::Heading, "3 Results", None),
    ])];
    (layouts, p)
}

#[test]
fn mentions_resolve_by_word_and_number() {
    let (layouts, p) = shared_counter_page();
    let structure = DocumentStructure {
        sections: vec![SectionNode {
            page: 0,
            block: 7,
            level: 1,
            number: Some("3".into()),
            title: "Results".into(),
            parent: None,
            appendix: false,
            level_guessed: false,
        }],
        diagnostics: Vec::new(),
    };
    let r = analyse_references(&layouts, &p.words(), &structure);
    let kinds: Vec<(&str, Option<&str>, bool)> = r.statements.iter().map(|s| (s.kind.as_str(), s.number.as_deref(), s.proof)).collect();
    assert_eq!(kinds, [("Lemma", Some("2.1"), false), ("Theorem", Some("2.1"), false), ("Proof", None, true)]);
    // The proof comes right after the theorem, and ends at its mark.
    assert_eq!(r.statements[2].proves, Some((1, ProvesBy::Previous)));
    assert!(r.statements[2].end_mark.is_some());
    let got: Vec<(String, Option<Target>, Option<Unresolved>)> = r.mentions.iter().map(|m| (mention_text(m), m.target, m.unresolved)).collect();
    let lemma = Some(Target::Statement(0));
    let theorem = Some(Target::Statement(1));
    let eq = Some(Target::Equation { page: 0, block: 6 });
    let expected: Vec<(&str, Option<Target>, Option<Unresolved>)> = vec![
        ("Lemma 2.1", lemma, None),
        ("Theorem 2.1", theorem, None),
        ("Lemmas 2.1", lemma, None),
        ("2.3", None, Some(Unresolved::Missing)),
        ("Lemmas 2.1", lemma, None),
        ("2.3", None, Some(Unresolved::Missing)),
        ("Proposition 2.1", None, Some(Unresolved::Missing)),
        ("Theorem 2.1", None, Some(Unresolved::External)),
        ("Lemma 2", None, Some(Unresolved::External)),
        ("Lemma 9.9", None, Some(Unresolved::Missing)),
        ("(3.4)", eq, None),
        ("(3.4)", eq, None),
        ("Eq. (3.4)", eq, None),
        ("§ 3", Some(Target::Heading { page: 0, block: 7 }), None),
        ("Section 3", Some(Target::Heading { page: 0, block: 7 }), None),
    ];
    let got_ref: Vec<(&str, Option<Target>, Option<Unresolved>)> = got.iter().map(|(t, a, b)| (t.as_str(), *a, *b)).collect();
    assert_eq!(got_ref, expected);
    // `(ii)` is kept as the extra, outside the link; `(7.7)` has no display and `the case (3.4)` no
    // citing word, so neither is a mention.
    assert_eq!(r.mentions[6].extra.as_deref(), Some("(ii)"));
    assert_eq!(r.mentions[6].pieces.iter().map(|p| p.glyphs.len()).sum::<usize>(), "Proposition2.1".chars().count());
    // A resolved mention is sure, an unresolved one has no confidence.
    assert!(r.mentions.iter().all(|m| (m.target.is_some() && m.confidence >= 0.7) || (m.target.is_none() && m.confidence == 0.0)));
}

#[test]
fn a_mention_broken_across_lines_is_one_mention() {
    let mut p = Page::new();
    p.line(0, BlockKind::Paragraph, true, &[("Proposition", BOLD), ("4.", BOLD), ("Every", ITALIC), ("set", ITALIC)]);
    p.line(1, BlockKind::Paragraph, true, &[("By", ROMAN), ("Propo-", ROMAN)]);
    p.line(1, BlockKind::Paragraph, false, &[("sition", ROMAN), ("4", ROMAN), ("and", ROMAN), ("Defini\u{FB01}", ROMAN)]);
    let r = analyse_references(
        &[layout_with(vec![block(BlockKind::Paragraph, "", None), block(BlockKind::Paragraph, "", None)])],
        &p.words(),
        &DocumentStructure::default(),
    );
    assert_eq!(r.mentions.len(), 1, "{:?}", r.mentions);
    assert_eq!((r.mentions[0].word.as_str(), r.mentions[0].target), ("Proposition", Some(Target::Statement(0))));
    assert_eq!(r.mentions[0].pieces[0].glyphs.len(), "Proposition4".len());
}

#[test]
fn twin_heads_make_no_link() {
    let mut p = Page::new();
    for b in 0..2 {
        p.line(b, BlockKind::Paragraph, true, &[("Lemma", BOLD), ("1.", BOLD), ("Every", ITALIC), ("set.", ITALIC)]);
    }
    p.text(2, "By Lemma 1 we are done.");
    let blocks = (0..3).map(|_| block(BlockKind::Paragraph, "", None)).collect();
    let r = analyse_references(&[layout_with(blocks)], &p.words(), &DocumentStructure::default());
    assert_eq!(r.statements.len(), 2);
    assert_eq!((r.mentions[0].target, r.mentions[0].unresolved), (None, Some(Unresolved::Ambiguous)));
}

/// A thousand `Lemma 1.` heads and a thousand mentions: all read, each mention ambiguous, in
/// linear time.
#[test]
fn a_thousand_twin_heads_stay_linear() {
    let mut p = Page::new();
    for b in 0..1000 {
        p.line(b, BlockKind::Paragraph, true, &[("Lemma", BOLD), ("1.", BOLD), ("Every", ITALIC), ("set.", ITALIC)]);
        p.line(b, BlockKind::Paragraph, false, &[("By", ITALIC), ("Lemma", ITALIC), ("1", ITALIC), ("again.", ITALIC)]);
    }
    let blocks = (0..1000).map(|_| block(BlockKind::Paragraph, "", None)).collect();
    let start = Instant::now();
    let r = analyse_references(&[layout_with(blocks)], &p.words(), &DocumentStructure::default());
    assert_eq!(r.statements.len(), 1000);
    assert_eq!(r.mentions.len(), 1000);
    assert!(r.mentions.iter().all(|m| m.unresolved == Some(Unresolved::Ambiguous)));
    assert!(start.elapsed().as_secs() < 10, "{:?}", start.elapsed());
}

#[test]
fn hostile_numbers_and_text_give_defined_results() {
    let mut p = Page::new();
    let huge = "9".repeat(5000);
    p.line(0, BlockKind::Paragraph, true, &[("Lemma", BOLD), (huge.as_str(), BOLD), ("Every", ITALIC)]);
    p.text(1, &format!("By Lemma {huge} and Theorem ٣ and Lemma 2.1.1.1.1.1.1.1.1.1.1.1.1.1.1 and (((((((( and ( and Lemmas and Lemma"));
    p.text(2, "");
    let mut words = p.words();
    // Coordinates that are not numbers.
    if let Some(w) = words[0].lines[1].words.get_mut(0) {
        w.rect = [f32::NAN, f32::INFINITY, -f32::INFINITY, f32::NAN];
        w.spans = vec![[f32::NAN, f32::NAN]; w.glyphs.len()];
    }
    let r = analyse_references(&[layout_with(vec![block(BlockKind::Paragraph, "", None); 3])], &words, &DocumentStructure::default());
    assert!(r.statements.is_empty(), "{:?}", r.statements);
    assert!(r.mentions.is_empty(), "{:?}", r.mentions);
    // Nothing at all.
    let r = analyse_references(&[], &[], &DocumentStructure::default());
    assert!(r.statements.is_empty() && r.mentions.is_empty());
    // Layouts and words that do not match (more of one than the other).
    let (layouts, p) = shared_counter_page();
    let r = analyse_references(&layouts[..0], &p.words(), &DocumentStructure::default());
    assert!(r.mentions.iter().all(|m| !matches!(m.target, Some(Target::Equation { .. }))));
    let r = analyse_references(&layouts, &[], &DocumentStructure::default());
    assert!(r.statements.is_empty());
}

#[test]
fn a_proof_without_a_mark_ends_where_space_sets_the_next_paragraph_apart() {
    let mut p = Page::new();
    p.line(0, BlockKind::Paragraph, true, &[("Lemma", BOLD), ("1.", BOLD), ("Every", ITALIC), ("set.", ITALIC)]);
    p.line(1, BlockKind::Paragraph, true, &[("Proof.", ITALIC), ("Clear", ROMAN), ("enough.", ROMAN)]);
    p.line(2, BlockKind::Paragraph, true, &[("More", ROMAN), ("of", ROMAN), ("the", ROMAN), ("proof.", ROMAN)]);
    // A gap as an environment leaves: the proof ended before it.
    p.y += 10.0;
    p.text(3, "After the proof comes text.");
    let blocks = (0..4).map(|_| block(BlockKind::Paragraph, "", None)).collect();
    let r = analyse_references(&[layout_with(blocks)], &p.words(), &DocumentStructure::default());
    let proof = &r.statements[1];
    assert!(proof.proof && proof.end_mark.is_none());
    assert_eq!(proof.last_line, (0, 2));
    assert!(proof.confidence < 0.7);
    assert!(r.diagnostics.iter().any(|d| d.code == "proof_unended"));
}

fn glyph(code: u32, x: f32, y: f32, text: &str) -> GlyphDetail {
    GlyphDetail {
        code,
        font: 0,
        glyph_id: Some(code),
        name: None,
        text: Some(text.to_owned()),
        unmapped: false,
        size: 10.0,
        rect: [x, y - 8.0, x + 5.0, y + 2.0],
        ink: Some([x, y - 7.0, x + 4.0, y]),
        baseline: [[x, y], [x + 5.0, y]],
        direction: [1.0, 0.0],
        invisible: false,
    }
}

fn font(name: &str) -> MathFont {
    MathFont { base_font: Some(name.into()), name: Some(name.into()), kind: crate::text::math::FontKind::Type1, program_bytes: None, key: 0 }
}

/// Pages from the glyph level: empty, hostile coordinates, text running down the page (a page
/// turned by `/Rotate` is in view space already; text in another direction is left out), and too
/// many words.
#[test]
fn page_words_never_fail() {
    let empty = MathPage::default();
    let layout = analyse_page(&empty, PageContext { index: 0, width: 612.0, height: 792.0 });
    assert!(page_words(&empty, &layout).lines.is_empty());

    let mut page = MathPage { fonts: vec![font("CMBX10"), font("CMTI10")], ..Default::default() };
    for (i, c) in "Lemma 1. Every".chars().enumerate() {
        let mut g = glyph(c as u32, 72.0 + 6.0 * i as f32, 100.0, &c.to_string());
        g.font = u32::from(i > 8);
        if c != ' ' {
            page.glyphs.push(g);
        }
    }
    page.glyphs.push(glyph(1, f32::NAN, f32::INFINITY, "x"));
    let mut down = glyph(2, 300.0, 300.0, "y");
    down.direction = [0.0, 1.0];
    page.glyphs.push(down);
    let layout = analyse_page(&page, PageContext { index: 0, width: 612.0, height: 792.0 });
    let words = page_words(&page, &layout);
    assert!(words.lines.iter().flat_map(|l| l.words.iter()).all(|w| w.rect.iter().all(|v| v.is_finite())));
    // A layout whose blocks point at glyphs the page does not have.
    let mut bad = layout.clone();
    for b in &mut bad.blocks {
        for l in &mut b.lines {
            l.glyphs.push(u32::MAX);
        }
    }
    bad.reading_order.push(usize::MAX);
    let _ = page_words(&page, &bad);

    // Past the word limit the page is cut, and says so.
    let mut many = MathPage { fonts: vec![font("CMR10")], ..Default::default() };
    let per_line = 50usize;
    let lines = MAX_WORDS_PER_PAGE / per_line + 10;
    for l in 0..lines {
        for k in 0..per_line {
            many.glyphs.push(glyph(65, 10.0 + 12.0 * k as f32, 20.0 + 0.01 * l as f32, "A"));
        }
    }
    let layout = PageLayout {
        blocks: vec![Block {
            lines: (0..lines)
                .map(|l| crate::text::page_layout::Line {
                    glyphs: ((l * per_line) as u32..((l + 1) * per_line) as u32).collect(),
                    baseline: 20.0,
                    rect: [10.0, 10.0, 600.0, 22.0],
                })
                .collect(),
            ..block(BlockKind::Paragraph, "", None)
        }],
        reading_order: vec![0],
        ..Default::default()
    };
    let w = page_words(&many, &layout);
    assert!(w.diagnostics.iter().any(|d| d.code == "capped"));
    assert!(w.lines.iter().map(|l| l.words.len()).sum::<usize>() <= MAX_WORDS_PER_PAGE);
}

#[test]
fn a_head_alone_on_its_line_before_a_display_is_a_statement() {
    let mut p = Page::new();
    // amsthm's definition style: a bold head, then the example opens with a display, and the
    // layout takes the short bold line for a heading.
    p.line(0, BlockKind::Heading, true, &[("Example", BOLD), ("1.4.", BOLD)]);
    p.line(1, BlockKind::DisplayEquation, true, &[("f(x)", ROMAN), ("=", ROMAN), ("x2", ROMAN)]);
    p.line(2, BlockKind::Paragraph, true, &[("is", ROMAN), ("a", ROMAN), ("convex", ROMAN), ("function.", ROMAN)]);
    p.y += 10.0;
    p.text(3, "After the example comes text.");
    let blocks = (0..4).map(|_| block(BlockKind::Paragraph, "", None)).collect();
    let r = analyse_references(&[layout_with(blocks)], &p.words(), &DocumentStructure::default());
    let s = r.statements.first().expect("the example");
    assert_eq!((s.kind.as_str(), s.number.as_deref(), s.italic_body), ("Example", Some("1.4"), false));
    assert_eq!((s.first_line, s.last_line), ((0, 0), (0, 2)), "through its display and the text after it");
    // A heading that is no statement stays a heading: an unnumbered one, or one followed by another
    // heading.
    let mut p = Page::new();
    p.line(0, BlockKind::Heading, true, &[("Examples.", BOLD)]);
    p.text(1, "Some text here.");
    p.line(2, BlockKind::Heading, true, &[("Example", BOLD), ("2.", BOLD)]);
    p.line(3, BlockKind::Heading, true, &[("Further", BOLD), ("results", BOLD)]);
    let blocks = (0..4).map(|_| block(BlockKind::Paragraph, "", None)).collect();
    let r = analyse_references(&[layout_with(blocks)], &p.words(), &DocumentStructure::default());
    assert!(r.statements.is_empty(), "{:?}", r.statements);
}

#[test]
fn an_upright_statement_takes_a_list_items_second_line_but_not_the_paragraph_after_it() {
    let mut p = Page::new();
    p.line(
        0,
        BlockKind::Paragraph,
        true,
        &[("Example", BOLD), ("2.1.", BOLD), ("Let", ROMAN), ("X", ROMAN), ("be", ROMAN), ("a", ROMAN), ("set.", ROMAN)],
    );
    // A list item, then its second line, which the layout made a block of its own.
    p.line(1, BlockKind::ListItem, true, &[("(1)", ROMAN), ("The", ROMAN), ("first", ROMAN), ("item", ROMAN), ("goes", ROMAN), ("on", ROMAN)]);
    p.line(2, BlockKind::Paragraph, true, &[("here.", ROMAN)]);
    if let Some(l) = p.lines.last_mut() {
        l.rect[0] += 30.0;
    }
    // A paragraph after it, indented as a paragraph is and with no more than a line's space before
    // it (LaTeX's space after an environment can shrink that far): not the example's.
    p.line(3, BlockKind::Paragraph, true, &[("After", ROMAN), ("the", ROMAN), ("example", ROMAN), ("comes", ROMAN), ("text.", ROMAN)]);
    if let Some(l) = p.lines.last_mut() {
        l.rect[0] += 15.0;
    }
    let blocks = (0..4).map(|_| block(BlockKind::Paragraph, "", None)).collect();
    let r = analyse_references(&[layout_with(blocks)], &p.words(), &DocumentStructure::default());
    let s = r.statements.first().expect("the example");
    assert!(!s.italic_body);
    assert_eq!(s.last_line, (0, 2), "{:?}", r.statements);
    // An italic statement still ends where upright text starts a paragraph.
    let mut p = Page::new();
    p.line(
        0,
        BlockKind::Paragraph,
        true,
        &[("Lemma", BOLD), ("2.2.", BOLD), ("Every", ITALIC), ("set", ITALIC), ("is", ITALIC), ("closed.", ITALIC)],
    );
    p.line(1, BlockKind::Paragraph, true, &[("We", ROMAN), ("turn", ROMAN), ("to", ROMAN), ("the", ROMAN), ("proof.", ROMAN)]);
    if let Some(l) = p.lines.last_mut() {
        l.rect[2] = 300.0;
    }
    let blocks = (0..2).map(|_| block(BlockKind::Paragraph, "", None)).collect();
    let r = analyse_references(&[layout_with(blocks)], &p.words(), &DocumentStructure::default());
    let s = r.statements.first().expect("the lemma");
    assert!(s.italic_body);
    assert_eq!(s.last_line, (0, 0));
}
