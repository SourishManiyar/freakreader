//! Edit ▸ Copy as LaTeX: drag a rectangle around a typeset formula; its LaTeX goes on the clipboard
//! (`pdfcraft_engine::math_recover`, the same reader as the `text_recover_latex` tool).
//!
//! The rectangle is the Marquee Zoom / Snapshot gesture ([`crate::zoom_snap::page_input`]). The
//! formula is read off the UI thread, like a whole-document copy ([`crate::DocumentCopy`]): a newer
//! drag cancels the older one, closing the document drops it, and the area stays outlined with a
//! spinner while it runs. The page's glyphs are kept for its next rectangle, so a second formula of
//! the same page, and an area with nothing in it, answer without reading the page again.
//!
//! What was copied is said in the toast, and how sure the reader is: a tentative answer is copied
//! with "low confidence" and the reason, never as a certain one, and when nothing is recognised
//! nothing is copied and the clipboard keeps what it held.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak, mpsc};

use egui::{Color32, CornerRadius, Stroke};
use pdfcraft_engine::DocId;
use pdfcraft_engine::math_recover::{self, Wrapper};
use pdfcraft_render::text::math::MathPage;
use pdfcraft_render::text::math::recover::{Confidence, ErrorKind, RecoverError, RecoverOptions, Recovered, Severity};

use crate::PdfCraftApp;
use crate::canvas::{DocView, PageXform};
use crate::page_cursor::Layer;
use crate::page_pointer::PagePointer;

/// The longest LaTeX the toast shows, in characters; the clipboard has all of it.
const PREVIEW_CHARS: usize = 60;
/// The longest reason the toast gives, in characters.
const REASON_CHARS: usize = 120;
/// The smallest rectangle taken as a drag, in points of the page: less is a click.
const MIN_SIDE: f32 = 1.0;

/// What the reader answered for one rectangle: `Err` is a page that could not be read or a panic
/// caught on the way; the inner result is the formula or why it was not read.
pub(crate) type Answer = Result<Result<Recovered, RecoverError>, String>;

/// One page's glyphs and rules, as read for the last rectangle: which document, page and snapshot
/// of its bytes they are of. The bytes are held weakly, so an edit (new bytes) makes them stale
/// without keeping the old bytes alive.
pub(crate) struct PageGlyphs {
    doc: DocId,
    page: usize,
    bytes: Weak<Vec<u8>>,
    glyphs: Arc<MathPage>,
}

impl PageGlyphs {
    fn of(&self, doc: DocId, page: usize, bytes: &Arc<Vec<u8>>) -> Option<Arc<MathPage>> {
        (self.doc == doc && self.page == page && Weak::ptr_eq(&self.bytes, &Arc::downgrade(bytes))).then(|| self.glyphs.clone())
    }
}

/// What the worker sends back: the answer, and the page's glyphs when it read them.
pub(crate) struct Finished {
    answer: Answer,
    read: Option<PageGlyphs>,
}

/// A formula being read off the UI thread. Dropping it cancels the work.
pub(crate) struct LatexCopy {
    doc: DocId,
    rx: mpsc::Receiver<Finished>,
    cancel: Arc<AtomicBool>,
}

