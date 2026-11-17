//! TeX as the oracle for typeset mathematics: the measuring instrument for LaTeX recovery.
//!
//! A recovered LaTeX string is right when TeX typesets it the way it typeset the original, not
//! when it is spelled the same: `\frac12` and `\frac{1}{2}`, `\text{max}` and `\mathrm{max}` in a
//! subscript, `\dots` and `\cdots` between plus signs set identical glyphs. So the check here is
//! [`typeset_equal`]: put the candidate where the original was (same document, same environment),
//! run it through `pdflatex` to DVI, let `dvitype` list every glyph (font, size, code, position)
//! and rule, and compare those with what the original set, up to a translation and
//! [`TOLERANCE_SP`]. The cheap, TeX-free signal beside it is [`edit_distance`] over LaTeX tokens.
//!
//! Each equation sits between `\special{pdfcraft:eq:ID:begin}` and `…:end` ([`wrap`]), which
//! `dvitype` prints in the order TeX shipped the material, so which glyph belongs to which
//! equation is TeX's own answer. Equation numbers are bracketed too (`pdfcraft:tag:…`, by a
//! one-line patch of amsmath's `\maketag@@@` in [`preamble`]) and belong to no equation. pdfTeX in
//! PDF mode ignores these specials ("Non-PDF special ignored"): the generator
//! (`cargo xtask math-oracle generate`) checks that the PDF is byte-identical with and without
//! them.
//!
//! Running TeX goes through the oracle policy of this crate: [`Tex::find`] reports a missing
//! `pdflatex` or `dvitype` as [`crate::Absent`], a skip or a failure as [`crate::REQUIRED_VAR`] says.
//! TeX runs without shell escape, with `openin_any`/`openout_any` paranoid, and under a time limit;
//! a candidate TeX cannot typeset is [`Verdict::Failed`] with the end of TeX's log, not a panic.

mod dvi;
pub mod faces;
pub mod layout;
mod mutate;
pub mod refs;
mod score;
mod truth;

pub use dvi::{DviPage, Item, MAX_EVENT_BYTES, MAX_ITEMS, MAX_PAGES, Mark, Owner, parse_dvitype};
pub use score::{MAX_TOKENS, Mutation, Outcome, Report, Tally, edit_distance, tokens};
pub use truth::{ALONE, EquationTruth, MAX_SOURCE_BYTES, MutantTruth, PageTruth, TruthSet};

use crate::{Absent, Oracle};
use std::collections::BTreeMap;
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
/// How far two typesets may place the same glyph or rule apart and still be equal, in sp (0.01 pt).
/// TeX places a glyph of identical input at an identical sp; this only absorbs the half-sp of a
/// centred display whose width differs by an odd sp.
pub const TOLERANCE_SP: i64 = 655;
/// Most glyphs and rules one equation may have for [`same_typeset`] to compare them.
pub const MAX_EQUATION_ITEMS: usize = 20_000;
/// The page the single-equation documents use, in bp: wide and tall enough for every equation of
/// the list (the generator checks each box falls inside it).
pub const SINGLE_PAGE_BP: [f64; 2] = [420.0, 200.0];

/// A position in sp from the DVI origin, as view-space bp (origin at the page's top-left, y down):
/// the conversion `crates/render/tests/it/math_glyphs.rs` uses.
pub fn view_bp(sp: i64) -> f64 {
    ORIGIN_BP + size_bp(sp)
}

/// A length in sp, in bp.
pub fn size_bp(sp: i64) -> f64 {
    sp as f64 / SP_PER_PT * BP_PER_PT
}

/// How an equation is set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Env {
    /// `$…$` in a line of text.
    Inline,
    /// `\[…\]`, unnumbered.
    Display,
    /// `\begin{equation}`, numbered.
    Equation,
    /// `\begin{align}`, `&` alignment, numbered rows.
    Align,
    /// `\begin{align*}`.
    AlignStar,
    /// `\begin{gather}`, centred numbered rows.
    Gather,
    /// `\begin{gather*}`.
    GatherStar,
}

impl Env {
    pub const ALL: [Env; 7] = [Env::Inline, Env::Display, Env::Equation, Env::Align, Env::AlignStar, Env::Gather, Env::GatherStar];

