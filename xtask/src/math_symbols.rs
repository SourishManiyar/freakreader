//! `cargo xtask math-symbols`: the math symbol table (`crates/render/src/text/math/symbols`).
//!
//! - `generate [--check]` reads the hand-written list of LaTeX commands (`commands.txt`),
//!   typesets each one alone with TeX, reads the font and character code of every glyph TeX set
//!   from the DVI exactly as the equation oracle does, and writes `table.tsv` beside the list.
//!   `--check` regenerates into memory and fails unless the file is byte-identical. Needs TeX Live;
//!   building and running the parser does not.
//!
//! The list says what a symbol *is* (Unicode, TeX class, kind); only TeX says which code of which
//! font draws it. Delimiters, radical signs and wide accents come in sizes: for those the generator
//! sets `\left D \vrule … \right.` (and `\sqrt{\vrule …}`, `\widehat{\vrule …}`) around rules of
//! growing size and records every glyph TeX picks, in the order of the size ladder, and the pieces
//! an extensible symbol is built from.

use crate::gates::root;
use crate::math_oracle::tfm::Fonts;
use anyhow::{Context, bail};
use pdfcraft_oracle::tex::{Env, Item, Mark, Tex};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// The list and the table, relative to the repository root.
pub const DIR: &str = "crates/render/src/text/math/symbols";
const COMMANDS: &str = "commands.txt";
const TABLE: &str = "table.tsv";
/// Heights of the rules a delimiter or radical grows around, in TeX points, half the extent each
/// side of the axis (for delimiters) or the height above the baseline (for radicals).
const MAX_SCAN_PT: u32 = 70;
/// Heights of the rules a radical sign grows to.
const MAX_RADICAL_PT: u32 = 150;
/// Widths a wide accent is set over, in TeX points.
const MAX_WIDE_PT: u32 = 120;
const WIDE_STEP_PT: u32 = 2;
/// `\big`, `\Big`, `\bigg`, `\Bigg`.
const BIG: [&str; 4] = ["big", "Big", "bigg", "Bigg"];

pub fn run(args: &[String]) -> anyhow::Result<()> {
    match args.first().map(String::as_str) {
        Some("generate") => generate(args.get(1..).unwrap_or_default()),
        _ => bail!("usage: cargo xtask math-symbols generate [--check]"),
    }
}

#[derive(Clone, Debug)]
struct Meta {
    unicode: String,
    class: String,
    kind: String,
    flags: BTreeSet<String>,
}

/// One TeX source to typeset and what to do with the result.
struct Job {
    env: Env,
    tex: String,
    what: What,
}

enum What {
    /// A line of the list: its command and metadata.
    Entry { latex: String, meta: Meta },
    /// A delimiter set around a rule of this half-height.
    Scan { group: usize, height: u32 },
    /// `\bigl D`-style delimiter of this size index (0 is `\big`).
    Big { group: usize, size: usize },
    /// A wide accent over a rule of this width.
    Wide { group: usize },
    /// A brace over or under a rule: pieces side by side.
    Brace { group: usize },
}

/// A delimiter, radical or wide accent family, which has sizes.
struct Family {
    /// The command as written: `(`, `\lfloor`, `\sqrt`, `\widehat`.
    latex: String,
    /// `l` or `r` for a delimiter, `radical` or `wide` otherwise.
    side: String,
    meta: Meta,
}

/// The pieces of one family's extensible symbols, by (font, code): role, the source that set it, the glyph.
type Pieces = BTreeMap<(String, u32), (&'static str, String, Glyph)>;

/// A glyph TeX set: family (`cmmi`), code, and its place.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Glyph {
    font: String,
    size: i64,
    code: u32,
    h: i64,
    v: i64,
}

/// A row of the table for one (family, code).
#[derive(Clone, Debug)]
struct Row {
    latex: String,
    meta: Meta,
    width: i64,
    italic: i64,
    height: i64,
    depth: i64,
    env: Env,
    tex: String,
}

fn family(font: &str) -> String {
    font.trim_end_matches(|c: char| c.is_ascii_digit()).to_owned()
}

