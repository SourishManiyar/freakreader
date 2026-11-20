//! Split two-page spreads, on synthetic scans built here: two columns of text-like ink with a blank
//! gutter, a shadow down the spine, a portrait page, a turned page and a page already cropped.

use std::sync::Arc;

use pdfcraft_cos::{Document as Cos, ObjRef, Object};
use pdfcraft_testkit::Pdf;

use super::*;

/// Scan width and height in pixels (half the page's points: about 36 dpi is plenty for the ink).
const SW: usize = 396;
const SH: usize = 306;

/// A grey scan of two book pages: lines of word-like dashes in a left column ending at
/// `gutter.0` and a right column starting at `gutter.1` (fractions of the width), blank between.
/// `shadow` darkens a band around the spine smoothly, as a book's fold does on a scanner.
fn scan(gutter: (f64, f64), shadow: Option<f64>) -> Vec<u8> {
    let mut g = vec![255u8; SW * SH];
    for y in 0..SH {
        for x in 0..SW {
            let f = x as f64 / SW as f64;
            let mut v = 255.0;
            if let Some(c) = shadow {
                let d = ((f - c).abs() / 0.06).min(1.0);
                v = 255.0 - 130.0 * (1.0 - d);
            }
            let text_row = y > SH / 10 && y < SH * 9 / 10 && y % 9 < 3;
            let in_column = (f > 0.08 && f < gutter.0) || (f > gutter.1 && f < 0.92);
            if text_row && in_column && (x / 5) % 6 != 5 {
                v = 20.0;
            }
            g[y * SW + x] = v as u8;
        }
    }
    g
}

fn hex(data: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = data.iter().flat_map(|b| format!("{b:02x}").into_bytes()).collect();
    out.push(b'>');
    out
}

/// One page of the fixture.
struct PageSpec {
    media: [f64; 4],
    crop: Option<[f64; 4]>,
    rotate: i64,
    /// The scan, or none for a page of text only.
    scan: Option<Vec<u8>>,
    annots: Vec<String>,
}

fn landscape(scan: Vec<u8>) -> PageSpec {
    PageSpec { media: [0.0, 0.0, 792.0, 612.0], crop: None, rotate: 0, scan: Some(scan), annots: Vec::new() }
}

/// Build the document. Object numbers: 1 catalog, 2 pages, then per page: page, content, image.
/// `extra_catalog` goes into the catalog; `extra` objects follow the pages.
fn build(pages: &[PageSpec], extra_catalog: &str, extra: &[String]) -> Vec<u8> {
    let first_page = 3;
    let per = 3;
    let kids: Vec<String> = (0..pages.len()).map(|i| format!("{} 0 R", first_page + i * per)).collect();
    let mut pdf = Pdf::new().obj(format!("<< /Type /Catalog /Pages 2 0 R {extra_catalog} >>")).obj(format!(
        "<< /Type /Pages /Kids [{}] /Count {} >>",
        kids.join(" "),
        pages.len()
    ));
    for (i, p) in pages.iter().enumerate() {
        let n = first_page + i * per;
        let [x0, y0, x1, y1] = p.media;
        let crop = p.crop.map(|c| format!("/CropBox [{} {} {} {}]", c[0], c[1], c[2], c[3])).unwrap_or_default();
        let annots = if p.annots.is_empty() { String::new() } else { format!("/Annots [{}]", p.annots.join(" ")) };
        pdf = pdf.obj(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [{x0} {y0} {x1} {y1}] {crop} /Rotate {} /Contents {} 0 R \
             /Resources << /XObject << /Im0 {} 0 R >> /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >> {annots} >>",
            p.rotate,
            n + 1,
            n + 2
        ));
        // A page turned 90° holds its scan turned the other way, so it shows upright.
        let (w, h) = (x1 - x0, y1 - y0);
        let draw = match (&p.scan, p.rotate) {
            (Some(_), 90) => format!("q 0 {h} {} 0 {w} 0 cm /Im0 Do Q", -w),
            (Some(_), _) => format!("q {w} 0 0 {h} {x0} {y0} cm /Im0 Do Q"),
            (None, _) => "BT /F1 24 Tf 72 400 Td (A single page) Tj ET".to_string(),
        };
        pdf = pdf.stream("", draw.as_bytes());
        let data = p.scan.clone().unwrap_or_else(|| vec![255; SW * SH]);
        pdf = pdf.stream(
            &format!("/Type /XObject /Subtype /Image /Width {SW} /Height {SH} /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /ASCIIHexDecode"),
            hex(&data),
        );
    }
    for e in extra {
        pdf = pdf.obj(e);
    }
    pdf.build()
}

