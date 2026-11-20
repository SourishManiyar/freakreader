//! The built-in engine: PaddleOCR's PP-OCRv5 models (Apache-2.0), run on the pure-Rust `rten`
//! runtime and wrapped as a [`Recognizer`] ([`PpOcrRecognizer`]). Reading is offline and local.
//!
//! The pre- and post-processing are written here from the published model cards and the
//! DB/CRNN papers, not taken from PaddleOCR's code:
//!
//! 1. **Detection** (DB, differentiable binarisation): the page is scaled so its long side is at
//!    most [`DET_MAX_SIDE`], each side rounded to a multiple of 32, normalised with the ImageNet
//!    mean and deviation in BGR order, and run through the detector, which gives each pixel the
//!    probability that it is text. The map is binarised at [`DET_THRESHOLD`], the outer contour
//!    of each component becomes its smallest rotated rectangle, boxes whose mean probability is
//!    under [`DET_BOX_THRESHOLD`] or that are tiny are dropped, and each box is grown ("unclipped")
//!    by `area × 1.5 / perimeter` on every side, as DB's shrunk training targets require.
//! 2. **Reading order**: boxes are ordered by a recursive cut of the page at empty gutters and
//!    gaps ([`reading_order`]), so a two-column paper reads one column, then the other.
//! 3. **Recognition** (CTC): each box is cut out of the original raster, turned upright, scaled
//!    to a height of 48 pixels at its own width, normalised to −1…1, and read in batches of
//!    similar width. The output is decoded greedily: repeated classes collapse, class 0 is the
//!    CTC blank, classes 1…N are the dictionary's characters and N+1 is the space.
//! 4. **Words**: the time step each character was read at says where it is along the line, so
//!    the line is split at its spaces into words with their own boxes ([`Word::rect`]); Chinese
//!    and Japanese, written without spaces, become one word per character. A word's confidence
//!    is the mean probability of its characters, 0–100.
//!
//! The recognisers are picked by language ([`RECOGNIZERS`]): the Latin model reads English and
//! the accented Latin alphabets (and Greek letters without tonos), the Greek model reads Greek,
//! and the Chinese model reads Simplified and Traditional Chinese, Japanese and English.

use std::path::{Path, PathBuf};

use rten_imageproc::{RetrievalMode, find_contours};
use rten_tensor::prelude::*;
use rten_tensor::{NdTensor, NdTensorView};

use super::{Line, OcrError, OcrImage, RecognizeOptions, Recognizer, Word};

/// The engine's id: what [`Word::source`] carries and settings and tools name it by.
pub const ENGINE_ID: &str = "ppocr";

/// The detection model file, as named in ATTRIBUTION.toml.
pub const DETECTION_MODEL: &str = "PP-OCRv5_mobile_det.onnx";
/// Its SHA-256, as in ATTRIBUTION.toml. Bytes are checked before rten parses them: rten's ONNX
/// parser can spin forever on bytes that aren't a model.
pub const DETECTION_MODEL_SHA256: &str = "a431985659dc921974177a95adcfbb90fd9e51989a5e04d70d0b75f597b6e61d";

/// One recognition model: its files and the languages it reads.
#[derive(Debug, PartialEq, Eq)]
pub struct RecognitionModel {
    /// A short name (`"latin"`).
    pub id: &'static str,
    /// The ONNX model file and its SHA-256.
    pub model: &'static str,
    pub model_sha256: &'static str,
    /// The model card (`inference.yml`) that holds its character dictionary, and its SHA-256.
    pub dictionary: &'static str,
    pub dictionary_sha256: &'static str,
    /// How many characters the dictionary lists (the model has two classes more: the CTC blank
    /// and the space).
    pub characters: usize,
    /// The [`LANGUAGES`] codes it reads.
    pub languages: &'static [&'static str],
    /// Whether the script is written without spaces (one word per character).
    pub unspaced: bool,
}

/// The recognisers, the Latin one first: it is the default, the one the web build downloads,
/// and the one an installation must have.
pub const RECOGNIZERS: &[RecognitionModel] = &[
    RecognitionModel {
        id: "latin",
        model: "latin_PP-OCRv5_mobile_rec.onnx",
        model_sha256: "7888113072263cb471b93f66dd5e2ad70548dc526fa1ace760d0d973dd121498",
        dictionary: "latin_PP-OCRv5_mobile_rec.yml",
        dictionary_sha256: "0bbe984570f597af3638e50bdf2e8276f3ab26a61966096538b3b0d1849f5c84",
        characters: 836,
        languages: &["en", "de", "fr", "es", "it", "pt", "nl", "pl", "cs", "hu", "sv"],
        unspaced: false,
    },
    RecognitionModel {
        id: "greek",
        model: "el_PP-OCRv5_mobile_rec.onnx",
        model_sha256: "2acf17fcaea2bc81b878e311e6263b8885f48bb03796f75f9f30ed3242bbaa6d",
        dictionary: "el_PP-OCRv5_mobile_rec.yml",
        dictionary_sha256: "17d85b2fe2d2f24cd4ab07bcbc33e0c126859b956ced36e281dc65e2d0c1f0bf",
        characters: 354,
        languages: &["el"],
        unspaced: false,
    },
    RecognitionModel {
        id: "chinese",
        model: "PP-OCRv5_mobile_rec.onnx",
        model_sha256: "da72dc72ca4dc220df0dfde68c1dedc31c58d3e76a25871122e5056227d50092",
        dictionary: "PP-OCRv5_mobile_rec.yml",
        dictionary_sha256: "5dfeb2777f6d0db8177d8128a8acfcf6e6276dc4ac73ea3bf0dc06d6a5e85d8e",
        characters: 18383,
        languages: &["zh", "ja"],
        unspaced: true,
    },
];

/// The default recogniser's files (the Latin one): what the web build downloads.
pub const RECOGNITION_MODEL: &str = "latin_PP-OCRv5_mobile_rec.onnx";
pub const RECOGNITION_DICTIONARY: &str = "latin_PP-OCRv5_mobile_rec.yml";

/// The languages the recognisers read (ISO 639-1), each mapped to one of [`RECOGNIZERS`].
pub const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("de", "German"),
    ("fr", "French"),
    ("es", "Spanish"),
    ("it", "Italian"),
    ("pt", "Portuguese"),
    ("nl", "Dutch"),
    ("pl", "Polish"),
    ("cs", "Czech"),
    ("hu", "Hungarian"),
    ("sv", "Swedish"),
    ("el", "Greek"),
    ("zh", "Chinese"),
    ("ja", "Japanese"),
];

/// Every model file with its SHA-256: what `cargo xtask models` fetches and ATTRIBUTION.toml lists.
pub fn model_files() -> Vec<(&'static str, &'static str)> {
    let mut files = vec![(DETECTION_MODEL, DETECTION_MODEL_SHA256)];
    for r in RECOGNIZERS {
        files.push((r.model, r.model_sha256));
        files.push((r.dictionary, r.dictionary_sha256));
    }
    files
}

/// The recogniser that reads `language` (trimmed, case-insensitive), if any.
pub fn recognizer_for(language: &str) -> Option<&'static RecognitionModel> {
    let code = language.trim().to_ascii_lowercase();
    RECOGNIZERS.iter().find(|r| r.languages.contains(&code.as_str()))
}

// ---- Limits -----------------------------------------------------------------------------------

