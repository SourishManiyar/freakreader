//! `cargo xtask refs-oracle`: the statement and reference oracle (`crates/render/tests/data/refs/oracle`).
//!
//! - `generate [--check] [--keep]` writes each paper of `papers.toml` as LaTeX with amsthm
//!   statements, proofs, labels and mentions (the prose comes from the layout oracle's seeded
//!   generator and word lists: no text from papers or books), typesets it with pdfTeX to DVI (twice,
//!   for the labels) and PDF, and writes the PDFs, the sources, the two bracket packages and
//!   `truth.tsv`. `--check` writes to a temporary directory and fails unless every file is
//!   byte-identical with the committed one. Needs TeX Live; nothing else does.
//! - `report [ARGS]` scores the analysis (`crates/render/examples/refs_oracle.rs`).
//! - `test` runs the reference oracle's tests.
//!
//! The truth comes from DVI ([`pdfcraft_oracle::tex::refs`] on top of
//! [`pdfcraft_oracle::tex::layout`]). Each PDF is compiled again with both bracket packages reduced
//! to empty macros and must come out byte-identical; every embedded font must be an AMS Type 1 font
//! (OFL-1.1); no reference may be left undefined.

use crate::gates::root;
use crate::layout_oracle::{ADJ, ALIGNS, EQUATIONS, MATH, NOUN, PACKAGE as LAYOUT_PACKAGE, Rng, TITLE_WORDS, sentence, sentences};
use crate::math_oracle::{AMS_FONT_DIR, cargo};
use anyhow::{Context, bail};
use pdfcraft_oracle::tex::layout::{self, LayoutTruth, PaperInfo};
use pdfcraft_oracle::tex::refs::{self, Anchor, MentionSpec, RefsPaper, RefsTruth, StatementSpec, TargetKind, TargetSpec};
use pdfcraft_oracle::tex::{DviPage, Mark, Output, Tex, WorkDir};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The paper list and the generated files, relative to the repository root.
pub const DATA_DIR: &str = "crates/render/tests/data/refs/oracle";
const PACKAGE: &str = "pdfcraftrefs.sty";
/// Most pages one paper may have.
const MAX_PAGES: usize = 12;
/// How far the brackets may move a glyph of a paper that loads hyperref, in sp (0.2 pt); without
/// hyperref the PDF must be byte-identical.
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
    /// How the plain style sets its heads: `bold` (amsthm's own), `italic` (body upright),
    /// `smallcaps` (body italic).
    heads: String,
    /// Statements and equations numbered within sections (`section`) or through the paper (`global`).
    numbering: String,
    /// Lemma, Proposition, Corollary, … share Theorem's counter.
    shared: bool,
    /// The end-of-proof mark: `open` (amsthm's box of rules), `black` ($\blacksquare$), `square`
    /// ($\square$), `text` (QED).
    qed: String,
    /// Load hyperref: the links are there to check the inferred references against.
    hyperref: bool,
    body: String,
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("generate") => generate(rest),
        Some("report") => cargo(&["run", "-q", "-p", "pdfcraft-render", "--example", "refs_oracle", "--"], rest),
        Some("test") => cargo(&["test", "-p", "pdfcraft-render", "--test", "it", "refs_oracle", "--"], rest),
        _ => bail!("usage: cargo xtask refs-oracle generate [--check] [--keep] | report [ARGS] | test"),
    }
}

/// The statement environments every paper defines: environment, printed word, theorem style
/// (`plain` takes the paper's head style), numbered.
const ENVS: [(&str, &str, &str, bool); 16] = [
    ("theorem", "Theorem", "plain", true),
    ("lemma", "Lemma", "plain", true),
    ("proposition", "Proposition", "plain", true),
    ("corollary", "Corollary", "plain", true),
    ("claim", "Claim", "plain", true),
    ("conjecture", "Conjecture", "plain", true),
    ("fact", "Fact", "plain", true),
    ("mainthm", "Main Theorem", "plain", false),
    ("utheorem", "Theorem", "plain", false),
    ("definition", "Definition", "definition", true),
    ("assumption", "Assumption", "definition", true),
    ("notation", "Notation", "definition", true),
    ("example", "Example", "definition", true),
    ("remark", "Remark", "remark", true),
    ("uremark", "Remark", "remark", false),
    ("observation", "Observation", "remark", true),
];