impl Drop for LatexCopy {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The work of one rectangle: read the page unless its glyphs are known, then the formula.
/// Stops before the second step when cancelled; `None` then.
fn read(
    doc: DocId,
    page: usize,
    region: [f32; 4],
    source: pdfcraft_engine::export::ExportSource,
    known: Option<Arc<MathPage>>,
    stop: &AtomicBool,
) -> Option<Finished> {
    let (glyphs, read) = match known {
        Some(g) => (g, None),
        None => {
            let bytes = Arc::downgrade(&source.bytes);
            match math_recover::page_glyphs(source, page) {
                Ok(g) => {
                    let g = Arc::new(g);
                    (g.clone(), Some(PageGlyphs { doc, page, bytes, glyphs: g }))
                }
                Err(e) => return Some(Finished { answer: Err(e), read: None }),
            }
        }
    };
    if stop.load(Ordering::Relaxed) {
        return None;
    }
    Some(Finished { answer: math_recover::recover_in_page(&glyphs, region, RecoverOptions::default()), read })
}

/// Whether some glyph the reader would take lies in `region`: its origin is inside and it is not
/// invisible (an OCR layer). Without one the reader answers "no glyph" at once, so it is asked on the
/// UI thread.
fn any_glyph_in(page: &MathPage, region: [f32; 4]) -> bool {
    let [x0, y0, x1, y1] = region;
    page.glyphs.iter().any(|g| {
        let [x, y] = g.baseline[0];
        !g.invisible && (x0..=x1).contains(&x) && (y0..=y1).contains(&y)
    })
}

/// The area being read stays outlined on its page, with a spinner beside it, whatever the tool now
/// is; the pointer over it shows that work is under way.
pub(crate) fn page_input(ui: &egui::Ui, xf: &PageXform, ptr: &PagePointer, page: usize, view: &mut DocView) {
    let Some((p, area)) = view.latex_pending else { return };
    if p != page || !area.iter().all(|v| v.is_finite()) {
        return;
    }
    let r = xf.view_rect(area);
    ui.painter().rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.5, Color32::from_rgb(0x14, 0x73, 0xE6)), egui::StrokeKind::Middle);
    let side = 16.0;
    let at = egui::Rect::from_center_size(egui::pos2(r.right() + 6.0 + side / 2.0, r.center().y), egui::vec2(side, side));
    egui::Spinner::new().size(side).paint_at(ui, at);
    if ptr.pos.is_some_and(|p| r.contains(p) || at.contains(p)) {
        view.cursor.claim(Layer::ToolArea, egui::CursorIcon::Progress);
    }
}

/// `latex` as the toast shows it: one line, no control or direction-changing characters (it is
/// shown as plain text, never as markup), at most [`PREVIEW_CHARS`] characters with "…" when cut.
pub(crate) fn preview(latex: &str, max: usize) -> String {
    let mut out = String::new();
    let mut n = 0usize;
    let mut space = false;
    for c in latex.chars() {
        let invisible = c.is_control() || matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}');
        if c.is_whitespace() || invisible {
            space = !out.is_empty();
            continue;
        }
        let extra = usize::from(space);
        if n.saturating_add(extra).saturating_add(1) > max {
            out.push('…');
            return out;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
        n = n.saturating_add(extra).saturating_add(1);
    }
    out
}

/// What the toast says after a rectangle was read, and the text for the clipboard (`None`: the
/// clipboard is left as it was).
#[derive(Debug, PartialEq)]
pub(crate) struct Outcome {
    pub message: String,
    pub copy: Option<String>,
}

/// The reason a tentative answer is unsure, or an area unread: the first diagnostic that is not
/// mere information, in the reader's words, shortened.
fn first_reason(ds: &[pdfcraft_render::text::math::recover::Diagnostic]) -> Option<String> {
    ds.iter().find(|d| d.severity != Severity::Info).or_else(|| ds.first()).map(|d| preview(&d.message, REASON_CHARS)).filter(|m| !m.is_empty())
}

