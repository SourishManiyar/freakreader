//! Previews of references the text names without a link ("by Lemma 1", "(2)"): resting the
//! pointer on such a mention, or a right-click on it, shows the block it names, captioned
//! "inferred"; a mention of another work shows nothing; the preference turns them off; a button
//! held down (selecting) opens none. The paper is the reference oracle's `art10_traps` (no links).

use egui::Pos2;
use egui_kittest::Harness;
use pdfcraft_engine::references::{ReferenceGraph, Source};
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::canvas::link_peek::{HOVER_DELAY, PeekSource, Trigger};

fn harness() -> Harness<'static, PdfCraftApp> {
    let path = format!("{}/../render/tests/data/refs/oracle/art10_traps.pdf", env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(path).expect("the oracle paper is committed");
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("traps.pdf", None, bytes.clone()).unwrap();
        app.set_option("zoom", "100").unwrap();
        app
    });
    h.run_steps(6);
    h
}

/// Frames until the document's reference graph is built (in the background).
fn graph(h: &mut Harness<'static, PdfCraftApp>) -> std::sync::Arc<ReferenceGraph> {
    for _ in 0..1000 {
        let app = h.state();
        let generation = app.session.get(app.views[0].id).map(|d| d.edit_generation()).unwrap();
        if let Some(g) = app.views[0].refs.graph(generation) {
            return g.clone();
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.step();
    }
    panic!("the reference graph was not built");
}

/// The middle of the first inferred mention on page 1 saying `text`, on screen.
fn mention(h: &Harness<'static, PdfCraftApp>, g: &ReferenceGraph, text: &str) -> (usize, Pos2) {
    let m = g.mentions.iter().find(|m| m.text == text && m.page() == 0 && m.source == Source::Inferred).unwrap_or_else(|| panic!("{text} on page 1"));
    let r = h.state().views[0].page_screen_rect(0).unwrap();
    let k = r.width() / 612.0;
    let e = m.extent[0].rect;
    (m.id, r.min + egui::vec2((e[0] + e[2]) / 2.0 * k, (e[1] + e[3]) / 2.0 * k))
}

fn rest(h: &mut Harness<'static, PdfCraftApp>, p: Pos2) {
    h.hover_at(p);
    h.run_steps(((HOVER_DELAY * 60.0) as usize) + 4);
}

#[test]
fn resting_on_a_mention_previews_the_block_it_names() {
    let mut h = harness();
    let g = graph(&mut h);
    for (text, title) in [("Lemma 1", "Lemma 1"), ("(2)", "Equation (2)")] {
        let (id, at) = mention(&h, &g, text);
        rest(&mut h, at);
        let peek = h.state().views[0].link_peek.as_ref().unwrap_or_else(|| panic!("{text}: no preview"));
        assert_eq!(peek.source(), PeekSource::Mention { page: 0, id });
        assert_eq!(peek.trigger(), Trigger::Hover);
        assert_eq!(peek.inferred(), Some(title));
        assert!(peek.caption().ends_with("· inferred"), "{}", peek.caption());
        assert!(peek.caption().starts_with(title), "{}", peek.caption());
        // The block's own extent: the statement or the display, not a band of the page.
        let block = g.block(g.mentions[id].target.unwrap()).unwrap();
        let [x0, y0, x1, y1] = block.first_rect();
        let a = peek.preview().area;
        assert!(a[0] <= x0 && a[1] <= y0 && a[2] >= x1 && a[3] >= y1 && a[3] - a[1] < y1 - y0 + 20.0, "{a:?} for {:?}", block.first_rect());
        // Leaving it closes it.
        h.hover_at(Pos2::new(5.0, 5.0));
        h.run_steps(40);
        assert!(h.state().views[0].link_peek.is_none(), "{text}: still open");
    }
}

#[test]
fn another_works_mention_and_a_held_button_preview_nothing() {
    let mut h = harness();
    let g = graph(&mut h);
    // `Theorem 1 ... in [n]` names another work: no target, no preview.
    let external = g.mentions.iter().find(|m| m.unresolved == Some("external")).expect("an external mention");
    assert!(external.target.is_none());
    if external.page() == 0 {
        let r = h.state().views[0].page_screen_rect(0).unwrap();
        let k = r.width() / 612.0;
        let e = external.extent[0].rect;
        rest(&mut h, r.min + egui::vec2((e[0] + e[2]) / 2.0 * k, (e[1] + e[3]) / 2.0 * k));
        assert!(h.state().views[0].link_peek.is_none());
    }
    // A button held down (a selection being dragged) opens none.
    let (_, at) = mention(&h, &g, "Lemma 1");
    h.hover_at(at);
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.run_steps(((HOVER_DELAY * 60.0) as usize) + 4);
    assert!(h.state().views[0].link_peek.is_none(), "no preview while a button is down");
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
}

#[test]
fn the_preference_turns_them_off_and_a_right_click_opens_one() {
    let mut h = harness();
    let g = graph(&mut h);
    let (id, at) = mention(&h, &g, "Lemma 1");
    h.state_mut().set_option("inferred-previews", "off").unwrap();
    assert!(h.state_mut().set_option("inferred-previews", "maybe").is_err());
    rest(&mut h, at);
    assert!(h.state().views[0].link_peek.is_none(), "off: no preview");
    h.state_mut().set_option("inferred-previews", "on").unwrap();
    h.hover_at(Pos2::new(5.0, 5.0));
    h.run_steps(30);
    // A right-click on the mention opens its preview, which stays.
    h.hover_at(at);
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: true, modifiers: Default::default() });
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: false, modifiers: Default::default() });
    h.run_steps(3);
    let peek = h.state().views[0].link_peek.as_ref().expect("the right-click opened the preview");
    assert_eq!((peek.source(), peek.trigger()), (PeekSource::Mention { page: 0, id }, Trigger::Click));
    h.hover_at(Pos2::new(5.0, 5.0));
    h.run_steps(40);
    assert!(h.state().views[0].link_peek.is_some(), "a right-click's preview stays");
}