/// How mentions abbreviate a word.
fn abbreviation(word: &str) -> Option<&'static str> {
    Some(match word {
        "Theorem" => "Thm.",
        "Lemma" => "Lem.",
        "Proposition" => "Prop.",
        "Corollary" => "Cor.",
        "Definition" => "Def.",
        "Section" => "Sec.",
        "Figure" => "Fig.",
        "Table" => "Tab.",
        _ => return None,
    })
}

const NAMES: [&str; 6] = ["Smith", "Okafor", "Lindqvist", "Moreau", "Tanaka", "Rossi"];
/// Words a sentence may lead a reference with.
const LEADS: [&str; 6] = ["By", "From", "Using", "In view of", "Combining this with", "As shown in"];
/// What a bare equation number may follow: the words a reader takes for a reference.
const BARE_LEADS: [&str; 5] = ["by", "see", "from", "in", "using"];

/// A label: what kind of block it names and how a mention words it.
#[derive(Clone)]
struct Label {
    kind: TargetKind,
    word: String,
}

/// A statement or proof being written.
struct Open {
    env: String,
}

struct Writer<'a> {
    paper: &'a Paper,
    rng: Rng,
    out: String,
    twocolumn: bool,
    displays: usize,
    statements: Vec<StatementSpec>,
    targets: Vec<TargetSpec>,
    mentions: Vec<MentionSpec>,
    labels: HashMap<String, Label>,
    open: Option<Open>,
    /// The last statement written (not a proof): what an unnamed proof proves.
    last_statement: Option<String>,
}

fn arg<'a>(args: &[&'a str], key: &str) -> Option<&'a str> {
    args.iter().find_map(|a| a.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
}

fn count(args: &[&str]) -> usize {
    args.iter().find_map(|a| a.parse::<usize>().ok()).unwrap_or(0)
}