/// The detector reads the page scaled so its long side is at most this many pixels. At 300 dpi an
/// A4 page is scaled by about 0.6, which keeps 9-point text about 15 pixels tall in the map:
/// plenty for the detector, and a third of the memory and time of the full raster.
pub const DET_MAX_SIDE: u32 = 2048;
/// Probability over which a map pixel is text.
const DET_THRESHOLD: f32 = 0.3;
/// Mean probability under which a found box is dropped.
const DET_BOX_THRESHOLD: f32 = 0.6;
/// How far a box is grown: `area × UNCLIP / perimeter` on every side.
const UNCLIP: f32 = 1.5;
/// Boxes whose short side is under this many map pixels (before growing) are specks.
const MIN_BOX_SIDE: f32 = 3.0;
/// Most contours looked at, and most boxes kept, on one page.
pub const MAX_CANDIDATES: usize = 20_000;
pub const MAX_BOXES: usize = 5_000;
/// The recogniser's input height, and the widest line image it is given.
const REC_HEIGHT: usize = 48;
const REC_MAX_WIDTH: usize = 3_200;
/// Most lines read in one batch, and most padded pixels in one batch.
const REC_BATCH: usize = 8;
const REC_BATCH_PIXELS: usize = REC_HEIGHT * 12_000;
/// Most line-image columns read on one page (about a thousand full-width lines): a page of
/// thousands of wide boxes is refused rather than read for minutes.
pub const MAX_REC_COLUMNS: usize = 2_000_000;
/// Most characters kept from one line, and most words from one page.
pub const MAX_LINE_CHARS: usize = 1_000;
pub const MAX_WORDS: usize = 50_000;
/// The largest dictionary file read, and the most characters it may list.
const MAX_DICTIONARY_BYTES: usize = 4 << 20;
const MAX_DICTIONARY_ENTRIES: usize = 30_000;
/// The largest model file read.
const MAX_MODEL_BYTES: u64 = 256 << 20;

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

// ---- Model discovery ----------------------------------------------------------------------------

/// Where the model files are: the detector and every recogniser whose two files are both there.
#[derive(Clone, Debug, PartialEq)]
pub struct Models {
    pub detection: PathBuf,
    /// `(recogniser, model file, dictionary file)`, in [`RECOGNIZERS`] order; the Latin one is
    /// always first.
    pub recognition: Vec<(&'static RecognitionModel, PathBuf, PathBuf)>,
}

impl Models {
    pub fn dirs_beside_exe(exe_dir: &Path) -> Vec<PathBuf> {
        vec![exe_dir.join("models"), exe_dir.join("../Resources/models"), exe_dir.join("../share/pdfcraft/models")]
    }

    /// The models in `dir`: the detector and the Latin recogniser (both its files) must be there,
    /// or there are none; every other recogniser is used when both its files are. A half-copied
    /// recogniser is left out rather than loaded with the wrong dictionary.
    pub fn in_dir(dir: &Path) -> Option<Models> {
        let detection = dir.join(DETECTION_MODEL);
        if !detection.is_file() {
            return None;
        }
        let recognition: Vec<_> =
            RECOGNIZERS.iter().map(|r| (r, dir.join(r.model), dir.join(r.dictionary))).filter(|(_, m, d)| m.is_file() && d.is_file()).collect();
        (recognition.first().is_some_and(|(r, _, _)| r.id == "latin")).then_some(Models { detection, recognition })
    }

    /// Look for the models: `$PDFCRAFT_MODELS`, then `models/` beside the executable (and
    /// `Resources/models` in a macOS bundle), then the source tree's `assets/models/`.
    pub fn find() -> Option<Models> {
        Self::search_dirs().iter().find_map(|d| Self::in_dir(d))
    }

    /// The directories [`Models::find`] looks in, in order.
    pub fn search_dirs() -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        if let Some(d) = std::env::var_os("PDFCRAFT_MODELS") {
            dirs.push(PathBuf::from(d));
        }
        if let Some(exe) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
            dirs.extend(Self::dirs_beside_exe(&exe));
        }
        dirs.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/models"));
        dirs
    }
}

// ---- The dictionary -----------------------------------------------------------------------------

/// The character list of a PP-OCR model card (`inference.yml`): the items of the
/// `character_dict:` list, plain or single-quoted (`''` is a quote). Anything else in the file is
/// skipped. The file is untrusted: it is size-capped, an item that is not one of those two forms
/// or is longer than a few characters is an error, and so is an empty or oversized list.
pub fn parse_dictionary(text: &str) -> Result<Vec<String>, String> {
    if text.len() > MAX_DICTIONARY_BYTES {
        return Err("the dictionary file is too large".into());
    }
    let mut lines = text.lines().skip_while(|l| l.trim_end() != "  character_dict:");
    if lines.next().is_none() {
        return Err("the file has no character_dict list".into());
    }
    let mut out = Vec::new();
    for line in lines {
        let Some(item) = line.strip_prefix("  - ") else { break };
        let value = if let Some(quoted) = item.strip_prefix('\'') {
            let inner = quoted.strip_suffix('\'').ok_or_else(|| format!("unterminated quote: {item}"))?;
            if inner.replace("''", "").contains('\'') {
                return Err(format!("a stray quote: {item}"));
            }
            inner.replace("''", "'")
        } else if item.starts_with('"') {
            return Err(format!("double-quoted items are not read: {item}"));
        } else {
            item.to_string()
        };
        if value.is_empty() || value.chars().count() > 8 {
            return Err(format!("not a character: {item:?}"));
        }
        out.push(value);
        if out.len() > MAX_DICTIONARY_ENTRIES {
            return Err("the dictionary lists too many characters".into());
        }
    }
    if out.is_empty() {
        return Err("the character_dict list is empty".into());
    }
    Ok(out)
}

// ---- Loading -------------------------------------------------------------------------------------

/// A loaded recogniser: its model and its characters.
struct Reader {
    spec: &'static RecognitionModel,
    net: rten::Model,
    characters: Vec<String>,
}

/// The loaded models. Loading takes a moment; keep one and reuse it.
pub struct Ocr {
    detector: rten::Model,
    readers: Vec<Reader>,
}

fn read_capped(path: &Path) -> Result<Vec<u8>, OcrError> {
    let name = path.display().to_string();
    let len = std::fs::metadata(path).map_err(|e| OcrError::Load(name.clone(), e.to_string()))?.len();
    if len > MAX_MODEL_BYTES {
        return Err(OcrError::Load(name, "the file is too large to be a model".into()));
    }
    std::fs::read(path).map_err(|e| OcrError::Load(name, e.to_string()))
}

/// `data` as the model file `name` with SHA-256 `sha256`, or why not.
fn checked_model(name: &str, sha256: &str, data: Vec<u8>) -> Result<rten::Model, OcrError> {
    if sha256_hex(&data) != sha256 {
        return Err(OcrError::Load(name.into(), "not the expected model file (SHA-256 differs)".into()));
    }
    rten::Model::load(data).map_err(|e| OcrError::Load(name.into(), e.to_string()))
}

fn checked_dictionary(spec: &RecognitionModel, data: &[u8]) -> Result<Vec<String>, OcrError> {
    let name = spec.dictionary;
    if sha256_hex(data) != spec.dictionary_sha256 {
        return Err(OcrError::Load(name.into(), "not the expected dictionary file (SHA-256 differs)".into()));
    }
    let text = std::str::from_utf8(data).map_err(|_| OcrError::Load(name.into(), "the dictionary is not UTF-8".into()))?;
    let characters = parse_dictionary(text).map_err(|e| OcrError::Load(name.into(), e))?;
    if characters.len() != spec.characters {
        return Err(OcrError::Load(name.into(), format!("{} characters, expected {}", characters.len(), spec.characters)));
    }
    Ok(characters)
}

impl Ocr {
    /// Load the models in `models` (each file checked against its SHA-256 first).
    pub fn load(models: &Models) -> Result<Ocr, OcrError> {
        let detector = checked_model(DETECTION_MODEL, DETECTION_MODEL_SHA256, read_capped(&models.detection)?)?;
        let mut readers = Vec::new();
        for (spec, model, dictionary) in &models.recognition {
            let characters = checked_dictionary(spec, &read_capped(dictionary)?)?;
            readers.push(Reader { spec, net: checked_model(spec.model, spec.model_sha256, read_capped(model)?)?, characters });
        }
        Self::checked(detector, readers)
    }