fn glyphs(items: &[Item]) -> Vec<Glyph> {
    items
        .iter()
        .filter_map(|i| match &i.mark {
            Mark::Glyph { font, size, code } => Some(Glyph { font: font.clone(), size: *size, code: *code, h: i.h, v: i.v }),
            Mark::Rule { .. } => None,
        })
        .collect()
}

fn has_rule(items: &[Item]) -> bool {
    items.iter().any(|i| matches!(i.mark, Mark::Rule { .. }))
}

fn parse_flags(s: &str) -> BTreeSet<String> {
    if s == "-" { BTreeSet::new() } else { s.split(',').map(str::to_owned).collect() }
}

fn hex(unicode: &str) -> anyhow::Result<()> {
    if unicode == "-"
        || unicode.split('+').all(|p| !p.is_empty() && p.len() <= 6 && p.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_lowercase()))
    {
        Ok(())
    } else {
        bail!("`{unicode}` is not upper-case hex code points joined by +")
    }
}

const CLASSES: [&str; 9] = ["ord", "op", "bin", "rel", "open", "close", "punct", "large", "inner"];
const KINDS: [&str; 4] = ["sym", "latin", "greek", "digit"];

fn meta(unicode: &str, class: &str, kind: &str, flags: &str) -> anyhow::Result<Meta> {
    hex(unicode)?;
    if !CLASSES.contains(&class) {
        bail!("unknown class `{class}`");
    }
    if !KINDS.contains(&kind) {
        bail!("unknown kind `{kind}`");
    }
    Ok(Meta { unicode: unicode.to_owned(), class: class.to_owned(), kind: kind.to_owned(), flags: parse_flags(flags) })
}

fn env_of(s: &str) -> anyhow::Result<Env> {
    match s {
        "i" => Ok(Env::Inline),
        "d" => Ok(Env::Display),
        other => bail!("env `{other}` is i or d"),
    }
}

fn tex_of(latex: &str) -> String {
    latex.replace('~', " ")
}

/// The Unicode of the letter `c` of an alphabet (the Mathematical Alphanumeric Symbols block and its
/// exceptions, from the Unicode charts).
fn alphabet_unicode(style: &str, c: char) -> String {
    let cp = |u: u32| format!("{u:04X}");
    let (cap, small, digit): (Option<u32>, Option<u32>, Option<u32>) = match style {
        "rm" => (Some(0x41), Some(0x61), Some(0x30)),
        "math" | "it" => (Some(0x1D434), Some(0x1D44E), None),
        "bf" => (Some(0x1D400), Some(0x1D41A), Some(0x1D7CE)),
        "bb" => (Some(0x1D538), None, None),
        "frak" => (Some(0x1D504), Some(0x1D51E), None),
        "cal" => (Some(0x1D49C), None, None),
        _ => (None, None, None),
    };
    let offset = |base: Option<u32>, first: char| base.map(|b| b + (c as u32 - first as u32));
    let special: &[(char, u32)] = match style {
        "bb" => &[('C', 0x2102), ('H', 0x210D), ('N', 0x2115), ('P', 0x2119), ('Q', 0x211A), ('R', 0x211D), ('Z', 0x2124)],
        "frak" => &[('C', 0x212D), ('H', 0x210C), ('I', 0x2111), ('R', 0x211C), ('Z', 0x2128)],
        "cal" => &[('B', 0x212C), ('E', 0x2130), ('F', 0x2131), ('H', 0x210B), ('I', 0x2110), ('L', 0x2112), ('M', 0x2133), ('R', 0x211B)],
        "math" | "it" => &[('h', 0x210E)],
        _ => &[],
    };
    if let Some((_, u)) = special.iter().find(|(k, _)| *k == c) {
        return cp(*u);
    }
    let u = if c.is_ascii_uppercase() {
        offset(cap, 'A')
    } else if c.is_ascii_lowercase() {
        offset(small, 'a')
    } else {
        offset(digit, '0')
    };
    u.map_or_else(|| "-".to_owned(), cp)
}

struct Plan {
    jobs: Vec<Job>,
    families: Vec<Family>,
}