impl Writer<'_> {
    fn mention_id(&mut self) -> String {
        format!("m{}", self.mentions.len() + 1)
    }

    fn label(&self, key: &str) -> anyhow::Result<Label> {
        self.labels.get(key).cloned().with_context(|| format!("paper `{}`: label `{key}` is used before it is set", self.paper.id))
    }

    /// `\pdfcraftref{ID}TEXT\pdfcraftrefend{}`, recorded.
    fn bracket(&mut self, text: &str, target: Option<&str>, form: &str) -> String {
        let id = self.mention_id();
        self.mentions.push(MentionSpec { id: id.clone(), target: target.map(str::to_owned), form: form.to_owned() });
        format!("\\pdfcraftref{{{id}}}{text}\\pdfcraftrefend{{}}")
    }

    /// The words of a mention of `key`: `Lemma~\ref{key}`, `Lem.~\ref{key}`, `\eqref{key}`, …
    fn words(&self, key: &str, form: &str) -> anyhow::Result<String> {
        let l = self.label(key)?;
        Ok(match (l.kind, form) {
            (TargetKind::Equation, "bare") => format!("\\eqref{{{key}}}"),
            (TargetKind::Equation, "abbr") => format!("Eq.~\\eqref{{{key}}}"),
            (TargetKind::Equation, _) => format!("equation~\\eqref{{{key}}}"),
            (_, "abbr") => format!("{}~\\ref{{{key}}}", abbreviation(&l.word).unwrap_or(&l.word)),
            (_, "auto") => format!("\\autoref{{{key}}}"),
            _ => format!("{}~\\ref{{{key}}}", l.word),
        })
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

    /// One sentence holding the mention `m` (already bracketed).
    fn frame(&mut self, m: &str) -> String {
        let clause = self.clause();
        match self.rng.below(4) {
            0 => format!("{} {m}, {clause}.", self.rng.pick(&LEADS)),
            1 => format!("This follows from {m}, since {clause}."),
            2 => format!("Compare with {m}, where {clause}."),
            _ => format!("We use {m} again: {clause}."),
        }
    }

    fn say(&mut self, args: &[&str]) -> anyhow::Result<String> {
        let first = args.first().copied().unwrap_or("");
        Ok(match first {
            "ext" => {
                let key = args.get(1).copied().unwrap_or("");
                let words = self.words(key, "word")?;
                let m = self.bracket(&words, None, "external");
                let n = 1 + self.rng.below(19);
                let name = self.rng.pick(&NAMES);
                let tail = match self.rng.below(4) {
                    0 => format!("in~[{n}]"),
                    1 => format!("of~[{name}]"),
                    2 => format!("of the appendix of~[{n}]"),
                    _ => format!("in the book~[{n}]"),
                };
                let clause = self.clause();
                format!("As in {m} {tail}, {clause}.")
            }
            "missing" => {
                let word = args.get(1).copied().unwrap_or("Lemma");
                let number = args.get(2).copied().unwrap_or("9.9");
                let m = self.bracket(&format!("{word}~{number}"), None, "missing");
                self.frame(&m)
            }
            "noise" => {
                let math = self.rng.pick(&MATH);
                match args.get(1).copied().unwrap_or("") {
                    "case" => format!("The case ({}) is the easy one, and {}.", 1 + self.rng.below(3), self.clause()),
                    "this" => format!("The lemma above and the next section say why {math} is {}.", self.rng.pick(&ADJ)),
                    "interval" => format!("For every $t$ in $(0,1)$ the {} {math} is {}.", self.rng.pick(&NOUN), self.rng.pick(&ADJ)),
                    "count" => format!("There are {} cases, and in {} of them {}.", 2 + self.rng.below(7), 1 + self.rng.below(2), self.clause()),
                    other => bail!("unknown noise `{other}`"),
                }
            }
            _ => {
                let form = args.get(1).copied().unwrap_or("word");
                if let Some((a, b)) = first.split_once('+') {
                    // Two of one kind: `Lemmas 2.1 and 2.3`, `Lemmas 2.1–2.3`.
                    let (la, lb) = (self.label(a)?, self.label(b)?);
                    if la.word != lb.word || la.kind == TargetKind::Equation {
                        bail!("`say {first}`: two labels of one kind of statement, heading or float");
                    }
                    let plural = format!("{}s", la.word);
                    let ma = self.bracket(&format!("{plural}~\\ref{{{a}}}"), Some(a), "plural");
                    let mb = self.bracket(&format!("\\ref{{{b}}}"), Some(b), "plural");
                    let joined = if form == "range" { format!("{ma}--{mb}") } else { format!("{ma} and~{mb}") };
                    return Ok(self.frame(&joined));
                }
                let l = self.label(first)?;
                if form == "bare" {
                    if l.kind != TargetKind::Equation {
                        bail!("`say {first} bare`: only an equation is cited by its number alone");
                    }
                    // Inside a group of its own: `\eqref`'s `\textup` looks at the glue before it
                    // (LaTeX's italic-correction check), which a bracket there would hide.
                    let m = format!("\\textup{{{}}}", self.bracket(&format!("\\eqref{{{first}}}"), Some(first), "bare"));
                    let lead = self.rng.pick(&BARE_LEADS);
                    let clause = self.clause();
                    return Ok(match self.rng.below(3) {
                        0 => format!("{} {lead} {m}.", capital(&clause)),
                        1 => format!("{} {m}, {clause}.", capital(lead)),
                        _ => format!("{}, {lead} {m}.", capital(&clause)),
                    });
                }
                let words = self.words(first, form)?;
                let m = if form == "extra" {
                    let roman = self.rng.pick(&["i", "ii", "iii"]);
                    format!("{}\\pdfcraftextra({roman})\\pdfcraftextraend{{}}", self.bracket(&words, Some(first), "extra"))
                } else {
                    self.bracket(&words, Some(first), form)
                };
                self.frame(&m)
            }
        })
    }

    fn statement(&mut self, args: &[&str]) -> anyhow::Result<()> {
        if self.open.is_some() {
            bail!("paper `{}`: a statement inside a statement", self.paper.id);
        }
        let env = args.first().copied().unwrap_or("");
        let Some(&(_, word, style, numbered)) = ENVS.iter().find(|e| e.0 == env) else { bail!("unknown statement `{env}`") };
        let id = format!("s{}", self.statements.len() + 1);
        let name = arg(args, "name").map(|n| n.replace('+', " "));
        let style = if style == "plain" { self.paper.heads.as_str() } else { style };
        self.statements.push(StatementSpec {
            id: id.clone(),
            kind: word.to_owned(),
            numbered,
            name: name.clone(),
            style: style.to_owned(),
            proof: false,
            proves: None,
            end_mark: false,
        });
        let _ = write!(self.out, "\n\n\\pdfcraftthm{{{id}}}\\begin{{{env}}}");
        if let Some(n) = &name {
            let _ = write!(self.out, "[{n}]");
        }
        if let Some(key) = arg(args, "label") {
            self.set_label(key, TargetKind::Statement, word, Anchor::Statement(id.clone()))?;
            let _ = write!(self.out, "\\label{{{key}}}");
        }
        self.out.push('\n');
        let n = count(args);
        let text = sentences(&mut self.rng, n);
        self.out.push_str(&text);
        self.last_statement = Some(id);
        self.open = Some(Open { env: env.to_owned() });
        Ok(())
    }

    fn proof(&mut self, args: &[&str]) -> anyhow::Result<()> {
        if self.open.is_some() {
            bail!("paper `{}`: a proof inside a statement", self.paper.id);
        }
        let id = format!("s{}", self.statements.len() + 1);
        let of = arg(args, "of");
        let proves = match of {
            Some(key) => match self.labels.get(key).map(|l| l.kind) {
                Some(TargetKind::Statement) => self.targets.iter().find(|t| t.id == key).and_then(|t| match &t.anchor {
                    Anchor::Statement(s) => Some(s.clone()),
                    _ => None,
                }),
                _ => bail!("`proof of={key}`: not a statement's label"),
            },
            None => self.last_statement.clone(),
        };
        let mark = !args.contains(&"nomark");
        let head = if let Some(key) = of {
            let words = self.words(key, "word")?;
            let m = self.bracket(&words, Some(key), "head");
            Some(format!("Proof of {m}"))
        } else if args.contains(&"sketch") {
            Some("Proof (sketch)".to_owned())
        } else {
            None
        };
        self.statements.push(StatementSpec {
            id: id.clone(),
            kind: "Proof".to_owned(),
            numbered: false,
            name: None,
            style: "proof".to_owned(),
            proof: true,
            proves,
            end_mark: mark,
        });
        let _ = write!(self.out, "\n\n\\pdfcraftthm{{{id}}}\\begin{{proof}}");
        if let Some(h) = head {
            let _ = write!(self.out, "[{h}]");
        }
        if !mark {
            self.out.push_str("\\renewcommand\\qedsymbol{}");
        }
        self.out.push('\n');
        let n = count(args);
        let text = sentences(&mut self.rng, n);
        self.out.push_str(&text);
        self.open = Some(Open { env: "proof".to_owned() });
        Ok(())
    }

    fn set_label(&mut self, key: &str, kind: TargetKind, word: &str, anchor: Anchor) -> anyhow::Result<()> {
        let ok = key.len() <= 40 && key.contains(':') && key.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b':');
        if !ok || self.targets.iter().any(|t| t.id == key) {
            bail!("paper `{}`: label `{key}` must be new, [a-z0-9:] with a colon", self.paper.id);
        }
        self.labels.insert(key.to_owned(), Label { kind, word: word.to_owned() });
        self.targets.push(TargetSpec { id: key.to_owned(), kind, anchor });
        Ok(())
    }

    fn display(&mut self, args: &[&str]) -> anyhow::Result<()> {
        self.displays += 1;
        let id = format!("{}_d{}", self.paper.id, self.displays);
        let env = args.first().copied().unwrap_or("equation");
        let pick = |rng: &mut Rng, list: &[&'static str]| list.get(rng.below(list.len())).copied().unwrap_or("");
        let (open, close, body) = match env {
            "equation" | "gather" => (format!("\\begin{{{env}}}"), format!("\\end{{{env}}}"), pick(&mut self.rng, &EQUATIONS)),
            "display" => ("\\[".to_owned(), "\\]".to_owned(), pick(&mut self.rng, &EQUATIONS)),
            "align" => ("\\begin{align}".to_owned(), "\\end{align}".to_owned(), pick(&mut self.rng, &ALIGNS)),
            other => bail!("paper `{}`: unknown display `{other}`", self.paper.id),
        };
        let mut tail = String::new();
        if args.contains(&"qedhere") {
            if !matches!(self.open.as_ref(), Some(o) if o.env == "proof") {
                bail!("paper `{}`: \\qedhere outside a proof", self.paper.id);
            }
            tail.push_str("\\qedhere");
        }
        if let Some(key) = arg(args, "label") {
            if env != "equation" {
                bail!("paper `{}`: only an equation takes a label", self.paper.id);
            }
            self.set_label(key, TargetKind::Equation, "equation", Anchor::Equation(id.clone()))?;
            let _ = write!(tail, "\\label{{{key}}}");
        }
        // On lines of their own so that no space lands beside a bracket.
        let _ = write!(self.out, "%\n{open}\\pdfcraftbegin{{{id}}}%\n{body}%\n\\pdfcraftend{{{id}}}{tail}{close}\n");
        Ok(())
    }

    fn float(&mut self, what: &str, args: &[&str]) -> anyhow::Result<()> {
        let placement = args.first().copied().unwrap_or("t");
        let wide = args.contains(&"wide");
        let env = if wide && self.twocolumn { format!("{what}*") } else { what.to_owned() };
        let width = if wide { "\\textwidth" } else { "\\columnwidth" };
        let caption = format!("{} {}", sentence(&mut self.rng).trim_end_matches('.'), "in the setting above.");
        let content = if what == "figure" {
            let labels: Vec<String> = (0..3).map(|_| format!("{} {}", self.rng.pick(&ADJ), self.rng.pick(&NOUN))).collect();
            format!("\\fbox{{\\parbox{{0.8{width}}}{{\\centering\\vspace{{4ex}}{}\\vspace{{4ex}}}}}}", labels.join(" "))
        } else {
            let mut rows = String::new();
            for _ in 0..3 {
                let _ = writeln!(rows, "{} & {} & {} \\\\", self.rng.pick(&NOUN), 10 + self.rng.below(90), self.rng.below(1000));
            }
            format!("\\begin{{tabular}}{{lrr}}\\hline\nname & size & count \\\\ \\hline\n{rows}\\hline\n\\end{{tabular}}")
        };
        let mut mark = String::new();
        let mut label = String::new();
        if let Some(key) = arg(args, "label") {
            let word = if what == "figure" { "Figure" } else { "Table" };
            self.set_label(key, TargetKind::Float, word, Anchor::Mark)?;
            mark = format!("\\pdfcraftmarkhere{{{key}}}");
            label = format!("\\label{{{key}}}");
        }
        let cap = format!("\\pdfcraftopen{{caption}}{mark}%\n\\caption{{{caption}}}{label}\\par\n\\pdfcraftclose{{caption}}%\n");
        let body = format!("\\pdfcraftopen{{float}}%\n{content}\\par\n\\pdfcraftclose{{float}}%\n");
        let inner = if what == "table" { format!("{cap}{body}") } else { format!("{body}{cap}") };
        let _ = write!(self.out, "\\begin{{{env}}}[{placement}]\\centering\n{inner}\\end{{{env}}}\n");
        Ok(())
    }

    fn line(&mut self, line: &str) -> anyhow::Result<()> {
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        let args: Vec<&str> = rest.split_whitespace().collect();
        match word {
            "title" => {
                let _ = writeln!(self.out, "\\title{{{rest}}}");
            }
            "author" => {
                let _ = writeln!(self.out, "\\author{{{rest}}}");
            }
            "date" => {
                let _ = writeln!(self.out, "\\date{{{rest}}}");
            }
            "maketitle" => self.out.push_str("\\maketitle\n"),
            "abstract" => {
                let text = sentences(&mut self.rng, count(&args));
                let _ = write!(self.out, "\\begin{{abstract}}\n{text}\n\\end{{abstract}}\n");
            }
            "section" | "subsection" | "section*" => {
                if self.open.is_some() {
                    bail!("paper `{}`: a heading inside a statement", self.paper.id);
                }
                let label = arg(&args, "label");
                let words: Vec<&str> = args.iter().copied().filter(|a| !a.starts_with("label=")).collect();
                let title = if words.is_empty() { format!("{} {}", self.rng.pick(&TITLE_WORDS), "on the problem") } else { words.join(" ") };
                self.out.push('\n');
                if let Some(key) = label {
                    self.set_label(key, TargetKind::Heading, "Section", Anchor::Mark)?;
                    let _ = write!(self.out, "\\pdfcraftmark{{{key}}}");
                }
                let _ = write!(self.out, "\\{word}{{{title}}}");
                if let Some(key) = label {
                    let _ = write!(self.out, "\\label{{{key}}}");
                }
                self.out.push('\n');
            }
            "appendix" => self.out.push_str("\n\\appendix\n"),
            "par" | "cont" => {
                let mut text = sentences(&mut self.rng, count(&args));
                if args.contains(&"footnote") {
                    let note = sentence(&mut self.rng);
                    let at = text.find(". ").map_or(text.len(), |i| i + 1);
                    text.insert_str(at, &format!("\\footnote{{{note}}}"));
                }
                if word == "par" {
                    self.out.push_str("\n\n");
                } else {
                    self.out.push('\n');
                }
                let _ = write!(self.out, "{text}");
            }
            "say" => {
                let s = self.say(&args)?;
                let _ = write!(self.out, "\n{s}");
            }
            "display" => self.display(&args)?,
            "list" => {
                let env = args.first().copied().unwrap_or("itemize");
                if !matches!(env, "itemize" | "enumerate") {
                    bail!("`{line}`: itemize or enumerate");
                }
                let n = args.get(1).and_then(|n| n.parse::<usize>().ok()).unwrap_or(3);
                let _ = write!(self.out, "\n\\begin{{{env}}}\\pdfcraftenv{{item}}");
                if args.contains(&"paren") {
                    self.out.push_str("\\renewcommand\\labelenumi{(\\arabic{enumi})}");
                }
                self.out.push('\n');
                for _ in 0..n {
                    let k = 1 + self.rng.below(2);
                    let _ = writeln!(self.out, "\\item {}", sentences(&mut self.rng, k));
                }
                let _ = write!(self.out, "\\end{{{env}}}");
            }
            "figure" | "table" => self.float(word, &args)?,
            "begin" => self.statement(&args)?,
            "proof" => self.proof(&args)?,
            "end" => {
                let Some(open) = self.open.take() else { bail!("paper `{}`: `end` with nothing open", self.paper.id) };
                let _ = write!(self.out, "\n\\end{{{}}}\\pdfcraftthmend\n", open.env);
            }
            "newpage" => self.out.push_str("\n\\newpage\n"),
            other => bail!("paper `{}`: unknown directive `{other}`", self.paper.id),
        }
        Ok(())
    }
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

fn preamble(paper: &Paper) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "% Generated by `cargo xtask refs-oracle generate` from papers.toml. Do not edit.");
    let _ = writeln!(s, "\\documentclass[{}]{{{}}}", paper.options.join(","), paper.class);
    s.push_str("\\usepackage{amsmath,amssymb,amsthm}\n");
    if let Some(g) = &paper.geometry {
        let _ = writeln!(s, "\\usepackage[{g}]{{geometry}}");
    }
    if paper.hyperref {
        s.push_str("\\usepackage{hyperref}\n");
    }
    s.push_str("\\usepackage{pdfcraftrefs}\n");
    let _ = writeln!(s, "\\pagestyle{{{}}}", paper.pagestyle);
    s.push_str("\\renewcommand\\labelitemi{$\\bullet$}\n");
    match paper.heads.as_str() {
        "italic" => s.push_str("\\newtheoremstyle{pcplain}{\\topsep}{\\topsep}{\\normalfont}{}{\\itshape}{.}{.5em}{}\n\\theoremstyle{pcplain}\n"),
        "smallcaps" => s.push_str("\\newtheoremstyle{pcplain}{\\topsep}{\\topsep}{\\itshape}{}{\\scshape}{.}{.5em}{}\n\\theoremstyle{pcplain}\n"),
        _ => s.push_str("\\theoremstyle{plain}\n"),
    }
    let within = if paper.numbering == "section" { "[section]" } else { "" };
    let mut style = "plain";
    for (env, word, st, numbered) in ENVS {
        if st != style {
            let _ = writeln!(s, "\\theoremstyle{{{st}}}");
            style = st;
        }
        if !numbered {
            let _ = writeln!(s, "\\newtheorem*{{{env}}}{{{word}}}");
        } else if env == "theorem" {
            let _ = writeln!(s, "\\newtheorem{{theorem}}{{Theorem}}{within}");
        } else if paper.shared {
            let _ = writeln!(s, "\\newtheorem{{{env}}}[theorem]{{{word}}}");
        } else {
            let _ = writeln!(s, "\\newtheorem{{{env}}}{{{word}}}{within}");
        }
    }
    if paper.numbering == "section" {
        s.push_str("\\numberwithin{equation}{section}\n");
    }
    match paper.qed.as_str() {
        "black" => s.push_str("\\renewcommand\\qedsymbol{$\\blacksquare$}\n"),
        "square" => s.push_str("\\renewcommand\\qedsymbol{$\\square$}\n"),
        "text" => s.push_str("\\renewcommand\\qedsymbol{\\textup{QED}}\n"),
        _ => {}
    }
    s.push_str("\\relpenalty=10000\n\\binoppenalty=10000\n\\sloppy\n\\begin{document}\n");
    s
}