    /// Load the detector and the Latin recogniser from the contents of their files, where there
    /// is no file system (the web build downloads them). Bytes that aren't the files
    /// ATTRIBUTION.toml names are an error.
    pub fn from_bytes(detection: Vec<u8>, recognition: Vec<u8>, dictionary: Vec<u8>) -> Result<Ocr, OcrError> {
        let spec = RECOGNIZERS.first().ok_or_else(|| OcrError::Load("recognisers".into(), "none is defined".into()))?;
        let characters = checked_dictionary(spec, &dictionary)?;
        let detector = checked_model(DETECTION_MODEL, DETECTION_MODEL_SHA256, detection)?;
        let model = checked_model(spec.model, spec.model_sha256, recognition)?;
        Self::checked(detector, vec![Reader { spec, net: model, characters }])
    }

    /// Load the models found by [`Models::find`].
    pub fn find() -> Result<Ocr, OcrError> {
        Self::load(&Models::find().ok_or(OcrError::NoModels)?)
    }

    /// Run each recogniser once on a blank line, so a model whose classes don't match its
    /// dictionary fails at load with a clear error instead of decoding garbage.
    fn checked(detector: rten::Model, readers: Vec<Reader>) -> Result<Ocr, OcrError> {
        for r in &readers {
            let probe = NdTensor::<f32, 4>::zeros([1, 3, REC_HEIGHT, 64]);
            let out = run_recognizer(&r.net, probe.view()).map_err(|e| OcrError::Load(r.spec.model.into(), e))?;
            let classes = out.size(2);
            if classes != r.characters.len() + 2 {
                return Err(OcrError::Load(
                    r.spec.model.into(),
                    format!("{classes} classes, but the dictionary has {} characters", r.characters.len()),
                ));
            }
        }
        Ok(Ocr { detector, readers })
    }

    /// The languages the loaded recognisers read, as `(code, name)` pairs in [`LANGUAGES`] order.
    pub fn languages(&self) -> Vec<(&'static str, &'static str)> {
        LANGUAGES.iter().filter(|(c, _)| self.readers.iter().any(|r| r.spec.languages.contains(c))).copied().collect()
    }

    /// Recognise the text in an RGBA image, `width` × `height` pixels, as English.
    pub fn recognize(&self, pixels: &[u8], width: u32, height: u32) -> Result<Vec<Line>, OcrError> {
        self.recognize_in(pixels, width, height, "en")
    }

    /// Recognise the text in an RGBA image in `language` (a [`LANGUAGES`] code).
    pub fn recognize_in(&self, pixels: &[u8], width: u32, height: u32, language: &str) -> Result<Vec<Line>, OcrError> {
        let reader = self.reader(language)?;
        let image = OcrImage::new(width, height, pixels.to_vec())?;
        let boxes = self.detect(&image)?;
        let boxes = reading_order(boxes);
        read_lines(reader, &image, &boxes)
    }

    fn reader(&self, language: &str) -> Result<&Reader, OcrError> {
        let wanted = recognizer_for(language);
        if let Some(r) = wanted.and_then(|w| self.readers.iter().find(|r| r.spec.id == w.id)) {
            return Ok(r);
        }
        let available: Vec<&str> = self.languages().iter().map(|(c, _)| *c).collect();
        Err(OcrError::Recognize(match wanted {
            Some(w) => format!("the {} recognition model ({}) is not installed; installed languages: {}", w.id, w.model, available.join(", ")),
            None => format!("the language {language:?} is not read; available: {}", available.join(", ")),
        }))
    }

    // ---- Detection ----------------------------------------------------------------------------

    /// The text boxes on `image`, in image pixels, in no particular order.
    fn detect(&self, image: &OcrImage) -> Result<Vec<TextBox>, OcrError> {
        let (w, h) = (image.width() as usize, image.height() as usize);
        let (dw, dh) = det_size(w, h);
        let input = det_input(image, dw, dh);
        let out = self.detector.run_one(input.view().into(), None).map_err(|e| OcrError::Recognize(e.to_string()))?;
        let map: NdTensor<f32, 4> = out.try_into().map_err(|e| OcrError::Recognize(format!("detector output: {e:?}")))?;
        let [n, c, mh, mw] = map.shape();
        if n != 1 || c != 1 || mh != dh || mw != dw {
            return Err(OcrError::Recognize(format!("the detector returned a {n}×{c}×{mh}×{mw} map for a {dw}×{dh} page")));
        }
        let prob = map.slice((0, 0));
        Ok(boxes_from_map(prob, w as f32 / dw as f32, h as f32 / dh as f32))
    }
}

/// The detector's input size for a `w` × `h` image: the long side at most [`DET_MAX_SIDE`],
/// each side a multiple of 32 and at least 32.
fn det_size(w: usize, h: usize) -> (usize, usize) {
    let long = w.max(h).max(1) as f32;
    let scale = (DET_MAX_SIDE as f32 / long).min(1.0);
    let round = |v: usize| -> usize { (((v as f32 * scale) / 32.0).round() as usize).max(1) * 32 };
    (round(w), round(h))
}

/// The page resampled to `dw` × `dh` (area-averaged when shrinking), normalised with the
/// ImageNet mean and deviation, channels in BGR order, as `1 × 3 × dh × dw`.
fn det_input(image: &OcrImage, dw: usize, dh: usize) -> NdTensor<f32, 4> {
    const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
    const STD: [f32; 3] = [0.229, 0.224, 0.225];
    let (w, h) = (image.width() as usize, image.height() as usize);
    let (sx, sy) = (w as f32 / dw as f32, h as f32 / dh as f32);
    let rgba = image.rgba();
    let mut t = NdTensor::<f32, 4>::zeros([1, 3, dh, dw]);
    for y in 0..dh {
        let y0 = ((y as f32 * sy) as usize).min(h - 1);
        let y1 = (((y + 1) as f32 * sy).ceil() as usize).clamp(y0 + 1, h);
        for x in 0..dw {
            let x0 = ((x as f32 * sx) as usize).min(w - 1);
            let x1 = (((x + 1) as f32 * sx).ceil() as usize).clamp(x0 + 1, w);
            let mut acc = [0u32; 3];
            for yy in y0..y1 {
                let row = yy * w * 4;
                for xx in x0..x1 {
                    let o = row + xx * 4;
                    if let Some(p) = rgba.get(o..o + 3) {
                        acc[0] += u32::from(p[0]);
                        acc[1] += u32::from(p[1]);
                        acc[2] += u32::from(p[2]);
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)).max(1) as f32;
            // BGR: channel 0 is blue.
            for (ch, rgb) in [(0usize, 2usize), (1, 1), (2, 0)] {
                let v = acc[rgb] as f32 / n / 255.0;
                if let Some(cell) = t.get_mut([0, ch, y, x]) {
                    *cell = (v - MEAN[ch]) / STD[ch];
                }
            }
        }
    }
    t
}

/// A text box: its centre, the unit vector along the text (left to right as read), its length
/// along that vector and its height across it. `inner_*` is the box as found, before it was grown
/// for the recogniser; words are placed within it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct TextBox {
    cx: f32,
    cy: f32,
    ux: f32,
    uy: f32,
    width: f32,
    height: f32,
    inner_width: f32,
    inner_height: f32,
}

impl TextBox {
    /// The axis-aligned bounds `[left, top, right, bottom]` of the part of the box from `a` to
    /// `b` along it (0 … width), `height` tall, centred across it.
    fn span(&self, a: f32, b: f32, height: f32) -> [f32; 4] {
        let (vx, vy) = (-self.uy, self.ux);
        let half = height / 2.0;
        let mut r = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
        for along in [a - self.width / 2.0, b - self.width / 2.0] {
            for across in [-half, half] {
                let x = self.cx + self.ux * along + vx * across;
                let y = self.cy + self.uy * along + vy * across;
                r = [r[0].min(x), r[1].min(y), r[2].max(x), r[3].max(y)];
            }
        }
        r
    }

    fn bounds(&self) -> [f32; 4] {
        self.span(0.0, self.width, self.height)
    }
}