fn plan(list: &str) -> anyhow::Result<Plan> {
    let mut jobs = Vec::new();
    let mut families: Vec<Family> = Vec::new();
    for (n, line) in list.lines().enumerate() {
        let at = |m: String| anyhow::anyhow!("{COMMANDS}:{}: {m}", n + 1);
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        match f.first().copied() {
            Some("@delim") => {
                let [_, latex, side, unicode, class] = f[..] else { return Err(at("@delim LATEX SIDE UNICODE CLASS".into())) };
                if side != "l" && side != "r" {
                    return Err(at("SIDE is l or r".into()));
                }
                let group = families.len();
                families.push(Family {
                    latex: latex.to_owned(),
                    side: side.to_owned(),
                    meta: meta(unicode, class, "sym", "-").map_err(|e| at(e.to_string()))?,
                });
                for height in 1..=MAX_SCAN_PT {
                    let (open, close) = if side == "l" { (latex, ".") } else { (".", latex) };
                    let tex = format!("\\left{open}\\vrule~height~{height}pt~depth~{height}pt~width~1pt\\right{close}");
                    jobs.push(Job { env: Env::Display, tex: tex_of(&tex), what: What::Scan { group, height } });
                }
                for (size, big) in BIG.iter().enumerate() {
                    let side_letter = if side == "l" { 'l' } else { 'r' };
                    let tex = format!("\\{big}{side_letter}{latex}");
                    jobs.push(Job { env: Env::Display, tex, what: What::Big { group, size } });
                }
            }
            Some("@radical") => {
                let [_, latex, unicode] = f[..] else { return Err(at("@radical LATEX UNICODE".into())) };
                let group = families.len();
                families.push(Family {
                    latex: latex.to_owned(),
                    side: "radical".to_owned(),
                    meta: meta(unicode, "ord", "sym", "-").map_err(|e| at(e.to_string()))?,
                });
                for height in 1..=MAX_RADICAL_PT {
                    let tex = format!("{latex}{{\\vrule~height~{height}pt~depth~0pt~width~1pt}}");
                    jobs.push(Job { env: Env::Display, tex: tex_of(&tex), what: What::Scan { group, height } });
                }
            }
            Some("@wide") => {
                let [_, latex, unicode] = f[..] else { return Err(at("@wide LATEX UNICODE".into())) };
                let group = families.len();
                families.push(Family {
                    latex: latex.to_owned(),
                    side: "wide".to_owned(),
                    meta: meta(unicode, "ord", "sym", "acc").map_err(|e| at(e.to_string()))?,
                });
                for width in (WIDE_STEP_PT..=MAX_WIDE_PT).step_by(WIDE_STEP_PT as usize) {
                    let tex = format!("{latex}{{\\vrule~height~5pt~width~{width}pt}}");
                    jobs.push(Job { env: Env::Display, tex: tex_of(&tex), what: What::Wide { group } });
                }
            }
            Some("@brace") => {
                let [_, latex, unicode] = f[..] else { return Err(at("@brace LATEX UNICODE".into())) };
                let group = families.len();
                families.push(Family {
                    latex: latex.to_owned(),
                    side: "brace".to_owned(),
                    meta: meta(unicode, "ord", "sym", "-").map_err(|e| at(e.to_string()))?,
                });
                for width in (4..=MAX_WIDE_PT).step_by(4) {
                    let tex = format!("{latex}{{\\vrule~height~5pt~width~{width}pt}}");
                    jobs.push(Job { env: Env::Display, tex: tex_of(&tex), what: What::Brace { group } });
                }
            }
            Some("@accent") => {
                let [_, latex, unicode] = f[..] else { return Err(at("@accent LATEX UNICODE".into())) };
                let tex = format!("{latex}{{\\vrule~height~5pt~width~3pt}}");
                jobs.push(Job {
                    env: Env::Inline,
                    tex: tex_of(&tex),
                    what: What::Entry { latex: latex.to_owned(), meta: meta(unicode, "ord", "sym", "acc").map_err(|e| at(e.to_string()))? },
                });
            }
            Some("@alphabet") => {
                let [_, command, sets] = f[..] else { return Err(at("@alphabet COMMAND SET".into())) };
                let (style, flag) = match command {
                    "-" => ("math", "alpha=math"),
                    "\\mathrm" => ("rm", "alpha=rm"),
                    "\\mathit" => ("it", "alpha=it"),
                    "\\mathbf" => ("bf", "alpha=bf"),
                    "\\mathbb" => ("bb", "alpha=bb"),
                    "\\mathfrak" => ("frak", "alpha=frak"),
                    "\\mathcal" => ("cal", "alpha=cal"),
                    other => return Err(at(format!("unknown alphabet command `{other}`"))),
                };
                let mut chars: Vec<char> = Vec::new();
                for set in sets.split('+') {
                    match set {
                        "latin" => chars.extend(('a'..='z').chain('A'..='Z')),
                        "letters" => chars.extend(('a'..='z').chain('A'..='Z')),
                        "capitals" => chars.extend('A'..='Z'),
                        "digits" => chars.extend('0'..='9'),
                        other => return Err(at(format!("unknown set `{other}`"))),
                    }
                }
                for c in chars {
                    let latex = if command == "-" { c.to_string() } else { format!("{command}{{{c}}}") };
                    let kind = if c.is_ascii_digit() { "digit" } else { "latin" };
                    let unicode = alphabet_unicode(style, c);
                    jobs.push(Job {
                        env: Env::Inline,
                        tex: latex.clone(),
                        what: What::Entry { latex, meta: meta(&unicode, "ord", kind, flag).map_err(|e| at(e.to_string()))? },
                    });
                }
            }
            Some(latex) => {
                let [_, env, unicode, class, kind, ..] = f[..] else { return Err(at("LATEX ENV UNICODE CLASS KIND [FLAGS]".into())) };
                let flags = f.get(5).copied().unwrap_or("-");
                let env = env_of(env).map_err(|e| at(e.to_string()))?;
                jobs.push(Job {
                    env,
                    tex: tex_of(latex),
                    what: What::Entry { latex: latex.to_owned(), meta: meta(unicode, class, kind, flags).map_err(|e| at(e.to_string()))? },
                });
            }
            None => {}
        }
    }
    Ok(Plan { jobs, families })
}

