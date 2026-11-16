use super::*;
use crate::{PageRenderer, RenderConfig};
use std::sync::Arc;

/// The renderer's own bundled fixed-pitch CFF face: a real program to embed, so no font file is added.
const CFF: &[u8] = include_bytes!("../../../../../vendor/hayro-interpret/assets/FoxitFixed.pfb");

/// A one-page file: `content`, drawn on a 200 × 200 pt page with `resources` and `page_extra`
/// (more page entries, such as `/Rotate 90`), and `objects` numbered from 5.
fn page_pdf(content: &str, resources: &str, page_extra: &str, objects: &[Vec<u8>]) -> Vec<u8> {
    let mut all: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Resources << {resources} >> {page_extra} >>").into_bytes(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()).into_bytes(),
    ];
    all.extend_from_slice(objects);
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in all.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        bytes.extend_from_slice(object);
        bytes.extend_from_slice(b"\nendobj\n");
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", all.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", all.len() + 1).as_bytes());
    bytes
}

fn pdf_of(bytes: Vec<u8>) -> Pdf {
    Pdf::new(Arc::new(bytes)).expect("the test file opens")
}

fn details(bytes: Vec<u8>) -> MathPage {
    extract_math_page(&pdf_of(bytes), 0, &InterpreterSettings::default()).expect("the page is read")
}

/// A page of nothing but `content` (no fonts).
fn drawn(content: &str) -> MathPage {
    details(page_pdf(content, "", "", &[]))
}

fn near(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.01)
}

/// An unembedded Type 1 font as TeX names its Computer Modern math italic, with its codes 65 and 66
/// renamed: `foo` is no name in the Adobe Glyph List, so the glyph has no Unicode.
fn math_italic() -> Vec<u8> {
    b"<< /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+CMMI10 /Encoding << /Type /Encoding /Differences [65 /foo /alpha] >> >>".to_vec()
}

#[test]
fn a_glyph_without_unicode_is_kept_and_flagged_where_the_text_layer_drops_it() {
    let bytes = page_pdf("BT /F1 12 Tf 20 100 Td (ABC) Tj ET", "/Font << /F1 5 0 R >>", "", &[math_italic()]);
    let page = details(bytes.clone());
    assert_eq!(page.glyphs.len(), 3, "{page:?}");
    let [foo, alpha, c] = &page.glyphs[..] else { panic!("three glyphs") };
    assert_eq!((foo.code, foo.name.as_deref(), foo.text.as_deref(), foo.unmapped), (65, Some("foo"), None, true));
    assert_eq!((alpha.code, alpha.name.as_deref(), alpha.text.as_deref(), alpha.unmapped), (66, Some("alpha"), Some("α"), false));
    assert_eq!((c.code, c.text.as_deref(), c.unmapped), (67, Some("C"), false));
    // The text layer reads `αC`: the glyph with no Unicode is not in it.
    let text = crate::text::extract_page(&pdf_of(bytes), 0, &InterpreterSettings::default()).expect("the page has text");
    assert_eq!(text.plain_text(), "αC");
    assert_eq!(text.glyphs.len(), 2);
    // What the two keep of the glyphs they share is the same em box.
    for (t, d) in text.glyphs.iter().zip([alpha, c]) {
        assert_eq!(t.rect, d.rect);
    }
}

#[test]
fn the_font_is_named_without_its_subset_prefix_and_shared_by_its_glyphs() {
    let page = details(page_pdf("BT /F1 12 Tf 20 100 Td (ABC) Tj ET", "/Font << /F1 5 0 R >>", "", &[math_italic()]));
    assert_eq!(page.fonts.len(), 1);
    let font = &page.fonts[0];
    assert_eq!((font.base_font.as_deref(), font.name.as_deref()), (Some("ABCDEF+CMMI10"), Some("CMMI10")));
    // Not embedded: a stand-in program, whose bytes aren't the document's.
    assert_eq!((font.kind, font.program_bytes), (FontKind::Standard, None));
    assert!(page.glyphs.iter().all(|g| g.font == 0));
}

