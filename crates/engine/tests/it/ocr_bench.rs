//! The scanned-paper OCR benchmark (`tests/data/ocr_bench/README.md`).
//!
//! Each `.tex` page is typeset twice by pdfTeX: to DVI, where `dvitype` lists every glyph and the
//! word ends `\ocrpar` marks, which spell the truth (`pdfcraft_oracle::tex::ocr`), and to PDF,
//! which PdfCraft renders at 300 dpi and degrades the way a scanner does (a 0.5° turn, blur,
//! noise, JPEG). The configured recogniser reads each raster, and its reading is scored by word
//! and character error rate against the truth. The truth is never read from the PDF.
//!
//! `ocr_benchmark` needs TeX (`pdflatex`, `dvitype`) and the OCR models, so it is `#[ignore]`d in
//! the plain run: `cargo test -p pdfcraft-engine --test it ocr_bench -- --ignored --nocapture`.
//! A missing TeX is a declared skip, or a failure when `PDFCRAFT_ORACLES_REQUIRED` is set.
//! `PDFCRAFT_OCR_BENCH_ENGINE` picks the engine (the default engine when unset).

use pdfcraft_engine::ocr::{EngineChoice, OcrSettings, recognizers};
use pdfcraft_ocr::{OcrImage, RecognizeOptions};
use pdfcraft_oracle::tex::ocr::{MACROS, Score, char_errors, page_words, special_words_found, split_words, word_errors};
use pdfcraft_oracle::tex::{Output, Tex, WorkDir};
use pdfcraft_render::{PageRenderer, RenderRequest};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The pages: `(name, held out)`. The held-out pages are never used to choose a parameter.
const PAGES: &[(&str, bool)] =
    &[("dev_onecol", false), ("dev_twocol", false), ("dev_greek", false), ("holdout_onecol", true), ("holdout_twocol", true)];

/// The resolution pages are scanned at.
const DPI: f32 = 300.0;

/// A scanner's defects, applied in this order when several are on.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Condition {
    name: &'static str,
    rotate_deg: f32,
    blur: bool,
    noise: f32,
    jpeg_quality: Option<u8>,
}

const CONDITIONS: &[Condition] = &[
    Condition { name: "clean", rotate_deg: 0.0, blur: false, noise: 0.0, jpeg_quality: None },
    Condition { name: "rotate 0.5°", rotate_deg: 0.5, blur: false, noise: 0.0, jpeg_quality: None },
    Condition { name: "blur", rotate_deg: 0.0, blur: true, noise: 0.0, jpeg_quality: None },
    Condition { name: "noise", rotate_deg: 0.0, blur: false, noise: 18.0, jpeg_quality: None },
    Condition { name: "JPEG q40", rotate_deg: 0.0, blur: false, noise: 0.0, jpeg_quality: Some(40) },
    Condition { name: "all four", rotate_deg: 0.5, blur: true, noise: 18.0, jpeg_quality: Some(40) },
];

/// A deterministic generator (xorshift64*), so every run degrades a page the same way.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Roughly normal, mean 0, standard deviation 1 (the sum of four uniforms).
    fn normal(&mut self) -> f32 {
        ((self.next() + self.next() + self.next() + self.next()) - 2.0) * 3f32.sqrt()
    }
}

fn gray_at(rgba: &[u8], w: usize, h: usize, x: isize, y: isize) -> f32 {
    if x < 0 || y < 0 || x as usize >= w || y as usize >= h {
        return 255.0;
    }
    let o = (y as usize * w + x as usize) * 4;
    (0.299 * rgba[o] as f32) + (0.587 * rgba[o + 1] as f32) + (0.114 * rgba[o + 2] as f32)
}

/// The page turned by `deg` about its centre (bilinear, paper outside), as grey RGBA.
fn rotate(rgba: &[u8], w: usize, h: usize, deg: f32) -> Vec<u8> {
    let (s, c) = deg.to_radians().sin_cos();
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let mut out = vec![255u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = (x as f32 - cx, y as f32 - cy);
            let (sx, sy) = (c * dx + s * dy + cx, -s * dx + c * dy + cy);
            let (x0, y0) = (sx.floor(), sy.floor());
            let (fx, fy) = (sx - x0, sy - y0);
            let (x0, y0) = (x0 as isize, y0 as isize);
            let v = gray_at(rgba, w, h, x0, y0) * (1.0 - fx) * (1.0 - fy)
                + gray_at(rgba, w, h, x0 + 1, y0) * fx * (1.0 - fy)
                + gray_at(rgba, w, h, x0, y0 + 1) * (1.0 - fx) * fy
                + gray_at(rgba, w, h, x0 + 1, y0 + 1) * fx * fy;
            let o = (y * w + x) * 4;
            let g = v.round().clamp(0.0, 255.0) as u8;
            out[o..o + 3].fill(g);
        }
    }
    out
}