    pub fn as_str(self) -> &'static str {
        match self {
            Env::Inline => "inline",
            Env::Display => "display",
            Env::Equation => "equation",
            Env::Align => "align",
            Env::AlignStar => "align*",
            Env::Gather => "gather",
            Env::GatherStar => "gather*",
        }
    }

    pub fn parse(s: &str) -> Option<Env> {
        Env::ALL.into_iter().find(|e| e.as_str() == s)
    }

    fn delimiters(self) -> (&'static str, &'static str) {
        match self {
            Env::Inline => ("$", "$"),
            Env::Display => ("\\[", "\\]"),
            Env::Equation => ("\\begin{equation}", "\\end{equation}"),
            Env::Align => ("\\begin{align}", "\\end{align}"),
            Env::AlignStar => ("\\begin{align*}", "\\end{align*}"),
            Env::Gather => ("\\begin{gather}", "\\end{gather}"),
            Env::GatherStar => ("\\begin{gather*}", "\\end{gather*}"),
        }
    }
}

/// The preamble every oracle document shares. `geometry` is the `geometry` package's options;
/// with `specials` false the bracket macros expand to nothing (the generator's check that pdfTeX
/// in PDF mode ignores them). Computer Modern and the AMS fonts only: never `lmodern`, whose
/// licence is not on the allowlist.
pub fn preamble(geometry: &str, specials: bool) -> String {
    let (begin, end, tag) =
        if specials { (r"\special{pdfcraft:eq:#1:begin}", r"\special{pdfcraft:eq:#1:end}", r"\special{pdfcraft:tag:#1}") } else { ("", "", "") };
    format!(
        "\\documentclass{{article}}\n\
         \\usepackage{{amsmath,amssymb}}\n\
         \\usepackage[{geometry}]{{geometry}}\n\
         \\pagestyle{{empty}}\n\
         \\setlength{{\\parindent}}{{0pt}}\n\
         \\newcommand*\\pdfcraftbegin[1]{{{begin}}}\n\
         \\newcommand*\\pdfcraftend[1]{{{end}}}\n\
         \\newcommand*\\pdfcrafttag[1]{{{tag}}}\n\
         % amsmath's \\maketag@@@, with the equation number bracketed.\n\
         \\makeatletter\n\
         \\def\\maketag@@@#1{{\\hbox{{\\m@th\\normalfont\\pdfcrafttag{{begin}}#1\\pdfcrafttag{{end}}}}}}\n\
         \\makeatother\n"
    )
}

/// The geometry of the single-equation pages ([`SINGLE_PAGE_BP`]).
pub fn single_page_geometry() -> String {
    let [w, h] = SINGLE_PAGE_BP;
    format!("paperwidth={w}bp,paperheight={h}bp,margin=20bp")
}

/// `body` in its environment, bracketed as equation `id`. The brackets sit on lines of their own,
/// so a `%` in the body cannot comment one out.
pub fn wrap(env: Env, id: &str, body: &str) -> String {
    let (open, close) = env.delimiters();
    format!("{open}\\pdfcraftbegin{{{id}}}\n{body}\n\\pdfcraftend{{{id}}}{close}")
}

/// A document of single-equation pages, one per entry `(id, env, body)`: the template the truth of
/// the equation list comes from and every candidate is typeset in. Each page restarts the
/// equation counter, so numbers (which belong to no equation) are the same in both.
pub fn single_pages_source(entries: &[(&str, Env, &str)], specials: bool) -> String {
    let mut s = preamble(&single_page_geometry(), specials);
    s.push_str("\\begin{document}\n");
    for (i, (id, env, body)) in entries.iter().enumerate() {
        if i > 0 {
            s.push_str("\\newpage\n");
        }
        s.push_str("\\setcounter{equation}{0}\n");
        s.push_str(&wrap(*env, id, body));
        s.push_str("\n\\par\n");
    }
    s.push_str("\\end{document}\n");
    s
}

/// Why the oracle could not answer at all (as opposed to a candidate that does not typeset, which
/// is a [`Verdict::Failed`]).
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

/// The answer of the typeset check for one candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// TeX set the same glyphs and rules at the same places relative to each other.
    Equal,
    /// It set something else; the text says the first difference found.
    Different(String),
    /// TeX could not typeset the candidate; the text is why (the end of its log).
    Failed(String),
}

