//! Finding the physical page a printed page number means. Printed numbers rarely equal page
//! indexes: front matter is numbered in roman numerals or not at all, and covers, plates and
//! blank pages shift the count. The page is found by evidence, strongest first:
//!
//! 1. the entry's title found as a heading on a candidate page (then the bookmark goes to the
//!    heading's position on it);
//! 2. the document's page labels (`/PageLabels`), when it has them;
//! 3. the offset most entries agree on, voted for by titles found and by the page numbers printed
//!    in the pages' own headers and footers;
//! 4. failing all of that, a guess that numbering starts after the table of contents.
//!
//! The offset may drift (an unnumbered plate between chapters): an entry whose title is found as
//! a heading a few pages off moves the offset for the entries after it. Every page read is
//! counted against [`MAX_READ_PAGES`].

use std::collections::{HashMap, HashSet};

use super::parse::{self, PageRef, compact};

/// Most pages read to check where entries go.
pub(crate) const MAX_READ_PAGES: usize = 1500;
/// Entries whose titles are looked for to vote on the offset.
const SAMPLE: usize = 30;
/// How far from the expected page a title is looked for, either way.
const DRIFT: i64 = 3;
/// Characters of a title compared (a long title is matched by its start).
const MATCH_CHARS: usize = 60;
/// Pages after the contents searched for the first entries' headings.
const WINDOW: usize = 120;
/// Shortest compacted title matched anywhere on a page rather than at the start of a line.
const ANYWHERE_CHARS: usize = 12;

/// What matching needs of one page: its lines (compacted, with where they are), and the page
/// number printed in its header or footer.
#[derive(Clone, Debug, Default)]
pub(crate) struct PageBody {
    /// Body lines in reading order: the compacted text, its top in user space, its size.
    pub(crate) lines: Vec<(String, f64, f64)>,
    /// Every body line compacted and joined, for titles set over several lines.
    pub(crate) all: String,
    pub(crate) number: Option<PageRef>,
}

/// Pages as mapping reads them; `body` is `None` for a page that can't be read (or once the read
/// budget is spent).
pub(crate) trait Pages {
    fn count(&self) -> usize;
    fn body(&mut self, page: usize) -> Option<&PageBody>;
}

/// An entry to place: its title and printed page (if any).
#[derive(Clone, Debug)]
pub(crate) struct Wanted {
    pub(crate) title: String,
    pub(crate) page: Option<PageRef>,
}

/// Where an entry goes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Placed {
    pub(crate) page: Option<usize>,
    /// The heading's top on the page (user space), when it was found there.
    pub(crate) top: Option<f64>,
    /// The title was found on that page.
    pub(crate) verified: bool,
}

/// How well `title` matches page `body`: 2 when a line starts with it (a heading), 1 when it is
/// somewhere in the text (only for long titles), 0 not at all; and the top of the line it starts.
pub(crate) fn find(body: &PageBody, title: &str) -> (u8, Option<f64>) {
    let full: String = compact(title).chars().take(MATCH_CHARS).collect();
    let bare: String = compact(parse::without_numbering(title)).chars().take(MATCH_CHARS).collect();
    if bare.chars().count() < 3 {
        return (0, None);
    }
    for (line, top, _) in &body.lines {
        // A heading that is the title, or begins it (a title set on two lines), or a line that
        // begins with the title (a heading with its own number before or after it).
        for t in [&full, &bare] {
            let line_len = line.chars().count();
            if !line.is_empty()
                && (line.starts_with(t.as_str()) || (line_len >= 6 && t.starts_with(line.as_str()) && line_len * 2 >= t.chars().count()))
            {
                return (2, Some(*top));
            }
        }
        // Numbered differently on the page ("Chapter 3" over "Results"): the title after a
        // short prefix.
        if let Some(at) = line.find(bare.as_str())
            && at > 0
            && numbering_prefix(line.get(..at).unwrap_or(""))
            && line.get(at.saturating_add(bare.len())..).is_some_and(|rest| rest.chars().all(|c| c.is_ascii_digit()))
        {
            return (2, Some(*top));
        }
    }
    if bare.chars().count() >= ANYWHERE_CHARS && body.all.contains(bare.as_str()) {
        return (1, None);
    }
    (0, None)
}