/// A 3×3 binomial blur (1 2 1 by 1 2 1), edges replicated.
fn blur(rgba: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = rgba.to_vec();
    let k = [1.0f32, 2.0, 1.0];
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0;
            for (j, kj) in k.iter().enumerate() {
                for (i, ki) in k.iter().enumerate() {
                    let xx = (x as isize + i as isize - 1).clamp(0, w as isize - 1);
                    let yy = (y as isize + j as isize - 1).clamp(0, h as isize - 1);
                    acc += gray_at(rgba, w, h, xx, yy) * kj * ki;
                }
            }
            let o = (y * w + x) * 4;
            out[o..o + 3].fill((acc / 16.0).round().clamp(0.0, 255.0) as u8);
        }
    }
    out
}

/// Gaussian-like noise of standard deviation `sigma` grey levels, seeded per page.
fn noise(rgba: &mut [u8], sigma: f32, seed: u64) {
    let mut rng = Rng(seed | 1);
    for px in rgba.chunks_exact_mut(4) {
        let g = (0.299 * px[0] as f32 + 0.587 * px[1] as f32 + 0.114 * px[2] as f32 + sigma * rng.normal()).round().clamp(0.0, 255.0) as u8;
        px[..3].fill(g);
    }
}

/// The page through a JPEG round trip at `quality`.
fn jpeg(rgba: &[u8], w: usize, h: usize, quality: u8) -> Vec<u8> {
    let rgb: Vec<u8> = rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    let mut buf = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality)
        .encode(&rgb, w as u32, h as u32, image::ExtendedColorType::Rgb8)
        .expect("JPEG encodes");
    image::load_from_memory(&buf).expect("JPEG decodes").to_rgba8().into_raw()
}

fn degrade(rgba: &[u8], w: usize, h: usize, c: &Condition, seed: u64) -> Vec<u8> {
    let mut px = if c.rotate_deg != 0.0 { rotate(rgba, w, h, c.rotate_deg) } else { rgba.to_vec() };
    if c.blur {
        px = blur(&px, w, h);
    }
    if c.noise > 0.0 {
        noise(&mut px, c.noise, seed);
    }
    if let Some(q) = c.jpeg_quality {
        px = jpeg(&px, w, h, q);
    }
    px
}