/// The boxes of the probability map `prob` (map pixels scaled to image pixels by `sx`, `sy`).
fn boxes_from_map(prob: NdTensorView<f32, 2>, sx: f32, sy: f32) -> Vec<TextBox> {
    let [mh, mw] = prob.shape();
    let mask = NdTensor::<bool, 2>::from_fn([mh, mw], |[y, x]| prob.get([y, x]).is_some_and(|p| *p > DET_THRESHOLD));
    let contours = find_contours(mask.view(), RetrievalMode::External);
    let mut out = Vec::new();
    for poly in contours.iter().take(MAX_CANDIDATES) {
        if out.len() >= MAX_BOXES {
            break;
        }
        if poly.len() < 3 {
            continue;
        }
        let points: Vec<(f32, f32)> = poly.iter().map(|p| (p.x as f32, p.y as f32)).collect();
        let Some(corners) = min_area_rect(&points) else { continue };
        let Some(found) = text_box(&corners) else { continue };
        if found.width.min(found.height) < MIN_BOX_SIDE {
            continue;
        }
        if mean_inside(prob, &found) < DET_BOX_THRESHOLD {
            continue;
        }
        let perimeter = 2.0 * (found.width + found.height);
        let grow = found.width * found.height * UNCLIP / perimeter;
        if !grow.is_finite() {
            continue;
        }
        // Back to image pixels: scale the corners (the map's two axes may scale a little
        // differently) and take the box again.
        let scaled = |b: &TextBox, w: f32, h: f32| -> Vec<(f32, f32)> {
            let (vx, vy) = (-b.uy, b.ux);
            [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
                .iter()
                .map(|(a, c)| ((b.cx + b.ux * a * w / 2.0 + vx * c * h / 2.0) * sx, (b.cy + b.uy * a * w / 2.0 + vy * c * h / 2.0) * sy))
                .collect()
        };
        let (Some(outer), Some(inner)) =
            (text_box(&scaled(&found, found.width + 2.0 * grow, found.height + 2.0 * grow)), text_box(&scaled(&found, found.width, found.height)))
        else {
            continue;
        };
        out.push(TextBox { inner_width: inner.width, inner_height: inner.height, ..outer });
    }
    out
}

/// The convex hull of `points` (Andrew's monotone chain), counter-clockwise in a y-up frame,
/// without repeated or collinear points.
fn convex_hull(points: &[(f32, f32)]) -> Vec<(f32, f32)> {
    let mut p: Vec<(f32, f32)> = points.iter().copied().filter(|(x, y)| x.is_finite() && y.is_finite()).collect();
    p.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    p.dedup();
    if p.len() < 3 {
        return p;
    }
    let cross = |o: (f32, f32), a: (f32, f32), b: (f32, f32)| (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0);
    let mut hull: Vec<(f32, f32)> = Vec::with_capacity(p.len() * 2);
    for pass in 0..2 {
        let start = hull.len();
        let iter: Box<dyn Iterator<Item = &(f32, f32)>> = if pass == 0 { Box::new(p.iter()) } else { Box::new(p.iter().rev()) };
        for &q in iter {
            while hull.len() >= start + 2 {
                let (Some(&a), Some(&b)) = (hull.get(hull.len() - 2), hull.last()) else { break };
                if cross(a, b, q) <= 0.0 {
                    hull.pop();
                } else {
                    break;
                }
            }
            hull.push(q);
        }
        // The last point of each chain is the first of the other.
        hull.pop();
    }
    hull
}

/// The corners of the smallest rectangle around `points` (one of its sides lies along an edge
/// of their convex hull), or `None` for fewer than three distinct points.
fn min_area_rect(points: &[(f32, f32)]) -> Option<Vec<(f32, f32)>> {
    let hull = convex_hull(points);
    if hull.len() < 3 {
        return None;
    }
    let mut best: Option<(f32, [(f32, f32); 4])> = None;
    for (k, &a) in hull.iter().enumerate() {
        let b = *hull.get((k + 1) % hull.len())?;
        let len = (b.0 - a.0).hypot(b.1 - a.1);
        if !(len > 0.0 && len.is_finite()) {
            continue;
        }
        let (ux, uy) = ((b.0 - a.0) / len, (b.1 - a.1) / len);
        let (vx, vy) = (-uy, ux);
        let (mut lo_u, mut hi_u, mut lo_v, mut hi_v) = (f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY);
        for q in &hull {
            let (dx, dy) = (q.0 - a.0, q.1 - a.1);
            let (pu, pv) = (dx * ux + dy * uy, dx * vx + dy * vy);
            lo_u = lo_u.min(pu);
            hi_u = hi_u.max(pu);
            lo_v = lo_v.min(pv);
            hi_v = hi_v.max(pv);
        }
        let area = (hi_u - lo_u) * (hi_v - lo_v);
        if best.as_ref().is_none_or(|(b, _)| area < *b) {
            let at = |u: f32, v: f32| (a.0 + ux * u + vx * v, a.1 + uy * u + vy * v);
            best = Some((area, [at(lo_u, lo_v), at(hi_u, lo_v), at(hi_u, hi_v), at(lo_u, hi_v)]));
        }
    }
    best.map(|(_, c)| c.to_vec())
}

/// The box with corners `c` (a rectangle, in any order and rotation), its "along" axis being the
/// side nearer the horizontal, pointing right.
fn text_box(c: &[(f32, f32)]) -> Option<TextBox> {
    let [a, b, _, d] = <[(f32, f32); 4]>::try_from(c).ok()?;
    let e1 = (b.0 - a.0, b.1 - a.1);
    let e2 = (d.0 - a.0, d.1 - a.1);
    let (l1, l2) = (e1.0.hypot(e1.1), e2.0.hypot(e2.1));
    if !(l1.is_finite() && l2.is_finite()) || l1 <= 0.0 || l2 <= 0.0 {
        return None;
    }
    // The side nearer the horizontal runs along the text.
    let (along, width, height) = if e1.0.abs() * l2 >= e2.0.abs() * l1 { (e1, l1, l2) } else { (e2, l2, l1) };
    let (mut ux, mut uy) = (along.0 / width, along.1 / width);
    if ux < 0.0 {
        (ux, uy) = (-ux, -uy);
    }
    let cx = c.iter().map(|p| p.0).sum::<f32>() / 4.0;
    let cy = c.iter().map(|p| p.1).sum::<f32>() / 4.0;
    (cx.is_finite() && cy.is_finite()).then_some(TextBox { cx, cy, ux, uy, width, height, inner_width: width, inner_height: height })
}

/// The mean of `prob` over the pixels inside `b`.
fn mean_inside(prob: NdTensorView<f32, 2>, b: &TextBox) -> f32 {
    let [mh, mw] = prob.shape();
    let [l, t, r, bo] = b.bounds();
    let x0 = l.floor().max(0.0) as usize;
    let y0 = t.floor().max(0.0) as usize;
    let x1 = (r.ceil().max(0.0) as usize).min(mw);
    let y1 = (bo.ceil().max(0.0) as usize).min(mh);
    let (vx, vy) = (-b.uy, b.ux);
    let (mut sum, mut n) = (0.0f64, 0u64);
    for y in y0..y1 {
        for x in x0..x1 {
            let (dx, dy) = (x as f32 + 0.5 - b.cx, y as f32 + 0.5 - b.cy);
            let along = dx * b.ux + dy * b.uy;
            let across = dx * vx + dy * vy;
            if along.abs() <= b.width / 2.0 + 0.5 && across.abs() <= b.height / 2.0 + 0.5 {
                sum += f64::from(prob.get([y, x]).copied().unwrap_or(0.0));
                n += 1;
            }
        }
    }
    if n == 0 { 0.0 } else { (sum / n as f64) as f32 }
}

// ---- Reading order -------------------------------------------------------------------------------

/// `boxes` in reading order, judged in a frame turned by the page's median text angle. The page is
/// cut recursively, with an explicit stack (no recursion):
/// a region is split at an empty vertical gutter (columns, read left to right) when it has one at
/// least about two lines wide; otherwise at the widest empty horizontal band (read top to bottom);
/// a region with neither is one line, read left to right.
fn reading_order(boxes: Vec<TextBox>) -> Vec<TextBox> {
    let mut heights: Vec<f32> = boxes.iter().map(|b| b.inner_height).filter(|h| h.is_finite()).collect();
    heights.sort_by(f32::total_cmp);
    let line = heights.get(heights.len() / 2).copied().unwrap_or(10.0).max(1.0);
    // Order in a frame turned by the page's median text angle, with each box as found (before it
    // was grown): on a page scanned a little askew a long line's upright bounds reach into the
    // lines above and below it, and no gap between rows would be left.
    let mut angles: Vec<f32> = boxes.iter().map(|b| b.uy.atan2(b.ux)).filter(|a| a.is_finite()).collect();
    angles.sort_by(f32::total_cmp);
    let angle = angles.get(angles.len() / 2).copied().unwrap_or(0.0);
    let (sin, cos) = angle.sin_cos();
    let bounds: Vec<[f32; 4]> = boxes
        .iter()
        .map(|b| {
            let (x, y) = (b.cx * cos + b.cy * sin, -b.cx * sin + b.cy * cos);
            let (hw, hh) = (b.inner_width / 2.0, b.inner_height / 2.0);
            [x - hw, y - hh, x + hw, y + hh]
        })
        .collect();
    let mut out = Vec::with_capacity(boxes.len());
    // Regions still to order, last pushed first: push the later parts first.
    let mut stack: Vec<Vec<usize>> = vec![(0..boxes.len()).collect()];
    while let Some(region) = stack.pop() {
        if region.len() <= 1 {
            out.extend(region.into_iter().filter_map(|i| boxes.get(i).copied()));
            continue;
        }
        let parts = split_columns(&region, &bounds, 2.0 * line).or_else(|| split_rows(&region, &bounds, line));
        match parts {
            Some(parts) => stack.extend(parts.into_iter().rev()),
            None => {
                let mut row = region;
                row.sort_by(|a, b| {
                    let (ba, bb) = (bounds.get(*a), bounds.get(*b));
                    ba.map(|r| r[0]).unwrap_or(0.0).total_cmp(&bb.map(|r| r[0]).unwrap_or(0.0))
                });
                out.extend(row.into_iter().filter_map(|i| boxes.get(i).copied()));
            }
        }
    }
    out
}

/// Splits `region` along the axis `lo..hi` picks (x for columns, y for rows) at every empty gap
/// wider than `min_gap`, or `None` when there is none. With `widest`, only at the widest gap.
fn split_at_gaps(region: &[usize], bounds: &[[f32; 4]], lo: usize, hi: usize, min_gap: f32, widest: bool) -> Option<Vec<Vec<usize>>> {
    let mut spans: Vec<(f32, f32, usize)> = region.iter().filter_map(|&i| bounds.get(i).map(|b| (b[lo], b[hi], i))).collect();
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut cuts: Vec<(usize, f32)> = Vec::new();
    let mut reach = f32::NEG_INFINITY;
    for (k, (start, end, _)) in spans.iter().enumerate() {
        if k > 0 && start - reach > min_gap {
            cuts.push((k, start - reach));
        }
        reach = reach.max(*end);
    }
    if cuts.is_empty() {
        return None;
    }
    if widest {
        let best = cuts.iter().copied().max_by(|a, b| a.1.total_cmp(&b.1))?;
        cuts = vec![best];
    }
    let mut parts = Vec::new();
    let mut from = 0;
    for (k, _) in cuts.into_iter().chain(std::iter::once((spans.len(), 0.0))) {
        parts.push(spans.get(from..k)?.iter().map(|s| s.2).collect());
        from = k;
    }
    Some(parts)
}

fn split_columns(region: &[usize], bounds: &[[f32; 4]], gutter: f32) -> Option<Vec<Vec<usize>>> {
    // A gutter only splits a region that is more than one line tall: two pieces of one line are
    // read left to right anyway.
    split_at_gaps(region, bounds, 0, 2, gutter, false)
}

fn split_rows(region: &[usize], bounds: &[[f32; 4]], line: f32) -> Option<Vec<Vec<usize>>> {
    // The widest band first; the halves are cut again, so every band is found in turn. A band
    // thinner than a tenth of a line is two lines touching, not a gap.
    split_at_gaps(region, bounds, 1, 3, line * 0.1, true)
}

// ---- Recognition ---------------------------------------------------------------------------------

/// The line image of `b`, upright, [`REC_HEIGHT`] tall and `width` wide, normalised to −1…1
/// (channels BGR), written into `out` (`3 × REC_HEIGHT × ≥width`) from column 0.
fn sample_line(image: &OcrImage, b: &TextBox, width: usize, out: &mut rten_tensor::NdTensorViewMut<f32, 3>) {
    let (w, h) = (image.width() as i64, image.height() as i64);
    let rgba = image.rgba();
    let (vx, vy) = (-b.uy, b.ux);
    // Pixels of the source per pixel of the line image, along and across.
    let (step_a, step_c) = (b.width / width.max(1) as f32, b.height / REC_HEIGHT as f32);
    // Supersample when shrinking, so thin strokes are not lost.
    let ss = (step_a.max(step_c).ceil() as usize).clamp(1, 4);
    let px = |x: i64, y: i64| -> [f32; 3] {
        if x < 0 || y < 0 || x >= w || y >= h {
            return [255.0; 3];
        }
        let o = ((y * w + x) * 4) as usize;
        rgba.get(o..o + 3).map_or([255.0; 3], |p| [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])])
    };
    for row in 0..REC_HEIGHT {
        for col in 0..width {
            let mut acc = [0.0f32; 3];
            for sy in 0..ss {
                for sx in 0..ss {
                    let a = (col as f32 + (sx as f32 + 0.5) / ss as f32) * step_a - b.width / 2.0;
                    let c = (row as f32 + (sy as f32 + 0.5) / ss as f32) * step_c - b.height / 2.0;
                    let x = b.cx + b.ux * a + vx * c - 0.5;
                    let y = b.cy + b.uy * a + vy * c - 0.5;
                    let (x0, y0) = (x.floor(), y.floor());
                    let (fx, fy) = (x - x0, y - y0);
                    let (xi, yi) = (x0 as i64, y0 as i64);
                    let (p00, p10, p01, p11) = (px(xi, yi), px(xi + 1, yi), px(xi, yi + 1), px(xi + 1, yi + 1));
                    for k in 0..3 {
                        acc[k] += p00[k] * (1.0 - fx) * (1.0 - fy) + p10[k] * fx * (1.0 - fy) + p01[k] * (1.0 - fx) * fy + p11[k] * fx * fy;
                    }
                }
            }
            let n = (ss * ss) as f32;
            for (ch, rgb) in [(0usize, 2usize), (1, 1), (2, 0)] {
                if let Some(v) = out.get_mut([ch, row, col]) {
                    *v = (acc[rgb] / n / 255.0 - 0.5) / 0.5;
                }
            }
        }
    }
}

