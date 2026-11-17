use super::*;
use crate::text::math::{FontKind, GlyphDetail, MathFont, MathPage, Rule};

const W: f32 = 612.0;
const H: f32 = 792.0;

fn ctx(index: usize) -> PageContext {
    PageContext { index, width: W, height: H }
}

/// A page builder: fonts 0 = CMR10, 1 = CMBX12, 2 = CMMI10.
struct P {
    page: MathPage,
}

impl P {
    fn new() -> P {
        let font = |name: &str, key| MathFont {
            base_font: Some(format!("ABCDEF+{name}")),
            name: Some(name.into()),
            kind: FontKind::Type1,
            program_bytes: None,
            key,
        };
        P { page: MathPage { fonts: vec![font("CMR10", 1), font("CMBX12", 2), font("CMMI10", 3)], ..Default::default() } }
    }

    /// Words of `text` set from `x` on baseline `y` at `size`, a glyph per character.
    fn text(&mut self, font: u32, x: f32, y: f32, size: f32, text: &str) -> f32 {
        let adv = 0.5 * size;
        let mut x = x;
        for c in text.chars() {
            if c == ' ' {
                x += 0.33 * size;
                continue;
            }
            self.page.glyphs.push(GlyphDetail {
                code: c as u32,
                font,
                glyph_id: Some(1),
                name: None,
                text: Some(c.to_string()),
                unmapped: false,
                size,
                rect: [x, y - 0.75 * size, x + adv, y + 0.25 * size],
                ink: Some([x, y - 0.68 * size, x + adv * 0.9, y]),
                baseline: [[x, y], [x + adv, y]],
                direction: [1.0, 0.0],
                invisible: false,
            });
            x += adv;
        }
        x
    }

    /// A paragraph of `lines` lines `width` wide from (`x`, `y`), the first indented.
    fn para(&mut self, x: f32, y: f32, width: f32, lines: usize) -> f32 {
        let mut y = y;
        for k in 0..lines {
            let indent = if k == 0 { 15.0 } else { 0.0 };
            let last = k + 1 == lines;
            let w = if last { width * 0.5 } else { width } - indent;
            let words = (w / 25.0) as usize;
            let mut cx = x + indent;
            for _ in 0..words {
                cx = self.text(0, cx, y, 10.0, "word");
                cx += 5.0;
            }
            y += 12.0;
        }
        y
    }

    fn done(self) -> MathPage {
        self.page
    }
}

fn kinds(l: &PageLayout) -> Vec<BlockKind> {
    l.reading_order.iter().filter_map(|&i| l.blocks.get(i)).map(|b| b.kind).collect()
}

#[test]
fn an_empty_page_is_an_empty_layout_that_says_why() {
    let l = analyse_page(&MathPage::default(), ctx(0));
    assert!(l.blocks.is_empty() && l.columns.is_empty() && l.reading_order.is_empty());
    assert!(l.diagnostics.iter().any(|d| d.code == "no_text"));
    // A page size that is no size still gives a defined layout.
    let l = analyse_page(&MathPage::default(), PageContext { index: 3, width: f32::NAN, height: -1.0 });
    assert_eq!(l.page, 3);
    assert!(l.size.iter().all(|v| v.is_finite() && *v > 0.0));
}

#[test]
fn invisible_text_is_left_out_with_a_diagnostic() {
    let mut p = P::new();
    p.para(72.0, 100.0, 400.0, 5);
    let mut page = p.done();
    for g in &mut page.glyphs {
        g.invisible = true;
    }
    let l = analyse_page(&page, ctx(0));
    assert!(l.blocks.is_empty());
    let codes: Vec<&str> = l.diagnostics.iter().map(|d| d.code).collect();
    assert!(codes.contains(&"invisible_text") && codes.contains(&"no_text"), "{codes:?}");
}

