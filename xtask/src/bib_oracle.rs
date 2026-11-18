//! `cargo xtask bib-oracle`: the bibliography and citation oracle (`crates/render/tests/data/bib/oracle`).
//!
//! - `generate [--check] [--keep] [--freeze]` writes each paper of `papers.toml` as LaTeX that cites
//!   entries of `library.bib` (our own invented entries) with `\cite`, natbib's `\citep`, `\citet`,
//!   `\citeauthor`, ranges and hand-typed citations, around prose from the layout oracle's seeded
//!   generator, with traps that look like citations and are none; typesets it with pdfTeX and BibTeX
//!   (DVI twice after BibTeX, for the labels; then PDF twice) and writes the PDFs, the sources, the
//!   `.bbl` files, the bracket packages and `truth.tsv`. `--check` writes to a temporary directory and
//!   fails unless every file is byte-identical with the committed one. Needs TeX Live and BibTeX;
//!   nothing else does. A held-out paper must match its fingerprint in `holdout.lock`; `--freeze` adds
//!   new held-out papers to it.
//! - `report [ARGS]` scores the analysis (`crates/render/examples/bib_oracle.rs`).
//! - `test` runs the bibliography oracle's tests.
//!
//! The truth comes from DVI ([`pdfcraft_oracle::tex::bib`] on top of [`pdfcraft_oracle::tex::layout`])
//! and, for the fields of each entry, from `library.bib` where the typeset entry shows the value (a
//! style that prints no DOI has none to find). Each PDF is compiled again with both bracket packages
//! reduced to source macros without specials and must come out byte-identical; every embedded font
//! must be an AMS Type 1 font (OFL-1.1); no citation may be left undefined.

use crate::gates::root;
use crate::layout_oracle::{ADJ, MATH, NOUN, PACKAGE as LAYOUT_PACKAGE, Rng, TITLE_WORDS, sentence, sentences};
use crate::tex_oracle::{AMS_FONT_DIR, cargo};
use anyhow::{Context, bail};
use pdfcraft_oracle::tex::bib::{self, BibPaper, BibTruth, CiteSpec, EntrySpec, FieldsTruth, TrapSpec, fold};
use pdfcraft_oracle::tex::layout::{self, PaperInfo};
use pdfcraft_oracle::tex::{DviPage, Mark, Output, Tex, WorkDir};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The paper list, the library and the generated files, relative to the repository root.
pub const DATA_DIR: &str = "crates/render/tests/data/bib/oracle";
const PACKAGE: &str = "pdfcraftbib.sty";
const LIBRARY: &str = "library.bib";
/// Most pages one paper may have.
const MAX_PAGES: usize = 14;
/// How far the brackets may move a glyph of a paper that loads hyperref, in sp (0.2 pt).
const HYPERREF_SHIFT_SP: i64 = 13_107;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    paper: Vec<Paper>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Paper {
    id: String,
    /// Held out: written before the analysis, scored only by its rates. The id starts `ho_`.
    holdout: bool,
    class: String,
    options: Vec<String>,
    geometry: Option<String>,
    pagestyle: String,
    seed: u64,
    /// A BibTeX style (`plain`, `alpha`, `abbrv`, `unsrt`, `amsplain`, `apalike`, `plainnat`) or
    /// `hand` for a list written out in the source.
    style: String,
    /// natbib's options (`numbers`, `authoryear`), or empty for no natbib.
    natbib: String,
    /// For a list written by hand: `numbracket` (`[1]`), `numdot` (`1.`), `authoryear` (no label,
    /// a gap between entries).
    hand: String,
    /// The list's font size command (`small`, `footnotesize`), or empty.
    bibsize: String,
    hyperref: bool,
    /// How many entries of the library the paper lists (the keys named in the body and others
    /// drawn by the seed); 0 for all of them.
    pool: usize,
    body: String,
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("generate") => generate(rest),
        Some("report") => cargo(&["run", "-q", "-p", "pdfcraft-render", "--example", "bib_oracle", "--"], rest),
        Some("test") => cargo(&["test", "-p", "pdfcraft-render", "--test", "it", "bib_oracle", "--"], rest),
        _ => bail!("usage: cargo xtask bib-oracle generate [--check] [--keep] [--freeze] | report [ARGS] | test"),
    }
}

// --- The library -----------------------------------------------------------------------------------

/// An entry of `library.bib`.
#[derive(Clone, Debug)]
struct BibEntry {
    kind: String,
    key: String,
    fields: BTreeMap<String, String>,
}

/// Reads `library.bib`, which is written one field to a line: `@type{key,`, `  name = {value},`, `}`.
fn read_library(text: &str) -> anyhow::Result<Vec<BibEntry>> {
    let mut out: Vec<BibEntry> = Vec::new();
    let mut open: Option<BibEntry> = None;
    for (n, line) in text.lines().enumerate() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('%') {
            continue;
        }
        if let Some(rest) = t.strip_prefix('@') {
            let (kind, key) = rest.split_once('{').with_context(|| format!("{LIBRARY}:{}: `{t}`", n + 1))?;
            if open.is_some() {
                bail!("{LIBRARY}:{}: an entry begins inside another", n + 1);
            }
            open = Some(BibEntry { kind: kind.to_lowercase(), key: key.trim_end_matches(',').to_owned(), fields: BTreeMap::new() });
        } else if t == "}" {
            out.push(open.take().with_context(|| format!("{LIBRARY}:{}: `}}` with no entry", n + 1))?);
        } else {
            let (name, value) = t.split_once(" = {").with_context(|| format!("{LIBRARY}:{}: `{t}`", n + 1))?;
            let value = value.strip_suffix("},").with_context(|| format!("{LIBRARY}:{}: a field ends `}},`", n + 1))?;
            open.as_mut().with_context(|| format!("{LIBRARY}:{}: a field outside an entry", n + 1))?.fields.insert(name.to_owned(), value.to_owned());
        }
    }
    if out.len() < 60 {
        bail!("{LIBRARY} has {} entries; the oracle needs at least 60", out.len());
    }
    Ok(out)
}

/// A person of an author list: their family name (with a von part) and given names, as TeX.
#[derive(Clone, Debug)]
struct Name {
    family: String,
    given: String,
}

