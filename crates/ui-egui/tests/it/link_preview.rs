//! The reference preview: a right-click with the Select tool on a link to a place in this document
//! shows that place by the link; another right-click, Escape or a click on it closes it.

use egui::Pos2;
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{PdfCraftApp, QuickTool};

/// Two letter pages. Page 1 has a link to page 2 at `/XYZ 72 500` (object 6) and a web link
/// (object 7); page 2 has a black bar just below the destination's line.
fn fixture() -> Vec<u8> {
    let bar = "0 g 100 470 200 20 re f";
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 612 792] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [6 0 R 7 0 R] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>".into(),
        format!("<< /Length {} >>\nstream\n{bar}\nendstream", bar.len()),
        "<< /Type /Annot /Subtype /Link /Rect [72 700 300 716] /Border [0 0 0] /Dest [4 0 R /XYZ 72 500 0] >>".into(),
        "<< /Type /Annot /Subtype /Link /Rect [72 640 300 656] /Border [0 0 0] /A << /S /URI /URI (https://example.com) >> >>".into(),
    ];
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        pdf.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    pdf
}

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).with_step_dt(1.0 / 60.0).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("refs.pdf", None, fixture()).unwrap();
        app.set_option("zoom", "100").unwrap();
        app
    });
    h.run_steps(6);
    h
}

/// The point (x, y), in points from the top-left of page 1, on screen.
fn on_page(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> Pos2 {
    let r = h.state().views[0].page_screen_rect(0).unwrap();
    let k = r.width() / 612.0;
    r.min + egui::vec2(x * k, y * k)
}

/// Inside the in-document link (user rect 72 700 300 716: 76 to 92 points from the top).
fn the_link(h: &Harness<'static, PdfCraftApp>) -> Pos2 {
    on_page(h, 150.0, 84.0)
}

fn right_click(h: &mut Harness<'static, PdfCraftApp>, p: Pos2) {
    h.hover_at(p);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed: true, modifiers: Default::default() });
    h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed: false, modifiers: Default::default() });
    h.run_steps(3);
}

/// Run frames until the preview's picture has come back from its render thread.
fn wait_drawn(h: &mut Harness<'static, PdfCraftApp>) {
    for _ in 0..600 {
        let peek = h.state().views[0].link_peek.as_ref().expect("the preview is open");
        assert!(!peek.failed(), "the preview could not be drawn");
        if peek.drawn() {
            h.run_steps(1);
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.step();
    }
    panic!("the preview was not drawn");
}

/// The preview window's picture, by its accessible name.
fn picture(h: &Harness<'static, PdfCraftApp>) -> Option<egui::Rect> {
    h.query_by_role_and_label(Role::Image, "Go to page 2").map(|n| n.rect())
}

#[test]
fn a_right_click_on_a_link_previews_its_destination_and_another_closes_it() {
    let mut h = harness();
    let link = the_link(&h);
    right_click(&mut h, link);
    let peek = h.state().views[0].link_peek.as_ref().expect("the right-click opened the preview");
    assert_eq!((peek.source(), peek.preview().page), ((0, 0), 1));
    // The band starts 14 points above the destination's line, 292 points down page 2.
    assert!((peek.preview().area[1] - 278.0).abs() < 0.01, "{:?}", peek.preview());
    // The right-click was the preview's: neither the link's menu nor the page's opened.
    assert!(h.query_by_label("Remove all links").is_none() && h.query_by_label("Select Tool").is_none(), "no menu");
    wait_drawn(&mut h);
    let shown = picture(&h).expect("the preview is on screen");
    // Below the link, under the window's caption.
    assert!(shown.top() > link.y && shown.top() - link.y < 70.0, "below the link: {shown:?}, link at {link:?}");
    // At the page's zoom: the band is as wide as the page on screen, its text the size of the page's.
    let page_width = h.state().views[0].page_screen_rect(0).unwrap().width();
    assert!((shown.width() - page_width).abs() < 1.0, "{shown:?} vs a page {page_width} wide");
    // The page stays where it was.
    assert_eq!(h.state().views[0].current, 0);
    // Another right-click, anywhere on the pages, closes it, and opens no menu.
    let away = on_page(&h, 400.0, 500.0);
    right_click(&mut h, away);
    assert!(h.state().views[0].link_peek.is_none());
    assert!(picture(&h).is_none());
    assert!(h.query_by_label("Select Tool").is_none(), "the closing right-click opens no menu");
    // The next right-click away from links is the page's again.
    right_click(&mut h, away);
    h.get_by_label("Select Tool");
}

#[test]
fn a_right_click_on_the_preview_or_escape_closes_it() {
    let mut h = harness();
    let link = the_link(&h);
    right_click(&mut h, link);
    wait_drawn(&mut h);
    let shown = picture(&h).unwrap();
    right_click(&mut h, shown.center());
    assert!(h.state().views[0].link_peek.is_none(), "a right-click on the window closes it");
    right_click(&mut h, link);
    assert!(h.state().views[0].link_peek.is_some());
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.state().views[0].link_peek.is_none(), "Escape closes it");
}