#[test]
fn hostile_geometry_gives_a_defined_layout() {
    let mut p = P::new();
    p.para(72.0, 100.0, 400.0, 4);
    let mut page = p.done();
    let mut bad = page.glyphs[0].clone();
    for (k, v) in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY].into_iter().enumerate() {
        let mut g = bad.clone();
        g.rect[k] = v;
        page.glyphs.push(g);
        let mut g = bad.clone();
        g.baseline[0][1] = v;
        page.glyphs.push(g);
        let mut g = bad.clone();
        g.size = v;
        page.glyphs.push(g);
        let mut g = bad.clone();
        g.direction = [v, 0.0];
        page.glyphs.push(g);
        let mut g = bad.clone();
        g.ink = Some([v, 0.0, 1.0, 1.0]);
        page.glyphs.push(g);
    }
    // Zero-size, inverted and piled-up glyphs.
    bad.size = 0.0;
    page.glyphs.push(bad.clone());
    bad.size = 10.0;
    bad.rect = [100.0, 100.0, 100.0, 100.0];
    page.glyphs.push(bad.clone());
    bad.rect = [200.0, 100.0, 100.0, 90.0];
    page.glyphs.push(bad.clone());
    for _ in 0..500 {
        page.glyphs.push(page.glyphs[3].clone());
    }
    // Rotated text, and rules with no finite place.
    let mut rotated = page.glyphs[1].clone();
    rotated.direction = [0.0, -1.0];
    page.glyphs.push(rotated);
    page.rules.push(Rule { rect: [f32::NAN, 0.0, 1.0, 1.0], thickness: 0.4, horizontal: true, stroked: false });
    page.rules.push(Rule { rect: [0.0, 0.0, 0.0, 0.0], thickness: 0.0, horizontal: true, stroked: false });
    page.truncated = true;
    page.capped = true;
    for c in [ctx(0), PageContext { index: 0, width: f32::NAN, height: f32::NAN }, PageContext { index: 0, width: 0.0, height: 0.0 }] {
        let l = analyse_page(&page, c);
        assert!(!l.blocks.is_empty());
        let codes: Vec<&str> = l.diagnostics.iter().map(|d| d.code).collect();
        for want in ["bad_glyphs", "other_direction", "truncated", "capped"] {
            assert!(codes.contains(&want), "{want}: {codes:?}");
        }
        for b in &l.blocks {
            assert!(b.rect.iter().all(|v| v.is_finite()), "{b:?}");
            assert!((0.0..=1.0).contains(&b.confidence));
        }
        let mut seen: Vec<usize> = l.reading_order.clone();
        seen.sort_unstable();
        assert_eq!(seen, (0..l.blocks.len()).collect::<Vec<_>>());
    }
}

#[test]
fn one_column_paragraphs_split_at_their_indents() {
    let mut p = P::new();
    let y = p.para(72.0, 100.0, 450.0, 5);
    let y = p.para(72.0, y, 450.0, 4);
    p.para(72.0, y, 450.0, 3);
    let l = analyse_page(&p.done(), ctx(1));
    assert_eq!(kinds(&l), [BlockKind::Paragraph; 3], "{:#?}", l.blocks);
    assert_eq!(l.blocks.iter().map(|b| b.lines.len()).collect::<Vec<_>>(), [5, 4, 3]);
    assert!(l.columns.len() == 1 && l.diagnostics.is_empty(), "{:?}", l.diagnostics);
}

#[test]
fn two_columns_read_left_then_right_under_a_spanning_title() {
    let mut p = P::new();
    p.text(1, 200.0, 80.0, 17.0, "A Title Of Sorts");
    let mut y = 130.0;
    for _ in 0..3 {
        y = p.para(72.0, y, 220.0, 6);
    }
    let mut y2 = 130.0;
    for _ in 0..3 {
        y2 = p.para(320.0, y2, 220.0, 6);
    }
    let _ = (y, y2);
    let l = analyse_page(&p.done(), ctx(0));
    assert_eq!(l.columns.iter().filter(|c| c.of == 2).count(), 2, "{:?}", l.columns);
    let k = kinds(&l);
    assert_eq!(k.first(), Some(&BlockKind::Title), "{k:?}");
    let xs: Vec<f32> = l.reading_order.iter().skip(1).filter_map(|&i| l.blocks.get(i)).map(|b| b.rect[0]).collect();
    assert_eq!(xs.len(), 6);
    assert!(xs[..3].iter().all(|x| *x < 300.0) && xs[3..].iter().all(|x| *x > 300.0), "{xs:?}");
}