/// Whether compacted text before a title is only its number: digits, a roman numeral or a letter,
/// maybe after "chapter", "section", "part" or "appendix" ("chapter3", "iv", "a2").
fn numbering_prefix(pre: &str) -> bool {
    let mut rest = pre;
    for word in ["chapter", "section", "part", "appendix", "lecture", "kapitel", "chapitre", "teil", "book"] {
        if let Some(r) = rest.strip_prefix(word) {
            rest = r;
            break;
        }
    }
    if rest.is_empty() {
        return pre != rest;
    }
    rest.chars().count() <= 8
        && (rest.chars().all(|c| c.is_ascii_digit())
            || parse::roman_value(rest).is_some()
            || (rest.chars().next().is_some_and(|c| c.is_ascii_lowercase()) && rest.chars().skip(1).all(|c| c.is_ascii_digit())))
}

fn value(p: &PageRef) -> Option<(bool, u32)> {
    match p {
        PageRef::Arabic(n) => Some((false, *n)),
        PageRef::Roman(n) => Some((true, *n)),
        PageRef::Label(_) => None,
    }
}

/// The page `n` (1-based printed) falls on with offset `d`, if that is a page.
fn at(n: u32, d: i64, count: usize) -> Option<usize> {
    let p = i64::from(n).checked_sub(1)?.checked_add(d)?;
    usize::try_from(p).ok().filter(|p| *p < count)
}

/// Place every entry. `labels` are the page labels when the document has its own, `toc` the
/// contents pages (never a target found by matching: they hold every title).
pub(crate) fn place(pages: &mut dyn Pages, wanted: &[Wanted], labels: Option<&[String]>, toc: &[usize]) -> Vec<Placed> {
    let count = pages.count();
    let toc_set: HashSet<usize> = toc.iter().copied().collect();
    let toc_end = toc.iter().max().map_or(0, |p| p.saturating_add(1));
    let label_pages: HashMap<String, Vec<usize>> = labels.map_or_else(HashMap::new, |labels| {
        let mut m: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, l) in labels.iter().enumerate() {
            m.entry(l.trim().to_lowercase()).or_default().push(i);
        }
        m
    });
    // The offset for arabic and for roman numbers, by evidence.
    let offsets: [Option<i64>; 2] = [false, true].map(|roman| vote(pages, wanted, roman, &toc_set, labels.is_some().then_some(&label_pages)));
    let guess = |roman: bool, n: u32| -> i64 {
        if roman {
            return 0;
        }
        // Numbering starts after the contents (and after the roman front matter listed).
        let first = wanted.iter().filter_map(|w| w.page.as_ref().and_then(value)).filter(|(r, _)| !r).map(|(_, n)| n).min().unwrap_or(n);
        let after_roman = wanted.iter().filter_map(|w| w.page.as_ref().and_then(value)).filter(|(r, _)| *r).map(|(_, n)| n).max().unwrap_or(0);
        let start = i64::try_from(toc_end.max(after_roman as usize)).unwrap_or(0);
        (start - (i64::from(first) - 1)).max(0)
    };
    let mut local: [Option<i64>; 2] = offsets;
    let mut previous: Option<usize> = None;
    let mut out = Vec::with_capacity(wanted.len());
    for w in wanted {
        let placed = match &w.page {
            None => Placed { page: None, top: None, verified: false },
            Some(p) => {
                let by_label = labels.and(label_pages.get(&p.printed().to_lowercase())).and_then(|candidates| {
                    // Labels can repeat (each chapter from 1): the first at or after the entry before.
                    candidates.iter().copied().find(|c| previous.is_none_or(|prev| *c >= prev)).or(candidates.first().copied())
                });
                match value(p) {
                    Some((roman, n)) => {
                        let slot = usize::from(roman);
                        let base = local.get(slot).copied().flatten().or(offsets.get(slot).copied().flatten());
                        let base =
                            base.unwrap_or_else(|| by_label.map_or_else(|| guess(roman, n), |l| i64::try_from(l).unwrap_or(0) - (i64::from(n) - 1)));
                        place_one(pages, &w.title, n, base, by_label, previous, &toc_set, count, &mut local, slot)
                    }
                    None => match by_label {
                        Some(l) => {
                            let (strength, top) = pages.body(l).map_or((0, None), |b| find(b, &w.title));
                            Placed { page: Some(l), top, verified: strength > 0 }
                        }
                        None => Placed { page: None, top: None, verified: false },
                    },
                }
            }
        };
        if placed.page.is_some() {
            previous = placed.page;
        }
        out.push(placed);
    }
    // An entry without a page (a part heading) goes where the entry after it goes.
    let mut next: Option<usize> = None;
    for p in out.iter_mut().rev() {
        match p.page {
            Some(page) => next = Some(page),
            None => p.page = next,
        }
    }
    out
}