/// The words of `s` split at spaces outside braces.
fn words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {}
        }
        if c == ' ' && depth == 0 {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// An author field as BibTeX reads it: names joined by ` and `, each `First von Last` or
/// `von Last, First`; `others` ends a list that goes on (`et al.`). Returns the names and whether
/// the list goes on.
fn names(field: &str) -> (Vec<Name>, bool) {
    let mut out = Vec::new();
    let mut more = false;
    for part in field.split(" and ") {
        let part = part.trim();
        if part == "others" {
            more = true;
            continue;
        }
        if let Some((last, first)) = part.split_once(", ") {
            out.push(Name { family: last.to_owned(), given: first.to_owned() });
            continue;
        }
        let w = words(part);
        // The family name starts at the first lower-case word (a von part) or is the last word.
        let von = w.iter().position(|x| x.starts_with(|c: char| c.is_lowercase())).filter(|&i| i > 0);
        let at = von.unwrap_or(w.len().saturating_sub(1));
        out.push(Name { family: w.get(at..).unwrap_or_default().join(" "), given: w.get(..at).unwrap_or_default().join(" ") });
    }
    (out, more)
}

/// The initial of a given name as TeX: `T.` of `Tom\'as`, `{\'E}.` of `{\'E}lodie`, `A.-L.` of
/// `Ana-Luc{\'\i}a`.
fn initials(given: &str) -> String {
    let one = |w: &str| -> String {
        if w.starts_with('{') {
            let mut depth = 0;
            for (i, c) in w.char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            return format!("{}.", w.get(..=i).unwrap_or(w));
                        }
                    }
                    _ => {}
                }
            }
            format!("{w}.")
        } else {
            w.chars().next().map(|c| format!("{c}.")).unwrap_or_default()
        }
    };
    words(given).iter().map(|w| w.split('-').map(one).collect::<Vec<_>>().join("-")).collect::<Vec<_>>().join("~")
}

/// The venue a field list gives: the journal, the book a chapter is in, the publisher of a book,
/// the school of a thesis, the institution of a report.
fn venue(e: &BibEntry) -> Option<&String> {
    ["journal", "booktitle", "publisher", "school", "institution"].iter().find_map(|f| e.fields.get(*f))
}

/// The arXiv id of an entry: its `eprint`.
fn arxiv(e: &BibEntry) -> Option<&String> {
    e.fields.get("eprint")
}

// --- Writing a paper -------------------------------------------------------------------------------

fn arg<'a>(args: &[&'a str], key: &str) -> Option<&'a str> {
    args.iter().find_map(|a| a.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
}

/// Words a citing sentence may lead with.
const LEADS: [&str; 6] = ["By", "From", "Using", "In view of", "Combining this with", "As shown in"];

struct Writer<'a> {
    paper: &'a Paper,
    library: &'a [BibEntry],
    rng: Rng,
    out: String,
    /// The entries the paper lists, in the order drawn (named ones first).
    pool: Vec<String>,
    /// The next pool entry a `?` draws.
    next: usize,
    serial: usize,
    cites: Vec<CiteSpec>,
    traps: Vec<TrapSpec>,
    /// Ranges, to be filled with their inner keys once the list's order is known: (first, last).
    ranges: Vec<(String, String)>,
    /// For a list by hand: the order of its entries and each one's printed label or year suffix.
    hand: Option<HandList>,
    theorems: usize,
}

/// A list written by hand: its entries in order, with each one's label (numeric) and year as
/// printed (author-year, with a suffix where two entries of one author list share a year).
struct HandList {
    order: Vec<String>,
    year: HashMap<String, String>,
}