#[test]
fn headings_take_their_level_from_their_number() {
    let mut p = P::new();
    p.text(1, 72.0, 100.0, 12.0, "3.2 Spectral bounds");
    p.para(72.0, 125.0, 450.0, 4);
    p.text(1, 72.0, 200.0, 12.0, "Remarks");
    p.para(72.0, 225.0, 450.0, 3);
    let mut pages = vec![analyse_page(&p.done(), ctx(2))];
    let first = &pages[0];
    let hs: Vec<&HeadingInfo> = first.blocks.iter().filter_map(|b| b.heading.as_ref()).collect();
    assert_eq!(hs.len(), 2, "{:#?}", first.blocks);
    assert_eq!((hs[0].level, hs[0].number.as_deref(), hs[0].level_guessed), (2, Some("3.2"), false));
    assert!(hs[1].level_guessed);
    assert!(first.diagnostics.iter().any(|d| d.code == "heading_level_guessed"));
    let doc = analyse_document(&mut pages);
    assert_eq!(doc.sections.len(), 2);
    assert_eq!(doc.sections[0].title, "Spectral bounds");
    // The unnumbered heading looks like the numbered one: it takes its level.
    assert_eq!(doc.sections[1].level, 2);
    assert!(!doc.sections[1].level_guessed);
}

#[test]
fn a_display_with_its_number_is_one_block_between_paragraphs() {
    let mut p = P::new();
    let y = p.para(72.0, 100.0, 450.0, 3);
    let y = y + 6.0;
    let x = p.text(2, 250.0, y, 10.0, "xyz");
    p.text(0, x, y, 10.0, "=");
    p.text(2, x + 10.0, y, 10.0, "ab");
    p.text(0, 495.0, y, 10.0, "(2.1)");
    p.page.glyphs.iter_mut().rev().take(5).for_each(|g| g.font = 0);
    let y = y + 18.0;
    let mut q = P { page: p.done() };
    q.text(0, 72.0, y, 10.0, "which is what we wanted to show here and more words follow.");
    let l = analyse_page(&q.done(), ctx(1));
    let k = kinds(&l);
    assert_eq!(k, [BlockKind::Paragraph, BlockKind::DisplayEquation, BlockKind::Paragraph], "{:#?}", l.blocks);
    let display = l.blocks.iter().find(|b| b.kind == BlockKind::DisplayEquation).unwrap();
    assert_eq!(display.equation_tag.as_deref(), Some("(2.1)"));
}

#[test]
fn running_heads_are_confirmed_by_repetition_only() {
    let page = |n: usize, head: &str| {
        let mut p = P::new();
        p.text(0, 280.0, 40.0, 9.0, head);
        p.para(72.0, 100.0, 450.0, 8);
        p.text(0, 300.0, 760.0, 10.0, &n.to_string());
        analyse_page(&p.done(), ctx(n))
    };
    let mut pages = vec![page(1, "SHORT TITLE"), page(2, "SHORT TITLE"), page(3, "SHORT TITLE")];
    assert!(pages[0].diagnostics.iter().any(|d| d.code == "running_unconfirmed"));
    analyse_document(&mut pages);
    for p in &pages {
        let k = kinds(p);
        assert_eq!(k.first(), Some(&BlockKind::RunningHeader), "{k:?}");
        assert_eq!(k.last(), Some(&BlockKind::PageNumber), "{k:?}");
        assert!(p.blocks.iter().filter(|b| b.kind.is_running()).all(|b| b.confidence > 0.9));
        assert!(!p.diagnostics.iter().any(|d| d.code == "running_unconfirmed"));
    }
    let mut alone = vec![page(1, "SHORT TITLE")];
    analyse_document(&mut alone);
    assert!(alone[0].diagnostics.iter().any(|d| d.code == "running_unconfirmed"));
}

#[test]
fn a_hundred_thousand_glyphs_stay_fast() {
    let mut p = P::new();
    let mut y = 20.0;
    let mut x = 10.0;
    while p.page.glyphs.len() < 100_000 {
        x = p.text(0, x, y, 2.0, "ab cd");
        if x > 600.0 {
            x = 10.0;
            y += 2.4;
            if y > 780.0 {
                y = 20.0;
            }
        }
    }
    let start = std::time::Instant::now();
    let l = analyse_page(&p.done(), ctx(0));
    assert!(!l.blocks.is_empty());
    // Generous for a debug build on a loaded machine; quadratic work would take minutes.
    assert!(start.elapsed().as_secs() < 30, "{:?}", start.elapsed());
}

