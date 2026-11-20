//! Splitting pages in two: where each kind of annotation goes, what is reported, page labels,
//! and refusals that leave the document as it was.

use std::sync::Arc;

use pdfcraft_cos::{Document, ObjRef, Object, SaveOptions, write_full};

use super::*;

fn serialise(objs: &[&str]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// Page 3 is an 800 × 400 spread whose split line is x = 400; page 4 a plain page.
fn fixture() -> Document {
    let bytes = serialise(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [10 0 R 6 0 R] >> /PageLabels << /Nums [0 << /S /D >> 1 << /S /r >>] >> >>", // 1
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 800 400] >>",                                                        // 2
        "<< /Type /Page /Parent 2 0 R /Contents 13 0 R /StructParents 3 /B [14 0 R] \
         /Annots [5 0 R 6 0 R 7 0 R 8 0 R 9 0 R 11 0 R << /Subtype /Link /Rect [500 10 520 20] /P 3 0 R >> 12 0 R] >>", // 3
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] >>",                                                                        // 4
        "<< /Type /Annot /Subtype /Widget /Parent 10 0 R /Rect [350 100 450 120] /P 3 0 R /StructParent 7 >>",                            // 5
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (solo) /Rect [380 200 420 220] /P 3 0 R >>",                                         // 6
        "<< /Type /Annot /Subtype /Text /Rect [600 300 620 320] /P 3 0 R /Popup 8 0 R >>",                                                // 7
        "<< /Type /Annot /Subtype /Popup /Rect [100 300 300 380] /P 3 0 R /Parent 7 0 R >>",                                              // 8
        "<< /Type /Annot /Subtype /Square /Rect [390 50 410 60] /P 3 0 R /Popup 11 0 R >>",                                               // 9
        "<< /FT /Btn /T (group) /Kids [5 0 R] >>",                                                                                        // 10
        "<< /Type /Annot /Subtype /Popup /Rect [600 50 700 90] /P 3 0 R /Parent 9 0 R >>",                                                // 11
        "<< /Type /Annot /Subtype /Text /P 3 0 R >>",                                                                                     // 12
        "<< /Length 0 >>\nstream\n\nendstream",                                                                                           // 13
        "<< /T 15 0 R /P 3 0 R /R [0 0 800 400] >>",                                                                                      // 14
    ]);
    Document::open(Arc::new(bytes)).unwrap()
}

fn spread(page: usize) -> PageSplit {
    PageSplit { page, first: [0.0, 0.0, 400.0, 400.0], second: [400.0, 0.0, 800.0, 400.0], axis: SplitAxis::X, at: 400.0, first_low: true }
}

fn annots(doc: &Document, page: usize) -> Vec<Object> {
    let p = crate::pages(doc).unwrap()[page].obj;
    doc.get(p).as_dict().unwrap().get(b"Annots").and_then(Object::as_array).cloned().unwrap_or_default()
}

fn r(n: u32) -> Object {
    Object::Ref(ObjRef::new(n, 0))
}

fn dict(doc: &Document, o: &Object) -> pdfcraft_cos::Dict {
    doc.resolve(o).as_dict().cloned().unwrap()
}