/// What a reader's answer means for the user, with the clipboard text wrapped in `wrapper`.
pub(crate) fn outcome(answer: Answer, wrapper: Wrapper) -> Outcome {
    match answer {
        Ok(Ok(r)) if r.latex.trim().is_empty() => nothing(tl!("nothing was read"), None),
        Ok(Ok(r)) => {
            let shown = preview(&r.latex, PREVIEW_CHARS);
            let message = match r.confidence {
                Confidence::Confident => crate::i18n::fmt(tl!("Copied LaTeX: {latex}"), &[("latex", &shown)]),
                Confidence::Tentative => {
                    let reason = first_reason(&r.diagnostics).unwrap_or_else(|| tl!("the reader is not sure of it").to_string());
                    crate::i18n::fmt(tl!("Copied LaTeX with low confidence, check it: {latex} ({reason})"), &[("latex", &shown), ("reason", &reason)])
                }
            };
            Outcome { message, copy: Some(wrapper.wrap(&r.latex)) }
        }
        Ok(Err(e)) => {
            let why = match e.kind {
                ErrorKind::NoGlyphs => tl!("there is no text or symbol in the area"),
                ErrorKind::BadRegion => tl!("the area is not a rectangle"),
                ErrorKind::TooLarge => tl!("the area holds more than one formula can"),
                ErrorKind::InvalidGlyph => tl!("a character in it can't be placed"),
                ErrorKind::UnknownSymbol => tl!("a character in it is in a font PdfCraft doesn't read yet"),
                ErrorKind::Unsupported => tl!("PdfCraft doesn't read this kind of formula yet"),
                ErrorKind::TooComplex => tl!("the formula is too complex to read"),
            };
            // "No glyph in the region" says no more than `why`.
            let detail = if e.kind == ErrorKind::NoGlyphs { None } else { first_reason(&e.diagnostics) };
            nothing(why, detail)
        }
        Err(e) => Outcome {
            message: crate::i18n::fmt(tl!("Couldn't read the formula: {e}. The clipboard is unchanged."), &[("e", &preview(&e, REASON_CHARS))]),
            copy: None,
        },
    }
}

fn nothing(why: &str, detail: Option<String>) -> Outcome {
    let message = match detail {
        Some(d) => crate::i18n::fmt(tl!("Nothing recognised: {why} ({detail}). The clipboard is unchanged."), &[("why", why), ("detail", &d)]),
        None => crate::i18n::fmt(tl!("Nothing recognised: {why}. The clipboard is unchanged."), &[("why", why)]),
    };
    Outcome { message, copy: None }
}