#[test]
fn many_rules_skip_the_frame_search() {
    let mut p = P::new();
    p.para(72.0, 100.0, 450.0, 3);
    let mut page = p.done();
    for k in 0..(MAX_FRAME_RULES + 1) {
        let y = 300.0 + (k % 400) as f32;
        page.rules.push(Rule { rect: [72.0, y, 500.0, y + 0.4], thickness: 0.4, horizontal: true, stroked: false });
    }
    let l = analyse_page(&page, ctx(0));
    assert!(l.diagnostics.iter().any(|d| d.code == "too_many_rules"));
}

#[test]
fn a_framed_region_with_a_caption_is_a_float() {
    let mut p = P::new();
    let y = p.para(72.0, 100.0, 450.0, 3);
    let (x0, y0, x1, y1) = (150.0, y + 10.0, 450.0, y + 90.0);
    for r in [[x0, y0, x1, y0 + 0.4], [x0, y1 - 0.4, x1, y1], [x0, y0, x0 + 0.4, y1], [x1 - 0.4, y0, x1, y1]] {
        p.page.rules.push(Rule { rect: r, thickness: 0.4, horizontal: r[2] - r[0] > r[3] - r[1], stroked: false });
    }
    p.text(0, 250.0, y0 + 40.0, 10.0, "a drawing");
    p.text(0, 180.0, y1 + 20.0, 10.0, "Figure 1. A frame around nothing.");
    p.para(72.0, y1 + 50.0, 450.0, 3);
    let l = analyse_page(&p.done(), ctx(1));
    let k = kinds(&l);
    assert_eq!(k, [BlockKind::Paragraph, BlockKind::Float, BlockKind::Caption, BlockKind::Paragraph], "{:#?}", l.blocks);
    let f = l.blocks.iter().position(|b| b.kind == BlockKind::Float).unwrap();
    let c = l.blocks.iter().position(|b| b.kind == BlockKind::Caption).unwrap();
    assert_eq!((l.blocks[f].partner, l.blocks[c].partner), (Some(c), Some(f)));
    assert_eq!(l.blocks[f].rules.len(), 4);
    assert!(l.blocks[f].text.contains("drawing"));
}

#[test]
fn labels_numbers_and_tags_read_as_they_should() {
    use super::analyse::{heading_number, is_tag, starts_with_label};
    assert!(is_tag("(3.2)") && is_tag("(12)") && is_tag("(A.1)") && is_tag(" (2a) "));
    assert!(!is_tag("(x)") && !is_tag("3.2") && !is_tag("()") && !is_tag("(1234567890123)"));
    assert!(starts_with_label("• item") && starts_with_label("1. first") && starts_with_label("(a) one") && starts_with_label("iii) three"));
    assert!(!starts_with_label("1.") && !starts_with_label("The 1. thing") && !starts_with_label("Abc. def"));
    assert_eq!(heading_number("3.2 Title"), Some(("3.2".into(), false)));
    assert_eq!(heading_number("A.1. Title"), Some(("A.1".into(), true)));
    assert_eq!(heading_number("Appendix B Proofs"), Some(("B".into(), true)));
    assert_eq!(heading_number("Abstract"), None);
    assert_eq!(heading_number("Introduction"), None);
    assert_eq!(heading_number(""), None);
    assert_eq!(heading_number("1234.5 x"), None);
}

#[test]
fn a_display_number_is_found_wherever_its_row_falls() {
    use super::analyse::equation_tag;
    assert_eq!(equation_tag("(2.1) x = y").as_deref(), Some("(2.1)"));
    assert_eq!(equation_tag("x = y (3)").as_deref(), Some("(3)"));
    assert_eq!(equation_tag("n sum (4) a = b k=0").as_deref(), Some("(4)"));
    assert_eq!(equation_tag("f(x) = g(T)"), None);
}
