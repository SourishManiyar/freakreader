//! Edit ▸ Recognize text in area: drag a rectangle over a scan; the words recognised there go on
//! the clipboard (`Session::area_job`, the same engine path as the `ocr_area_text` tool).
//!
//! The rectangle is the Marquee Zoom / Snapshot gesture ([`crate::zoom_snap::page_input`]). The
//! area is rendered and read off the UI thread with the Recognize Text dialog's engine and
//! language: a newer drag cancels the older one, closing the document drops it, and the area stays
//! outlined with a spinner while it runs. Where the page has its own text in the area it is
//! already selectable: that text is copied instead, and the notice says so. Nothing in the
//! document changes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use egui::{Color32, CornerRadius, Stroke};
use pdfcraft_engine::DocId;
use pdfcraft_engine::ocr::{AreaJob, AreaReading, ConfidenceBand, OcrSettings};

use crate::PdfCraftApp;
use crate::canvas::{DocView, PageXform};
use crate::page_cursor::Layer;
use crate::page_pointer::PagePointer;

/// The resolution an area is read at (a scan is read at most at its own).
const AREA_DPI: f32 = 300.0;

/// What reading an area answered: the reading, or why there is none.
pub(crate) type Answer = Result<AreaReading, String>;

/// An area being recognised off the UI thread. Dropping it cancels the work.
pub(crate) struct AreaOcr {
    doc: DocId,
    rx: mpsc::Receiver<Outcome>,
    cancel: Arc<AtomicBool>,
    /// Started this frame: its answer is delivered from the next one on, never in the frame of
    /// the drag that asked for it (that frame may be laid out again, and its copy lost with it).
    fresh: bool,
}