fn open(bytes: Vec<u8>) -> (Session, DocId) {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("spreads.pdf", None, Arc::new(bytes), None).expect("opens");
    (s, id)
}

fn cos(s: &Session, id: DocId) -> &Cos {
    &s.get(id).unwrap().editor.as_ref().unwrap().cos
}

fn crops(s: &Session, id: DocId) -> Vec<[f64; 4]> {
    let doc = cos(s, id);
    pdfcraft_model::pages(doc).iter().map(|p| p.crop(doc)).collect()
}

fn close(a: [f64; 4], b: [f64; 4], tol: f64) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tol)
}

fn annot_rects(doc: &Cos, page: usize) -> Vec<[f64; 4]> {
    let pages = pdfcraft_model::pages(doc);
    let Some(p) = pages.get(page) else { return Vec::new() };
    let list = p.dict.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
    list.iter()
        .filter_map(|a| {
            let d = doc.resolve(a);
            let r: Vec<f64> = d.as_dict()?.get(b"Rect")?.as_array()?.iter().filter_map(Object::as_f64).collect();
            <[f64; 4]>::try_from(r).ok()
        })
        .collect()
}

fn image_objects(doc: &Cos) -> usize {
    doc.object_numbers()
        .into_iter()
        .filter(|n| match &*doc.get(ObjRef::new(*n, doc.generation(*n))) {
            Object::Stream(st) => st.dict.name(b"Subtype") == Some(b"Image"),
            _ => false,
        })
        .count()
}

// ---- the gutter search on its own ----

#[test]
fn the_gutter_is_found_off_centre() {
    let g = find_gutter(&scan((0.42, 0.48), None), SW, SH);
    assert!(g.found, "{g:?}");
    assert!((g.fraction - 0.45).abs() < 0.015, "{g:?}");
    assert!(g.confidence > 0.9, "{g:?}");
}

#[test]
fn a_spine_shadow_is_no_ink() {
    // The fold's shadow fills the gutter: darkness alone would push the split onto the text.
    let g = find_gutter(&scan((0.47, 0.57), Some(0.52)), SW, SH);
    assert!(g.found, "{g:?}");
    assert!((g.fraction - 0.52).abs() < 0.015, "{g:?}");
}

#[test]
fn no_clear_gutter_falls_back_to_the_centre() {
    let blank = vec![255u8; SW * SH];
    assert_eq!(find_gutter(&blank, SW, SH), Gutter::CENTRE);
    // One column of text across the whole middle: nothing empty in the window.
    let one_column = scan((0.92, 0.92), None);
    let g = find_gutter(&one_column, SW, SH);
    assert!(!g.found && g.fraction == 0.5, "{g:?}");
    // A raster that doesn't hold what its size says is no gutter, not a panic.
    assert_eq!(find_gutter(&blank[..10], SW, SH), Gutter::CENTRE);
    assert_eq!(find_gutter(&[], 0, 0), Gutter::CENTRE);
}