#[test]
fn size_and_baseline_come_from_the_transform_that_places_the_glyph() {
    // Scaled 2 × 3, then 10 pt text with its origin at (5, 20): an em is 30 pt tall on the page.
    let page = details(page_pdf("q 2 0 0 3 0 0 cm BT /F1 10 Tf 1 0 0 1 5 20 Tm (A) Tj ET Q", "/Font << /F1 5 0 R >>", "", &[math_italic()]));
    let g = &page.glyphs[0];
    assert!((g.size - 30.0).abs() < 1e-3, "size {}", g.size);
    // y flips: the baseline is 60 pt up the page, which is 140 down the 200 pt view.
    assert!(g.baseline[0][0] == 10.0 && (g.baseline[0][1] - 140.0).abs() < 1e-3, "{:?}", g.baseline);
    // Along the writing direction by the glyph's advance (more than a tenth of an em).
    assert!(g.baseline[1][0] > g.baseline[0][0] + 1.0 && (g.baseline[1][1] - g.baseline[0][1]).abs() < 1e-3);
    assert_eq!(g.direction, [1.0, 0.0]);
    // Turned a quarter, counter-clockwise: the text runs up the page, which is -y in the view.
    let page = details(page_pdf("BT /F1 10 Tf 0 1 -1 0 100 100 Tm (A) Tj ET", "/Font << /F1 5 0 R >>", "", &[math_italic()]));
    let g = &page.glyphs[0];
    assert!((g.size - 10.0).abs() < 1e-3);
    assert!(g.direction[0].abs() < 1e-6 && (g.direction[1] + 1.0).abs() < 1e-6, "{:?}", g.direction);
    assert!(g.baseline[1][1] < g.baseline[0][1], "{:?}", g.baseline);
}

/// A Type 1 font whose `FontFile3` is the bundled CFF program, drawn at 60 pt.
fn cff_page() -> Vec<u8> {
    let mut program = format!("<< /Subtype /Type1C /Length {} >>\nstream\n", CFF.len()).into_bytes();
    program.extend_from_slice(CFF);
    program.extend_from_slice(b"\nendstream");
    let widths = vec!["600"; 39].join(" ");
    page_pdf(
        "BT /F1 60 Tf 10 100 Td (AgW) Tj ET",
        "/Font << /F1 5 0 R >>",
        "",
        &[
            format!("<< /Type /Font /Subtype /Type1 /BaseFont /XYZABC+Synthetic /Encoding /WinAnsiEncoding /FirstChar 65 /LastChar 103 /Widths [{widths}] /FontDescriptor 6 0 R >>").into_bytes(),
            b"<< /Type /FontDescriptor /FontName /XYZABC+Synthetic /Flags 34 /FontBBox [0 -200 1000 900] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 120 /FontFile3 7 0 R >>".to_vec(),
            program,
        ],
    )
}

#[test]
fn an_embedded_program_is_named_by_kind_and_its_ink_is_measured_from_the_outline() {
    let page = details(cff_page());
    assert_eq!(page.glyphs.len(), 3, "{page:?}");
    let font = &page.fonts[0];
    assert_eq!((font.kind, font.program_bytes, font.name.as_deref()), (FontKind::Cff, Some(CFF.len()), Some("Synthetic")));
    for (g, (code, name)) in page.glyphs.iter().zip([(65, "A"), (103, "g"), (87, "W")]) {
        assert_eq!((g.code, g.name.as_deref(), g.text.as_deref()), (code, Some(name), Some(name)));
        assert!(g.glyph_id.is_some());
        let ink = g.ink.expect("a letter has ink");
        // The ink is the outline's own bounds: within the em box, and tighter than it.
        assert!(ink[0] >= g.rect[0] - 0.01 && ink[2] <= g.rect[2] + 0.01, "{ink:?} in {:?}", g.rect);
        assert!(ink[2] > ink[0] && ink[3] > ink[1]);
        assert!(ink[3] - ink[1] < g.rect[3] - g.rect[1], "{ink:?} is not shorter than the em box {:?}", g.rect);
    }
    // 'g' hangs below its baseline; 'A' sits on it.
    let (a, g) = (&page.glyphs[0], &page.glyphs[1]);
    assert!(g.ink.unwrap()[3] > g.baseline[0][1] + 1.0, "{g:?}");
    assert!((a.ink.unwrap()[3] - a.baseline[0][1]).abs() < 1.0, "{a:?}");
}