/// One page's truth and its 300-dpi raster.
struct Page {
    doc: &'static str,
    holdout: bool,
    truth: Vec<String>,
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

fn typeset(tex: &Tex, name: &'static str, holdout: bool) -> Vec<Page> {
    let work = WorkDir::new("ocr-bench").expect("a work directory");
    let dir = work.path();
    let source = std::fs::read_to_string(format!("{}/tests/data/ocr_bench/{name}.tex", env!("CARGO_MANIFEST_DIR"))).expect("the page source");
    std::fs::write(dir.join(format!("{name}.tex")), source).unwrap();
    std::fs::write(dir.join("ocrbench.tex"), MACROS).unwrap();
    tex.compile(dir, name, Output::Dvi, Duration::from_secs(120)).unwrap_or_else(|e| panic!("{name}: {e}"));
    let listed = tex.list(dir, name).unwrap_or_else(|e| panic!("{name}: {e}"));
    tex.compile(dir, name, Output::Pdf, Duration::from_secs(120)).unwrap_or_else(|e| panic!("{name}: {e}"));
    let pdf = std::fs::read(dir.join(format!("{name}.pdf"))).unwrap();
    let mut renderer = PageRenderer::new(Arc::new(pdf), Default::default());
    assert_eq!(renderer.page_count(), listed.len(), "{name}: the PDF and the DVI have the same pages");
    listed
        .iter()
        .enumerate()
        .map(|(k, p)| {
            let truth = page_words(p).unwrap_or_else(|e| panic!("{name} page {}: {e}", k + 1));
            let shot = renderer.render(RenderRequest { page: k, scale: DPI / 72.0, ..Default::default() });
            assert!(shot.error.is_none(), "{name}: {:?}", shot.error);
            Page { doc: name, holdout, truth, rgba: shot.rgba.to_vec(), width: shot.width, height: shot.height }
        })
        .collect()
}

#[derive(Default, Clone, Copy)]
struct Tally {
    words: Score,
    chars: Score,
    special: (usize, usize),
    seconds: f64,
}

impl Tally {
    fn add(&mut self, o: &Tally) {
        self.words.add(o.words);
        self.chars.add(o.chars);
        self.special = (self.special.0 + o.special.0, self.special.1 + o.special.1);
        self.seconds += o.seconds;
    }
}

fn pct(s: Score) -> String {
    s.rate().map_or("–".into(), |r| format!("{:.1}%", r * 100.0))
}

#[test]
#[ignore = "needs TeX and the OCR models; run with --ignored"]
fn ocr_benchmark() {
    let tex = pdfcraft_oracle::or_skip!(Tex::find());
    let engine = std::env::var("PDFCRAFT_OCR_BENCH_ENGINE").map(|e| EngineChoice::parse(&e)).unwrap_or_default();
    let settings = OcrSettings { engine, ..Default::default() };
    let recognizers = match recognizers(&settings) {
        Ok(r) => r,
        Err(why) => {
            assert!(std::env::var_os("PDFCRAFT_OCR_BENCH_REQUIRED").is_none(), "{why}");
            eprintln!("ocr_benchmark skipped: {why}");
            return;
        }
    };
    let options = RecognizeOptions { language: settings.language.clone() };
    let pages: Vec<Page> = PAGES.iter().flat_map(|(name, holdout)| typeset(&tex, name, *holdout)).collect();
    println!(
        "\nengine `{}` (language `{}`), {} pages at {DPI} dpi, TeX: {}\n",
        recognizers.primary.id(),
        options.language,
        pages.len(),
        tex.version()
    );
    println!("| condition | set | WER | CER | accented/Greek words read | seconds |");
    println!("|---|---|---|---|---|---|");
    let mut per_doc: Vec<(String, Tally)> = Vec::new();
    for (ci, c) in CONDITIONS.iter().enumerate() {
        let (mut dev, mut held) = (Tally::default(), Tally::default());
        for (pi, p) in pages.iter().enumerate() {
            let seed = 0x9E37_79B9_7F4A_7C15u64 ^ ((ci as u64) << 32) ^ pi as u64;
            let px = degrade(&p.rgba, p.width as usize, p.height as usize, c, seed);
            let image = OcrImage::new(p.width, p.height, px).expect("a valid raster");
            let start = Instant::now();
            let lines = recognizers.primary.recognize(&image, &options).unwrap_or_else(|e| panic!("{}: {e}", p.doc));
            let seconds = start.elapsed().as_secs_f64();
            let read: Vec<String> = lines.iter().flat_map(|l| split_words(&l.text())).collect();
            let t = Tally {
                words: word_errors(&p.truth, &read),
                chars: char_errors(&p.truth, &read),
                special: special_words_found(&p.truth, &read),
                seconds,
            };
            if std::env::var_os("PDFCRAFT_OCR_BENCH_VERBOSE").is_some() && ci == 0 {
                eprintln!("--- {} truth\n{}\n--- read\n{}", p.doc, p.truth.join(" "), read.join(" "));
            }
            per_doc.push((format!("{} / {}", c.name, p.doc), t));
            if p.holdout { held.add(&t) } else { dev.add(&t) }
        }
        for (set, t) in [("dev", dev), ("held out", held)] {
            println!("| {} | {set} | {} | {} | {}/{} | {:.1} |", c.name, pct(t.words), pct(t.chars), t.special.1, t.special.0, t.seconds);
        }
    }
    println!("\n| condition / page | WER | CER |\n|---|---|---|");
    for (name, t) in &per_doc {
        println!("| {name} | {} | {} |", pct(t.words), pct(t.chars));
    }
}

/// The degradations are seeded: the same page and seed give the same pixels, the page keeps its
/// size, and a different seed gives different noise.
#[test]
fn degradations_are_reproducible() {
    let (w, h) = (40usize, 30usize);
    let mut page = vec![255u8; w * h * 4];
    for y in 10..20 {
        for x in 5..35 {
            page[(y * w + x) * 4..(y * w + x) * 4 + 3].fill(0);
        }
    }
    let all = CONDITIONS.last().unwrap();
    let a = degrade(&page, w, h, all, 7);
    assert_eq!(a.len(), page.len());
    assert_eq!(a, degrade(&page, w, h, all, 7));
    assert_ne!(a, degrade(&page, w, h, all, 8));
    // A turn about the centre leaves the centre's ink where it was.
    let turned = rotate(&page, w, h, 0.5);
    assert!(turned[(15 * w + 20) * 4] < 10);
}