struct Source {
    tex: String,
    statements: Vec<StatementSpec>,
    targets: Vec<TargetSpec>,
    mentions: Vec<MentionSpec>,
}

fn source(paper: &Paper) -> anyhow::Result<Source> {
    let twocolumn = paper.options.iter().any(|o| o == "twocolumn");
    let mut w = Writer {
        paper,
        rng: Rng::new(paper.seed),
        out: preamble(paper),
        twocolumn,
        displays: 0,
        statements: Vec::new(),
        targets: Vec::new(),
        mentions: Vec::new(),
        labels: HashMap::new(),
        open: None,
        last_statement: None,
    };
    let lines: Vec<&str> = paper.body.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
    // Labels first, so a mention may come before what it names ("Lemma 3.2 below").
    for line in &lines {
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        let args: Vec<&str> = rest.split_whitespace().collect();
        let Some(key) = arg(&args, "label") else { continue };
        let label = match word {
            "begin" => ENVS.iter().find(|e| Some(e.0) == args.first().copied()).map(|e| Label { kind: TargetKind::Statement, word: e.1.to_owned() }),
            "section" | "subsection" => Some(Label { kind: TargetKind::Heading, word: "Section".to_owned() }),
            "display" => Some(Label { kind: TargetKind::Equation, word: "equation".to_owned() }),
            "figure" => Some(Label { kind: TargetKind::Float, word: "Figure".to_owned() }),
            "table" => Some(Label { kind: TargetKind::Float, word: "Table".to_owned() }),
            _ => None,
        };
        if let Some(l) = label {
            w.labels.insert(key.to_owned(), l);
        }
    }
    for line in lines {
        w.line(line).with_context(|| format!("paper `{}`: `{line}`", paper.id))?;
    }
    if w.open.is_some() {
        bail!("paper `{}`: a statement is still open at the end", paper.id);
    }
    w.out.push_str("\n\\end{document}\n");
    Ok(Source { tex: w.out, statements: w.statements, targets: w.targets, mentions: w.mentions })
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
        if !matches!(p.heads.as_str(), "bold" | "italic" | "smallcaps")
            || !matches!(p.numbering.as_str(), "section" | "global")
            || !matches!(p.qed.as_str(), "open" | "black" | "square" | "text")
        {
            bail!("paper `{}`: heads bold|italic|smallcaps, numbering section|global, qed open|black|square|text", p.id);
        }
        if p.body.contains(['%', '\\', '$', '&', '#', '_', '^', '~', '{', '}']) {
            bail!("paper `{}`: the body is directives and plain words only", p.id);
        }
    }
    let holdout = list.paper.iter().filter(|p| p.holdout).count();
    if holdout * 3 < list.paper.len() {
        bail!("{holdout} of {} papers are held out; at least a third must be", list.paper.len());
    }
    Ok(list)
}