impl<'a> Writer<'a> {
    fn entry(&self, key: &str) -> anyhow::Result<&'a BibEntry> {
        self.library.iter().find(|e| e.key == key).with_context(|| format!("no entry `{key}` in {LIBRARY}"))
    }

    /// The keys of a directive: named ones, and `?` for the next of the pool.
    fn keys(&mut self, args: &[&str]) -> Vec<String> {
        args.iter()
            .take_while(|a| !a.contains('='))
            .map(|a| {
                if *a == "?" {
                    let k = self.pool.get(self.next % self.pool.len().max(1)).cloned().unwrap_or_default();
                    self.next += 1;
                    k
                } else {
                    (*a).to_owned()
                }
            })
            .collect()
    }

    fn clause(&mut self) -> String {
        let s = sentence(&mut self.rng);
        let s = s.trim_end_matches('.');
        let mut c = s.chars();
        match c.next() {
            Some(first) => first.to_lowercase().chain(c).collect(),
            None => String::new(),
        }
    }

    /// Records the citations of `keys` (one each, in order) as `form`.
    fn record(&mut self, keys: &[String], form: &str) {
        for k in keys {
            self.serial += 1;
            self.cites.push(CiteSpec { serial: Some(self.serial), key: k.clone(), form: form.to_owned() });
        }
    }

    /// A sentence around a citation `c`.
    fn frame(&mut self, c: &str) -> String {
        let clause = self.clause();
        match self.rng.below(5) {
            0 => format!("{} {c}, {clause}.", self.rng.pick(&LEADS)),
            1 => format!("This was shown in~{c}, and {clause}."),
            2 => format!("We refer to~{c} for the {} case.", self.rng.pick(&ADJ)),
            3 => format!("Then {clause}~{c}."),
            _ => format!("See~{c}: {clause}."),
        }
    }

    fn trap_id(&mut self, kind: &str) -> String {
        let id = format!("t{}", self.traps.len() + 1);
        self.traps.push(TrapSpec { id: id.clone(), kind: kind.to_owned() });
        id
    }

    /// The text of a hand-typed citation of `key`: `[3]` in a numbered list; `Okafor (2020a)` or
    /// `(Okafor, 2020a)` in an author-year one.
    fn hand_text(&self, key: &str, form: &str) -> anyhow::Result<String> {
        let h = self.hand.as_ref().context("a hand-typed citation needs a list by hand")?;
        let e = self.entry(key)?;
        if self.paper.hand != "authoryear" {
            let n = h.order.iter().position(|k| k == key).context("hand: a key not in the list")? + 1;
            return Ok(format!("[{n}]"));
        }
        let (people, more) = names(e.fields.get("author").map_or("", String::as_str));
        let who = match (people.as_slice(), more) {
            ([one], false) => one.family.clone(),
            ([a, b], false) => format!("{} and {}", a.family, b.family),
            (list, _) => format!("{} et~al.", list.first().map_or("", |n| n.family.as_str())),
        };
        let year = h.year.get(key).cloned().unwrap_or_default();
        Ok(if form == "t" { format!("{who} ({year})") } else { format!("({who}, {year})") })
    }

    fn line(&mut self, line: &str) -> anyhow::Result<()> {
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        let args: Vec<&str> = rest.split_whitespace().collect();
        let natbib = !self.paper.natbib.is_empty();
        let text = match word {
            "section" => format!("\\section{{{}}}", if rest.is_empty() { self.rng.pick(&TITLE_WORDS).to_owned() } else { rest.to_owned() }),
            "para" => sentences(&mut self.rng, args.first().and_then(|a| a.parse().ok()).unwrap_or(3)),
            "cite" | "citep" | "citet" | "citeauthor" => {
                if natbib == (word == "cite") {
                    bail!("`{word}`: \\cite in a paper without natbib, \\citep, \\citet and \\citeauthor with it");
                }
                let keys = self.keys(&args);
                if keys.is_empty() || (word != "cite" && word != "citep" && keys.len() > 1) {
                    bail!("`{word}`: one key, or several for \\cite and \\citep");
                }
                let locator = arg(&args, "at").map(|l| l.replace('_', "~"));
                let form = match (word, keys.len() > 1, &locator) {
                    (w, _, Some(_)) => format!("{w}_locator"),
                    (w, true, None) => format!("{w}_group"),
                    (w, false, None) => w.to_owned(),
                };
                self.record(&keys, &form);
                let call = match &locator {
                    Some(l) => format!("\\{word}[{l}]{{{}}}", keys.join(",")),
                    None => format!("\\{word}{{{}}}", keys.join(",")),
                };
                match word {
                    "citet" => format!("{call} proved that {}.", self.clause()),
                    "citeauthor" => format!("As \\citeauthor{{{}}} observed, {}.", keys.join(","), self.clause()),
                    "citep" => format!("{} {call}.", capital(&self.clause())),
                    _ if arg(&args, "in").is_some() => {
                        let what = arg(&args, "in").unwrap_or("Theorem_3.1").replace('_', "~");
                        format!("By {what} in~{call}, {}.", self.clause())
                    }
                    _ => self.frame(&call),
                }
            }
            "range" => {
                if natbib || self.paper.style == "hand" {
                    bail!("`range` needs \\cite's numbers (no natbib, no list by hand)");
                }
                let keys = self.keys(&args);
                let [a, b] = keys.as_slice() else { bail!("`range`: two keys") };
                self.record(std::slice::from_ref(a), "range");
                self.record(std::slice::from_ref(b), "range");
                self.ranges.push((a.clone(), b.clone()));
                let c = format!("\\pdfcraftrange{{{a}}}{{{b}}}");
                self.frame(&c)
            }
            "hand" => {
                let keys = self.keys(&args);
                let form = arg(&args, "form").unwrap_or("p");
                let mut parts = Vec::new();
                for k in &keys {
                    let t = self.hand_text(k, form)?;
                    self.record(std::slice::from_ref(k), "hand");
                    parts.push(format!("\\pdfcrafthand{{{k}}}{{{t}}}"));
                }
                let c = parts.join(", ");
                if form == "t" { format!("{c} proved that {}.", self.clause()) } else { self.frame(&c) }
            }
            "statement" => {
                // A theorem whose name cites, and whose body cites again.
                let keys = self.keys(&args);
                let [a, b] = keys.as_slice() else { bail!("`statement`: two keys") };
                self.theorems += 1;
                let (ca, cb) = if natbib {
                    (format!("\\citet{{{a}}}"), format!("\\citep{{{b}}}"))
                } else {
                    (format!("\\cite{{{a}}}"), format!("\\cite{{{b}}}"))
                };
                let form = if natbib { ("citet", "citep") } else { ("cite", "cite") };
                self.record(std::slice::from_ref(a), form.0);
                self.record(std::slice::from_ref(b), form.1);
                format!(
                    "\\begin{{theorem}}[{ca}]\nEvery {} {} is {}, as in~{cb}.\n\\end{{theorem}}",
                    self.rng.pick(&ADJ),
                    self.rng.pick(&NOUN),
                    self.rng.pick(&ADJ)
                )
            }
            "trap" => {
                let kind = args.first().copied().unwrap_or("interval");
                let id = self.trap_id(kind);
                let t = |body: &str| format!("\\pdfcrafttrap{{{id}}}{body}\\pdfcrafttrapend{{}}");
                let n = 2 + self.rng.below(2);
                match kind {
                    "interval" => {
                        format!("For every $t$ in {} the {} {} is {}.", t("$[0,1]$"), self.rng.pick(&ADJ), self.rng.pick(&NOUN), self.rng.pick(&ADJ))
                    }
                    "interval2" => format!("On {} the {} is {}.", t("$[a,b]$"), self.rng.pick(&NOUN), self.rng.pick(&ADJ)),
                    "numinterval" => format!("The map is {} on {}, and {}.", self.rng.pick(&ADJ), t(&format!("$[1,{n}]$")), self.clause()),
                    "class" => format!("The class {} of {} is {}.", t("$[x]$"), self.rng.pick(&MATH), self.rng.pick(&ADJ)),
                    "commutator" => format!("The commutator {} vanishes, so {}.", t("$[x,y]$"), self.clause()),
                    "qnumber" => format!("The weight {} is {}.", t(&format!("$[{n}]_q$")), self.rng.pick(&ADJ)),
                    "section" => format!("The proof is in Section~{}, where {}.", t(&format!("[{n}]")), self.clause()),
                    "display" => format!("Hence\n\\[ {} \\]\nfor all {}.", t("[a,b] \\subset [0,1]"), self.rng.pick(&MATH)),
                    "item" => format!(
                        "\\begin{{itemize}}\n\\item[{}] {}\n\\item[{{[{}]}}] {}\n\\end{{itemize}}",
                        t(&format!("{{[{n}]}}")),
                        capital(&format!("{}.", self.clause())),
                        n + 1,
                        capital(&format!("{}.", self.clause()))
                    ),
                    other => bail!("unknown trap `{other}`"),
                }
            }
            "appendix" => "\\appendix".to_owned(),
            "newpage" => "\\newpage".to_owned(),
            "bibliography" => self.bibliography()?,
            other => bail!("unknown directive `{other}`"),
        };
        self.out.push_str(&text);
        self.out.push_str("\n\n");
        Ok(())
    }

    fn bibliography(&mut self) -> anyhow::Result<String> {
        let size = if self.paper.bibsize.is_empty() { String::new() } else { format!("\\{}\n", self.paper.bibsize) };
        if self.paper.style != "hand" {
            let mut s = format!("{size}\\nocite{{{}}}\n\\bibliographystyle{{{}}}\n\\bibliography{{library}}", self.pool.join(","), self.paper.style);
            if !size.is_empty() {
                s = format!("{{{s}\n}}");
            }
            return Ok(s);
        }
        let h = self.hand.as_ref().context("hand list")?;
        let mut s = format!("{{{size}\\begin{{thebibliography}}{{99}}\n");
        if self.paper.hand == "authoryear" {
            s.push_str("\\setlength{\\itemsep}{3pt}\n");
        }
        for key in &h.order {
            let e = self.entry(key)?;
            let (text, _) = hand_entry(e, &self.paper.hand, h.year.get(key).map_or("", String::as_str));
            let _ = writeln!(s, "\\bibitem{{{key}}} {text}");
        }
        s.push_str("\\end{thebibliography}\n}");
        Ok(s)
    }
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// An entry written by hand, and the fields it shows. `style`: `numbracket`/`numdot` (an amsplain-like
/// line: initials first, the title in italic, the volume in bold) or `authoryear` (family names
/// first, the year after them, the venue in italic); `year`: the year as printed (with a suffix).
fn hand_entry(e: &BibEntry, style: &str, year: &str) -> (String, FieldsTruth) {
    let f = |k: &str| e.fields.get(k).cloned();
    let (people, more) = names(f("author").as_deref().unwrap_or(""));
    let mut t = FieldsTruth { authors: people.iter().map(|n| n.family.clone()).collect(), year: Some(year.to_owned()), ..Default::default() };
    let mut s = String::new();
    let tail = |s: &mut String, t: &mut FieldsTruth| {
        if let Some(d) = f("doi") {
            let _ = write!(s, ", doi:\\allowbreak\\url{{{d}}}");
            t.doi = Some(d);
        }
        if let Some(a) = arxiv(e) {
            let _ = write!(s, ", arXiv:{a}");
            t.arxiv = Some(a.clone());
        }
        if let Some(u) = f("url") {
            let _ = write!(s, ", \\url{{{u}}}");
            t.url = Some(u);
        }
        if let Some(m) = f("mrnumber") {
            let _ = write!(s, ". MR~{m}");
            t.mr = Some(m);
        }
        if let Some(z) = f("zbl") {
            let _ = write!(s, ". Zbl~{z}");
            t.zbl = Some(z);
        }
        s.push('.');
    };
    let title = f("title").unwrap_or_default();
    t.title = Some(title.clone());
    t.venue = venue(e).cloned();
    if style == "authoryear" {
        let list: Vec<String> = people.iter().map(|n| format!("{}, {}", n.family, initials(&n.given))).collect();
        let who = match (list.as_slice(), more) {
            (l, true) => format!("{} et~al.", l.join(", ")),
            ([a], false) => a.clone(),
            ([a, b], false) => format!("{a} and {b}"),
            (l, false) => {
                let (last, first) = l.split_last().map_or((String::new(), &[][..]), |(x, r)| (x.clone(), r));
                format!("{}, and {last}", first.join(", "))
            }
        };
        let _ = write!(s, "{who} ({year}). {title}.");
        if let Some(j) = f("journal") {
            let _ = write!(s, " \\emph{{{j}}}");
            if let Some(v) = f("volume") {
                let _ = write!(s, ", {v}");
                t.volume = Some(v);
            }
            if let Some(n) = f("number") {
                let _ = write!(s, "({n})");
                t.number = Some(n);
            }
            if let Some(p) = f("pages") {
                let _ = write!(s, ":{p}");
                t.pages = Some(p);
            }
        } else if let Some(b) = f("booktitle") {
            let _ = write!(s, " In \\emph{{{b}}}");
            if let Some(p) = f("pages") {
                let _ = write!(s, ", pages {p}");
                t.pages = Some(p);
            }
        } else if let Some(v) = venue(e) {
            let _ = write!(s, " {v}");
        } else if e.kind == "misc" {
            s.push_str(" Preprint");
        }
        tail(&mut s, &mut t);
        return (s, t);
    }
    let list: Vec<String> = people.iter().map(|n| format!("{}~{}", initials(&n.given), n.family)).collect();
    let who = match (list.as_slice(), more) {
        (l, true) => format!("{} et~al.", l.join(", ")),
        ([a], false) => a.clone(),
        ([a, b], false) => format!("{a} and {b}"),
        (l, false) => {
            let (last, first) = l.split_last().map_or((String::new(), &[][..]), |(x, r)| (x.clone(), r));
            format!("{}, and {last}", first.join(", "))
        }
    };
    let _ = write!(s, "{who}, \\emph{{{title}}}");
    if let Some(j) = f("journal") {
        let _ = write!(s, ", {j}");
        if let Some(v) = f("volume") {
            let _ = write!(s, " \\textbf{{{v}}}");
            t.volume = Some(v);
        }
        let _ = write!(s, " ({year})");
        if let Some(n) = f("number") {
            let _ = write!(s, ", no.~{n}");
            t.number = Some(n);
        }
        if let Some(p) = f("pages") {
            let _ = write!(s, ", {p}");
            t.pages = Some(p);
        }
    } else {
        if let Some(b) = f("booktitle") {
            let _ = write!(s, ", {b}");
        }
        if let (Some(p), true) = (f("publisher"), f("booktitle").is_some()) {
            let _ = write!(s, ", {p}");
        } else if let Some(v) = venue(e).filter(|_| f("booktitle").is_none()) {
            let _ = write!(s, ", {v}");
        } else if e.kind == "misc" {
            s.push_str(", preprint");
        }
        let _ = write!(s, ", {year}");
        if let Some(p) = f("pages") {
            let _ = write!(s, ", pp.~{p}");
            t.pages = Some(p);
        }
    }
    tail(&mut s, &mut t);
    (s, t)
}

