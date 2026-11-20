//! TeX as an oracle: compile a LaTeX document and let `dvitype` list every glyph (font, size,
//! code, position), rule and `\special` TeX set, as the truth that the layout, reference and
//! bibliography oracles ([`layout`], [`refs`], [`bib`]) score PdfCraft's page reading against.
//!
//! Running TeX goes through the oracle policy of this crate: [`Tex::find`] reports a missing
//! `pdflatex` or `dvitype` as [`crate::Absent`], a skip or a failure as [`crate::REQUIRED_VAR`] says.
//! TeX runs without shell escape, with `openin_any`/`openout_any` paranoid, and under a time limit;
//! a document TeX cannot typeset is a [`TexError::Failed`] with the end of TeX's log, not a panic.

pub mod bib;
mod dvi;
pub mod layout;
pub mod ocr;
pub mod refs;

pub use dvi::{DviPage, Item, MAX_EVENT_BYTES, MAX_ITEMS, MAX_PAGES, Mark, Owner, parse_dvitype};

use crate::{Absent, Oracle};
use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Scaled points per TeX point.
pub const SP_PER_PT: f64 = 65536.0;
/// Big points (PDF units) per TeX point: pdfTeX writes positions in bp.
pub const BP_PER_PT: f64 = 72.0 / 72.27;
/// pdfTeX puts the DVI origin one inch (72 bp) right of and below the page's top-left corner
/// (`\pdfhorigin`, `\pdfvorigin`).
pub const ORIGIN_BP: f64 = 72.0;
/// A position in sp from the DVI origin, as view-space bp (origin at the page's top-left, y down):
/// the conversion `crates/render/tests/it/math_glyphs.rs` uses.
pub fn view_bp(sp: i64) -> f64 {
    ORIGIN_BP + size_bp(sp)
}

/// A length in sp, in bp.
pub fn size_bp(sp: i64) -> f64 {
    sp as f64 / SP_PER_PT * BP_PER_PT
}

/// Why the oracle could not answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TexError {
    /// `pdflatex` or `dvitype` is missing.
    Absent(Absent),
    /// A TeX program failed; `log_tail` is the end of its log.
    Failed {
        what: String,
        log_tail: String,
    },
    /// A listing or truth file that does not read.
    Listing(String),
    Io(String),
}

impl fmt::Display for TexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TexError::Absent(a) => write!(f, "{a}"),
            TexError::Failed { what, log_tail } if log_tail.is_empty() => write!(f, "{what}"),
            TexError::Failed { what, log_tail } => write!(f, "{what}; the log ends:\n{log_tail}"),
            TexError::Listing(s) | TexError::Io(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for TexError {}

/// Which file `pdflatex` writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    Dvi,
    Pdf,
}

/// Environment for every TeX run: reproducible dates, no shell escape, no reading or writing
/// outside the working directory and the TeX tree, and log lines that do not wrap (so font file
/// names can be read from the log).
pub const TEX_ENV: [(&str, &str); 6] = [
    ("SOURCE_DATE_EPOCH", "1700000000"),
    ("FORCE_SOURCE_DATE", "1"),
    ("shell_escape", "f"),
    ("openin_any", "p"),
    ("openout_any", "p"),
    ("max_print_line", "10000"),
];

/// Longest a TeX run of one document may take.
pub const RUN_TIMEOUT: Duration = Duration::from_secs(300);
/// Largest `dvitype` listing read.
pub const MAX_LISTING_BYTES: u64 = 256 << 20;

/// A fresh directory under the system temporary directory, removed on drop.
#[derive(Debug)]
pub struct WorkDir(PathBuf);