impl Drop for AreaOcr {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The area being read stays outlined on its page, with a spinner beside it, whatever the tool now
/// is; the pointer over it shows that work is under way.
pub(crate) fn page_input(ui: &egui::Ui, xf: &PageXform, ptr: &PagePointer, page: usize, view: &mut DocView) {
    let Some((p, area)) = view.area_ocr_pending else { return };
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

/// What the notice says after an area was read, and the text for the clipboard (`None`: the
/// clipboard is left as it was).
#[derive(Debug, PartialEq)]
pub(crate) struct Outcome {
    pub message: String,
    pub copy: Option<String>,
}

/// The band's name as the notice says it.
fn band_name(band: Option<ConfidenceBand>) -> &'static str {
    match band {
        Some(ConfidenceBand::High) => tl!("high"),
        Some(ConfidenceBand::Medium) => tl!("medium"),
        Some(ConfidenceBand::Low) => tl!("low, check the text"),
        None => tl!("not reported"),
    }
}

/// What a recognised area means for the user.
pub(crate) fn outcome(answer: Answer) -> Outcome {
    match answer {
        Ok(r) if r.words == 0 || r.text.trim().is_empty() => {
            Outcome { message: tl!("No text was recognised in the area. The clipboard is unchanged.").to_string(), copy: None }
        }
        Ok(r) => Outcome {
            message: crate::i18n::fmt(
                tl!("Copied {n} recognised words (lowest confidence: {band})."),
                &[("n", &r.words.to_string()), ("band", band_name(r.lowest))],
            ),
            copy: Some(r.text),
        },
        Err(e) => {
            Outcome { message: crate::i18n::fmt(tl!("Couldn't recognise the area: {e}. The clipboard is unchanged."), &[("e", &e)]), copy: None }
        }
    }
}

/// What copying a page's own text from the area means for the user.
pub(crate) fn own_text_outcome(text: String) -> Outcome {
    let words = text.split_whitespace().count();
    Outcome {
        message: crate::i18n::fmt(
            tl!("The page already has selectable text here: copied its {n} words without recognising them."),
            &[("n", &words.to_string())],
        ),
        copy: (words > 0).then_some(text),
    }
}

impl PdfCraftApp {
    /// A rectangle was dragged with Recognize text in area on page `page` of view `index`
    /// (view-space points).
    pub(crate) fn start_area_ocr(&mut self, index: usize, page: usize, region: [f32; 4]) {
        let Some(id) = self.views.get(index).map(|v| v.id) else { return };
        // A newer drag replaces the older one, whatever comes of it.
        self.cancel_area_ocr();
        let [x0, y0, x1, y1] = region;
        let small = pdfcraft_engine::ocr::MIN_AREA_SIDE;
        if !region.iter().all(|v| v.is_finite()) || (x1 - x0).abs() < small || (y1 - y0).abs() < small {
            self.notify_tr("Drag a rectangle around the text to recognise. Esc to finish.");
            return;
        }
        // The tool outlives the tab it was chosen on: a document that doesn't allow copying reads nothing.
        if !self.may_copy(id) {
            return;
        }
        let settings = OcrSettings { dpi: AREA_DPI, ..self.ocr_draft.settings() };
        // An answer known at once is delivered at the end of this frame, like a read one.
        let answer_now = |app: &mut PdfCraftApp, out: Outcome| {
            let (tx, rx) = mpsc::channel();
            let _ = tx.send(out);
            app.area_ocr = Some(AreaOcr { doc: id, rx, cancel: Arc::new(AtomicBool::new(false)), fresh: true });
        };
        let job = match self.session.area_job(id, page, region, settings) {
            Ok(AreaJob::Text(text)) => return answer_now(self, own_text_outcome(text)),
            Ok(AreaJob::Read(job)) => job,
            Err(e) => return answer_now(self, outcome(Err(e))),
        };
        // Models missing (or Tesseract chosen and absent): say how to fix it before reading.
        if let Some(why) = pdfcraft_engine::ocr::engine_unavailable_reason(job.settings.engine) {
            #[cfg(target_arch = "wasm32")]
            if let Some(ctx) = self.ctx.clone() {
                crate::ocr_web::start(ctx);
            }
            self.notify_fmt("Text recognition isn't available: {e}", &[("e", crate::i18n::t(&why))]);
            return;
        }
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let read = move |stop: &AtomicBool| -> Option<Outcome> {
            let engines = match pdfcraft_engine::ocr::recognizers(&job.settings) {
                Ok(e) => e,
                Err(e) => return Some(outcome(Err(e))),
            };
            let found = job.run(&engines, |_, _| !stop.load(Ordering::Relaxed));
            (!stop.load(Ordering::Relaxed)).then(|| outcome(pdfcraft_engine::ocr::area_reading(&found)))
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (stop, ctx) = (cancel.clone(), self.ctx.clone());
            let spawned = std::thread::Builder::new().name("recognize-area".into()).spawn(move || {
                // A panic in the engine is an answer, not a dead thread the UI waits on forever.
                let answer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| read(&stop)))
                    .unwrap_or_else(|_| Some(outcome(Err("text recognition stopped unexpectedly".into()))));
                if let Some(answer) = answer
                    && !stop.load(Ordering::Relaxed)
                {
                    let _ = tx.send(answer);
                    if let Some(ctx) = ctx {
                        ctx.request_repaint();
                    }
                }
            });
            if let Err(e) = spawned {
                self.notify_fmt("Couldn't recognise the area: {e}. The clipboard is unchanged.", &[("e", &e.to_string())]);
                return;
            }
        }
        #[cfg(target_arch = "wasm32")]
        if let Some(answer) = read(&cancel) {
            let _ = tx.send(answer);
        }
        if let Some(v) = self.views.get_mut(index) {
            v.area_ocr_pending = Some((page, region));
        }
        self.area_ocr = Some(AreaOcr { doc: id, rx, cancel, fresh: true });
        self.notify_tr("Recognising the text in the area…");
    }

    /// Cancel the area being read, if any, and put its outline away.
    pub(crate) fn cancel_area_ocr(&mut self) {
        self.area_ocr = None;
        for v in &mut self.views {
            v.area_ocr_pending = None;
        }
    }

    /// Whether an area is being recognised (tests and the control channel).
    pub fn area_ocr_running(&self) -> bool {
        self.area_ocr.is_some()
    }

    /// Take the area read off the UI thread, once it is ready. Run at the end of the frame, after
    /// this frame's copies: a copy made since is newer and wins, and a document closed meanwhile
    /// takes its work with it.
    pub(crate) fn finish_area_ocr(&mut self, ctx: &egui::Context) {
        let Some(job) = &mut self.area_ocr else { return };
        if std::mem::take(&mut job.fresh) {
            ctx.request_repaint();
            return;
        }
        let doc = job.doc;
        let copied = ctx.output(|o| o.commands.iter().any(|c| matches!(c, egui::OutputCommand::CopyText(_) | egui::OutputCommand::CopyImage(_))));
        if copied || !self.views.iter().any(|v| v.id == doc) {
            self.cancel_area_ocr();
            return;
        }
        match job.rx.try_recv() {
            Ok(out) => {
                self.cancel_area_ocr();
                self.deliver_area_ocr(doc, out, ctx);
            }
            Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(50)),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.cancel_area_ocr();
                self.notify_fmt("Couldn't recognise the area: {e}. The clipboard is unchanged.", &[("e", "the reading stopped")]);
            }
        }
    }

    /// Say what was read and put it on the clipboard, by the same path as copied page text.
    fn deliver_area_ocr(&mut self, doc: DocId, out: Outcome, ctx: &egui::Context) {
        if let Some(text) = out.copy {
            // Checked again: the text comes from a document whose copying must still be allowed.
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

    fn reading(text: &str, words: usize, lowest: Option<ConfidenceBand>) -> Answer {
        Ok(AreaReading { text: text.into(), words, lowest })
    }

    #[test]
    fn recognised_words_are_copied_with_the_lowest_band() {
        let out = outcome(reading("Hölder bound\nnext", 3, Some(ConfidenceBand::Medium)));
        assert_eq!(out.copy.as_deref(), Some("Hölder bound\nnext"));
        assert_eq!(out.message, "Copied 3 recognised words (lowest confidence: medium).");
        let out = outcome(reading("x", 1, Some(ConfidenceBand::Low)));
        assert!(out.message.contains("low, check the text"), "{}", out.message);
        assert!(outcome(reading("x", 1, None)).message.contains("not reported"));
    }

    #[test]
    fn nothing_recognised_leaves_the_clipboard_alone_and_says_why() {
        let out = outcome(reading("", 0, None));
        assert_eq!(out.copy, None);
        assert!(out.message.contains("unchanged"), "{}", out.message);
        let why = pdfcraft_engine::ocr::OcrError::NoModels.to_string();
        let out = outcome(Err(why.clone()));
        assert_eq!(out.copy, None);
        assert!(out.message.contains(&why) && out.message.contains("unchanged"), "{}", out.message);
    }

    #[test]
    fn a_pages_own_text_is_copied_and_said_to_be_selectable() {
        let out = own_text_outcome("Already there".into());
        assert_eq!(out.copy.as_deref(), Some("Already there"));
        assert!(out.message.contains("already has selectable text") && out.message.contains('2'), "{}", out.message);
        assert_eq!(own_text_outcome("  ".into()).copy, None);
    }
}