#[test]
fn halves_follow_rotation_reading_order_and_margin() {
    let crop = [0.0, 0.0, 600.0, 400.0];
    let s = split_of(crop, 0, 1.0, 0, 0.25, 10.0, ReadingOrder::LeftToRight).unwrap();
    assert_eq!((s.first, s.second, s.axis, s.at, s.first_low), ([0.0, 0.0, 140.0, 400.0], [160.0, 0.0, 600.0, 400.0], SplitAxis::X, 150.0, true));
    let s = split_of(crop, 0, 1.0, 0, 0.25, 0.0, ReadingOrder::RightToLeft).unwrap();
    assert_eq!((s.first, s.second), ([150.0, 0.0, 600.0, 400.0], [0.0, 0.0, 150.0, 400.0]));
    // Upside down: the displayed left is the high x side.
    let s = split_of(crop, 180, 1.0, 0, 0.25, 0.0, ReadingOrder::LeftToRight).unwrap();
    assert_eq!((s.first, s.second), ([450.0, 0.0, 600.0, 400.0], [0.0, 0.0, 450.0, 400.0]));
    // Turned 90° clockwise: the displayed left is the bottom (low y); 270°: the top.
    let s = split_of(crop, 90, 1.0, 0, 0.25, 0.0, ReadingOrder::LeftToRight).unwrap();
    assert_eq!((s.first, s.second, s.axis), ([0.0, 0.0, 600.0, 100.0], [0.0, 100.0, 600.0, 400.0], SplitAxis::Y));
    let s = split_of(crop, 270, 1.0, 0, 0.25, 0.0, ReadingOrder::LeftToRight).unwrap();
    assert_eq!((s.first, s.second), ([0.0, 300.0, 600.0, 400.0], [0.0, 0.0, 600.0, 300.0]));
    // UserUnit 2: a 10 pt margin is 5 units.
    let s = split_of(crop, 0, 2.0, 0, 0.5, 10.0, ReadingOrder::LeftToRight).unwrap();
    assert_eq!((s.first[2], s.second[0]), (295.0, 305.0));
    // Hostile numbers are refused, not cast.
    for f in [f64::NAN, f64::INFINITY, 0.0, 1.0, -0.5] {
        assert!(split_of(crop, 0, 1.0, 0, f, 0.0, ReadingOrder::LeftToRight).is_err(), "{f}");
    }
    assert!(split_of(crop, 0, 1.0, 0, 0.5, 300.0, ReadingOrder::LeftToRight).is_err(), "a margin past the edge");
}

// ---- the whole operation ----

/// Pages: 0 a spread with an off-centre gutter and three links, 1 a portrait page, 2 a spread
/// with a spine shadow, 3 a spread on a page turned 90°, 4 a spread already cropped. Two
/// bookmarks (pages 1 and 3 of the original), page labels i, 1, 2, …, and a link on page 2
/// that goes to page 0.
fn fixture() -> Vec<u8> {
    let mut p0 = landscape(scan((0.42, 0.48), None));
    // Page objects are 3, 6, 9, 12, 15; the extra objects start at 18.
    p0.annots = vec!["18 0 R".into(), "19 0 R".into(), "20 0 R".into()];
    let portrait = PageSpec { media: [0.0, 0.0, 612.0, 792.0], crop: None, rotate: 0, scan: None, annots: Vec::new() };
    let mut p2 = landscape(scan((0.47, 0.57), Some(0.52)));
    p2.annots = vec!["21 0 R".into()];
    let turned = PageSpec { media: [0.0, 0.0, 612.0, 792.0], crop: None, rotate: 90, scan: Some(scan((0.42, 0.48), None)), annots: Vec::new() };
    let cropped = PageSpec { crop: Some([40.0, 50.0, 760.0, 562.0]), ..landscape(scan((0.42, 0.48), None)) };
    let extra = vec![
        // 18–20: links on page 0, left of the gutter (x 356), right of it, and across it.
        "<< /Type /Annot /Subtype /Link /Rect [100 300 200 320] /P 3 0 R /Border [0 0 0] /Dest [9 0 R /Fit] >>".to_string(),
        "<< /Type /Annot /Subtype /Link /Rect [500 300 600 320] /P 3 0 R /Border [0 0 0] /Dest [6 0 R /Fit] >>".to_string(),
        "<< /Type /Annot /Subtype /Link /Rect [300 300 420 320] /P 3 0 R /Border [0 0 0] /Dest [6 0 R /Fit] >>".to_string(),
        // 21: a link on page 2 to page 0.
        "<< /Type /Annot /Subtype /Link /Rect [100 100 200 120] /P 9 0 R /Border [0 0 0] /Dest [3 0 R /Fit] >>".to_string(),
        // 22–24: an outline with two bookmarks.
        "<< /Type /Outlines /First 23 0 R /Last 24 0 R /Count 2 >>".to_string(),
        "<< /Title (Chapter 1) /Parent 22 0 R /Next 24 0 R /Dest [3 0 R /Fit] >>".to_string(),
        "<< /Title (Chapter 2) /Parent 22 0 R /Prev 23 0 R /Dest [9 0 R /Fit] >>".to_string(),
    ];
    build(&[p0, portrait, p2, turned, cropped], "/Outlines 22 0 R /PageLabels << /Nums [0 << /S /r >> 1 << /S /D >>] >>", &extra)
}