#[test]
fn each_annotation_goes_to_the_half_that_holds_it() {
    let mut doc = fixture();
    let out = split_pages_in_two(&mut doc, &[spread(0)]).unwrap();
    let pages = crate::pages(&doc).unwrap();
    assert_eq!(pages.len(), 3);
    let (first, second) = (pages[0].obj, pages[1].obj);
    assert_eq!(first, ObjRef::new(3, 0), "the page object stays as the first half");

    // Across the line or without a /Rect: on the first half still.
    assert_eq!(annots(&doc, 0), [r(5), r(6), r(9), r(11), r(12)]);
    let given = annots(&doc, 1);
    assert_eq!(given.len(), 6, "{given:?}");
    // The widget across the line: one more widget of its field, without the original's structure key.
    let widget = given[0].as_ref().unwrap();
    let w = dict(&doc, &given[0]);
    assert_eq!((w.reference(b"Parent"), w.reference(b"P"), w.contains(b"StructParent")), (Some(ObjRef::new(10, 0)), Some(second), false));
    let kids = dict(&doc, &r(10)).get(b"Kids").and_then(Object::as_array).cloned().unwrap();
    assert_eq!(kids, [r(5), Object::Ref(widget)]);
    // The note on the right and its popup (drawn on the left) move together, /P following.
    assert_eq!(&given[1..3], [r(7), r(8)]);
    assert_eq!(dict(&doc, &r(7)).reference(b"P"), Some(second));
    assert_eq!(dict(&doc, &r(8)).reference(b"P"), Some(second));
    // The square across the line: a copy, with its own copy of the popup, linked both ways.
    let (copy, popup) = (given[3].as_ref().unwrap(), given[4].as_ref().unwrap());
    assert!(copy != ObjRef::new(9, 0) && popup != ObjRef::new(11, 0));
    assert_eq!(doc.get(copy).as_dict().unwrap().reference(b"Popup"), Some(popup));
    assert_eq!(doc.get(popup).as_dict().unwrap().reference(b"Parent"), Some(copy));
    assert_eq!(dict(&doc, &r(9)).reference(b"Popup"), Some(ObjRef::new(11, 0)), "the original keeps its own popup");
    // A direct link dictionary moves as it is.
    assert_eq!(dict(&doc, &given[5]).reference(b"P"), Some(second));

    // Crop boxes, and what the second half doesn't carry, said.
    let crop = |p: ObjRef| doc.get(p).as_dict().unwrap().get(b"CropBox").cloned();
    assert_eq!(crop(first), Some(rect_obj([0.0, 0.0, 400.0, 400.0])));
    assert_eq!(crop(second), Some(rect_obj([400.0, 0.0, 800.0, 400.0])));
    let new = doc.get(second).as_dict().cloned().unwrap();
    assert!(!new.contains(b"StructParents") && !new.contains(b"B") && new.get(b"MediaBox").is_some());
    assert_eq!(new.get(b"Contents"), Some(&r(13)), "the content is shared");
    let o = &out[0];
    assert_eq!((o.page, o.first, o.moved, o.on_both), (0, 0, 3, 2));
    let said = o.first_only.join("; ");
    for what in ["article threads", "tagged structure", "without a usable /Rect", "its own widget"] {
        assert!(said.contains(what), "{what}: {said}");
    }
    // The labels run on through the new page; the next range starts one page later.
    assert_eq!(crate::page_labels(&doc).unwrap(), ["1", "2", "i"]);
}

#[test]
fn refusals_leave_the_document_as_it_was() {
    let base = fixture();
    let before = write_full(&base, &SaveOptions::default()).unwrap();
    let narrow = PageSplit { second: [400.0, 0.0, 400.5, 400.0], ..spread(0) };
    let nan = PageSplit { at: f64::NAN, ..spread(0) };
    let inf = PageSplit { first: [0.0, 0.0, f64::INFINITY, 400.0], ..spread(0) };
    let cases: Vec<(Vec<PageSplit>, &str)> = vec![
        (vec![], "at least one page"),
        (vec![spread(0), spread(0)], "listed twice"),
        (vec![spread(5)], "does not exist"),
        (vec![spread(1), narrow], "smaller than a point"),
        (vec![nan], "not a number"),
        (vec![inf], "not a number"),
        (vec![spread(0); MAX_SPLIT_PAGES + 1], "at most"),
    ];
    for (splits, why) in cases {
        let mut doc = base.clone();
        let e = split_pages_in_two(&mut doc, &splits).unwrap_err().to_string();
        assert!(e.contains(why), "{why}: {e}");
        assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before, "{why}");
    }
}

#[test]
fn several_pages_split_and_their_labels_shift_together() {
    let mut doc = fixture();
    let second = PageSplit { first: [0.0, 0.0, 200.0, 400.0], second: [200.0, 0.0, 400.0, 400.0], at: 200.0, ..spread(1) };
    let out = split_pages_in_two(&mut doc, &[spread(0), second]).unwrap();
    assert_eq!(out.iter().map(|o| o.first).collect::<Vec<_>>(), [0, 2]);
    assert_eq!(crate::page_count(&doc).unwrap(), 4);
    assert_eq!(crate::page_labels(&doc).unwrap(), ["1", "2", "i", "ii"]);
}
