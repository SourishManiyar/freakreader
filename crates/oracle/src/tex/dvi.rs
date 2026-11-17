//! Reading `dvitype -output-level=4` listings: every glyph and rule TeX shipped out, where it put
//! it, and which equation it belongs to.
//!
//! The listing is the same one `crates/render/tests/data/math/dvi_truth.awk` reads, and positions
//! are read the same way: `h` and `v` in scaled points (sp) from the DVI origin, `v` down, a glyph
//! at its reference point on the baseline and a rule at its bottom-left corner. On top of that,
//! `\special{pdfcraft:eq:ID:begin}` and `…:end` bracket an equation and `pdfcraft:tag:begin`/`end`
//! bracket an equation number (see [`super::preamble`]); the brackets come out in the order TeX
//! shipped the material, so membership comes from TeX itself, not from where things landed.
//!
//! Document structure (the layout oracle, [`super::layout`]) is bracketed by specials of its own,
//! `pdfcraft:layout:…`. They are not interpreted here: each is kept as an [`DviPage::events`] entry
//! with the place in the item sequence where TeX shipped it.

use super::TexError;
use std::collections::HashMap;
use std::fmt;

/// Most glyphs and rules read from one listing. A page of mathematics has a few hundred; this
/// bounds what a runaway candidate can make the parser hold.
pub const MAX_ITEMS: usize = 2_000_000;
/// Most pages read from one listing.
pub const MAX_PAGES: usize = 10_000;
/// Longest equation id accepted in a `\special`.
pub const MAX_ID_BYTES: usize = 128;
/// Longest layout special kept, in bytes (after `pdfcraft:layout:`).
pub const MAX_EVENT_BYTES: usize = 256;

/// What was set: a glyph of a font at a size, or a rule of a width and height.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Mark {
    /// `font` is the TFM name (`cmmi10`); `size` the size it was loaded at, in sp.
    Glyph {
        font: String,
        size: i64,
        code: u32,
    },
    Rule {
        width: i64,
        height: i64,
    },
}

impl fmt::Display for Mark {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Mark::Glyph { font, size, code } => write!(f, "{font} at {size}sp code {code}"),
            Mark::Rule { width, height } => write!(f, "rule {width}sp by {height}sp"),
        }
    }
}

/// One mark and its place: `h`, `v` in sp from the DVI origin, `v` down. A glyph's place is its
/// reference point on the baseline, a rule's its bottom-left corner.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Item {
    pub mark: Mark,
    pub h: i64,
    pub v: i64,
}

/// Which part of the page an item was shipped in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    /// Outside every bracket: running text, or anything else on the page.
    Outside,
    /// An equation number (amsmath's tag), wherever it falls.
    Tag,
    /// The equation [`DviPage::equations`] lists at this index.
    Equation(usize),
}

/// One page of a listing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DviPage {
    /// Every glyph and rule, in the order TeX shipped them.
    pub items: Vec<(Item, Owner)>,
    /// The ids of the equations bracketed on the page, in order.
    pub equations: Vec<String>,
    /// The layout specials (`pdfcraft:layout:TEXT`) shipped on the page, in order: `(n, TEXT)`
    /// where `n` is how many of [`items`](DviPage::items) were shipped before it.
    pub events: Vec<(usize, String)>,
}

impl DviPage {
    /// The items of the equation `id`, or `None` when the page has no such bracket.
    pub fn equation_items(&self, id: &str) -> Option<Vec<Item>> {
        let index = self.equations.iter().position(|e| e == id)?;
        Some(self.items.iter().filter(|(_, o)| *o == Owner::Equation(index)).map(|(i, _)| i.clone()).collect())
    }
}

fn bad(line_no: usize, what: impl fmt::Display) -> TexError {
    TexError::Listing(format!("dvitype listing line {line_no}: {what}"))
}