/// The recogniser's output for `input` (`N × 3 × 48 × W`): `N × T × classes` probabilities.
fn run_recognizer(model: &rten::Model, input: NdTensorView<f32, 4>) -> Result<NdTensor<f32, 3>, String> {
    let out = model.run_one(input.into(), None).map_err(|e| e.to_string())?;
    let mut probs: NdTensor<f32, 3> = out.try_into().map_err(|e| format!("recogniser output: {e:?}"))?;
    if probs.size(0) != input.size(0) || probs.size(2) < 2 {
        return Err(format!("the recogniser returned {:?} for {} lines", probs.shape(), input.size(0)));
    }
    // The published models end in a softmax; anything else is normalised here.
    let is_prob = probs.iter().all(|p| (0.0..=1.0).contains(p));
    if !is_prob {
        let [n, t, _] = probs.shape();
        for i in 0..n {
            for s in 0..t {
                let mut row = probs.slice_mut((i, s));
                let max = row.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                let mut sum = 0.0;
                for v in row.iter_mut() {
                    *v = if v.is_finite() { (*v - max).exp() } else { 0.0 };
                    sum += *v;
                }
                if sum > 0.0 && sum.is_finite() {
                    for v in row.iter_mut() {
                        *v /= sum;
                    }
                }
            }
        }
    }
    Ok(probs)
}

