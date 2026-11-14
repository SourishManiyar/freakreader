//! Putting a place on a page into view before a press lands on it, without assuming a window
//! size or a scroll position: at fit width a page can be taller than the window, so content low on
//! it starts below the pages area. Each helper reads the pages area it scrolls within
//! ([`pdfcraft_ui_egui::canvas::DocView::viewport_rect`]) instead of pixel constants.

use egui::Pos2;
use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

/// How far inside the pages area a point must be to count as in view, in points: clear of its
/// edges, where scroll bars and overlays sit.
const IN_VIEW_MARGIN: f32 = 40.0;

/// Scroll the first tab's page `page` so the point `frac` of its height below its top is at the top
/// of the pages area (as far as the scroll range allows).
pub fn scroll_to_page_fraction(h: &mut Harness<'static, PdfCraftApp>, page: usize, frac: f32) {
    h.state_mut().views[0].goto = Some((page, frac));
    h.run_steps(3);
}

/// The middle of the part of the first tab's page `page` that is inside the pages area, for a
/// press that must land on the page and on screen.
pub fn visible_page_point(h: &Harness<'static, PdfCraftApp>, page: usize) -> Pos2 {
    let view = &h.state().views[0];
    let visible = view.page_screen_rect(page).expect("the page is laid out").intersect(view.viewport_rect());
    assert!(visible.is_positive(), "page {page} is on screen: {visible:?}");
    visible.center()
}

/// The point `user` (user space) of the first tab's page `page` on screen, as it is laid out now.
pub fn page_point(h: &Harness<'static, PdfCraftApp>, page: usize, user: [f64; 2]) -> Pos2 {
    let app = h.state();
    let info = &app.session.get(app.views[0].id).expect("the document is open").info.pages[page];
    let v = info.user_to_view(user[0] as f32, user[1] as f32);
    app.views[0].page_xform(page).expect("the page is laid out").norm_to_screen(v[0] / info.width, v[1] / info.height)
}

/// [`page_point`], first scrolling the page so the point sits a third of the way down the pages
/// area when it is not well inside it. Fails the test when it still isn't.
pub fn page_point_in_view(h: &mut Harness<'static, PdfCraftApp>, page: usize, user: [f64; 2]) -> Pos2 {
    let in_view = |h: &Harness<'static, PdfCraftApp>, p: Pos2| h.state().views[0].viewport_rect().shrink(IN_VIEW_MARGIN).contains(p);
    let p = page_point(h, page, user);
    if in_view(h, p) {
        return p;
    }
    let (frac, back) = {
        let app = h.state();
        let view = &app.views[0];
        let info = &app.session.get(view.id).expect("the document is open").info.pages[page];
        let v = info.user_to_view(user[0] as f32, user[1] as f32);
        let page_height = view.page_screen_rect(page).expect("the page is laid out").height().max(1.0);
        (v[1] / info.height, view.viewport_rect().height() / 3.0 / page_height)
    };
    scroll_to_page_fraction(h, page, (frac - back).max(0.0));
    super::settle(h);
    let p = page_point(h, page, user);
    assert!(in_view(h, p), "{user:?} on page {page} scrolled into the pages area: {p:?} in {:?}", h.state().views[0].viewport_rect());
    p
}