/// The value after the last `=` of the `tag` assignment on `line` (` h:=a+b=c, hh:=…` gives c),
/// as dvi_truth.awk's `last_value` reads it.
fn last_value(line: &str, tag: &str) -> Option<i64> {
    let start = line.find(tag)?.checked_add(tag.len())?;
    let rest = line.get(start..)?;
    let segment = rest.split(',').next()?;
    segment.rsplit('=').next()?.trim().parse().ok()
}

/// The leading run of ASCII digits of `s`, as a number.
fn leading_number<T: std::str::FromStr>(s: &str) -> Option<T> {
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    s.get(..end).filter(|d| !d.is_empty())?.parse().ok()
}

/// `level N:(h=…,v=…` gives the restored or saved position after a push or pop.
fn level_position(line: &str) -> Option<(i64, i64)> {
    let rest = line.strip_prefix("level ")?;
    let rest = rest.get(rest.find(":(h=")?.checked_add(4)?..)?;
    let (h, rest) = rest.split_once(",v=")?;
    let v = rest.split(',').next()?;
    Some((h.parse().ok()?, v.parse().ok()?))
}

/// The font table at the head of the listing: `Font 30: cmmi10---loaded at size 655360 DVI units`
/// (or `cmr10 scaled 1200---loaded at …`).
fn font_definition(line: &str) -> Option<(u64, String, i64)> {
    let rest = line.strip_prefix("Font ")?;
    let (number, rest) = rest.split_once(": ")?;
    let (name, rest) = rest.split_once("---")?;
    let name = name.split_whitespace().next()?;
    let size = rest.split("loaded at size ").nth(1)?;
    Some((number.trim().parse().ok()?, name.to_owned(), leading_number(size)?))
}

/// The font number of a `fntnumN` or `fntK N` command line that ends `current font is NAME`.
fn font_selection(op: &str, args: &str) -> Option<u64> {
    if let Some(n) = op.strip_prefix("fntnum") {
        return n.parse().ok();
    }
    if op.starts_with("fnt") && !op.starts_with("fntdef") {
        return leading_number(args);
    }
    None
}

struct Brackets {
    open: Option<usize>,
    in_tag: bool,
}