/// One decoded character: the class read, its probability and the time step it was read at.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Read {
    class: usize,
    prob: f32,
    step: usize,
}

/// Greedy CTC decoding of `probs` (`T × classes`), its first `steps` steps: the best class per
/// step, repeats collapsed, the blank (0) dropped; at most [`MAX_LINE_CHARS`] characters.
fn ctc_decode(probs: NdTensorView<f32, 2>, steps: usize) -> Vec<Read> {
    let mut out = Vec::new();
    let mut prev = 0usize;
    for s in 0..steps.min(probs.size(0)) {
        let row = probs.slice(s);
        let (mut best, mut p) = (0usize, f32::NEG_INFINITY);
        for (c, v) in row.iter().enumerate() {
            if *v > p {
                (best, p) = (c, *v);
            }
        }
        if best != 0 && best != prev && p.is_finite() {
            out.push(Read { class: best, prob: p, step: s });
            if out.len() >= MAX_LINE_CHARS {
                break;
            }
        }
        prev = best;
    }
    out
}

/// A shape that stands on its own: a check box or bullet the recogniser read as a character
/// (`□Haupt` beside a form's check box), kept as a word apart from the text next to it.
fn is_lone_symbol(c: char) -> bool {
    matches!(c as u32, 0x25A0..=0x25FF | 0x2610..=0x2612)
}

fn is_unspaced_char(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2FA1F | 0x3000..=0x303F | 0xFF00..=0xFFEF)
}

/// The words of one line read in `b` (`step_px` line-image pixels per time step, `px_to_box` box
/// pixels per line-image pixel): `reads` decoded against `characters`, split at spaces, around
/// box and bullet shapes, and for unspaced scripts around each Chinese or Japanese character, each with the part of the
/// line its characters were read in and the mean probability of its characters.
fn line_words(reads: &[Read], characters: &[String], unspaced: bool, b: &TextBox, step_px: f32, px_to_box: f32) -> Vec<Word> {
    let space = characters.len() + 1;
    // Groups of (text, first step, last step, probabilities).
    let mut groups: Vec<(String, usize, usize, Vec<f32>)> = Vec::new();
    // Whether the next character may join the last group (no space or lone character since).
    let mut open = false;
    for r in reads {
        let text = match r.class {
            c if c == space => " ",
            c => characters.get(c.wrapping_sub(1)).map_or(" ", String::as_str),
        };
        if text.trim().is_empty() {
            open = false;
            continue;
        }
        let single = (unspaced && text.chars().any(is_unspaced_char)) || text.chars().any(is_lone_symbol);
        match groups.last_mut() {
            Some(g) if open && !single => {
                g.0.push_str(text);
                g.2 = r.step;
                g.3.push(r.prob);
            }
            _ => groups.push((text.to_string(), r.step, r.step, vec![r.prob])),
        }
        open = !single;
    }
    // Where each word starts and ends along the line, in the line image's pixels: half a step
    // before its first character and after its last, but never past the midpoint to the next
    // word or outside the box as found.
    let scale = px_to_box;
    let margin = (b.width - b.inner_width).max(0.0) / 2.0;
    let centre = |s: usize| (s as f32 + 0.5) * step_px * scale;
    let mut words = Vec::with_capacity(groups.len());
    for (k, (text, first, last, probs)) in groups.iter().enumerate() {
        let mut a = centre(*first) - step_px * scale;
        let mut z = centre(*last) + step_px * scale;
        if let Some(prev) = k.checked_sub(1).and_then(|p| groups.get(p)) {
            a = a.max((centre(prev.2) + centre(*first)) / 2.0);
        }
        if let Some(next) = groups.get(k + 1) {
            z = z.min((centre(*last) + centre(next.1)) / 2.0);
        }
        let (lo, hi) = (margin, b.width - margin);
        let (a, z) = (a.clamp(lo, hi), z.clamp(lo, hi));
        if z <= a {
            continue;
        }
        let conf = probs.iter().sum::<f32>() / probs.len().max(1) as f32 * 100.0;
        let rect = b.span(a, z, b.inner_height);
        if rect.iter().all(|v| v.is_finite()) {
            words.push(Word::new(text.clone(), rect, ENGINE_ID).with_confidence(conf));
        }
    }
    words
}

/// Read every box in `boxes` (in order) with `reader`: one [`Line`] per box that has text.
fn read_lines(reader: &Reader, image: &OcrImage, boxes: &[TextBox]) -> Result<Vec<Line>, OcrError> {
    // Each line's width in the recogniser's input, and the lines grouped by width so a batch
    // pads little.
    let widths: Vec<usize> = boxes
        .iter()
        .map(|b| {
            let ratio = b.width / b.height.max(1.0);
            ((REC_HEIGHT as f32 * ratio).ceil() as usize).clamp(REC_HEIGHT / 2, REC_MAX_WIDTH)
        })
        .collect();
    if widths.iter().sum::<usize>() > MAX_REC_COLUMNS {
        return Err(OcrError::Recognize(format!("the page holds more text than one reading takes ({} boxes); read it in parts", boxes.len())));
    }
    let mut order: Vec<usize> = (0..boxes.len()).collect();
    order.sort_by_key(|&i| widths.get(i).copied().unwrap_or(0));
    let mut lines: Vec<Option<Line>> = vec![None; boxes.len()];
    let mut words_total = 0usize;
    let mut k = 0;
    while k < order.len() {
        // The batch: up to REC_BATCH lines, at most REC_BATCH_PIXELS once padded to the widest.
        let mut end = k;
        let mut widest = 0usize;
        while end < order.len() && end - k < REC_BATCH {
            let w = order.get(end).and_then(|&i| widths.get(i)).copied().unwrap_or(0);
            let next = widest.max(w);
            if end > k && next * REC_HEIGHT * (end - k + 1) > REC_BATCH_PIXELS {
                break;
            }
            widest = next;
            end += 1;
        }
        let batch = order.get(k..end).unwrap_or(&[]);
        k = end.max(k + 1);
        // Pad to a multiple of 8 (the recogniser's step), with 0 (mid-grey after normalising).
        let padded = widest.div_ceil(8) * 8;
        let mut input = NdTensor::<f32, 4>::zeros([batch.len(), 3, REC_HEIGHT, padded.max(8)]);
        for (slot, &i) in batch.iter().enumerate() {
            if let (Some(b), Some(&w)) = (boxes.get(i), widths.get(i)) {
                sample_line(image, b, w, &mut input.slice_mut(slot));
            }
        }
        let probs = run_recognizer(&reader.net, input.view()).map_err(OcrError::Recognize)?;
        if probs.size(2) != reader.characters.len() + 2 {
            return Err(OcrError::Recognize(format!("the recogniser returned {} classes for {} characters", probs.size(2), reader.characters.len())));
        }
        let t = probs.size(1);
        let step_px = padded.max(8) as f32 / t.max(1) as f32;
        for (slot, &i) in batch.iter().enumerate() {
            let (Some(b), Some(&w)) = (boxes.get(i), widths.get(i)) else { continue };
            let steps = ((w as f32 / step_px).ceil() as usize).min(t);
            let reads = ctc_decode(probs.slice(slot), steps);
            let words = line_words(&reads, &reader.characters, reader.spec.unspaced, b, step_px, b.width / w.max(1) as f32);
            words_total = words_total.saturating_add(words.len());
            if words_total > MAX_WORDS {
                return Err(OcrError::Recognize(format!("the {MAX_WORDS}-word limit was exceeded; the incomplete reading was rejected")));
            }
            if let Some(slot) = lines.get_mut(i) {
                *slot = (!words.is_empty()).then_some(Line { words });
            }
        }
    }
    Ok(lines.into_iter().flatten().collect())
}

// ---- The Recognizer ------------------------------------------------------------------------------