fn preamble(paper: &Paper) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "% Generated by `cargo xtask bib-oracle generate` from papers.toml. Do not edit.");
    let _ = writeln!(s, "\\documentclass[{}]{{{}}}", paper.options.join(","), paper.class);
    s.push_str("\\usepackage{amsmath,amssymb,amsthm}\n\\usepackage{url}\n");
    if let Some(g) = &paper.geometry {
        let _ = writeln!(s, "\\usepackage[{g}]{{geometry}}");
    }
    if !paper.natbib.is_empty() {
        let _ = writeln!(s, "\\usepackage[{}]{{natbib}}", paper.natbib);
    }
    if paper.hyperref {
        s.push_str("\\usepackage{hyperref}\n");
    }
    s.push_str("\\usepackage{pdfcraftbib}\n");
    let _ = writeln!(s, "\\pagestyle{{{}}}", paper.pagestyle);
    s.push_str("\\newtheorem{theorem}{Theorem}\n");
    if paper.hand == "numdot" {
        s.push_str("\\makeatletter\\renewcommand\\@biblabel[1]{#1.}\\makeatother\n");
    } else if paper.hand == "authoryear" {
        s.push_str("\\makeatletter\\renewcommand\\@biblabel[1]{}\\makeatother\n");
    }
    s.push_str("\\relpenalty=10000\n\\binoppenalty=10000\n\\sloppy\n\\begin{document}\n");
    s
}