#[test]
fn a_type3_glyphs_ink_is_what_its_procedure_paints() {
    // 'a' paints the rectangle (100,0)–(400,700) in glyph space; at 10 pt with (20, 50) as origin:
    // x 21–24, y 50–57 up from the page's bottom, so 143–150 down the 200 pt view.
    let proc = "500 0 0 0 500 700 d1 100 0 300 700 re f";
    let page = details(page_pdf(
        "BT /F1 10 Tf 20 50 Td (a) Tj ET",
        "/Font << /F1 5 0 R >>",
        "",
        &[
            b"<< /Type /Font /Subtype /Type3 /Name /T3 /FontBBox [0 0 500 700] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a 6 0 R >> /Encoding << /Type /Encoding /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [500] >>".to_vec(),
            format!("<< /Length {} >>\nstream\n{proc}\nendstream", proc.len()).into_bytes(),
        ],
    ));
    assert_eq!(page.glyphs.len(), 1, "{page:?}");
    let g = &page.glyphs[0];
    assert_eq!((g.code, g.name.as_deref(), g.text.as_deref(), g.glyph_id), (97, Some("a"), Some("a"), None));
    assert!(near(g.ink.expect("it paints"), [21.0, 143.0, 24.0, 150.0]), "{:?}", g.ink);
    let font = &page.fonts[g.font as usize];
    assert_eq!((font.kind, font.program_bytes, font.name.as_deref()), (FontKind::Type3, None, Some("T3")));
}

#[test]
fn thin_filled_rectangles_and_straight_lines_are_rules_and_nothing_else_is() {
    let page = drawn(
        "10 100 50 0.5 re f \
         100 10 0.5 40 re f \
         10 50 40 40 re f \
         5 5 3 3 re f \
         10 20 30 0 re f \
         1 w 10 150 m 60 150 l S \
         1 w [3] 0 d 10 160 m 60 160 l S [] 0 d \
         1 w 10 170 m 60 175 l S \
         1 w 10 180 m 60 180 l 60 190 l S \
         1 w 80 20 m 120 20 l h S \
         1 w 10 190 m 20 195 30 195 40 190 c S \
         20 w 10 140 m 60 140 l S \
         0 0 1 rg 70 120 20 0.5 re 70 125 20 0.5 re f",
    );
    let seen: Vec<([f32; 4], bool, bool)> = page.rules.iter().map(|r| (r.rect, r.horizontal, r.stroked)).collect();
    let expect = [
        // 10 100 50 0.5 re: x 10–60, y 100–100.5 up, so 99.5–100 down the view.
        ([10.0, 99.5, 60.0, 100.0], true, false),
        ([100.0, 150.0, 100.5, 190.0], false, false),
        // A rectangle with no height: the interpreter fills it as a one point hairline (as it
        // draws it), so it reaches the device, and the page, as the rule it looks like.
        ([10.0, 180.0, 40.0, 181.0], true, false),
        // The stroke is widened by half its width each side of its line (y = 50 down the view).
        ([10.0, 49.5, 60.0, 50.5], true, true),
        // Two thin rectangles drawn by one fill.
        ([70.0, 79.5, 90.0, 80.0], true, false),
        ([70.0, 74.5, 90.0, 75.0], true, false),
    ];
    assert_eq!(seen.len(), expect.len(), "{seen:?}");
    for (got, want) in seen.iter().zip(expect) {
        assert!(near(got.0, want.0) && (got.1, got.2) == (want.1, want.2), "{got:?} is not {want:?}");
    }
    assert!((page.rules[3].thickness - 1.0).abs() < 1e-6 && (page.rules[0].thickness - 0.5).abs() < 1e-6);
}

#[test]
fn a_strokes_width_is_scaled_by_the_page_transform_and_a_turned_page_turns_the_rule() {
    // 0.5 wide in user space under a 2× scale: a 1 pt rule.
    let page = drawn("q 2 0 0 2 0 0 cm 0.5 w 10 10 m 60 10 l S Q");
    assert!((page.rules[0].thickness - 1.0).abs() < 1e-6 && page.rules[0].horizontal, "{:?}", page.rules);
    assert!(near(page.rules[0].rect, [20.0, 179.5, 120.0, 180.5]), "{:?}", page.rules);
    // The same line on a page shown turned a quarter runs up and down the view.
    let turned = details(page_pdf("1 w 10 10 m 60 10 l S", "", "/Rotate 90", &[]));
    assert!(!turned.rules[0].horizontal && (turned.rules[0].thickness - 1.0).abs() < 1e-6, "{:?}", turned.rules);
}