/// The PP-OCR engine as a [`Recognizer`] (the default engine).
pub struct PpOcrRecognizer {
    engine: Ocr,
    /// Loaded from bytes ([`PpOcrRecognizer::from_ocr`]), not from model files: it stays
    /// available without them.
    in_memory: bool,
}

impl PpOcrRecognizer {
    /// Load the models from `models`.
    pub fn load(models: &Models) -> Result<PpOcrRecognizer, OcrError> {
        Ok(PpOcrRecognizer { engine: Ocr::load(models)?, in_memory: false })
    }

    /// A recogniser loaded some other way than from model files: the web build downloads the
    /// models and loads them with [`Ocr::from_bytes`].
    pub fn from_ocr(engine: Ocr) -> PpOcrRecognizer {
        PpOcrRecognizer { engine, in_memory: true }
    }

    /// Load the models found by [`Models::find`].
    pub fn find() -> Result<PpOcrRecognizer, OcrError> {
        Self::load(&Models::find().ok_or(OcrError::NoModels)?)
    }
}

impl Recognizer for PpOcrRecognizer {
    fn id(&self) -> &'static str {
        ENGINE_ID
    }

    fn available(&self) -> bool {
        self.in_memory || Models::find().is_some()
    }

    fn languages(&self) -> Vec<(String, String)> {
        self.engine.languages().iter().map(|(c, n)| ((*c).to_string(), (*n).to_string())).collect()
    }

    fn recognize(&self, image: &OcrImage, options: &RecognizeOptions) -> Result<Vec<Line>, OcrError> {
        let reader = self.engine.reader(&options.language)?;
        let boxes = reading_order(self.engine.detect(image)?);
        read_lines(reader, image, &boxes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxed(cx: f32, cy: f32, w: f32, h: f32) -> TextBox {
        TextBox { cx, cy, ux: 1.0, uy: 0.0, width: w, height: h, inner_width: w, inner_height: h }
    }

    #[test]
    fn the_dictionary_is_read_from_the_model_card() {
        let card = "Global:\n  model_name: x\nPostProcess:\n  name: CTCLabelDecode\n  character_dict:\n  - '0'\n  - a\n  - ''''\n  - é\n  - 𝑢\nother: 1\n  - z\n";
        assert_eq!(parse_dictionary(card).unwrap(), ["0", "a", "'", "é", "𝑢"]);
    }

    /// The dictionary file is untrusted: what isn't a plain or single-quoted short item is an
    /// error, as is an empty, missing or oversized list.
    #[test]
    fn hostile_dictionaries_are_errors_not_panics() {
        for bad in [
            "",
            "  character_dict:\n",
            "PostProcess:\n  character_dict:\n  - 'a\n",
            "  character_dict:\n  - 'a'b'\n",
            "  character_dict:\n  - \"a\"\n",
            "  character_dict:\n  - ''\n",
            "  character_dict:\n  - abcdefghij\n",
        ] {
            assert!(parse_dictionary(bad).is_err(), "{bad:?}");
        }
        let huge = format!("  character_dict:\n{}", "  - a\n".repeat(MAX_DICTIONARY_ENTRIES + 1));
        assert!(parse_dictionary(&huge).is_err());
        assert!(parse_dictionary(&" ".repeat(MAX_DICTIONARY_BYTES + 1)).is_err());
    }

    /// A dictionary that isn't the pinned file, or lists the wrong number of characters, is
    /// refused at load.
    #[test]
    fn a_wrong_dictionary_is_refused() {
        let spec = RECOGNIZERS.first().unwrap();
        assert!(matches!(checked_dictionary(spec, b"  character_dict:\n  - a\n"), Err(OcrError::Load(..))));
        assert!(matches!(checked_dictionary(spec, &[0xff, 0xfe]), Err(OcrError::Load(..))));
    }

    #[test]
    fn the_detector_input_is_capped_and_a_multiple_of_32() {
        assert_eq!(det_size(2480, 3508), (1440, 2048));
        assert_eq!(det_size(100, 50), (96, 64));
        // Degenerate shapes still give a valid input, never a zero side.
        assert_eq!(det_size(10_000, 1), (2048, 32));
        assert_eq!(det_size(1, 1), (32, 32));
        let (w, h) = det_size(9_999, 6_400);
        assert!(w <= DET_MAX_SIDE as usize && h <= DET_MAX_SIDE as usize && w % 32 == 0 && h % 32 == 0);
    }

    #[test]
    fn the_smallest_rectangle_lies_along_the_hull() {
        // A thin line of points turned 30°.
        let (s, c) = 30f32.to_radians().sin_cos();
        let pts: Vec<(f32, f32)> = (0..=100).flat_map(|i| [0.0, 10.0].map(|v| (i as f32 * c - v * s, i as f32 * s + v * c))).collect();
        let b = text_box(&min_area_rect(&pts).unwrap()).unwrap();
        assert!((b.width - 100.0).abs() < 0.01 && (b.height - 10.0).abs() < 0.01, "{b:?}");
        assert!((b.uy.atan2(b.ux).to_degrees() - 30.0).abs() < 0.01, "{b:?}");
        // Fewer than three distinct points, NaN and collinear points make no rectangle.
        assert_eq!(min_area_rect(&[(1.0, 1.0), (1.0, 1.0)]), None);
        assert_eq!(min_area_rect(&[(f32::NAN, 0.0), (1.0, 0.0), (2.0, 0.0)]), None);
        assert_eq!(min_area_rect(&[(0.0, 0.0), (1.0, 1.0), (2.0, 2.0)]), None);
    }

    /// The text runs along the side nearer the horizontal, left to right, whichever corner the
    /// rectangle starts at.
    #[test]
    fn a_box_reads_left_to_right() {
        let corners = [(100.0, 20.0), (100.0, 0.0), (0.0, 0.0), (0.0, 20.0)];
        let b = text_box(&corners).unwrap();
        assert_eq!((b.ux, b.uy, b.width, b.height, b.cx, b.cy), (1.0, 0.0, 100.0, 20.0, 50.0, 10.0));
        assert_eq!(b.bounds(), [0.0, 0.0, 100.0, 20.0]);
        assert_eq!(text_box(&[(0.0, 0.0), (0.0, 0.0), (0.0, 0.0), (0.0, 0.0)]), None);
        assert_eq!(text_box(&[(0.0, 0.0)]), None);
    }

    /// Two columns read one after the other; a heading across both comes first; lines read top
    /// to bottom; pieces of one line left to right.
    #[test]
    fn two_columns_read_column_by_column() {
        let heading = boxed(500.0, 20.0, 900.0, 20.0);
        let left: Vec<TextBox> = (0..5).map(|i| boxed(250.0, 100.0 + 40.0 * i as f32, 400.0, 20.0)).collect();
        let right: Vec<TextBox> = (0..5).map(|i| boxed(750.0, 100.0 + 40.0 * i as f32, 400.0, 20.0)).collect();
        // Interleaved, as a detector might return them.
        let mut boxes = vec![right[2], left[4], heading, right[0], left[0], right[4], left[1], left[3], right[1], left[2], right[3]];
        boxes.reverse();
        let order = reading_order(boxes);
        let want: Vec<TextBox> = std::iter::once(heading).chain(left).chain(right).collect();
        assert_eq!(order, want);
        // Two pieces of one line, the second a little lower.
        let line = reading_order(vec![boxed(300.0, 52.0, 100.0, 20.0), boxed(100.0, 50.0, 100.0, 20.0)]);
        assert_eq!(line.iter().map(|b| b.cx).collect::<Vec<_>>(), [100.0, 300.0]);
    }

    /// A page scanned askew still reads line by line: the upright bounds of a long turned line
    /// reach into its neighbours, the turned frame's do not.
    #[test]
    fn a_skewed_page_reads_line_by_line() {
        let (s, c) = 0.8f32.to_radians().sin_cos();
        let lines: Vec<TextBox> = (0..6)
            .map(|i| {
                let (x, y) = (1000.0, 100.0 + 45.0 * i as f32);
                TextBox { cx: x * c - y * s, cy: x * s + y * c, ux: c, uy: s, width: 1800.0, height: 20.0, inner_width: 1800.0, inner_height: 20.0 }
            })
            .collect();
        let mut shuffled = lines.clone();
        shuffled.rotate_left(3);
        assert_eq!(reading_order(shuffled), lines);
        assert!(reading_order(Vec::new()).is_empty());
    }

    /// Thousands of boxes in one region are ordered without recursion.
    #[test]
    fn many_boxes_are_ordered_without_recursing() {
        let boxes: Vec<TextBox> = (0..MAX_BOXES).map(|i| boxed(100.0, 30.0 * i as f32, 50.0, 10.0)).rev().collect();
        let order = reading_order(boxes);
        assert_eq!(order.len(), MAX_BOXES);
        assert!(order.windows(2).all(|w| w[0].cy < w[1].cy));
    }

    fn probs(rows: &[(usize, f32)], classes: usize) -> NdTensor<f32, 2> {
        let mut t = NdTensor::<f32, 2>::zeros([rows.len(), classes]);
        for (s, (c, p)) in rows.iter().enumerate() {
            t[[s, *c]] = *p;
        }
        t
    }

    #[test]
    fn ctc_collapses_repeats_and_drops_blanks() {
        // a a _ a b b _ (blank 0) → a a b
        let t = probs(&[(1, 0.9), (1, 0.8), (0, 0.9), (1, 0.7), (2, 0.6), (2, 0.9), (0, 0.5)], 4);
        let read = ctc_decode(t.view(), 7);
        assert_eq!(read.iter().map(|r| r.class).collect::<Vec<_>>(), [1, 1, 2]);
        assert_eq!(read.iter().map(|r| r.step).collect::<Vec<_>>(), [0, 3, 4]);
        // Steps past the line's own width (the batch's padding) are not read; nor past the map.
        assert_eq!(ctc_decode(t.view(), 2).len(), 1);
        assert_eq!(ctc_decode(t.view(), 1000).len(), 3);
    }

    #[test]
    fn a_runaway_line_is_cut_at_the_character_cap() {
        let rows: Vec<(usize, f32)> = (0..MAX_LINE_CHARS * 2 + 10).map(|i| (1 + i % 2, 0.9)).collect();
        assert_eq!(ctc_decode(probs(&rows, 3).view(), rows.len()).len(), MAX_LINE_CHARS);
    }

    /// A line splits into words at its spaces, each word boxed where its characters were read
    /// and scored by their mean probability; Chinese becomes one word per character.
    #[test]
    fn words_get_their_own_boxes_and_confidences() {
        let chars: Vec<String> = ["a", "b", "日", "本"].iter().map(|c| c.to_string()).collect();
        let space = chars.len() + 1;
        let r = |class, prob, step| Read { class, prob, step };
        // "ab ba" over 20 steps of 8 pixels, a 160-pixel line in a 160-pixel box.
        let reads = [r(1, 0.9, 2), r(2, 0.7, 4), r(space, 0.9, 8), r(2, 1.0, 12), r(1, 1.0, 14)];
        let b = boxed(80.0, 10.0, 160.0, 20.0);
        let words = line_words(&reads, &chars, false, &b, 8.0, 1.0);
        assert_eq!(words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), ["ab", "ba"]);
        assert!(words[0].confidence.is_some_and(|c| (c - 80.0).abs() < 1e-3), "{words:?}");
        assert_eq!(words[1].confidence, Some(100.0));
        assert!(words[0].rect[2] <= words[1].rect[0], "words do not overlap: {words:?}");
        assert!(words.iter().all(|w| w.rect[0] >= 0.0 && w.rect[2] <= 160.0 && w.source == ENGINE_ID));
        // Unspaced: each Chinese character is its own word, Latin runs stay whole.
        let reads = [r(3, 0.9, 1), r(4, 0.9, 3), r(1, 0.9, 5), r(2, 0.9, 6), r(3, 0.9, 8)];
        let words = line_words(&reads, &chars, true, &b, 8.0, 1.0);
        assert_eq!(words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), ["日", "本", "ab", "日"]);
        // A check box read as a character is a word of its own, not the start of its label.
        let boxes: Vec<String> = ["□", "H", "a"].iter().map(|c| c.to_string()).collect();
        let words = line_words(&[r(1, 0.9, 1), r(2, 0.9, 3), r(3, 0.9, 4)], &boxes, false, &b, 8.0, 1.0);
        assert_eq!(words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), ["□", "Ha"]);
        // A class outside the dictionary is a space, not a panic.
        let words = line_words(&[r(1, 0.9, 1), r(99, 0.9, 2), r(2, 0.9, 3)], &chars, false, &b, 8.0, 1.0);
        assert_eq!(words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    }

    /// Model discovery: the detector and the Latin recogniser (both files) or nothing; another
    /// recogniser only with both its files.
    #[test]
    fn model_discovery_wants_a_complete_set() {
        let dir = tempfile::tempdir().unwrap();
        let put = |f: &str| std::fs::write(dir.path().join(f), b"model").unwrap();
        assert_eq!(Models::in_dir(dir.path()), None);
        put(DETECTION_MODEL);
        put(RECOGNITION_MODEL);
        assert_eq!(Models::in_dir(dir.path()), None, "the Latin dictionary is missing");
        put(RECOGNITION_DICTIONARY);
        let m = Models::in_dir(dir.path()).unwrap();
        assert_eq!(m.recognition.iter().map(|r| r.0.id).collect::<Vec<_>>(), ["latin"]);
        let greek = RECOGNIZERS.iter().find(|r| r.id == "greek").unwrap();
        put(greek.model);
        assert_eq!(Models::in_dir(dir.path()).unwrap().recognition.len(), 1, "half a recogniser is left out");
        put(greek.dictionary);
        assert_eq!(Models::in_dir(dir.path()).unwrap().recognition.iter().map(|r| r.0.id).collect::<Vec<_>>(), ["latin", "greek"]);
        // Without the detector, nothing.
        std::fs::remove_file(dir.path().join(DETECTION_MODEL)).unwrap();
        assert_eq!(Models::in_dir(dir.path()), None);
    }

    /// Every language offered maps to a recogniser, and every recogniser's languages are offered.
    #[test]
    fn every_language_has_a_recognizer() {
        for (code, _) in LANGUAGES {
            assert!(recognizer_for(code).is_some(), "{code}");
        }
        for r in RECOGNIZERS {
            assert!(r.languages.iter().all(|l| LANGUAGES.iter().any(|(c, _)| c == l)), "{}", r.id);
        }
        assert_eq!(recognizer_for(" EN ").map(|r| r.id), Some("latin"));
        assert_eq!(recognizer_for("el").map(|r| r.id), Some("greek"));
        assert_eq!(recognizer_for("ja").map(|r| r.id), Some("chinese"));
        assert_eq!(recognizer_for("xx"), None);
        assert_eq!(RECOGNIZERS.first().map(|r| r.model), Some(RECOGNITION_MODEL));
    }

    /// With the models installed: an uninstalled or unknown language is an error naming what is
    /// installed, and a page with nothing on it reads as nothing.
    #[test]
    fn languages_without_a_recognizer_are_named() {
        let Some(models) = Models::find() else {
            eprintln!("skipped: OCR models not installed");
            return;
        };
        let ocr = Ocr::load(&Models { recognition: models.recognition.into_iter().take(1).collect(), ..models }).unwrap();
        let white = vec![255u8; 64 * 64 * 4];
        assert!(ocr.recognize(&white, 64, 64).unwrap().is_empty());
        let e = ocr.recognize_in(&white, 64, 64, "el").unwrap_err().to_string();
        assert!(e.contains("greek") && e.contains("en, de"), "{e}");
        let e = ocr.recognize_in(&white, 64, 64, "tlh").unwrap_err().to_string();
        assert!(e.contains("\"tlh\"") && e.contains("en"), "{e}");
        // Hostile sizes are refused before anything is read.
        assert!(ocr.recognize(&white, 0, 64).is_err());
        assert!(ocr.recognize(&white, 65, 64).is_err());
    }
}