impl WorkDir {
    pub fn new(tag: &str) -> Result<WorkDir, TexError> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir();
        let mut last = String::new();
        for _ in 0..16 {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
            let dir = base.join(format!("pdfcraft-{tag}-{}-{n}-{nanos}", std::process::id()));
            match std::fs::create_dir(&dir) {
                Ok(()) => return Ok(WorkDir(dir)),
                Err(e) => last = e.to_string(),
            }
        }
        Err(TexError::Io(format!("cannot make a working directory under {}: {last}", base.display())))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Leaves the directory in place (for looking at TeX's files after a failure) and returns it.
    pub fn keep(self) -> PathBuf {
        let path = self.0.clone();
        std::mem::forget(self);
        path
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The last lines of a TeX log, and its first error message (`! …`) with the lines after it.
/// Reads at most the last 64 KiB.
pub fn log_tail(path: &Path) -> String {
    const MAX: u64 = 64 << 10;
    let Ok(mut file) = File::open(path) else { return format!("(no log at {})", path.display()) };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if file.seek(SeekFrom::Start(len.saturating_sub(MAX))).is_err() {
        return String::new();
    }
    let mut bytes = Vec::new();
    let _ = file.take(MAX).read_to_end(&mut bytes);
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<&str> = Vec::new();
    if let Some(at) = lines.iter().position(|l| l.starts_with('!')) {
        out.extend(lines.iter().skip(at).take(4));
        out.push("…");
    }
    out.extend(lines.iter().skip(lines.len().saturating_sub(12)));
    out.join("\n")
}

/// Runs `cmd` to completion or until `timeout`, when it is killed.
fn run(mut cmd: Command, timeout: Duration, what: &str) -> Result<std::process::ExitStatus, TexError> {
    let mut child = cmd.spawn().map_err(|e| TexError::Io(format!("cannot start {what}: {e}")))?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(TexError::Failed {
                    what: format!("{what} ran for more than {} s and was stopped", timeout.as_secs()),
                    log_tail: String::new(),
                });
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => return Err(TexError::Io(format!("waiting for {what}: {e}"))),
        }
    }
}

/// The TeX programs the oracle runs.
#[derive(Clone, Debug)]
pub struct Tex {
    pdflatex: Oracle,
    dvitype: Oracle,
}

impl Tex {
    /// Finds `pdflatex` and `dvitype`, or says which is missing (see [`crate::find`]).
    pub fn find() -> Result<Tex, Absent> {
        Ok(Tex { pdflatex: crate::pdflatex()?, dvitype: crate::dvitype()? })
    }

    /// `pdflatex --version`'s first line.
    pub fn version(&self) -> &str {
        self.pdflatex.version()
    }

    /// Runs `pdflatex` on `dir/stem.tex`, stopping at the first error.
    pub fn compile(&self, dir: &Path, stem: &str, output: Output, timeout: Duration) -> Result<(), TexError> {
        let mut c = self.pdflatex.command();
        c.current_dir(dir).args(["-no-shell-escape", "-interaction=batchmode", "-halt-on-error"]);
        if output == Output::Dvi {
            c.arg("-output-format=dvi");
        }
        c.arg(format!("{stem}.tex")).envs(TEX_ENV).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        let what = format!("pdflatex {stem}.tex ({})", if output == Output::Dvi { "DVI" } else { "PDF" });
        let status = run(c, timeout, &what)?;
        if status.success() {
            Ok(())
        } else {
            Err(TexError::Failed { what: format!("{what} failed ({status})"), log_tail: log_tail(&dir.join(format!("{stem}.log"))) })
        }
    }

    /// Runs BibTeX (`bibtex`, found by [`crate::bibtex`]) on `dir/stem.aux`, writing `stem.bbl`.
    /// BibTeX's warnings (an empty field) are no failure; an error is, with the end of its log.
    pub fn bibtex(&self, bibtex: &Oracle, dir: &Path, stem: &str) -> Result<(), TexError> {
        let mut c = bibtex.command();
        c.current_dir(dir).arg(stem).envs(TEX_ENV).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        let what = format!("bibtex {stem}");
        let status = run(c, RUN_TIMEOUT, &what)?;
        // BibTeX exits 1 after warnings, 2 after errors.
        if status.code().is_some_and(|c| c <= 1) {
            Ok(())
        } else {
            Err(TexError::Failed { what: format!("{what} failed ({status})"), log_tail: log_tail(&dir.join(format!("{stem}.blg"))) })
        }
    }

    /// Lists `dir/stem.dvi` with `dvitype` and reads the listing ([`parse_dvitype`]).
    pub fn list(&self, dir: &Path, stem: &str) -> Result<Vec<DviPage>, TexError> {
        let listing = dir.join(format!("{stem}.dvitype"));
        let errors = dir.join(format!("{stem}.dvitype-errors"));
        let out = File::create(&listing).map_err(|e| TexError::Io(format!("{}: {e}", listing.display())))?;
        let err = File::create(&errors).map_err(|e| TexError::Io(format!("{}: {e}", errors.display())))?;
        let mut c = self.dvitype.command();
        c.current_dir(dir)
            .args(["-output-level=4", "-dpi=72.27", "-magnification=0"])
            .arg(format!("{stem}.dvi"))
            .envs(TEX_ENV)
            .stdin(Stdio::null())
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err));
        let status = run(c, RUN_TIMEOUT, &format!("dvitype {stem}.dvi"))?;
        if !status.success() {
            return Err(TexError::Failed { what: format!("dvitype {stem}.dvi failed ({status})"), log_tail: log_tail(&errors) });
        }
        let mut text = String::new();
        File::open(&listing)
            .and_then(|f| f.take(MAX_LISTING_BYTES.saturating_add(1)).read_to_string(&mut text))
            .map_err(|e| TexError::Io(format!("{}: {e}", listing.display())))?;
        if text.len() as u64 > MAX_LISTING_BYTES {
            return Err(TexError::Listing(format!("the dvitype listing of {stem}.dvi is larger than {MAX_LISTING_BYTES} bytes")));
        }
        parse_dvitype(&text)
    }
}