/// The pages and crop boxes after the split.
fn check_crops(s: &Session, id: DocId, plans: &[SpreadPlan]) {
    let d = s.get(id).unwrap();
    assert_eq!(d.info.pages.len(), 9);
    assert_eq!(d.can_undo(), Some("Split page spreads"));
    let c = crops(s, id);
    let x = 792.0 * plans[0].fraction;
    assert!(close(c[0], [0.0, 0.0, x, 612.0], 1e-6) && close(c[1], [x, 0.0, 792.0, 612.0], 1e-6), "{c:?}");
    assert!(close(c[2], [0.0, 0.0, 612.0, 792.0], 0.0), "the portrait page keeps its box: {:?}", c[2]);
    // Turned 90°: the displayed left half is the bottom of user space.
    let y = 792.0 * plans[2].fraction;
    assert!(close(c[5], [0.0, 0.0, 612.0, y], 1e-6) && close(c[6], [0.0, y, 612.0, 792.0], 1e-6), "{:?}", &c[5..7]);
    let x = 40.0 + 720.0 * plans[3].fraction;
    assert!(close(c[7], [40.0, 50.0, x, 562.0], 1e-6) && close(c[8], [x, 50.0, 760.0, 562.0], 1e-6), "{:?}", &c[7..9]);
    // Both halves show their half the right way up, and side by side make up the page.
    for (a, b) in [(0, 1), (3, 4), (5, 6), (7, 8)] {
        let (pa, pb) = (&d.info.pages[a], &d.info.pages[b]);
        assert!(pa.height == pb.height && (pa.width + pb.width - if a == 7 { 720.0 } else { 792.0 }).abs() < 0.01, "{a}: {pa:?} {pb:?}");
    }
}

/// Left stays, right moves, across is on both.
fn check_annotations(s: &Session, id: DocId, done: &[pdfcraft_organize::SplitPage]) {
    let doc = cos(s, id);
    assert_eq!(annot_rects(doc, 0), [[100.0, 300.0, 200.0, 320.0], [300.0, 300.0, 420.0, 320.0]]);
    assert_eq!(annot_rects(doc, 1), [[500.0, 300.0, 600.0, 320.0], [300.0, 300.0, 420.0, 320.0]]);
    assert_eq!((done[0].moved, done[0].on_both), (1, 1));
    let pages = pdfcraft_model::pages(doc);
    let second = pages[1].obj;
    let annots: Vec<ObjRef> = pages[1].dict.get(b"Annots").and_then(Object::as_array).unwrap().iter().filter_map(Object::as_ref).collect();
    assert!(annots.iter().all(|a| doc.get(*a).as_dict().unwrap().reference(b"P") == Some(second)), "/P names the new page");
    assert_ne!(annots[1], ObjRef::new(20, 0), "the copy across the line is its own object");
}