/// Roles of the glyphs of an extensible stack, top to bottom: `top`, `mid`, `bot`, `rep`.
fn stack_roles(stack: &[Glyph]) -> Vec<&'static str> {
    let n = stack.len();
    let codes: Vec<(&str, u32)> = stack.iter().map(|g| (g.font.as_str(), g.code)).collect();
    if codes.iter().all(|c| *c == codes[0]) {
        return vec!["rep"; n];
    }
    let count = |c: &(&str, u32)| codes.iter().filter(|x| *x == c).count();
    let interior: Vec<&(&str, u32)> = codes.iter().skip(1).take(n.saturating_sub(2)).collect();
    let distinct: BTreeSet<&(&str, u32)> = interior.iter().copied().collect();
    let mid = if distinct.len() >= 2 { interior.iter().copied().find(|c| count(c) == 1) } else { None };
    codes
        .iter()
        .enumerate()
        .map(|(i, c)| {
            if i == 0 {
                "top"
            } else if i + 1 == n {
                // The last piece is a bottom piece unless it is the repeated one.
                if interior.contains(&c) { "rep" } else { "bot" }
            } else if Some(c) == mid {
                "mid"
            } else {
                "rep"
            }
        })
        .collect()
}

fn generate(args: &[String]) -> anyhow::Result<()> {
    let check = args.iter().any(|a| a == "--check");
    let dir = root().join(DIR);
    let list = std::fs::read_to_string(dir.join(COMMANDS)).with_context(|| format!("reading {DIR}/{COMMANDS}"))?;
    let Plan { jobs, families } = plan(&list)?;
    let tex = Tex::find().map_err(|absent| anyhow::anyhow!("{absent}"))?;
    let entries: Vec<(Env, &str)> = jobs.iter().map(|j| (j.env, j.tex.as_str())).collect();
    eprintln!("math-symbols: typesetting {} sources", entries.len());
    // In chunks, so a source TeX cannot set costs one chunk of single runs, not all of them.
    let mut typeset = Vec::with_capacity(entries.len());
    for chunk in entries.chunks(150) {
        typeset.extend(tex.typeset(chunk)?);
    }
    let mut fonts = Fonts::default();

    let mut rows: BTreeMap<(String, u32), Row> = BTreeMap::new();
    let mut compounds: BTreeMap<(String, bool), (Row, Vec<Glyph>)> = BTreeMap::new();
    let mut errors: Vec<String> = Vec::new();
    let mut aliases = 0usize;
    // Per family: the ladder (glyphs in order of first appearance), the pieces, the `\big` sizes.
    let mut ladders: Vec<Vec<Glyph>> = vec![Vec::new(); families.len()];
    let mut ladder_tex: Vec<Vec<(String, u32)>> = vec![Vec::new(); families.len()];
    let mut pieces: Vec<Pieces> = vec![BTreeMap::new(); families.len()];
    let mut bigs: Vec<Vec<Option<(String, u32)>>> = vec![vec![None; BIG.len()]; families.len()];
    let mut wides: Vec<Vec<(Glyph, String)>> = vec![Vec::new(); families.len()];

    let mut metrics = |g: &Glyph| -> anyhow::Result<[i64; 4]> {
        let c = fonts.get(&g.font)?.char_box(g.code, g.size).with_context(|| format!("{} has no character {}", g.font, g.code))?;
        let per_mille = |v: i64| if g.size == 0 { 0 } else { (v.saturating_mul(1000) + g.size / 2) / g.size };
        Ok([per_mille(c.width), per_mille(c.italic), per_mille(c.height), per_mille(c.depth)])
    };

    for (job, result) in jobs.iter().zip(typeset) {
        let items = match result {
            Ok(items) => items,
            Err(why) => {
                errors.push(format!("`{}` does not typeset: {}", job.tex, why.lines().next().unwrap_or("")));
                continue;
            }
        };
        let gl = glyphs(&items);
        match &job.what {
            What::Entry { latex, meta } => {
                let accent = meta.flags.contains("acc");
                if (has_rule(&items) && !accent) || gl.is_empty() {
                    errors.push(format!("`{}` set {} glyphs and {} rule(s); expected glyphs only", job.tex, gl.len(), usize::from(has_rule(&items))));
                    continue;
                }
                if accent {
                    // The accent over a rule: exactly one glyph.
                    if gl.len() != 1 {
                        errors.push(format!("`{}`: an accent over a rule set {} glyphs", job.tex, gl.len()));
                        continue;
                    }
                }
                if let [g] = &gl[..] {
                    let [width, italic, height, depth] = metrics(g)?;
                    let row = Row { latex: latex.clone(), meta: meta.clone(), width, italic, height, depth, env: job.env, tex: job.tex.clone() };
                    let key = (family(&g.font), g.code);
                    match rows.get_mut(&key) {
                        None => {
                            rows.insert(key, row);
                        }
                        Some(old) => {
                            // The same glyph under another name: the first name stays, and with it its
                            // alphabet (`2` is a digit, not `\mathrm{2}`); only the sizes it has are added.
                            old.meta.flags.extend(
                                row.meta.flags.into_iter().filter(|f| f.starts_with("var=") || f.starts_with("ext=") || f.starts_with("big=")),
                            );
                            aliases += 1;
                        }
                    }
                } else {
                    let row = Row {
                        latex: latex.clone(),
                        meta: meta.clone(),
                        width: 0,
                        italic: 0,
                        height: 0,
                        depth: 0,
                        env: job.env,
                        tex: job.tex.clone(),
                    };
                    if compounds.insert((latex.clone(), job.env == Env::Display), (row, gl)).is_some() {
                        errors.push(format!("`{latex}` is listed twice"));
                    }
                }
            }
            What::Scan { group, height } => {
                // The glyphs of this size, top to bottom.
                let mut stack = gl;
                stack.sort_by_key(|g| g.v);
                match &stack[..] {
                    [] => {}
                    [g] => {
                        let key = (g.font.clone(), g.code);
                        if !ladders[*group].iter().any(|x| (x.font.clone(), x.code) == key) {
                            ladders[*group].push(g.clone());
                            ladder_tex[*group].push((job.tex.clone(), *height));
                        }
                    }
                    _ => {
                        let roles = stack_roles(&stack);
                        for (g, role) in stack.iter().zip(roles) {
                            pieces[*group].entry((g.font.clone(), g.code)).or_insert_with(|| (role, job.tex.clone(), g.clone()));
                        }
                    }
                }
            }
            What::Big { group, size } => {
                // The largest `\Bigg` of some delimiters is an extensible stack: no single glyph.
                if let [g] = &gl[..] {
                    bigs[*group][*size] = Some((g.font.clone(), g.code));
                }
            }
            What::Brace { group } => {
                let mut row = gl;
                row.sort_by_key(|g| g.h);
                let roles = stack_roles(&row);
                for (g, role) in row.iter().zip(roles) {
                    let role = match role {
                        "top" => "left",
                        "bot" => "right",
                        other => other,
                    };
                    pieces[*group].entry((g.font.clone(), g.code)).or_insert_with(|| (role, job.tex.clone(), g.clone()));
                }
            }
            What::Wide { group } => {
                if let [g] = &gl[..] {
                    if !wides[*group].iter().any(|(x, _)| x.font == g.font && x.code == g.code) {
                        wides[*group].push((g.clone(), job.tex.clone()));
                    }
                } else {
                    errors.push(format!("`{}` set {} glyphs, not one", job.tex, gl.len()));
                }
            }
        }
    }

    // Families with sizes: ladder ranks, pieces, `\big` names, wide accents.
    for (k, fam) in families.iter().enumerate() {
        let mut add = |g: &Glyph, flags: Vec<String>, env_tex: &str, _errors: &mut Vec<String>| -> anyhow::Result<()> {
            let [width, italic, height, depth] = metrics(g)?;
            let mut meta = fam.meta.clone();
            meta.flags.extend(flags);
            let key = (family(&g.font), g.code);
            let row = Row { latex: fam.latex.clone(), meta, width, italic, height, depth, env: Env::Display, tex: env_tex.to_owned() };
            match rows.get_mut(&key) {
                None => {
                    rows.insert(key, row);
                }
                // Delimiters share pieces (`[` and `\lceil` have one bottom piece), and a size of a
                // family may be a symbol of its own (`\surd` is the smallest radical): the first name
                // stays and the sizes of every family it belongs to are recorded.
                Some(old) => old
                    .meta
                    .flags
                    .extend(row.meta.flags.into_iter().filter(|f| f.starts_with("var=") || f.starts_with("ext=") || f.starts_with("big="))),
            }
            Ok(())
        };
        if fam.side == "wide" {
            for (rank, (g, tex)) in wides[k].iter().enumerate() {
                add(g, vec![format!("var={}:{rank}", fam.latex)], tex, &mut errors)?;
            }
            continue;
        }
        if ladders[k].is_empty() && pieces[k].is_empty() {
            errors.push(format!("{} has no glyph at any size", fam.latex));
        }
        for (rank, g) in ladders[k].iter().enumerate() {
            let mut flags = vec![format!("var={}:{rank}", fam.latex)];
            if let Some(n) = bigs[k].iter().position(|b| b.as_ref() == Some(&(g.font.clone(), g.code))) {
                flags.push(format!("big={}", n + 1));
            }
            let tex = ladder_tex[k].get(rank).map(|(t, _)| t.clone()).unwrap_or_default();
            add(g, flags, &tex, &mut errors)?;
        }
        for ((_, _), (role, tex, g)) in &pieces[k] {
            add(g, vec![format!("ext={}:{role}", fam.latex)], tex, &mut errors)?;
        }
    }

    let baselines = compound_baselines(&tex, &compounds)?;

    if !errors.is_empty() {
        errors.sort();
        errors.dedup();
        bail!("{} problem(s):\n  {}", errors.len(), errors.join("\n  "));
    }

    let mut out = String::new();
    let _ = writeln!(
        out,
        "# Generated by `cargo xtask math-symbols generate` from commands.txt with {}. Do not edit.\n\
         # S family code latex unicode class kind flags width italic height depth env tex   (the four metrics: thousandths of an em)\n\
         # C latex unicode class kind flags env tex base parts   (base: the row's baseline below the first glyph's origin; parts: family:code:dx:dy:size; thousandths of the first glyph's size)",
        tex.version()
    );
    let env_name = |e: Env| if e == Env::Inline { "i" } else { "d" };
    let flags = |m: &Meta| if m.flags.is_empty() { "-".to_owned() } else { m.flags.iter().cloned().collect::<Vec<_>>().join(",") };
    for ((fam, code), r) in &rows {
        let _ = writeln!(
            out,
            "S\t{fam}\t{code}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            r.latex,
            r.meta.unicode,
            r.meta.class,
            r.meta.kind,
            flags(&r.meta),
            r.width,
            r.italic,
            r.height,
            r.depth,
            env_name(r.env),
            r.tex
        );
    }
    for ((latex, display), (r, gl)) in &compounds {
        let first = gl.first().context("compound without glyphs")?;
        let round = |v: i64| div_round(v.saturating_mul(1000), first.size);
        let parts: Vec<String> = gl
            .iter()
            .map(|g| format!("{}:{}:{}:{}:{}", family(&g.font), g.code, round(g.h - first.h), round(g.v - first.v), round(g.size)))
            .collect();
        let _ = writeln!(
            out,
            "C\t{latex}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            r.meta.unicode,
            r.meta.class,
            r.meta.kind,
            flags(&r.meta),
            env_name(r.env),
            r.tex,
            baselines.get(&(latex.clone(), *display)).copied().unwrap_or(0),
            parts.join(";")
        );
    }

    let path = dir.join(TABLE);
    if check {
        let old = std::fs::read_to_string(&path).unwrap_or_default();
        if old != out {
            bail!("{DIR}/{TABLE} differs from what the generator writes; run `cargo xtask math-symbols generate`");
        }
        println!("math-symbols: {DIR}/{TABLE} is byte-identical ({} symbols, {} compounds)", rows.len(), compounds.len());
    } else {
        std::fs::write(&path, out).with_context(|| format!("writing {}", path.display()))?;
        println!(
            "math-symbols: wrote {DIR}/{TABLE}: {} symbols, {} compounds, {aliases} extra names for a glyph already listed",
            rows.len(),
            compounds.len()
        );
    }
    Ok(())
}