fn info(paper: &Paper) -> (PaperInfo, RefsPaper) {
    let size = paper.options.iter().find(|o| o.ends_with("pt")).cloned().unwrap_or_else(|| "10pt".to_owned());
    let columns = if paper.options.iter().any(|o| o == "twocolumn") { "2col" } else { "1col" };
    (
        PaperInfo { doc: paper.id.clone(), holdout: paper.holdout, class: paper.class.clone(), columns: columns.to_owned(), size },
        RefsPaper { doc: paper.id.clone(), heads: paper.heads.clone(), numbering: paper.numbering.clone(), hyperref: paper.hyperref },
    )
}

/// Typesets `stem` in `work`: to DVI twice (the second run has the labels) and lists it, then to
/// PDF twice with the brackets and twice without, and requires identical PDFs (with hyperref:
/// the same glyphs within [`HYPERREF_SHIFT_SP`]), AMS Type 1 fonts only, and no reference left
/// undefined.
fn build(
    tex: &Tex,
    work: &Path,
    stem: &str,
    src: &str,
    packages: &[(&str, String, String)],
    hyperref: bool,
) -> anyhow::Result<(Vec<DviPage>, Vec<u8>)> {
    let dir = work.join(stem);
    let plain = dir.join("plain");
    std::fs::create_dir_all(&plain)?;
    std::fs::write(dir.join(format!("{stem}.tex")), src)?;
    std::fs::write(plain.join(format!("{stem}.tex")), src)?;
    for (name, on, off) in packages {
        std::fs::write(dir.join(name), on)?;
        std::fs::write(plain.join(name), off)?;
    }
    let timeout = Duration::from_secs(300);
    tex.compile(&dir, stem, Output::Dvi, timeout)?;
    tex.compile(&dir, stem, Output::Dvi, timeout)?;
    let pages = tex.list(&dir, stem)?;
    for d in [&dir, &plain] {
        tex.compile(d, stem, Output::Pdf, timeout)?;
        tex.compile(d, stem, Output::Pdf, timeout)?;
    }
    let pdf = std::fs::read(dir.join(format!("{stem}.pdf")))?;
    if pdf != std::fs::read(plain.join(format!("{stem}.pdf")))? {
        if !hyperref {
            bail!("{stem}.pdf differs when the \\special brackets expand to nothing: a bracket moved something");
        }
        // hyperref moves the glue around a link by a hair when a whatsit stands near it: every
        // glyph and rule must still be there, in order, within HYPERREF_SHIFT_SP.
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
    if log.contains("There were undefined references") || log.contains("Reference `") {
        bail!("{stem}: a reference is undefined");
    }
    let fonts: Vec<&str> = log.split('<').filter_map(|s| s.split_once('>').map(|(f, _)| f)).filter(|f| f.ends_with(".pfb")).collect();
    if fonts.is_empty() {
        bail!("{stem}.log names no embedded Type 1 font");
    }
    if let Some(f) = fonts.iter().find(|f| !f.contains(AMS_FONT_DIR)) {
        bail!("{stem}.pdf embeds {f}, which is not an AMS Type 1 font (OFL-1.1)");
    }
    Ok((pages, pdf))
}

fn generate(args: &[String]) -> anyhow::Result<()> {
    let scratch = WorkDir::new("refs-oracle")?;
    if args.iter().any(|a| a == "--keep") {
        let kept = scratch.keep();
        eprintln!("refs-oracle: keeping TeX's files in {}", kept.display());
        return generate_in(args, &kept);
    }
    generate_in(args, scratch.path())
}

fn generate_in(args: &[String], scratch: &Path) -> anyhow::Result<()> {
    let data = root().join(DATA_DIR);
    let check = args.iter().any(|a| a == "--check");
    let out: PathBuf = if check { scratch.join("out") } else { data.clone() };
    let list = read_list(&data.join("papers.toml"))?;
    let tex = Tex::find().map_err(|absent| anyhow::anyhow!("{absent}"))?;
    let packages = [(LAYOUT_PACKAGE, layout::package(true), layout::package(false)), (PACKAGE, refs::package(true), refs::package(false))];
    let mut truth = RefsTruth { layout: LayoutTruth::default(), ..Default::default() };
    let mut files: Vec<(String, Vec<u8>)> = packages.iter().map(|(n, on, _)| ((*n).to_owned(), on.clone().into_bytes())).collect();
    for paper in &list.paper {
        let src = source(paper)?;
        let (pages, pdf) = build(&tex, scratch, &paper.id, &src.tex, &packages, paper.hyperref).with_context(|| format!("paper `{}`", paper.id))?;
        if pages.len() > MAX_PAGES {
            bail!("paper `{}` has {} pages; at most {MAX_PAGES}", paper.id, pages.len());
        }
        let lp = layout::build(&paper.id, &pages).with_context(|| format!("paper `{}`", paper.id))?;
        for p in &lp {
            let unowned = p.unowned.iter().filter(|&&i| matches!(p.items.get(i).map(|i| &i.mark), Some(Mark::Glyph { .. }))).count();
            if unowned > 0 {
                bail!("paper `{}` page {}: {unowned} glyphs belong to no block", paper.id, p.page);
            }
        }
        let (st, tt, mt) =
            refs::build(&paper.id, &pages, &lp, &src.statements, &src.targets, &src.mentions).with_context(|| format!("paper `{}`", paper.id))?;
        let proofs = st.iter().filter(|s| s.spec.proof).count();
        println!(
            "{:<20} {} pages, {} statements, {proofs} proofs, {} targets, {} mentions",
            paper.id,
            lp.len(),
            st.len() - proofs,
            tt.len(),
            mt.len()
        );
        let (li, ri) = info(paper);
        truth.layout.papers.push(li);
        truth.layout.pages.extend(lp);
        truth.papers.push(ri);
        truth.statements.extend(st);
        truth.targets.extend(tt);
        truth.mentions.extend(mt);
        files.push((format!("{}.tex", paper.id), src.tex.into_bytes()));
        files.push((format!("{}.pdf", paper.id), pdf));
    }
    let header = format!(
        "Generated by `cargo xtask refs-oracle generate` from papers.toml with {}. Do not edit.\n\
         Rows: see pdfcraft_oracle::tex::refs::RefsTruth. Positions in sp from the DVI origin.",
        tex.version()
    );
    files.push(("truth.tsv".to_owned(), truth.to_tsv(&header).into_bytes()));
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
        println!("refs-oracle: every generated file is byte-identical with {DATA_DIR}");
    } else {
        println!("refs-oracle: wrote {} files to {}", files.len(), out.display());
    }
    Ok(())
}