/// Bookmarks, links and labels go to a split page's first half.
fn check_destinations(s: &Session, id: DocId) {
    let doc = cos(s, id);
    let pages = pdfcraft_model::pages(doc);
    let marks = pdfcraft_organize::bookmarks(doc);
    let targets: Vec<Option<usize>> = marks.iter().map(|b| b.page).collect();
    assert_eq!(targets, [Some(0), Some(3)], "{marks:?}");
    assert_eq!(annot_rects(doc, 3), [[100.0, 100.0, 200.0, 120.0]], "the link stays on the first half of its page");
    let link = pages[3].dict.get(b"Annots").and_then(Object::as_array).and_then(|a| a.first()).and_then(Object::as_ref).unwrap();
    let dest = doc.get(link).as_dict().unwrap().get(b"Dest").and_then(Object::as_array).and_then(|a| a.first()).and_then(Object::as_ref);
    assert_eq!(dest, Some(pages[0].obj), "a link to a split page goes to its first half");
    assert_eq!(pdfcraft_organize::page_labels(doc).unwrap(), ["i", "ii", "1", "2", "3", "4", "5", "6", "7"]);
}

/// The scans are shared, not copied.
fn check_shared(s: &Session, id: DocId, images_before: usize) {
    let doc = cos(s, id);
    let pages = pdfcraft_model::pages(doc);
    assert_eq!(image_objects(doc), images_before);
    for (a, b) in [(0, 1), (3, 4), (5, 6), (7, 8)] {
        assert_eq!(pages[a].dict.get(b"Contents"), pages[b].dict.get(b"Contents"), "{a}");
        assert_eq!(pages[a].dict.get(b"Resources"), pages[b].dict.get(b"Resources"), "{a}");
    }
}

#[test]
fn spreads_become_two_pages_each_and_everything_follows() {
    let (mut s, id) = open(fixture());
    let images_before = image_objects(cos(&s, id));
    let before = s.save_full_bytes(id).unwrap();
    let (plans, done) = s.split_spreads(id, &SpreadOptions::default()).unwrap();

    // The portrait page is left alone; the four spreads were split, each by its gutter.
    assert_eq!(plans.iter().map(|p| p.page).collect::<Vec<_>>(), [0, 2, 3, 4]);
    assert!(plans.iter().all(|p| p.source == SplitSource::Gutter), "{plans:?}");
    assert!((plans[0].fraction - 0.45).abs() < 0.015, "{plans:?}");
    assert!((plans[1].fraction - 0.52).abs() < 0.015, "the spine shadow's middle: {plans:?}");
    assert!((plans[2].fraction - 0.45).abs() < 0.015, "the turned page, as displayed: {plans:?}");
    // The cropped page's gutter, in its visible width: (0.45 × 792 − 40) ÷ 720.
    assert!((plans[3].fraction - (0.45 * 792.0 - 40.0) / 720.0).abs() < 0.015, "{plans:?}");
    assert_eq!(done.iter().map(|d| d.first).collect::<Vec<_>>(), [0, 3, 5, 7]);

    check_crops(&s, id, &plans);
    check_annotations(&s, id, &done);
    check_destinations(&s, id);
    check_shared(&s, id, images_before);

    // Saved and opened again, it is all still there, and the image is still stored once.
    let saved = s.save_bytes(id).unwrap();
    let (s2, id2) = open(saved.to_vec());
    assert_eq!(s2.get(id2).unwrap().info.pages.len(), 9);
    assert_eq!(crops(&s2, id2), crops(&s, id));
    assert_eq!(annot_rects(cos(&s2, id2), 1), annot_rects(cos(&s, id), 1));
    assert_eq!(pdfcraft_organize::page_labels(cos(&s2, id2)).unwrap().len(), 9);
    let full = s.save_full_bytes(id).unwrap();
    let (s3, id3) = open(full.to_vec());
    assert_eq!(image_objects(cos(&s3, id3)), images_before, "a full rewrite stores each scan once");

    // One undo puts it all back.
    s.undo(id).unwrap();
    assert_eq!(s.save_full_bytes(id).unwrap(), before);
    assert_eq!(s.get(id).unwrap().info.pages.len(), 5);
}