impl PdfCraftApp {
    /// A rectangle was dragged with Copy as LaTeX on page `page` of view `index` (view-space points).
    pub(crate) fn start_latex_copy(&mut self, index: usize, page: usize, region: [f32; 4]) {
        let Some(id) = self.views.get(index).map(|v| v.id) else { return };
        // A newer drag replaces the older one, whatever comes of it.
        self.cancel_latex_copy();
        let [x0, y0, x1, y1] = region;
        if !region.iter().all(|v| v.is_finite()) || x1 - x0 < MIN_SIDE || y1 - y0 < MIN_SIDE {
            self.notify_tr("Drag a rectangle around a formula to copy its LaTeX");
            return;
        }
        // The tool outlives the tab it was chosen on: a document that doesn't allow copying reads nothing.
        if !self.may_copy(id) {
            return;
        }
        let Some(source) = self.session.get(id).map(pdfcraft_engine::Document::export_source) else { return };
        if page >= source.pages {
            return;
        }
        let known = self.latex_glyphs.as_ref().and_then(|g| g.of(id, page, &source.bytes));
        if let Some(g) = &known
            && !any_glyph_in(g, region)
        {
            // Nothing there: the reader says so at once, on this thread.
            let answer = math_recover::recover_in_page(g, region, RecoverOptions::default());
            self.deliver_latex(id, answer);
            return;
        }
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (stop, ctx) = (cancel.clone(), self.ctx.clone());
            let spawned = std::thread::Builder::new().name("copy-as-latex".into()).spawn(move || {
                if let Some(done) = read(id, page, region, source, known, &stop)
                    && !stop.load(Ordering::Relaxed)
                {
                    let _ = tx.send(done);
                    if let Some(ctx) = ctx {
                        ctx.request_repaint();
                    }
                }
            });
            if let Err(e) = spawned {
                self.notify_fmt("Couldn't read the formula: {e}. The clipboard is unchanged.", &[("e", &e.to_string())]);
                return;
            }
        }
        #[cfg(target_arch = "wasm32")]
        if let Some(done) = read(id, page, region, source, known, &cancel) {
            let _ = tx.send(done);
        }
        if let Some(v) = self.views.get_mut(index) {
            v.latex_pending = Some((page, region));
        }
        self.latex_copy = Some(LatexCopy { doc: id, rx, cancel });
        self.notify_tr("Reading the formula…");
    }

    /// Cancel the formula being read, if any, and put its outline away.
    pub(crate) fn cancel_latex_copy(&mut self) {
        self.latex_copy = None;
        for v in &mut self.views {
            v.latex_pending = None;
        }
    }

    /// Whether a formula is being read (tests and the control channel).
    pub fn latex_copy_running(&self) -> bool {
        self.latex_copy.is_some()
    }

    /// Take the formula read off the UI thread, once it is ready. Run at the end of the frame, after
    /// this frame's copies: a copy made since (⌘C, a whole-document copy) is newer and wins, and a
    /// document closed meanwhile takes its work with it.
    pub(crate) fn finish_latex_copy(&mut self, ctx: &egui::Context) {
        if let Some(g) = &self.latex_glyphs
            && !self.views.iter().any(|v| v.id == g.doc)
        {
            self.latex_glyphs = None;
        }
        let Some(job) = &self.latex_copy else { return };
        let doc = job.doc;
        let copied = ctx.output(|o| o.commands.iter().any(|c| matches!(c, egui::OutputCommand::CopyText(_) | egui::OutputCommand::CopyImage(_))));
        if copied || !self.views.iter().any(|v| v.id == doc) {
            self.cancel_latex_copy();
            return;
        }
        match job.rx.try_recv() {
            Ok(done) => {
                self.cancel_latex_copy();
                if let Some(read) = done.read {
                    self.latex_glyphs = Some(read);
                }
                self.deliver_latex_to(doc, done.answer, ctx);
            }
            Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(50)),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.cancel_latex_copy();
                self.notify_fmt("Couldn't read the formula: {e}. The clipboard is unchanged.", &[("e", "the reading stopped")]);
            }
        }
    }

    fn deliver_latex(&mut self, doc: DocId, answer: Answer) {
        match self.ctx.clone() {
            Some(ctx) => self.deliver_latex_to(doc, answer, &ctx),
            // No window (some tests): what would be said is said; there is no clipboard to write.
            None => {
                let out = outcome(answer, self.latex_wrapper);
                self.notify(out.message);
            }
        }
    }

    /// Say what was read and put it on the clipboard, by the same path as copied page text.
    fn deliver_latex_to(&mut self, doc: DocId, answer: Answer, ctx: &egui::Context) {
        let out = outcome(answer, self.latex_wrapper);
        if let Some(text) = out.copy {
            // Checked again: the answer comes from a document whose copying must still be allowed.
            if self.session.get(doc).is_some_and(|d| !d.allows_copying()) {
                self.notify(crate::copy_refused().to_string());
                return;
            }
            // A newer copy wins: a whole-document copy still reading must not replace this one.
            self.document_copy = None;
            ctx.copy_text(text);
        }
        self.notify(out.message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_render::text::math::recover::Diagnostic;

    fn recovered(latex: &str, confidence: Confidence, diagnostics: Vec<Diagnostic>) -> Answer {
        Ok(Ok(Recovered { latex: latex.into(), ast: pdfcraft_render::text::math::ast::Node::Row(Vec::new()), confidence, diagnostics }))
    }

    fn info(m: &str) -> Diagnostic {
        Diagnostic { severity: Severity::Info, message: m.into() }
    }

    #[test]
    fn a_confident_answer_is_copied_with_a_short_preview() {
        let out = outcome(recovered(r"x^{2} + y^{2}", Confidence::Confident, vec![info("read 7 marks")]), Wrapper::None);
        assert_eq!(out.copy.as_deref(), Some(r"x^{2} + y^{2}"));
        assert_eq!(out.message, r"Copied LaTeX: x^{2} + y^{2}");
        let out = outcome(recovered("a", Confidence::Confident, vec![]), Wrapper::Display);
        assert_eq!(out.copy.as_deref(), Some(r"\[a\]"), "the wrapper goes on the clipboard, not in the preview");
        assert_eq!(out.message, "Copied LaTeX: a");
    }

    #[test]
    fn a_tentative_answer_is_never_said_to_be_certain() {
        let ds = vec![info("glue stretched"), Diagnostic { severity: Severity::Ambiguous, message: r"\frac or \dfrac".into() }];
        let out = outcome(recovered(r"\frac{a}{b}", Confidence::Tentative, ds), Wrapper::Inline);
        assert_eq!(out.copy.as_deref(), Some(r"$\frac{a}{b}$"));
        assert!(out.message.contains("low confidence"), "{}", out.message);
        assert!(out.message.contains(r"(\frac or \dfrac)"), "the first diagnostic that isn't information: {}", out.message);
        assert!(!out.message.starts_with("Copied LaTeX:"), "{}", out.message);
        // With no diagnostic at all it still says it is unsure.
        let out = outcome(recovered("a", Confidence::Tentative, vec![]), Wrapper::None);
        assert!(out.message.contains("low confidence") && out.message.contains("not sure"), "{}", out.message);
    }

    #[test]
    fn nothing_recognised_leaves_the_clipboard_alone_and_says_why() {
        let unread =
            |kind, m: &str| Ok(Err(RecoverError { kind, diagnostics: vec![Diagnostic { severity: Severity::Unsupported, message: m.into() }] }));
        let out = outcome(unread(ErrorKind::Unsupported, "a vertical rule (a table or array line)"), Wrapper::Inline);
        assert_eq!(out.copy, None);
        assert!(
            out.message.starts_with("Nothing recognised:") && out.message.contains("vertical rule") && out.message.contains("unchanged"),
            "{}",
            out.message
        );
        let out = outcome(unread(ErrorKind::NoGlyphs, "no glyph in the region"), Wrapper::None);
        assert_eq!(
            out,
            Outcome { message: "Nothing recognised: there is no text or symbol in the area. The clipboard is unchanged.".into(), copy: None }
        );
        let out = outcome(Err("glyph extraction crashed on page 1: boom".into()), Wrapper::None);
        assert_eq!(out.copy, None);
        assert!(out.message.starts_with("Couldn't read the formula: glyph extraction crashed"), "{}", out.message);
        // An empty answer is not copied as if it were a formula.
        assert_eq!(outcome(recovered("  ", Confidence::Confident, vec![]), Wrapper::Display).copy, None);
    }

    #[test]
    fn the_preview_is_one_short_line_of_plain_text() {
        assert_eq!(preview("a\n\tb\u{202E}c\u{0}", 60), "a b c");
        let long = "x + ".repeat(40);
        let p = preview(&long, PREVIEW_CHARS);
        assert!(p.ends_with('…') && p.chars().count() <= PREVIEW_CHARS + 1, "{p}");
        assert_eq!(preview("αβγδ", 2), "αβ…", "cut at characters, not bytes");
        assert_eq!(preview("", 10), "");
        assert_eq!(preview("ab", 0), "…");
    }

    #[test]
    fn a_glyph_counts_when_its_origin_is_in_the_area_and_it_is_visible() {
        let glyph = |x: f32, y: f32, invisible: bool| pdfcraft_render::text::math::GlyphDetail {
            code: 120,
            font: 0,
            glyph_id: None,
            name: None,
            text: None,
            unmapped: false,
            size: 10.0,
            rect: [x, y - 7.0, x + 5.0, y + 2.0],
            ink: None,
            baseline: [[x, y], [x + 5.0, y]],
            direction: [1.0, 0.0],
            invisible,
        };
        let page = MathPage {
            fonts: vec![],
            glyphs: vec![glyph(10.0, 20.0, false), glyph(50.0, 20.0, true)],
            rules: vec![],
            truncated: false,
            capped: false,
        };
        assert!(any_glyph_in(&page, [5.0, 10.0, 15.0, 25.0]));
        assert!(!any_glyph_in(&page, [45.0, 10.0, 60.0, 25.0]), "an invisible glyph (OCR text) is not read");
        assert!(!any_glyph_in(&page, [11.0, 10.0, 60.0, 19.0]));
        assert!(!any_glyph_in(&page, [f32::NAN, 0.0, 100.0, 100.0]));
    }

    /// One page of `lines` lines of 100 `x` each, in Helvetica (a font the symbol table does not
    /// know).
    fn many_glyphs(lines: usize) -> Vec<u8> {
        let mut content = String::from("BT /F1 4 Tf 10 790 Td 4.2 TL ");
        for _ in 0..lines {
            content.push_str(&format!("({}) '\n", "x".repeat(100)));
        }
        content.push_str("ET");
        format!(
            "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 800] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj\n4 0 obj << /Length {} >> stream\n{content}\nendstream endobj\n5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF",
            content.len()
        )
        .into_bytes()
    }

    fn app_with(bytes: Vec<u8>) -> PdfCraftApp {
        let mut app = PdfCraftApp::new();
        app.system_clipboard = false;
        app.open_bytes("t.pdf", None, bytes).expect("opens");
        app
    }

    /// Run frames until the formula has been read (or 30 seconds), and return what was copied.
    fn wait(app: &mut PdfCraftApp, ctx: &egui::Context) -> Vec<String> {
        let mut copied = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while app.latex_copy_running() && std::time::Instant::now() < deadline {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| app.finish_latex_copy(ui.ctx()));
            out.textures_delta.clear();
            copied.extend(out.platform_output.commands.into_iter().filter_map(|c| match c {
                egui::OutputCommand::CopyText(t) => Some(t),
                _ => None,
            }));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!app.latex_copy_running(), "answered");
        copied
    }

    fn said(app: &PdfCraftApp) -> String {
        app.toast.as_ref().map(|t| t.0.clone()).unwrap_or_default()
    }

    /// A page with thousands of glyphs in the area is refused with a message, off the UI thread,
    /// and the clipboard is left alone.
    #[test]
    fn thousands_of_glyphs_are_refused_with_a_message() {
        let mut app = app_with(many_glyphs(60));
        let ctx = egui::Context::default();
        app.start_latex_copy(0, 0, [0.0, 0.0, 612.0, 800.0]);
        assert!(app.latex_copy_running(), "read off the UI thread");
        assert_eq!(app.views[0].latex_pending, Some((0, [0.0, 0.0, 612.0, 800.0])), "outlined while it runs");
        assert_eq!(said(&app), "Reading the formula…");
        let copied = wait(&mut app, &ctx);
        assert!(copied.is_empty(), "{copied:?}");
        assert!(said(&app).starts_with("Nothing recognised: the area holds more than one formula can"), "{}", said(&app));
        assert_eq!(app.views[0].latex_pending, None);
        // The page's glyphs are kept: an empty area of it is answered at once.
        app.start_latex_copy(0, 0, [400.0, 700.0, 600.0, 790.0]);
        assert!(!app.latex_copy_running());
        assert_eq!(said(&app), "Nothing recognised: there is no text or symbol in the area. The clipboard is unchanged.");
        // A few unknown glyphs: not guessed.
        app.start_latex_copy(0, 0, [0.0, 0.0, 30.0, 16.0]);
        assert!(wait(&mut app, &ctx).is_empty());
        assert!(said(&app).starts_with("Nothing recognised: a character in it is in a font PdfCraft doesn't read yet"), "{}", said(&app));
    }

    #[test]
    fn an_area_that_is_not_a_rectangle_reads_nothing() {
        let mut app = app_with(many_glyphs(2));
        for area in [[f32::NAN, 0.0, 10.0, 10.0], [0.0, 0.0, f32::INFINITY, 10.0], [5.0, 5.0, 5.0, 5.0], [10.0, 10.0, 0.0, 0.0]] {
            app.toast = None;
            app.start_latex_copy(0, 0, area);
            assert!(!app.latex_copy_running(), "{area:?}");
            assert_eq!(said(&app), "Drag a rectangle around a formula to copy its LaTeX", "{area:?}");
        }
        // A view or page that is not there is ignored.
        app.start_latex_copy(7, 0, [0.0, 0.0, 10.0, 10.0]);
        app.start_latex_copy(0, 9, [0.0, 0.0, 10.0, 10.0]);
        assert!(!app.latex_copy_running());
    }

    /// A newer drag cancels the older run; closing the document drops its run and copies nothing.
    #[test]
    fn a_newer_drag_cancels_and_closing_the_document_drops_the_run() {
        let mut app = app_with(many_glyphs(60));
        let ctx = egui::Context::default();
        app.start_latex_copy(0, 0, [0.0, 0.0, 612.0, 800.0]);
        let first = app.latex_copy.as_ref().map(|j| j.cancel.clone()).expect("running");
        app.start_latex_copy(0, 0, [0.0, 0.0, 300.0, 400.0]);
        assert!(first.load(Ordering::Relaxed), "the older run is cancelled");
        let second = app.latex_copy.as_ref().map(|j| j.cancel.clone()).expect("running");
        assert!(!second.load(Ordering::Relaxed));
        app.close_tab(0);
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| app.finish_latex_copy(ui.ctx()));
        out.textures_delta.clear();
        assert!(!app.latex_copy_running(), "dropped with its document");
        assert!(second.load(Ordering::Relaxed), "and cancelled");
        assert!(!out.platform_output.commands.iter().any(|c| matches!(c, egui::OutputCommand::CopyText(_))));
        assert!(app.latex_glyphs.is_none());
    }

    /// A copy made while the formula is read is newer and wins: the formula is not copied over it.
    #[test]
    fn a_copy_made_meanwhile_wins() {
        let mut app = app_with(many_glyphs(2));
        let ctx = egui::Context::default();
        app.start_latex_copy(0, 0, [0.0, 0.0, 612.0, 800.0]);
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.ctx().copy_text("newer".into());
            app.finish_latex_copy(ui.ctx());
        });
        out.textures_delta.clear();
        assert!(!app.latex_copy_running());
        let copies: Vec<_> = out.platform_output.commands.iter().filter(|c| matches!(c, egui::OutputCommand::CopyText(_))).collect();
        assert_eq!(copies, vec![&egui::OutputCommand::CopyText("newer".into())]);
    }

    /// A document that withholds copying is not read at all: no work starts, and the notice says why.
    #[test]
    fn a_document_that_withholds_copying_starts_no_reading() {
        let mut doc = pdfcraft_cos::Document::open(Arc::new(many_glyphs(2))).unwrap();
        doc.set_encryption(&pdfcraft_cos::NewEncryption {
            algorithm: pdfcraft_cos::Algorithm::Aes256,
            user_password: "",
            owner_password: "owner",
            permissions: -1 ^ 0b1_0000,
            encrypt_metadata: true,
            seed: [4; 32],
        })
        .unwrap();
        let mut app = app_with(pdfcraft_cos::write_full(&doc, &Default::default()).unwrap());
        app.start_latex_copy(0, 0, [0.0, 0.0, 612.0, 800.0]);
        assert!(!app.latex_copy_running(), "nothing is read");
        assert_eq!(app.views[0].latex_pending, None);
        assert_eq!(said(&app), crate::copy_refused());
    }
}
