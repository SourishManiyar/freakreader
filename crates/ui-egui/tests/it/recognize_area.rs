//! Edit ▸ Recognize text in area end to end: drag a rectangle over a scan, and the words
//! recognised there are on the clipboard with a notice naming how many and the lowest confidence
//! band; over a page with its own text, that text is copied instead; Escape finishes the tool; a
//! document that withholds copying reads nothing.

use egui_kittest::Harness;
use pdfcraft_engine::{ImageResolution, Session};
use pdfcraft_ui_egui::{PdfCraftApp, QuickTool};

/// A typed page holding `text`, and the same page scanned at 300 dpi (an image, no text).
fn pages(text: &str) -> (Vec<u8>, Vec<u8>) {
    let mut s = Session::new();
    let typed = s.create_from_text("t", text).unwrap();
    let id = s.open("typed.pdf", None, typed.clone(), None).unwrap();
    let png = pdfcraft_engine::export::Exporter::new(s.get(id).unwrap()).png(0, 300.0).unwrap();
    let scan = s.create_from_images_with_resolution(&[("scan.png".into(), png)], ImageResolution::Dpi(300.0)).unwrap();
    (typed.to_vec(), scan.to_vec())
}

fn harness(name: &str, bytes: Vec<u8>) -> Harness<'static, PdfCraftApp> {
    let name = name.to_string();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes(&name, None, bytes).expect("opens");
        app.set_option("left", "closed").unwrap();
        app.system_clipboard = false;
        app
    });
    crate::support::settle(&mut h);
    h
}

fn notice(h: &Harness<'static, PdfCraftApp>) -> String {
    h.state().toast.as_ref().map(|t| t.0.clone()).unwrap_or_default()
}

/// Drag across the top quarter of page 1 (where the typed line is), then wait for the answer.
/// Returns every text put on the clipboard meanwhile.
fn drag_top(h: &mut Harness<'static, PdfCraftApp>) -> Vec<String> {
    let shown = crate::support::step_until(h, |h| h.state().views[0].page_screen_rect(0).is_some() && !h.state().render_pending());
    assert!(shown, "page 1 on screen");
    h.run_steps(2);
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let at = |fx: f32, fy: f32| egui::pos2(r.left() + fx * r.width(), r.top() + fy * r.height());
    let (a, b) = (at(0.03, 0.02), at(0.97, 0.25));
    let mut copied = Vec::new();
    let mut take = |h: &Harness<'static, PdfCraftApp>| {
        for c in &h.output().platform_output.commands {
            if let egui::OutputCommand::CopyText(t) = c {
                copied.push(t.clone());
            }
        }
    };
    h.hover_at(a);
    h.run_steps(1);
    h.drag_at(a);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(a + (b - a) * (k as f32 / 4.0));
        h.run();
        take(h);
    }
    h.drop_at(b);
    h.step();
    take(h);
    let answered = crate::support::step_until(h, |h| {
        take(h);
        !h.state().area_ocr_running()
    });
    assert!(answered, "the area was read");
    h.step();
    take(h);
    copied
}

#[test]
fn an_area_of_a_scan_is_recognised_onto_the_clipboard() {
    if pdfcraft_engine::ocr::Models::find().is_none() {
        eprintln!("skipped: OCR models not installed");
        return;
    }
    let (_, scan) = pages("Hölder and Lévy bounds");
    let mut h = harness("scan.pdf", scan);
    let before = h.state().session.get(h.state().views[0].id).unwrap().bytes.clone();
    assert!(h.state_mut().execute("edit.recognize_area"));
    assert_eq!(h.state().quick_tool, QuickTool::RecognizeArea);
    assert_eq!(notice(&h), "Drag a rectangle around the text to recognise. Esc to finish.");
    let copied = drag_top(&mut h);
    assert_eq!(copied, vec!["Hölder and Lévy bounds".to_string()]);
    let said = notice(&h);
    assert!(said.starts_with("Copied 4 recognised words (lowest confidence: "), "{said}");
    let after = h.state().session.get(h.state().views[0].id).unwrap().bytes.clone();
    assert!(std::sync::Arc::ptr_eq(&before, &after), "the document is not changed");
    assert_eq!(h.state().quick_tool, QuickTool::RecognizeArea, "the tool stays for the next area");
}

#[test]
fn a_page_with_its_own_text_copies_that_text() {
    let (typed, _) = pages("Already selectable words");
    let mut h = harness("typed.pdf", typed);
    assert!(h.state_mut().execute("edit.recognize_area"));
    let copied = drag_top(&mut h);
    assert_eq!(copied, vec!["Already selectable words".to_string()], "{}", notice(&h));
    assert_eq!(notice(&h), "The page already has selectable text here: copied its 3 words without recognising them.");
}

#[test]
fn a_document_that_withholds_copying_is_not_read() {
    let (typed, _) = pages("Secret words");
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(typed)).unwrap();
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: "",
        owner_password: "owner",
        permissions: -1 ^ 0b1_0000,
        encrypt_metadata: true,
        seed: [4; 32],
    })
    .unwrap();
    let mut h = harness("locked.pdf", pdfcraft_cos::write_full(&doc, &Default::default()).unwrap());
    // The command is disabled; the tool chosen on another tab still refuses here.
    assert!(!h.state_mut().execute("edit.recognize_area"));
    h.state_mut().set_option("quick", "recognize-area").unwrap();
    let copied = drag_top(&mut h);
    assert!(copied.is_empty(), "{copied:?}");
    assert_eq!(notice(&h), "The document's security settings don't allow copying text and images");
}

/// Escape finishes the tool, back to selecting; V does too.
#[test]
fn escape_finishes_recognize_text_in_area() {
    let (typed, _) = pages("x");
    let mut h = harness("typed.pdf", typed);
    for key in [egui::Key::Escape, egui::Key::V] {
        assert!(h.state_mut().execute("edit.recognize_area"));
        h.run_steps(2);
        assert_eq!(h.state().quick_tool, QuickTool::RecognizeArea);
        h.key_press(key);
        h.run_steps(2);
        assert_eq!(h.state().quick_tool, QuickTool::Select, "{key:?}");
        assert!(!h.state().area_ocr_running());
    }
}