struct Source {
    tex: String,
    cites: Vec<CiteSpec>,
    traps: Vec<TrapSpec>,
    ranges: Vec<(String, String)>,
    /// For a list by hand, its entries.
    hand: Option<Vec<EntrySpec>>,
    pool: Vec<String>,
}

/// The year of each entry as an author-year list prints it: with `a`, `b`, … where entries of one
/// author list share a year (in list order).
fn year_suffixes(order: &[String], library: &[BibEntry]) -> HashMap<String, String> {
    let mut groups: HashMap<(String, String), Vec<String>> = HashMap::new();
    for k in order {
        let Some(e) = library.iter().find(|e| &e.key == k) else { continue };
        let who: Vec<String> = names(e.fields.get("author").map_or("", String::as_str)).0.into_iter().map(|n| n.family).collect();
        let year = e.fields.get("year").cloned().unwrap_or_default();
        groups.entry((who.join("|"), year)).or_default().push(k.clone());
    }
    let mut out = HashMap::new();
    for ((_, year), keys) in groups {
        for (i, k) in keys.iter().enumerate() {
            let suffix = if keys.len() > 1 { char::from(b'a'.saturating_add(i as u8)).to_string() } else { String::new() };
            out.insert(k.clone(), format!("{year}{suffix}"));
        }
    }
    out
}

fn source(paper: &Paper, library: &[BibEntry]) -> anyhow::Result<Source> {
    let mut rng = Rng::new(paper.seed);
    let lines: Vec<&str> = paper.body.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
    // The pool: keys the body names, then others drawn by the seed.
    let mut pool: Vec<String> = Vec::new();
    for line in &lines {
        for w in line.split_whitespace().skip(1) {
            if library.iter().any(|e| e.key == w) && !pool.iter().any(|k| k == w) {
                pool.push(w.to_owned());
            }
        }
    }
    let mut rest: Vec<&str> = library.iter().map(|e| e.key.as_str()).filter(|k| !pool.iter().any(|p| p == k)).collect();
    let want = if paper.pool == 0 { library.len() } else { paper.pool };
    while pool.len() < want && !rest.is_empty() {
        let i = rng.below(rest.len());
        pool.push(rest.swap_remove(i).to_owned());
    }
    // A list by hand: numbered in pool order; author-year sorted by family names, then year.
    let hand = (paper.style == "hand").then(|| {
        let mut order = pool.clone();
        if paper.hand == "authoryear" {
            let sort_key = |k: &String| {
                let e = library.iter().find(|e| &e.key == k);
                let who: Vec<String> = e
                    .map(|e| names(e.fields.get("author").map_or("", String::as_str)).0.into_iter().map(|n| fold(&n.family)).collect())
                    .unwrap_or_default();
                (who, e.and_then(|e| e.fields.get("year").cloned()).unwrap_or_default(), k.clone())
            };
            order.sort_by_key(sort_key);
        }
        let year = year_suffixes(&order, library);
        HandList { order, year }
    });
    let mut w = Writer {
        paper,
        library,
        rng,
        out: preamble(paper),
        pool: pool.clone(),
        next: 0,
        serial: 0,
        cites: Vec::new(),
        traps: Vec::new(),
        ranges: Vec::new(),
        hand,
        theorems: 0,
    };
    let title = format!("{} {} of {} {}s", capital(w.rng.pick(&ADJ)), w.rng.pick(&TITLE_WORDS).to_lowercase(), w.rng.pick(&ADJ), w.rng.pick(&NOUN));
    let _ = write!(w.out, "\\title{{{title}}}\n\\author{{A. Writer}}\n\\date{{}}\n\\maketitle\n\n");
    for line in lines {
        w.line(line).with_context(|| format!("paper `{}`: `{line}`", paper.id))?;
    }
    w.out.push_str("\\end{document}\n");
    let hand = w.hand.as_ref().map(|h| {
        h.order
            .iter()
            .enumerate()
            .filter_map(|(i, k)| {
                let e = library.iter().find(|e| &e.key == k)?;
                let (_, fields) = hand_entry(e, &paper.hand, h.year.get(k).map_or("", String::as_str));
                let label = (paper.hand != "authoryear").then(|| (i + 1).to_string());
                Some(EntrySpec { key: k.clone(), label, fields })
            })
            .collect()
    });
    Ok(Source { tex: w.out, cites: w.cites, traps: w.traps, ranges: w.ranges, hand, pool })
}

// --- The truth of a BibTeX list --------------------------------------------------------------------

/// The entries of a `.bbl` file in order: key, the optional label argument, and the entry's text.
fn read_bbl(text: &str) -> Vec<(String, Option<String>, String)> {
    let mut out = Vec::new();
    let body = text.split("\\end{thebibliography}").next().unwrap_or("");
    for chunk in body.split("\\bibitem").skip(1) {
        let mut rest = chunk;
        let mut label = None;
        if let Some(r) = rest.strip_prefix('[') {
            // The optional argument, with its braces balanced.
            let mut depth = 0i32;
            let mut end = None;
            for (i, c) in r.char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    ']' if depth == 0 => {
                        end = Some(i);
                        break;
                    }
                    _ => {}
                }
            }
            let Some(end) = end else { continue };
            label = r.get(..end).map(str::to_owned);
            rest = r.get(end + 1..).unwrap_or("");
        }
        let Some(r) = rest.strip_prefix('{') else { continue };
        let Some((key, text)) = r.split_once('}') else { continue };
        out.push((key.to_owned(), label, text.to_owned()));
    }
    out
}