#[test]
fn a_rule_outside_the_crop_box_is_not_on_the_page() {
    let page = drawn("300 100 50 0.5 re f 10 -50 50 0.5 re f 10 100 50 0.5 re f");
    assert_eq!(page.rules.len(), 1, "{:?}", page.rules);
}

#[test]
fn text_with_a_font_size_beyond_any_page_is_not_recorded() {
    // As the text layer drops it: a box tens of thousands of points tall is damage, not text.
    let bytes = page_pdf("BT /F1 100000 Tf 20 100 Td (AB) Tj ET", "/Font << /F1 5 0 R >>", "", &[math_italic()]);
    assert!(details(bytes).glyphs.is_empty());
}

#[test]
fn the_caps_stop_recording_and_say_so() {
    let run = |content: &str, resources: &str, objects: &[Vec<u8>], caps: Caps| {
        extract_math_page_with(&pdf_of(page_pdf(content, resources, "", objects)), 0, &InterpreterSettings::default(), caps).unwrap()
    };
    let big = Caps { glyphs: 1000, rules: 1000, fonts: 1000, type3_ink: 1000 };

    let font = "/Font << /F1 5 0 R >>";
    let text = "BT /F1 12 Tf 20 100 Td (ABCDE) Tj ET";
    let page = run(text, font, &[math_italic()], Caps { glyphs: 3, ..big });
    assert_eq!((page.glyphs.len(), page.capped), (3, true));
    let page = run(text, font, &[math_italic()], Caps { glyphs: 5, ..big });
    assert_eq!((page.glyphs.len(), page.capped), (5, false), "exactly at the cap is not over it");

    let rules = "10 100 50 0.5 re f 10 110 50 0.5 re f 10 120 50 0.5 re f";
    let page = run(rules, "", &[], Caps { rules: 2, ..big });
    assert_eq!((page.rules.len(), page.capped), (2, true));
    assert_eq!(run(rules, "", &[], big).rules.len(), 3);

    // Two fonts, a cap of one: the second font's glyphs are left out.
    let two = "BT /F1 12 Tf 20 100 Td (A) Tj /F2 12 Tf (A) Tj ET";
    let fonts = "/Font << /F1 5 0 R /F2 6 0 R >>";
    let other = b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>".to_vec();
    let page = run(two, fonts, &[math_italic(), other], Caps { fonts: 1, ..big });
    assert_eq!((page.fonts.len(), page.glyphs.len(), page.capped), (1, 1, true));

    // Type 3 ink is measured for as many glyphs as the budget allows; the rest have glyphs and no ink.
    let proc = "500 0 0 0 500 700 d1 100 0 300 700 re f";
    let t3 = [
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 500 700] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a 6 0 R >> /Encoding << /Type /Encoding /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [500] >>".to_vec(),
        format!("<< /Length {} >>\nstream\n{proc}\nendstream", proc.len()).into_bytes(),
    ];
    let page = run("BT /F1 10 Tf 20 50 Td (aaa) Tj ET", font, &t3, Caps { type3_ink: 2, ..big });
    let inks: Vec<bool> = page.glyphs.iter().map(|g| g.ink.is_some()).collect();
    assert_eq!(inks, [true, true, false]);
}

#[test]
fn a_panic_inside_the_interpreter_comes_back_as_an_error_and_the_renderer_goes_on() {
    let bytes = page_pdf("BT /F1 12 Tf 20 100 Td (ABC) Tj ET", "/Font << /F1 5 0 R >>", "", &[math_italic()]);
    let mut renderer = PageRenderer::new(Arc::new(bytes), RenderConfig::default());
    PANIC_AT_GLYPH.with(|p| p.set(Some(1)));
    let crashed = renderer.glyph_details(0);
    PANIC_AT_GLYPH.with(|p| p.set(None));
    let error = crashed.expect_err("the injected panic is an error, not an unwind");
    assert!(error.contains("crashed on page 1") && error.contains("injected panic"), "{error}");
    // The renderer still works.
    assert_eq!(renderer.glyph_details(0).expect("the next page read is fine").glyphs.len(), 3);
    assert!(renderer.glyph_details(7).is_err(), "a page that does not exist is an error");
}