/// Whether `placed` (an equation in running text) is `alone` (the same source in the
/// single-equation template) with only its glue stretched or shrunk by the line: the same marks
/// in the same order, each on the same baseline relative to the others. A line break inside the
/// equation fails this.
pub fn same_up_to_glue(alone: &[Item], placed: &[Item]) -> Verdict {
    if alone.len() != placed.len() {
        return Verdict::Different(format!("{} glyphs and rules placed, {} alone", placed.len(), alone.len()));
    }
    let top = |items: &[Item]| items.iter().map(|i| i.v).min().unwrap_or(0);
    let (ta, tp) = (top(alone), top(placed));
    for (a, p) in alone.iter().zip(placed) {
        if a.mark != p.mark {
            return Verdict::Different(format!("{} placed where {} was set alone", p.mark, a.mark));
        }
        if a.v.saturating_sub(ta).abs_diff(p.v.saturating_sub(tp)) > TOLERANCE_SP.unsigned_abs() {
            return Verdict::Different(format!("{} sits on another baseline in running text", p.mark));
        }
    }
    Verdict::Equal
}

/// Compares two typesets: the same marks (glyph font, size and code; rule width and height) at
/// the same places, up to one translation of the whole and [`TOLERANCE_SP`]. The translation is
/// taken out by measuring each from the least `h` and least `v` of its own items.
pub fn same_typeset(truth: &[Item], candidate: &[Item]) -> Verdict {
    if truth.len() > MAX_EQUATION_ITEMS || candidate.len() > MAX_EQUATION_ITEMS {
        return Verdict::Different(format!("more than {MAX_EQUATION_ITEMS} glyphs and rules"));
    }
    let count = |items: &[Item]| items.iter().filter(|i| matches!(i.mark, Mark::Glyph { .. })).count();
    let (tg, cg) = (count(truth), count(candidate));
    if tg != cg || truth.len() != candidate.len() {
        return Verdict::Different(format!(
            "{cg} glyphs and {} rules typeset, {tg} glyphs and {} rules expected",
            candidate.len().saturating_sub(cg),
            truth.len().saturating_sub(tg)
        ));
    }
    let corner = |items: &[Item]| (items.iter().map(|i| i.h).min().unwrap_or(0), items.iter().map(|i| i.v).min().unwrap_or(0));
    let (th, tv) = corner(truth);
    let (ch, cv) = corner(candidate);
    let mut pool: BTreeMap<&Mark, Vec<(i64, i64, bool)>> = BTreeMap::new();
    for i in candidate {
        pool.entry(&i.mark).or_default().push((i.h.saturating_sub(ch), i.v.saturating_sub(cv), false));
    }
    for i in truth {
        let (h, v) = (i.h.saturating_sub(th), i.v.saturating_sub(tv));
        let found = pool.get_mut(&i.mark).and_then(|group| {
            group
                .iter_mut()
                .find(|(gh, gv, used)| !*used && gh.abs_diff(h) <= TOLERANCE_SP.unsigned_abs() && gv.abs_diff(v) <= TOLERANCE_SP.unsigned_abs())
        });
        match found {
            Some(slot) => slot.2 = true,
            None => {
                return Verdict::Different(format!(
                    "nothing like {} at ({:.2}, {:.2}) pt from the equation's corner",
                    i.mark,
                    h as f64 / SP_PER_PT,
                    v as f64 / SP_PER_PT
                ));
            }
        }
    }
    Verdict::Equal
}

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
/// Longest a TeX run of one candidate may take when candidates are typeset one by one.
pub const CANDIDATE_TIMEOUT: Duration = Duration::from_secs(30);
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

    /// Typesets each `(env, body)` on a page of its own in the single-equation template and returns
    /// the glyphs and rules of each, or why it could not be typeset. All go through one TeX run;
    /// if that run fails, or a page holds anything but its candidate, each is run alone so one bad
    /// candidate does not cost the others their answers.
    pub fn typeset(&self, entries: &[(Env, &str)]) -> Result<Vec<Result<Vec<Item>, String>>, TexError> {
        let work = WorkDir::new("typeset")?;
        let ids: Vec<String> = (0..entries.len()).map(|i| format!("c{i}")).collect();
        let mut out: Vec<Option<Result<Vec<Item>, String>>> = entries.iter().map(|(_, body)| check_candidate(body).err().map(Err)).collect();
        let todo: Vec<usize> = out.iter().enumerate().filter(|(_, o)| o.is_none()).map(|(i, _)| i).collect();
        if todo.is_empty() {
            return Ok(out.into_iter().flatten().collect());
        }
        let batch: Vec<(&str, Env, &str)> = todo.iter().filter_map(|&i| Some((ids.get(i)?.as_str(), entries.get(i)?.0, entries.get(i)?.1))).collect();
        let together = self.typeset_in(work.path(), "batch", &batch, RUN_TIMEOUT).ok().filter(|r| r.iter().all(Result::is_ok));
        match together {
            Some(results) => {
                for (&i, r) in todo.iter().zip(results) {
                    if let Some(slot) = out.get_mut(i) {
                        *slot = Some(r);
                    }
                }
            }
            None => {
                for (k, &i) in todo.iter().enumerate() {
                    let Some(&entry) = batch.get(k) else { continue };
                    let r = match self.typeset_in(work.path(), &format!("single{k}"), &[entry], CANDIDATE_TIMEOUT) {
                        Ok(mut one) => one.pop().unwrap_or_else(|| Err("nothing was typeset".to_owned())),
                        Err(TexError::Failed { what, log_tail }) => Err(format!("{what}\n{log_tail}")),
                        Err(e @ (TexError::Absent(_) | TexError::Io(_))) => return Err(e),
                        Err(TexError::Listing(s)) => Err(s),
                    };
                    if let Some(slot) = out.get_mut(i) {
                        *slot = Some(r);
                    }
                }
            }
        }
        Ok(out.into_iter().map(|o| o.unwrap_or_else(|| Err("not typeset".to_owned()))).collect())
    }

    /// One TeX run of `entries`; per entry its items, or why its page is not just its equation.
    fn typeset_in(
        &self,
        dir: &Path,
        stem: &str,
        entries: &[(&str, Env, &str)],
        timeout: Duration,
    ) -> Result<Vec<Result<Vec<Item>, String>>, TexError> {
        let tex = dir.join(format!("{stem}.tex"));
        std::fs::write(&tex, single_pages_source(entries, true)).map_err(|e| TexError::Io(format!("{}: {e}", tex.display())))?;
        self.compile(dir, stem, Output::Dvi, timeout)?;
        let pages = self.list(dir, stem)?;
        if pages.len() != entries.len() {
            return Err(TexError::Listing(format!("{} pages typeset for {} candidates", pages.len(), entries.len())));
        }
        Ok(entries.iter().zip(&pages).map(|((id, _, _), page)| alone_on_page(page, id)).collect())
    }

    /// [`typeset_equal`] for many candidates in one TeX run; `None` is no answer, which is
    /// [`Verdict::Different`] without running TeX.
    pub fn typeset_equal_all(&self, pairs: &[(&EquationTruth, Option<&str>)]) -> Result<Vec<Verdict>, TexError> {
        let entries: Vec<(Env, &str)> = pairs.iter().filter_map(|(t, c)| Some((t.env, (*c)?))).collect();
        let mut typeset = self.typeset(&entries)?.into_iter();
        Ok(pairs
            .iter()
            .map(|(truth, candidate)| match candidate {
                None => Verdict::Different("no answer".to_owned()),
                Some(_) => match typeset.next() {
                    Some(Ok(items)) => same_typeset(&truth.reference, &items),
                    Some(Err(why)) => Verdict::Failed(why),
                    None => Verdict::Failed("not typeset".to_owned()),
                },
            })
            .collect())
    }
}