/// What a typeset entry shows of its `.bib` fields: each field whose value (after
/// [`fold`]) is in the entry's text.
fn printed_fields(e: &BibEntry, text: &str) -> FieldsTruth {
    let shown = fold(text);
    let has = |v: &str| {
        let v = fold(v);
        !v.is_empty() && shown.contains(&v)
    };
    let f = |k: &str| e.fields.get(k).filter(|v| has(v)).cloned();
    let (people, _) = names(e.fields.get("author").map_or("", String::as_str));
    let year = e.fields.get("year").and_then(|y| {
        // The year standing alone (not inside an MR number or a page range), with a suffix the
        // style adds (`2020a`).
        let fy = fold(y);
        let bytes = shown.as_bytes();
        let mut from = 0;
        while let Some(at) = shown.get(from..).and_then(|r| r.find(&fy)).map(|i| i + from) {
            from = at + fy.len();
            let before = at.checked_sub(1).and_then(|i| bytes.get(i)).copied();
            let after = bytes.get(at + fy.len()).copied();
            let after2 = bytes.get(at + fy.len() + 1).copied();
            if before.is_some_and(|b| b.is_ascii_alphanumeric()) || after.is_some_and(|b| b.is_ascii_digit()) {
                continue;
            }
            return Some(match after {
                Some(c) if c.is_ascii_lowercase() && !after2.is_some_and(|n| n.is_ascii_alphanumeric()) => format!("{y}{}", char::from(c)),
                Some(c) if c.is_ascii_alphabetic() => continue,
                _ => y.clone(),
            });
        }
        None
    });
    FieldsTruth {
        authors: people.into_iter().map(|n| n.family).filter(|f| has(f)).collect(),
        title: f("title"),
        venue: venue(e).filter(|v| has(v)).cloned(),
        year,
        volume: f("volume"),
        number: f("number"),
        pages: f("pages"),
        doi: f("doi"),
        arxiv: arxiv(e).filter(|v| has(v)).cloned(),
        url: f("url"),
        mr: f("mrnumber"),
        zbl: f("zbl"),
    }
}

/// The printed label from a `.bbl` optional argument, as TeX (scoring folds accents and braces):
/// `~` a space.
fn clean_label(s: &str) -> String {
    s.replace('~', " ")
}

fn mode(paper: &Paper) -> &'static str {
    match (paper.style.as_str(), paper.natbib.as_str(), paper.hand.as_str()) {
        (_, "authoryear", _) | ("hand", _, "authoryear") => "authoryear",
        ("alpha", _, _) => "alpha",
        _ => "numeric",
    }
}

fn read_list(path: &Path) -> anyhow::Result<List> {
    let list: List = toml::from_str(&std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?)
        .with_context(|| format!("parsing {}", path.display()))?;
    let mut ids = HashSet::new();
    for p in &list.paper {
        let ok_id = !p.id.is_empty() && p.id.len() <= 40 && p.id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
        if !ok_id || !ids.insert(p.id.as_str()) {
            bail!("paper `{}`: the id must be unique, [a-z0-9_]", p.id);
        }
        if p.holdout != p.id.starts_with("ho_") {
            bail!("paper `{}`: a held-out paper says `holdout = true` and its id starts with `ho_`; no other does", p.id);
        }
        if !matches!(p.class.as_str(), "article" | "amsart") {
            bail!("paper `{}`: class article or amsart", p.id);
        }
        if !matches!(p.style.as_str(), "plain" | "alpha" | "abbrv" | "unsrt" | "amsplain" | "apalike" | "plainnat" | "hand") {
            bail!("paper `{}`: style plain|alpha|abbrv|unsrt|amsplain|apalike|plainnat|hand", p.id);
        }
        if (p.style == "hand") != !p.hand.is_empty() || !matches!(p.hand.as_str(), "" | "numbracket" | "numdot" | "authoryear") {
            bail!("paper `{}`: a list by hand says how (numbracket|numdot|authoryear), and only it", p.id);
        }
        if !matches!(p.natbib.as_str(), "" | "numbers" | "authoryear") || (p.natbib.is_empty() == matches!(p.style.as_str(), "apalike" | "plainnat"))
        {
            bail!("paper `{}`: natbib (numbers|authoryear) with apalike and plainnat, and only with them", p.id);
        }
        if p.body.contains(['%', '\\', '$', '&', '#', '^', '{', '}']) {
            bail!("paper `{}`: the body is directives and plain words only", p.id);
        }
    }
    for linked in [false, true] {
        let all = list.paper.iter().filter(|p| p.hyperref == linked).count();
        let holdout = list.paper.iter().filter(|p| p.hyperref == linked && p.holdout).count();
        if holdout * 3 < all {
            bail!("{holdout} of {all} papers are held out; at least a third must be");
        }
    }
    Ok(list)
}

fn info(paper: &Paper) -> (PaperInfo, BibPaper) {
    let size = paper.options.iter().find(|o| o.ends_with("pt")).cloned().unwrap_or_else(|| "10pt".to_owned());
    let columns = if paper.options.iter().any(|o| o == "twocolumn") { "2col" } else { "1col" };
    let style = if paper.style == "hand" { format!("hand-{}", paper.hand) } else { paper.style.clone() };
    (
        PaperInfo { doc: paper.id.clone(), holdout: paper.holdout, class: paper.class.clone(), columns: columns.to_owned(), size },
        BibPaper { doc: paper.id.clone(), style, mode: mode(paper).to_owned(), hyperref: paper.hyperref },
    )
}