/// Place one numbered entry at offset `base`, looking a few pages either way for its heading.
#[allow(clippy::too_many_arguments)]
fn place_one(
    pages: &mut dyn Pages,
    title: &str,
    n: u32,
    base: i64,
    by_label: Option<usize>,
    previous: Option<usize>,
    toc: &HashSet<usize>,
    count: usize,
    local: &mut [Option<i64>; 2],
    slot: usize,
) -> Placed {
    // Candidates nearest first: the expected page, the label's page, then a few pages either way.
    let mut candidates: Vec<(i64, bool)> = vec![(base, false)];
    if let Some(l) = by_label {
        candidates.push((i64::try_from(l).unwrap_or(0) - (i64::from(n) - 1), false));
    }
    for k in 1..=DRIFT {
        candidates.push((base - k, true));
        candidates.push((base + k, true));
    }
    for (d, drifted) in candidates {
        let Some(page) = at(n, d, count) else { continue };
        if toc.contains(&page) || previous.is_some_and(|p| page < p) {
            continue;
        }
        let Some(body) = pages.body(page) else { continue };
        let (strength, top) = find(body, title);
        // Drifting off the expected page needs a heading, not a word somewhere on the page.
        if strength == 2 || (strength == 1 && !drifted) {
            if let Some(o) = local.get_mut(slot) {
                *o = Some(d);
            }
            return Placed { page: Some(page), top, verified: true };
        }
    }
    let page = by_label.or_else(|| at(n, base, count));
    Placed { page, top: None, verified: false }
}