/// Reads a `dvitype -output-level=4 -dpi=72.27 -magnification=0` listing.
///
/// Fails, rather than guessing, on a listing it cannot follow: a glyph before any font, an
/// equation that is opened twice or not closed on its page, an unknown `pdfcraft:` special, or more
/// than [`MAX_ITEMS`] items or [`MAX_PAGES`] pages.
pub fn parse_dvitype(listing: &str) -> Result<Vec<DviPage>, TexError> {
    let mut fonts: HashMap<u64, (String, i64)> = HashMap::new();
    let mut pages: Vec<DviPage> = Vec::new();
    let (mut h, mut v) = (0i64, 0i64);
    let mut font: Option<(String, i64)> = None;
    let mut brackets = Brackets { open: None, in_tag: false };
    let mut items = 0usize;
    for (n, line) in listing.lines().enumerate() {
        let line_no = n.saturating_add(1);
        if let Some(def) = font_definition(line) {
            fonts.insert(def.0, (def.1, def.2));
            continue;
        }
        if let Some((lh, lv)) = level_position(line) {
            (h, v) = (lh, lv);
            continue;
        }
        // Command lines: `123: op args`. Others may still move the position: dvitype prints the
        // advance of a `setrule` on a line of its own (` h:=a+w=b, hh:=…`).
        let Some((_, command)) = line.split_once(": ").filter(|(o, _)| !o.is_empty() && o.bytes().all(|b| b.is_ascii_digit())) else {
            if let Some(x) = last_value(line, " h:=") {
                h = x;
            }
            if let Some(y) = last_value(line, " v:=") {
                v = y;
            }
            continue;
        };
        let (op, args) = command.split_once(' ').unwrap_or((command, ""));
        if command.starts_with("beginning of page") {
            close_page(&pages, &brackets, line_no)?;
            if pages.len() >= MAX_PAGES {
                return Err(bad(line_no, format!("more than {MAX_PAGES} pages")));
            }
            pages.push(DviPage::default());
            (h, v) = (0, 0);
            font = None;
            brackets = Brackets { open: None, in_tag: false };
            continue;
        }
        let Some(page) = pages.last_mut() else { continue };
        if let Some(number) = font_selection(op, args) {
            match fonts.get(&number) {
                Some(f) => font = Some(f.clone()),
                None => return Err(bad(line_no, format!("font {number} is not in the font table"))),
            }
        }
        let glyph_code: Option<u32> = if let Some(c) = op.strip_prefix("setchar") {
            Some(c.parse().map_err(|_| bad(line_no, "unreadable setchar"))?)
        } else if matches!(op, "set1" | "set2" | "set3" | "set4" | "put1" | "put2" | "put3" | "put4") {
            Some(leading_number(args).ok_or_else(|| bad(line_no, "unreadable character code"))?)
        } else {
            None
        };
        let mark = if let Some(code) = glyph_code {
            let Some((name, size)) = &font else { return Err(bad(line_no, "a character before any font")) };
            Some(Mark::Glyph { font: name.clone(), size: *size, code })
        } else if matches!(op, "setrule" | "putrule") {
            let dims = args.strip_prefix("height ").ok_or_else(|| bad(line_no, "unreadable rule"))?;
            let (height, width) = dims.split_once(", width ").ok_or_else(|| bad(line_no, "unreadable rule"))?;
            let width: i64 = width.split_whitespace().next().and_then(|w| w.parse().ok()).ok_or_else(|| bad(line_no, "unreadable rule width"))?;
            let height: i64 = height.trim().parse().map_err(|_| bad(line_no, "unreadable rule height"))?;
            Some(Mark::Rule { width, height })
        } else {
            None
        };
        if let Some(mark) = mark {
            items = items.saturating_add(1);
            if items > MAX_ITEMS {
                return Err(bad(line_no, format!("more than {MAX_ITEMS} glyphs and rules")));
            }
            let owner = if brackets.in_tag { Owner::Tag } else { brackets.open.map_or(Owner::Outside, Owner::Equation) };
            page.items.push((Item { mark, h, v }, owner));
        }
        if op == "xxx" {
            special(page, &mut brackets, args, line_no)?;
            if page.events.len() > MAX_ITEMS {
                return Err(bad(line_no, format!("more than {MAX_ITEMS} layout specials")));
            }
        }
        if let Some(x) = last_value(line, " h:=") {
            h = x;
        }
        if let Some(y) = last_value(line, " v:=") {
            v = y;
        }
    }
    close_page(&pages, &brackets, usize::MAX)?;
    Ok(pages)
}

fn close_page(pages: &[DviPage], brackets: &Brackets, line_no: usize) -> Result<(), TexError> {
    let Some(page) = pages.last() else { return Ok(()) };
    if let Some(open) = brackets.open {
        let id = page.equations.get(open).map_or("?", String::as_str);
        return Err(bad(line_no, format!("equation `{id}` does not end on the page it begins on")));
    }
    if brackets.in_tag {
        return Err(bad(line_no, "an equation number does not end on its page"));
    }
    Ok(())
}