/// Typesets `stem` in `work`: to DVI, BibTeX (unless the list is by hand), to DVI twice and lists
/// it, then to PDF twice with the brackets and twice without (the `.bbl` copied), and requires
/// identical PDFs (with hyperref: the same glyphs within [`HYPERREF_SHIFT_SP`]), AMS Type 1 fonts
/// only, and no citation left undefined. Returns the pages, the PDF and the `.bbl`.
fn build(
    tex: &Tex,
    bibtex: Option<&pdfcraft_oracle::Oracle>,
    work: &Path,
    paper: &Paper,
    src: &str,
    packages: &[(&str, String, String)],
    library: &str,
) -> anyhow::Result<(Vec<DviPage>, Vec<u8>, Option<String>)> {
    let (stem, hyperref) = (paper.id.as_str(), paper.hyperref);
    let dir = work.join(stem);
    let plain = dir.join("plain");
    std::fs::create_dir_all(&plain)?;
    for d in [&dir, &plain] {
        std::fs::write(d.join(format!("{stem}.tex")), src)?;
        std::fs::write(d.join(LIBRARY), library)?;
    }
    for (name, on, off) in packages {
        std::fs::write(dir.join(name), on)?;
        std::fs::write(plain.join(name), off)?;
    }
    let timeout = Duration::from_secs(300);
    tex.compile(&dir, stem, Output::Dvi, timeout)?;
    let bbl = match bibtex {
        Some(b) => {
            tex.bibtex(b, &dir, stem)?;
            let text = std::fs::read_to_string(dir.join(format!("{stem}.bbl")))?;
            std::fs::write(plain.join(format!("{stem}.bbl")), &text)?;
            Some(text)
        }
        None => None,
    };
    tex.compile(&dir, stem, Output::Dvi, timeout)?;
    tex.compile(&dir, stem, Output::Dvi, timeout)?;
    let pages = tex.list(&dir, stem)?;
    for d in [&dir, &plain] {
        tex.compile(d, stem, Output::Pdf, timeout)?;
        tex.compile(d, stem, Output::Pdf, timeout)?;
        tex.compile(d, stem, Output::Pdf, timeout)?;
    }
    let pdf = std::fs::read(dir.join(format!("{stem}.pdf")))?;
    if pdf != std::fs::read(plain.join(format!("{stem}.pdf")))? {
        if !hyperref {
            bail!("{stem}.pdf differs when the \\special brackets expand to nothing: a bracket moved something");
        }
        tex.compile(&plain, stem, Output::Dvi, timeout)?;
        let bare = tex.list(&plain, stem)?;
        let a = pages.iter().flat_map(|p| p.items.iter().map(|(i, _)| i));
        let b: Vec<_> = bare.iter().flat_map(|p| p.items.iter().map(|(i, _)| i)).collect();
        if pages.len() != bare.len() || a.clone().count() != b.len() {
            bail!("{stem}: with hyperref, the brackets changed what was set");
        }
        let shift = a.zip(&b).map(|(x, y)| if x.mark == y.mark { (x.h - y.h).abs().max((x.v - y.v).abs()) } else { i64::MAX }).max().unwrap_or(0);
        if shift > HYPERREF_SHIFT_SP {
            bail!("{stem}: with hyperref, the brackets moved a glyph by {shift} sp (at most {HYPERREF_SHIFT_SP})");
        }
        println!("{stem}: hyperref: the brackets move no glyph by more than {shift} sp");
    }
    let log = std::fs::read_to_string(dir.join(format!("{stem}.log")))?;
    if log.contains("There were undefined") || log.contains("Citation `") || log.contains("Reference `") {
        bail!("{stem}: a citation or reference is undefined");
    }
    let fonts: Vec<&str> = log.split('<').filter_map(|s| s.split_once('>').map(|(f, _)| f)).filter(|f| f.ends_with(".pfb")).collect();
    if fonts.is_empty() {
        bail!("{stem}.log names no embedded Type 1 font");
    }
    if let Some(f) = fonts.iter().find(|f| !f.contains(AMS_FONT_DIR)) {
        bail!("{stem}.pdf embeds {f}, which is not an AMS Type 1 font (OFL-1.1)");
    }
    Ok((pages, pdf, bbl))
}

/// The held-out papers' fingerprints, one `<sha256>  <what>` line each: the source, the PDF and the
/// paper's truth rows.
const LOCK: &str = "holdout.lock";

fn holdout_lock(list: &List, files: &[(String, Vec<u8>)], tsv: &str) -> String {
    let mut s = String::from(
        "# Generated by `cargo xtask bib-oracle generate --freeze`. Do not edit: a held-out paper is frozen before it\n\
         # is scored, and `generate` fails if one changes (README.md, \"Held-out papers\").\n",
    );
    for p in list.paper.iter().filter(|p| p.holdout) {
        for name in [format!("{}.tex", p.id), format!("{}.pdf", p.id)] {
            let bytes = files.iter().find(|(n, _)| *n == name).map(|(_, b)| b.as_slice()).unwrap_or_default();
            let _ = writeln!(s, "{}  {name}", crate::assets::sha256_hex(bytes));
        }
        let rows: String = tsv.lines().filter(|l| l.split('\t').nth(1) == Some(p.id.as_str())).map(|l| format!("{l}\n")).collect();
        let _ = writeln!(s, "{}  truth.tsv rows of {}", crate::assets::sha256_hex(rows.as_bytes()), p.id);
    }
    s
}

fn generate(args: &[String]) -> anyhow::Result<()> {
    let scratch = WorkDir::new("bib-oracle")?;
    if args.iter().any(|a| a == "--keep") {
        let kept = scratch.keep();
        eprintln!("bib-oracle: keeping TeX's files in {}", kept.display());
        return generate_in(args, &kept);
    }
    generate_in(args, scratch.path())
}