/// Refuses candidates that would forge the oracle's own brackets, and absurd lengths.
fn check_candidate(body: &str) -> Result<(), String> {
    if body.len() > MAX_SOURCE_BYTES {
        return Err(format!("the candidate is longer than {MAX_SOURCE_BYTES} bytes"));
    }
    if body.contains("special") || body.contains("pdfcraft") {
        return Err("the candidate uses \\special or the oracle's own macros".to_owned());
    }
    Ok(())
}

/// The items of equation `id` on `page`, provided the page holds that equation and nothing else
/// but equation numbers.
fn alone_on_page(page: &DviPage, id: &str) -> Result<Vec<Item>, String> {
    if page.equations.len() != 1 || page.equations.first().is_none_or(|e| e != id) {
        return Err(format!("the page of `{id}` brackets {:?}", page.equations));
    }
    if let Some((stray, _)) = page.items.iter().find(|(_, o)| *o == Owner::Outside) {
        return Err(format!("the candidate set {} outside its equation", stray.mark));
    }
    page.equation_items(id).ok_or_else(|| format!("no equation `{id}`"))
}

/// Whether TeX typesets `candidate` as it typesets the truth's source: the candidate is set in the
/// truth's environment on a single-equation page and compared with [`same_typeset`] against the
/// truth's [`reference`](EquationTruth::reference). Equivalence is a property of the two strings,
/// not of where the equation was found. Needs `pdflatex` and
/// `dvitype`; their absence is [`TexError::Absent`], which callers handle by the crate's policy.
pub fn typeset_equal(truth: &EquationTruth, candidate: &str) -> Result<Verdict, TexError> {
    let tex = Tex::find().map_err(TexError::Absent)?;
    let mut v = tex.typeset_equal_all(&[(truth, Some(candidate))])?;
    Ok(v.pop().unwrap_or(Verdict::Failed("not typeset".to_owned())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyph(font: &str, code: u32, h: i64, v: i64) -> Item {
        Item { mark: Mark::Glyph { font: font.into(), size: 655360, code }, h, v }
    }

    fn frac() -> Vec<Item> {
        vec![
            glyph("cmmi10", 97, 1000, 0),
            Item { mark: Mark::Rule { width: 400_000, height: 26_214 }, h: 0, v: 300_000 },
            glyph("cmmi10", 98, 1000, 700_000),
        ]
    }

    #[test]
    fn same_typeset_ignores_translation_and_order_only() {
        let truth = frac();
        let moved: Vec<Item> = truth.iter().rev().map(|i| Item { h: i.h + 5_000_000, v: i.v - 77, ..i.clone() }).collect();
        assert_eq!(same_typeset(&truth, &moved), Verdict::Equal);
        let mut nudged = truth.clone();
        nudged[0].h += TOLERANCE_SP;
        assert_eq!(same_typeset(&truth, &nudged), Verdict::Equal);
        nudged[0].h += 1;
        assert!(matches!(same_typeset(&truth, &nudged), Verdict::Different(_)));
    }

    #[test]
    fn same_typeset_sees_every_kind_of_difference() {
        let truth = frac();
        // Numerator and denominator swapped: same marks, other places.
        let mut swapped = truth.clone();
        swapped[0].mark = Mark::Glyph { font: "cmmi10".into(), size: 655360, code: 98 };
        swapped[2].mark = Mark::Glyph { font: "cmmi10".into(), size: 655360, code: 97 };
        let mut other_font = truth.clone();
        other_font[0].mark = Mark::Glyph { font: "cmr10".into(), size: 655360, code: 97 };
        let mut other_size = truth.clone();
        other_size[0].mark = Mark::Glyph { font: "cmmi10".into(), size: 458752, code: 97 };
        let mut other_rule = truth.clone();
        other_rule[1].mark = Mark::Rule { width: 500_000, height: 26_214 };
        let fewer = truth[..2].to_vec();
        let mut glyph_for_rule = truth.clone();
        glyph_for_rule[1] = glyph("cmsy10", 0, 0, 300_000);
        for (name, c) in [
            ("swapped", swapped),
            ("font", other_font),
            ("size", other_size),
            ("rule", other_rule),
            ("fewer", fewer),
            ("glyph for rule", glyph_for_rule),
        ] {
            assert!(matches!(same_typeset(&truth, &c), Verdict::Different(_)), "{name}");
        }
        assert_eq!(same_typeset(&[], &[]), Verdict::Equal);
    }

    #[test]
    fn wrapped_bodies_keep_their_brackets_apart_from_comments() {
        let w = wrap(Env::Align, "eq1", "a &= b % note");
        assert_eq!(w, "\\begin{align}\\pdfcraftbegin{eq1}\na &= b % note\n\\pdfcraftend{eq1}\\end{align}");
        for e in Env::ALL {
            assert_eq!(Env::parse(e.as_str()), Some(e));
        }
        let doc = single_pages_source(&[("a", Env::Inline, "x"), ("b", Env::Display, "y")], false);
        assert_eq!(doc.matches("\\newpage").count(), 1);
        assert!(doc.contains("\\newcommand*\\pdfcraftbegin[1]{}"));
        assert!(single_pages_source(&[], true).contains("\\special{pdfcraft:eq:#1:begin}"));
    }

    #[test]
    fn forged_brackets_are_refused_before_tex_runs() {
        assert!(check_candidate(r"x\special{pdfcraft:eq:c0:end}").is_err());
        assert!(check_candidate(r"\pdfcraftend{c0}").is_err());
        assert!(check_candidate(&"x".repeat(MAX_SOURCE_BYTES + 1)).is_err());
        assert!(check_candidate(r"\frac{1}{2}").is_ok());
    }

    #[test]
    fn view_space_matches_the_glyph_test() {
        assert_eq!(view_bp(0), 72.0);
        assert!((view_bp(65536) - (72.0 + 72.0 / 72.27)).abs() < 1e-12);
        assert!((size_bp(-65536 * 72) + 72.0 * 72.0 / 72.27).abs() < 1e-9);
    }
}