/// Follows one `xxx 'text'` line. Specials that are not ours (`header=…`, `papersize=…`) are
/// ignored.
fn special(page: &mut DviPage, brackets: &mut Brackets, args: &str, line_no: usize) -> Result<(), TexError> {
    let (Some(first), Some(last)) = (args.find('\''), args.rfind('\'')) else { return Ok(()) };
    let Some(text) = args.get(first.saturating_add(1)..last) else { return Ok(()) };
    let Some(ours) = text.strip_prefix("pdfcraft:") else { return Ok(()) };
    if let Some(event) = ours.strip_prefix("layout:") {
        if event.is_empty() || event.len() > MAX_EVENT_BYTES {
            return Err(bad(line_no, format!("bad layout special `{text}`")));
        }
        page.events.push((page.items.len(), event.to_owned()));
        return Ok(());
    }
    match ours {
        "tag:begin" if !brackets.in_tag => brackets.in_tag = true,
        "tag:end" if brackets.in_tag => brackets.in_tag = false,
        _ => {
            let Some((id, what)) = ours.strip_prefix("eq:").and_then(|r| r.rsplit_once(':')) else {
                return Err(bad(line_no, format!("unknown special `{text}`")));
            };
            if id.is_empty() || id.len() > MAX_ID_BYTES {
                return Err(bad(line_no, format!("bad equation id in `{text}`")));
            }
            match (what, brackets.open) {
                ("begin", None) => {
                    if page.equations.iter().any(|e| e == id) {
                        return Err(bad(line_no, format!("equation `{id}` begins twice on one page")));
                    }
                    brackets.open = Some(page.equations.len());
                    page.equations.push(id.to_owned());
                }
                ("end", Some(open)) if page.equations.get(open).is_some_and(|e| e == id) => brackets.open = None,
                _ => return Err(bad(line_no, format!("`{text}` does not match the brackets open"))),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A listing in the shape dvitype writes (trimmed): the font table, then a page with an equation
    /// whose second row's number falls inside its brackets.
    const LISTING: &str = "\
Font 30: cmmi10---loaded at size 655360 DVI units
Font 26: cmr7---loaded at size 458752 DVI units
Font 5: cmr10 scaled 1200---loaded at size 786432 DVI units

42: beginning of page 1
87: push
level 0:(h=0,v=0,w=0,x=0,y=0,z=0,hh=0,vv=0)
88: xxx 'header=l3backend-dvips.pro'
116: pop
level 0:(h=100,v=200,w=0,x=0,y=0,z=0,hh=0,vv=0)
144: xxx 'pdfcraft:eq:a:begin'
165: fntdef1 30: cmmi10
187: fntnum30 current font is cmmi10
188: setchar120 h:=100+374556=374656, hh:=68
[x]
189: push
level 3:(h=374656,v=200,w=0,x=0,y=0,z=0,hh=68,vv=63)
190: down3 -237825 v:=200-237825=-237625, vv:=59
215: fnt1 26 current font is cmr7
216: set1 178 h:=374656+261235=635891, hh:=72
217: pop
level 3:(h=374656,v=200,w=0,x=0,y=0,z=0,hh=68,vv=63)
218: xxx 'pdfcraft:tag:begin'
219: put1 49
220: xxx 'pdfcraft:tag:end'
464: putrule height 26213, width 327681 (1x6 pixels)
487: xxx 'pdfcraft:eq:a:end'
488: setrule height 78641, width 702682 (2x11 pixels)
 h:=374656+702682=1077338, hh:=93
490: fntnum5 current font is cmr10
491: setchar65 h:=1077338+500000=1577338, hh:=80
500: beginning of page 2
";

    #[test]
    fn reads_marks_places_and_brackets() {
        let pages = parse_dvitype(LISTING).unwrap();
        assert_eq!(pages.len(), 2);
        let p = &pages[0];
        assert_eq!(p.equations, ["a"]);
        let glyph = |font: &str, size, code| Mark::Glyph { font: font.into(), size, code };
        let expect = [
            (Item { mark: glyph("cmmi10", 655360, 120), h: 100, v: 200 }, Owner::Equation(0)),
            (Item { mark: glyph("cmr7", 458752, 178), h: 374656, v: -237625 }, Owner::Equation(0)),
            (Item { mark: glyph("cmr7", 458752, 49), h: 374656, v: 200 }, Owner::Tag),
            (Item { mark: Mark::Rule { width: 327681, height: 26213 }, h: 374656, v: 200 }, Owner::Equation(0)),
            // A glyph after a `setrule` sits past the rule.
            (Item { mark: Mark::Rule { width: 702682, height: 78641 }, h: 374656, v: 200 }, Owner::Outside),
            (Item { mark: glyph("cmr10", 786432, 65), h: 1077338, v: 200 }, Owner::Outside),
        ];
        assert_eq!(p.items, expect);
        assert_eq!(p.equation_items("a").map(|i| i.len()), Some(3));
        assert_eq!(p.equation_items("b"), None);
        assert!(pages[1].items.is_empty());
    }

    #[test]
    fn layout_specials_are_kept_in_shipping_order() {
        let listing = "Font 1: cmr10---loaded at size 655360 DVI units\n1: beginning of page 1\n\
                       2: xxx 'pdfcraft:layout:par:begin:par:0'\n3: fntnum1 current font is cmr10\n\
                       4: setchar65 h:=0+1=1\n5: xxx 'pdfcraft:layout:par:end:0'\n6: xxx 'pdfcraft:layout:open:head'\n";
        let pages = parse_dvitype(listing).unwrap();
        let p = &pages[0];
        assert_eq!(p.items.len(), 1);
        assert_eq!(p.events, [(0, "par:begin:par:0".to_owned()), (1, "par:end:0".to_owned()), (1, "open:head".to_owned())]);
        let empty = format!("{}2: xxx 'pdfcraft:layout:'\n", &listing[..listing.find("2:").unwrap()]);
        assert!(parse_dvitype(&empty).unwrap_err().to_string().contains("bad layout special"));
        let long = format!("{}2: xxx 'pdfcraft:layout:{}'\n", &listing[..listing.find("2:").unwrap()], "x".repeat(MAX_EVENT_BYTES + 1));
        assert!(parse_dvitype(&long).is_err());
    }

    #[test]
    fn broken_brackets_are_errors_not_guesses() {
        let page = "Font 1: cmr10---loaded at size 655360 DVI units\n1: beginning of page 1\n";
        for (body, why) in [
            ("2: xxx 'pdfcraft:eq:a:begin'\n", "does not end"),
            ("2: xxx 'pdfcraft:eq:a:begin'\n3: xxx 'pdfcraft:eq:b:begin'\n", "does not match"),
            ("2: xxx 'pdfcraft:eq:a:end'\n", "does not match"),
            ("2: xxx 'pdfcraft:eq:a:begin'\n3: xxx 'pdfcraft:eq:b:end'\n", "does not match"),
            ("2: xxx 'pdfcraft:eq:a:begin'\n3: xxx 'pdfcraft:eq:a:end'\n4: xxx 'pdfcraft:eq:a:begin'\n", "twice"),
            ("2: xxx 'pdfcraft:what'\n", "unknown special"),
            ("2: xxx 'pdfcraft:tag:begin'\n", "number does not end"),
            ("2: setchar65 h:=0+1=1\n", "before any font"),
            ("2: fntnum9 current font is cmr10\n", "not in the font table"),
            ("2: xxx 'pdfcraft:eq::begin'\n", "bad equation id"),
        ] {
            let err = parse_dvitype(&format!("{page}{body}")).unwrap_err().to_string();
            assert!(err.contains(why), "{body:?}: {err}");
        }
    }

    #[test]
    fn hostile_lines_do_not_panic() {
        for line in [
            "",
            ": ",
            "1: ",
            "1: setchar",
            "1: setchar99999999999999",
            "1: set1",
            "1: setrule height",
            "1: setrule height 1, width",
            "1: xxx '",
            "1: xxx",
            "level 0:(h=",
            "level 0:(h=1,v=",
            "Font : ---",
            "Font 1: x---loaded at size",
            "1: right3 5 h:=",
            "1: right3 5 h:=1+2=é",
            "1: fnt1 current font is x",
        ] {
            let _ = parse_dvitype(&format!("1: beginning of page 1\n{line}\n"));
            let _ = parse_dvitype(line);
        }
    }
}