fn generate_in(args: &[String], scratch: &Path) -> anyhow::Result<()> {
    let data = root().join(DATA_DIR);
    let check = args.iter().any(|a| a == "--check");
    let out: PathBuf = if check { scratch.join("out") } else { data.clone() };
    let list = read_list(&data.join("papers.toml"))?;
    let library_text = std::fs::read_to_string(data.join(LIBRARY)).with_context(|| format!("reading {LIBRARY}"))?;
    let library = read_library(&library_text)?;
    let tex = Tex::find().map_err(|absent| anyhow::anyhow!("{absent}"))?;
    let bibtex = pdfcraft_oracle::bibtex().map_err(|absent| anyhow::anyhow!("{absent}"))?;
    let packages = [(LAYOUT_PACKAGE, layout::package(true), layout::package(false)), (PACKAGE, bib::package(true), bib::package(false))];
    let mut truth = BibTruth::default();
    let mut files: Vec<(String, Vec<u8>)> = packages.iter().map(|(n, on, _)| ((*n).to_owned(), on.clone().into_bytes())).collect();
    for paper in &list.paper {
        let mut src = source(paper, &library)?;
        let by_hand = paper.style == "hand";
        let (pages, pdf, bbl) = build(&tex, (!by_hand).then_some(&bibtex), scratch, paper, &src.tex, &packages, &library_text)
            .with_context(|| format!("paper `{}`", paper.id))?;
        if pages.len() > MAX_PAGES {
            bail!("paper `{}` has {} pages; at most {MAX_PAGES}", paper.id, pages.len());
        }
        let entries: Vec<EntrySpec> = match (&src.hand, &bbl) {
            (Some(hand), _) => hand.clone(),
            (None, Some(bbl)) => {
                let numeric = mode(paper) == "numeric";
                read_bbl(bbl)
                    .into_iter()
                    .enumerate()
                    .map(|(i, (key, label, text))| {
                        let e = library.iter().find(|e| e.key == key).with_context(|| format!("`{key}` of the .bbl is not in {LIBRARY}"))?;
                        let label = match mode(paper) {
                            _ if numeric => Some((i + 1).to_string()),
                            "alpha" => label.map(|l| clean_label(&l)),
                            _ => None,
                        };
                        Ok(EntrySpec { key, label, fields: printed_fields(e, &text) })
                    })
                    .collect::<anyhow::Result<_>>()?
            }
            (None, None) => bail!("paper `{}`: no list", paper.id),
        };
        if entries.len() != src.pool.len() {
            bail!("paper `{}`: the list has {} entries, the pool {}", paper.id, entries.len(), src.pool.len());
        }
        // Keys inside a range: those numbered between its ends.
        for (a, b) in &src.ranges {
            let pos = |k: &str| entries.iter().position(|e| e.key == k);
            let (Some(i), Some(j)) = (pos(a), pos(b)) else { bail!("paper `{}`: a range of keys not listed", paper.id) };
            if j <= i + 1 {
                bail!("paper `{}`: the range {a}..{b} is numbered {}..{}; it must hold a key between its ends", paper.id, i + 1, j + 1);
            }
            for e in entries.get(i + 1..j).unwrap_or_default() {
                src.cites.push(CiteSpec { serial: None, key: e.key.clone(), form: "range_inner".into() });
            }
        }
        let (li, bi) = info(paper);
        files.push((format!("{}.tex", paper.id), src.tex.clone().into_bytes()));
        files.push((format!("{}.pdf", paper.id), pdf));
        if let Some(bbl) = bbl {
            files.push((format!("{}.bbl", paper.id), bbl.into_bytes()));
        }
        if paper.hyperref {
            println!("{:<22} {} pages, {} entries, {} citations, linked (no DVI truth)", paper.id, pages.len(), entries.len(), src.cites.len());
            truth.papers.push(bi);
            continue;
        }
        let lp = layout::build(&paper.id, &pages).with_context(|| format!("paper `{}`", paper.id))?;
        for p in &lp {
            let unowned = p.unowned.iter().filter(|&&i| matches!(p.items.get(i).map(|i| &i.mark), Some(Mark::Glyph { .. }))).count();
            if unowned > 0 {
                bail!("paper `{}` page {}: {unowned} glyphs belong to no block", paper.id, p.page);
            }
        }
        let (head, et, ct, tt) =
            bib::build(&paper.id, &pages, &lp, &entries, &src.cites, &src.traps).with_context(|| format!("paper `{}`", paper.id))?;
        println!(
            "{:<22} {} pages, {} entries, {} citations ({} inside ranges), {} traps",
            paper.id,
            lp.len(),
            et.len(),
            ct.len(),
            ct.iter().filter(|c| c.spec.serial.is_none()).count(),
            tt.len()
        );
        truth.layout.papers.push(li);
        truth.layout.pages.extend(lp);
        truth.papers.push(bi);
        truth.headings.push((paper.id.clone(), head));
        truth.entries.extend(et);
        truth.cites.extend(ct);
        truth.traps.extend(tt);
    }
    let header = format!(
        "Generated by `cargo xtask bib-oracle generate` from papers.toml and library.bib with {} and {}. Do not edit.\n\
         Rows: see pdfcraft_oracle::tex::bib::BibTruth. Positions in sp from the DVI origin.",
        tex.version(),
        bibtex.version()
    );
    let tsv = truth.to_tsv(&header);
    let lock = holdout_lock(&list, &files, &tsv);
    let frozen = std::fs::read_to_string(data.join(LOCK)).unwrap_or_default();
    if args.iter().any(|a| a == "--freeze") {
        if frozen.lines().any(|f| !f.starts_with('#') && !lock.lines().any(|l| l == f)) {
            bail!("--freeze only adds held-out papers; one already frozen would change");
        }
        let fresh = lock.lines().filter(|l| !frozen.lines().any(|f| f == *l)).count();
        println!("bib-oracle: freezing {fresh} new held-out entries");
        files.push((LOCK.to_owned(), lock.into_bytes()));
    } else if lock != frozen {
        bail!(
            "the held-out papers differ from {LOCK}. A held-out paper is frozen before it is scored (README.md), so \
             nothing tuned after its scores were read can pass as held out. Do not edit one: move it to dev (a new id \
             without `ho_`) and write a new one, then `generate --freeze`.\nexpected:\n{frozen}\ngenerated:\n{lock}"
        );
    } else {
        files.push((LOCK.to_owned(), lock.into_bytes()));
    }
    files.push(("truth.tsv".to_owned(), tsv.into_bytes()));
    std::fs::create_dir_all(&out)?;
    for (name, bytes) in &files {
        std::fs::write(out.join(name), bytes).with_context(|| format!("writing {name}"))?;
    }
    if check {
        let differ: Vec<&str> =
            files.iter().map(|(n, _)| n.as_str()).filter(|n| std::fs::read(out.join(n)).ok() != std::fs::read(data.join(n)).ok()).collect();
        if !differ.is_empty() {
            bail!("generated files differ from the committed ones: {differ:?}");
        }
        println!("bib-oracle: every generated file is byte-identical with {DATA_DIR}");
    } else {
        println!("bib-oracle: wrote {} files to {}", files.len(), out.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_read_as_bibtex_reads_them() {
        let (n, more) = names(r"Tom\'as Lindqvist and Holm, Kwame and Mateus van der Berg and others");
        assert!(more);
        let fam: Vec<&str> = n.iter().map(|n| n.family.as_str()).collect();
        assert_eq!(fam, ["Lindqvist", "Holm", "van der Berg"]);
        assert_eq!(initials(r"Tom\'as"), "T.");
        assert_eq!(initials(r"{\'E}lodie"), r"{\'E}.");
        assert_eq!(initials("Ana-Luc{\\'\\i}a"), "A.-L.");
        assert_eq!(initials("Jean-Baptiste Paul"), "J.-B.~P.");
    }

    #[test]
    fn a_bbl_reads_as_keys_labels_and_text() {
        let bbl = "\\begin{thebibliography}{1}\n\\bibitem[{\\c{C}}el20]{a01}\nC. Celik, \\emph{T}, 2020.\n\n\\bibitem{a02}\nB. Kay.\n\\end{thebibliography}\n";
        let r = read_bbl(bbl);
        assert_eq!(r.len(), 2);
        assert_eq!((r[0].0.as_str(), r[0].1.as_deref()), ("a01", Some("{\\c{C}}el20")));
        assert_eq!(fold(&clean_label("{\\c{C}}el20")), "cel20");
        assert_eq!(r[1].1, None);
    }
}