/// The offset (physical index minus printed number plus one) most evidence agrees on, for arabic
/// or roman numbers: titles found as headings at candidate offsets, and printed page numbers in
/// headers and footers. `None` when nothing agrees.
fn vote(
    pages: &mut dyn Pages,
    wanted: &[Wanted],
    roman: bool,
    toc: &HashSet<usize>,
    label_pages: Option<&HashMap<String, Vec<usize>>>,
) -> Option<i64> {
    let count = pages.count();
    let numbered: Vec<(&Wanted, u32)> =
        wanted.iter().filter_map(|w| w.page.as_ref().and_then(value).filter(|(r, _)| *r == roman).map(|(_, n)| (w, n))).collect();
    if numbered.is_empty() {
        return None;
    }
    let mut candidates: Vec<i64> = Vec::new();
    let add = |d: i64, c: &mut Vec<i64>| {
        if !c.contains(&d) {
            c.push(d);
        }
    };
    if let Some(labels) = label_pages {
        for (w, _) in numbered.iter().take(SAMPLE) {
            if let (Some(p), Some(n)) = (
                labels.get(&w.page.as_ref().map(PageRef::printed).unwrap_or_default().to_lowercase()).and_then(|v| v.first()),
                w.page.as_ref().and_then(value),
            ) {
                add(i64::try_from(*p).unwrap_or(0) - (i64::from(n.1) - 1), &mut candidates);
            }
        }
    }
    // Printed page numbers in headers and footers: pages just after the contents and a spread
    // across the document.
    let toc_end = toc.iter().max().map_or(0, |p| p.saturating_add(1));
    let mut footer_votes: HashMap<i64, usize> = HashMap::new();
    let step = (count / 40).max(1);
    let probes: Vec<usize> = (toc_end..count.min(toc_end.saturating_add(12))).chain((0..count).step_by(step)).collect();
    for page in probes {
        if toc.contains(&page) {
            continue;
        }
        if let Some(PageRef::Arabic(n) | PageRef::Roman(n)) =
            pages.body(page).and_then(|b| b.number.clone()).filter(|p| matches!(p, PageRef::Roman(_)) == roman)
            && let Ok(p) = i64::try_from(page)
        {
            *footer_votes.entry(p - (i64::from(n) - 1)).or_default() += 1;
        }
    }
    let mut by_footer: Vec<(i64, usize)> = footer_votes.into_iter().filter(|(_, v)| *v >= 2).collect();
    by_footer.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for (d, _) in by_footer.iter().take(4) {
        add(*d, &mut candidates);
    }
    // The first few entries looked for over the pages after the contents: wherever their
    // headings are found is a candidate offset.
    let mut first: Vec<&(&Wanted, u32)> = numbered.iter().collect();
    first.sort_by_key(|(_, n)| *n);
    for (w, n) in first.into_iter().take(3) {
        for page in toc_end..count.min(toc_end.saturating_add(WINDOW)) {
            if !toc.contains(&page)
                && pages.body(page).is_some_and(|b| find(b, &w.title).0 == 2)
                && let Ok(p) = i64::try_from(page)
            {
                add(p - (i64::from(*n) - 1), &mut candidates);
            }
        }
    }
    // The usual shapes: printed equals physical, or numbering starts after the contents.
    add(0, &mut candidates);
    if let Some((_, first)) = numbered.iter().min_by_key(|(_, n)| *n) {
        add(i64::try_from(toc_end).unwrap_or(0) - (i64::from(*first) - 1), &mut candidates);
    }
    // Each candidate offset scored by the sampled titles found as headings there.
    let step = (numbered.len() / SAMPLE).max(1);
    let sample: Vec<&(&Wanted, u32)> = numbered.iter().step_by(step).take(SAMPLE).collect();
    let mut best: Option<(i64, usize)> = None;
    for d in candidates.iter().copied() {
        let mut hits = 0usize;
        for (w, n) in &sample {
            let Some(page) = at(*n, d, count) else { continue };
            if toc.contains(&page) {
                continue;
            }
            if pages.body(page).is_some_and(|b| find(b, &w.title).0 == 2) {
                hits += 1;
            }
        }
        if hits > 0 && best.is_none_or(|(_, h)| hits > h) {
            best = Some((d, hits));
        }
    }
    match best {
        Some((d, _)) => Some(d),
        // No title found anywhere: trust the printed page numbers if enough pages agree.
        None => by_footer.first().filter(|(_, v)| *v >= 3).map(|(d, _)| *d),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Pages given as their heading lines; `numbers` are the printed page numbers in footers.
    pub(crate) struct Fake {
        pub(crate) pages: Vec<PageBody>,
        pub(crate) reads: usize,
    }

    impl Fake {
        pub(crate) fn new(headings: &[&str], numbers: &[Option<PageRef>]) -> Self {
            let pages = headings
                .iter()
                .enumerate()
                .map(|(i, h)| {
                    let lines: Vec<(String, f64, f64)> = h.split('|').filter(|l| !l.is_empty()).map(|l| (compact(l), 700.0, 12.0)).collect();
                    let all = lines.iter().map(|l| l.0.as_str()).collect::<String>();
                    PageBody { lines, all, number: numbers.get(i).cloned().flatten() }
                })
                .collect();
            Self { pages, reads: 0 }
        }
    }

    impl Pages for Fake {
        fn count(&self) -> usize {
            self.pages.len()
        }
        fn body(&mut self, page: usize) -> Option<&PageBody> {
            self.reads += 1;
            self.pages.get(page)
        }
    }

    fn w(title: &str, page: Option<PageRef>) -> Wanted {
        Wanted { title: title.into(), page }
    }

    fn pages_of(placed: &[Placed]) -> Vec<Option<usize>> {
        placed.iter().map(|p| p.page).collect()
    }

    /// A book: cover, title page, contents (2), preface (roman iii.. on 4-5), chapters from page 6
    /// printed as 1.
    fn book() -> Fake {
        let mut h = vec!["Cover", "A Book", "Contents", "Contents", "Preface", "more preface"];
        h.extend(["1 Introduction", "text", "text", "2 Methods", "text", "Plate", "3 Results and their meaning", "text", "Bibliography"]);
        Fake::new(&h, &[])
    }

    #[test]
    fn the_offset_is_found_from_the_headings_and_drift_is_followed() {
        let mut pages = book();
        let wanted = [
            w("Preface", Some(PageRef::Roman(5))),
            w("1 Introduction", Some(PageRef::Arabic(1))),
            w("2 Methods", Some(PageRef::Arabic(4))),
            // An unnumbered plate before chapter 3 moves it one page on.
            w("3 Results and their meaning", Some(PageRef::Arabic(6))),
            w("Bibliography", Some(PageRef::Arabic(8))),
        ];
        let placed = place(&mut pages, &wanted, None, &[2, 3]);
        assert_eq!(pages_of(&placed), [Some(4), Some(6), Some(9), Some(12), Some(14)]);
        assert!(placed.iter().all(|p| p.verified), "{placed:?}");
        assert_eq!(placed[1].top, Some(700.0), "goes to the heading");
    }

    #[test]
    fn page_labels_map_printed_numbers_directly() {
        let mut pages = Fake::new(&["", "", "", "", "", ""], &[]);
        let labels: Vec<String> = ["i", "ii", "1", "2", "3", "4"].iter().map(|s| s.to_string()).collect();
        let wanted = [
            w("Foreword", Some(PageRef::Roman(2))),
            w("Start", Some(PageRef::Arabic(1))),
            w("End", Some(PageRef::Arabic(4))),
            w("Appendix", Some(PageRef::Label("A-1".into()))),
        ];
        let placed = place(&mut pages, &wanted, Some(&labels), &[]);
        assert_eq!(pages_of(&placed), [Some(1), Some(2), Some(5), None]);
        assert!(placed.iter().all(|p| !p.verified), "no headings to check against");
    }

    #[test]
    fn printed_footers_vote_when_titles_cannot_be_found() {
        // Headings in a font with no text: only the footers say "1" is on page 3.
        let numbers: Vec<Option<PageRef>> = (0..10).map(|i| (i >= 3).then(|| PageRef::Arabic(i - 2))).collect();
        let mut pages = Fake::new(&[""; 10], &numbers);
        let placed = place(&mut pages, &[w("Alpha", Some(PageRef::Arabic(1))), w("Beta", Some(PageRef::Arabic(5)))], None, &[1]);
        assert_eq!(pages_of(&placed), [Some(3), Some(7)]);
    }

    #[test]
    fn with_no_evidence_numbering_is_guessed_to_start_after_the_contents() {
        let mut pages = Fake::new(&[""; 12], &[]);
        let placed = place(&mut pages, &[w("Alpha", Some(PageRef::Arabic(1))), w("Beta", Some(PageRef::Arabic(5)))], None, &[1, 2]);
        assert_eq!(pages_of(&placed), [Some(3), Some(7)]);
        assert!(placed.iter().all(|p| !p.verified));
    }

    #[test]
    fn a_part_without_a_page_goes_where_its_first_chapter_does_and_past_the_end_is_nowhere() {
        let mut pages = Fake::new(&["Contents", "1 One", "2 Two"], &[]);
        let wanted = [w("Part I", None), w("1 One", Some(PageRef::Arabic(1))), w("Ghost", Some(PageRef::Arabic(900))), w("Part II", None)];
        let placed = place(&mut pages, &wanted, None, &[0]);
        assert_eq!(pages_of(&placed), [Some(1), Some(1), None, None]);
    }

    #[test]
    fn a_title_on_the_contents_page_is_never_taken_as_its_heading() {
        // printed 1 = physical 0 would be the contents page, which lists every title.
        let mut pages = Fake::new(&["1 Intro|2 Next", "cover", "1 Intro", "2 Next"], &[]);
        let placed = place(&mut pages, &[w("1 Intro", Some(PageRef::Arabic(1))), w("2 Next", Some(PageRef::Arabic(2)))], None, &[0]);
        assert_eq!(pages_of(&placed), [Some(2), Some(3)]);
    }

    #[test]
    fn titles_match_headings_however_they_are_spaced_numbered_or_broken() {
        let body = |lines: &[&str]| {
            let lines: Vec<(String, f64, f64)> = lines.iter().map(|l| (compact(l), 500.0, 12.0)).collect();
            let all = lines.iter().map(|l| l.0.as_str()).collect();
            PageBody { lines, all, number: None }
        };
        assert_eq!(find(&body(&["I N T R O D U C T I O N"]), "1 Introduction").0, 2);
        assert_eq!(find(&body(&["CHAPTER 3", "Results"]), "3 Results").0, 2);
        assert_eq!(find(&body(&["Chapter 3 Results"]), "3 Results").0, 2);
        assert_eq!(find(&body(&["A very long heading that is", "set on two lines"]), "A very long heading that is set on two lines").0, 2);
        assert_eq!(find(&body(&["body text mentions the method section in passing"]), "The method section").0, 1);
        assert_eq!(find(&body(&["we discuss results here"]), "Results").0, 0, "a short title must be a heading");
        assert_eq!(find(&body(&["x"]), "1").0, 0);
    }

    #[test]
    fn reads_stay_bounded_on_a_long_document_with_many_entries() {
        let headings: Vec<String> = (0..3000).map(|i| format!("Heading {i}")).collect();
        let refs: Vec<&str> = headings.iter().map(String::as_str).collect();
        let mut pages = Fake::new(&refs, &[]);
        let wanted: Vec<Wanted> = (0..2000u32).map(|i| w(&format!("Heading {}", i + 10), Some(PageRef::Arabic(i + 1)))).collect();
        let placed = place(&mut pages, &wanted, None, &[]);
        assert_eq!(placed[0].page, Some(10));
        assert_eq!(placed[1999].page, Some(2009));
        assert!(pages.reads < 20 * wanted.len(), "{} reads", pages.reads);
    }
}