#[test]
fn right_to_left_puts_the_right_half_first() {
    let (mut s, id) = open(build(&[landscape(scan((0.42, 0.48), None))], "", &[]));
    let opts = SpreadOptions { position: SplitPosition::Fixed(0.5), order: ReadingOrder::RightToLeft, margin: 6.0, ..Default::default() };
    s.split_spreads(id, &opts).unwrap();
    assert_eq!(crops(&s, id), [[402.0, 0.0, 792.0, 612.0], [0.0, 0.0, 390.0, 612.0]]);
}

#[test]
fn portrait_pages_split_only_when_named_and_landscape_only_is_off() {
    let portrait = || PageSpec { media: [0.0, 0.0, 612.0, 792.0], crop: None, rotate: 0, scan: None, annots: Vec::new() };
    let (mut s, id) = open(build(&[portrait(), portrait()], "", &[]));
    let all = SpreadOptions { landscape_only: false, position: SplitPosition::Fixed(0.5), ..Default::default() };
    let e = s.split_spreads(id, &all).unwrap_err();
    assert!(e.to_string().contains("nothing was split"), "{e}");
    let named = SpreadOptions { pages: Some(vec![1]), ..all.clone() };
    assert_eq!(s.split_spreads(id, &named).unwrap().0.len(), 1);
    assert_eq!(s.get(id).unwrap().info.pages.len(), 3);
    let landscape_only = SpreadOptions { pages: Some(vec![0]), landscape_only: true, ..all };
    assert!(s.split_spreads(id, &landscape_only).is_err());
}

#[test]
fn a_failure_on_one_page_changes_nothing() {
    // A margin wider than the narrow page's half: that page fails, so the wide one isn't split either.
    let narrow = PageSpec { media: [0.0, 0.0, 300.0, 200.0], ..landscape(scan((0.42, 0.48), None)) };
    let (mut s, id) = open(build(&[landscape(scan((0.42, 0.48), None)), narrow], "", &[]));
    let before = s.save_full_bytes(id).unwrap();
    let opts = SpreadOptions { position: SplitPosition::Fixed(0.5), margin: 200.0, ..Default::default() };
    let e = s.split_spreads(id, &opts).unwrap_err();
    assert!(e.to_string().contains("page 2"), "{e}");
    assert_eq!(s.get(id).unwrap().can_undo(), None);
    assert_eq!(s.save_full_bytes(id).unwrap(), before);
}

#[test]
fn hostile_options_are_refused_with_what_is_allowed() {
    let (mut s, id) = open(build(&[landscape(scan((0.42, 0.48), None))], "", &[]));
    let bad = [
        SpreadOptions { position: SplitPosition::Fixed(f64::NAN), ..Default::default() },
        SpreadOptions { position: SplitPosition::Fixed(0.95), ..Default::default() },
        SpreadOptions { margin: -1.0, ..Default::default() },
        SpreadOptions { margin: f64::INFINITY, ..Default::default() },
        SpreadOptions { min_ratio: f64::NAN, ..Default::default() },
        SpreadOptions { pages: Some(vec![usize::MAX]), ..Default::default() },
    ];
    for o in bad {
        let e = s.split_spreads(id, &o).unwrap_err().to_string();
        assert!(e.contains("between") || e.contains("out of range"), "{o:?}: {e}");
    }
    // The edit itself refuses what the planner would never send.
    for splits in [vec![(0, f64::NAN)], vec![(0, 0.5), (0, 0.5)], vec![(7, 0.5)]] {
        assert!(
            s.apply(id, crate::Edit::SplitSpreads { splits: splits.clone(), margin: 0.0, order: ReadingOrder::LeftToRight }).is_err(),
            "{splits:?}"
        );
    }
    assert_eq!(s.get(id).unwrap().info.pages.len(), 1);
}