/// Where the row's baseline is relative to a compound's first glyph: TeX sets `A` followed by the
/// compound on one line, and `A` stands on the baseline. In thousandths of the first glyph's size.
fn compound_baselines(tex: &Tex, compounds: &BTreeMap<(String, bool), (Row, Vec<Glyph>)>) -> anyhow::Result<BTreeMap<(String, bool), i64>> {
    let mut baselines: BTreeMap<(String, bool), i64> = BTreeMap::new();
    let keys: Vec<&(String, bool)> = compounds.keys().collect();
    let sources: Vec<(Env, String)> =
        keys.iter().filter_map(|k| compounds.get(*k).map(|(r, _)| (r.env, format!("\\mathrm{{A}}{}", r.tex)))).collect();
    let entries: Vec<(Env, &str)> = sources.iter().map(|(e, t)| (*e, t.as_str())).collect();
    let mut results = Vec::with_capacity(entries.len());
    for chunk in entries.chunks(150) {
        results.extend(tex.typeset(chunk)?);
    }
    for (k, result) in keys.iter().zip(results) {
        let items = result.map_err(|why| anyhow::anyhow!("`A{}` does not typeset: {}", k.0, why.lines().next().unwrap_or("")))?;
        let a = glyphs(&items).into_iter().find(|g| family(&g.font) == "cmr" && g.code == 65).with_context(|| format!("no A before {}", k.0))?;
        let first = compounds.get(*k).and_then(|(_, gl)| gl.first()).context("compound without glyphs")?;
        baselines.insert((*k).clone(), div_round((a.v - first_v_in_pass(&items, first)).saturating_mul(1000), first.size));
    }
    Ok(baselines)
}

/// `a / b` rounded to the nearest integer, halves away from zero; `b` is positive.
fn div_round(a: i64, b: i64) -> i64 {
    if b <= 0 {
        return 0;
    }
    if a >= 0 { (a + b / 2) / b } else { -((-a + b / 2) / b) }
}

/// The place of the first glyph of a compound in the typeset of `A` + the compound: the glyph of the
/// same font and code that comes after the `A`, in shipping order.
fn first_v_in_pass(items: &[Item], first: &Glyph) -> i64 {
    glyphs(items).into_iter().filter(|g| g.font == first.font && g.code == first.code).map(|g| g.v).next().unwrap_or(first.v)
}