#[test]
fn a_click_on_the_preview_goes_to_the_destination() {
    let mut h = harness();
    let link = the_link(&h);
    right_click(&mut h, link);
    wait_drawn(&mut h);
    let shown = picture(&h).unwrap();
    h.hover_at(shown.center());
    h.run_steps(1);
    h.get_by_role_and_label(Role::Image, "Go to page 2").click();
    h.run_steps(4);
    assert!(h.state().views[0].link_peek.is_none());
    assert_eq!(h.state().views[0].current, 1, "on page 2");
}

#[test]
fn a_web_link_and_the_link_tool_keep_the_link_menu() {
    let mut h = harness();
    // A web link has nothing in this document to show: its menu, as before.
    let web = on_page(&h, 150.0, 144.0);
    right_click(&mut h, web);
    assert!(h.state().views[0].link_peek.is_none());
    h.get_by_label("Remove all links");
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    // The Link tool edits links: an in-document one gets the menu there too.
    h.state_mut().quick_tool = QuickTool::Link;
    h.run_steps(2);
    let link = the_link(&h);
    right_click(&mut h, link);
    assert!(h.state().views[0].link_peek.is_none());
    h.get_by_label("Remove all links");
}

#[test]
fn an_edit_closes_the_preview() {
    let mut h = harness();
    let link = the_link(&h);
    right_click(&mut h, link);
    assert!(h.state().views[0].link_peek.is_some());
    assert!(h.state_mut().execute("page.rotate"), "a page command ran");
    h.run_steps(4);
    assert!(h.state().views[0].link_peek.is_none(), "the pages it was drawn from changed");
}

/// Wait until the preview's render thread has exited.
fn wait_stopped(watch: &pdfcraft_render::PoolWatch, what: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !watch.stopped() {
        assert!(std::time::Instant::now() < deadline, "{what}: the preview's render thread never exited");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

/// The preview's render thread starts with the preview, not with the document, and exits, letting
/// go of the document, when the preview closes, when its document closes, and when the app goes.
#[test]
fn the_previews_render_thread_ends_with_the_preview_its_document_or_the_app() {
    let mut h = harness();
    assert!(h.state().views[0].link_peek.is_none(), "no preview, so no render thread of its own, on open");
    let link = the_link(&h);
    right_click(&mut h, link);
    wait_drawn(&mut h);
    let watch = h.state().views[0].link_peek.as_ref().unwrap().render_watch();
    assert!(!watch.stopped());
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.state().views[0].link_peek.is_none());
    wait_stopped(&watch, "closing the preview");

    right_click(&mut h, link);
    wait_drawn(&mut h);
    let watch = h.state().views[0].link_peek.as_ref().unwrap().render_watch();
    h.state_mut().close_tab(0);
    h.run_steps(2);
    wait_stopped(&watch, "closing its document");

    h.state_mut().open_bytes("refs.pdf", None, fixture()).unwrap();
    h.state_mut().set_option("zoom", "100").unwrap();
    h.run_steps(6);
    let link = the_link(&h);
    right_click(&mut h, link);
    wait_drawn(&mut h);
    let watch = h.state().views[0].link_peek.as_ref().unwrap().render_watch();
    drop(h);
    wait_stopped(&watch, "the app going");
}
