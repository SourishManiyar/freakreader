//! The structure of a formula recovered from its typeset glyphs.
//!
//! A small typed tree that LaTeX is written from ([`Node::to_latex`]) and that MathML or a search
//! index can be written from later. It records what was read, not how it was spelled: a fraction is a
//! [`Node::Fraction`] whether the source said `\frac`, `\over` or `{a \over b}`.

use super::symbols::{Alphabet, Class};

/// A symbol of the symbol table, as it appears in a formula.
#[derive(Clone, Debug, PartialEq)]
pub struct Sym {
    /// The LaTeX that sets it: `\alpha`, `+`, `x`.
    pub latex: String,
    /// Its Unicode text; empty when it has none.
    pub unicode: String,
    pub class: Class,
}

/// How a delimiter's size was chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sizing {
    /// `\left … \right`: as tall as what it encloses.
    Auto,
    /// `\bigl … \bigr` and its larger forms, 1 (`\big`) to 4 (`\Bigg`): chosen by hand.
    Manual(u8),
}

/// A space the glyphs leave between two marks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    /// `\,`.
    Thin,
    /// `\:`.
    Medium,
    /// `\;`.
    Thick,
    /// `\!`.
    NegThin,
    /// `\quad`.
    Quad,
    /// `\qquad`.
    Qquad,
}

impl Space {
    pub fn latex(self) -> &'static str {
        match self {
            Space::Thin => "\\,",
            Space::Medium => "\\:",
            Space::Thick => "\\;",
            Space::NegThin => "\\!",
            Space::Quad => "\\quad ",
            Space::Qquad => "\\qquad ",
        }
    }
}

/// How the limits of a large operator are placed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limits {
    /// Above and below, as TeX does for the operator in display style (`\sum` in a display).
    Above,
    /// At the right, as scripts.
    Side,
}

/// A recovered formula.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    /// Things in reading order.
    Row(Vec<Node>),
    /// One symbol.
    Symbol(Sym),
    /// Letters or digits in a particular alphabet: `\mathbb{R}`, `\mathrm{tr}`.
    Styled {
        alphabet: Alphabet,
        text: String,
    },
    /// A function name set upright, as `\sin`, `\lim`.
    Function(String),
    /// Words set in text: `\text{ and }`. Spaces in it are as drawn.
    Text(String),
    /// Something with a subscript, a superscript, or both.
    Scripts {
        base: Box<Node>,
        sub: Option<Box<Node>>,
        sup: Option<Box<Node>>,
    },
    Fraction {
        numerator: Box<Node>,
        denominator: Box<Node>,
        style: FractionStyle,
    },
    Radical {
        index: Option<Box<Node>>,
        body: Box<Node>,
    },
    /// A large operator (`\sum`, `\int`) with the things above and below it or at its side.
    BigOp {
        op: Sym,
        sub: Option<Box<Node>>,
        sup: Option<Box<Node>>,
        limits: Limits,
        forced: bool,
    },
    /// Delimiters around something. A side may be missing (`\left.`).
    Delimited {
        left: Option<Sym>,
        body: Box<Node>,
        right: Option<Sym>,
        sizing: Sizing,
    },
    /// An accent over a base: `\hat{x}`.
    Accent {
        accent: String,
        base: Box<Node>,
    },
    Overline(Box<Node>),
    Space(Space),
}

/// `\frac`, or the form that forces a style.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FractionStyle {
    Plain,
    /// `\dfrac`: display style whatever surrounds it.
    Display,
    /// `\tfrac`: text style whatever surrounds it.
    Text,
}

impl Node {
    /// The LaTeX of the formula.
    pub fn to_latex(&self) -> String {
        let mut w = Writer::default();
        w.node(self);
        w.out
    }

    /// Whether the node is the empty row.
    pub fn is_empty(&self) -> bool {
        matches!(self, Node::Row(items) if items.is_empty())
    }
}

#[derive(Default)]
struct Writer {
    out: String,
}

/// Whether `s` ends in a control word (a backslash and letters), which needs a space before a letter.
fn ends_in_control_word(s: &str) -> bool {
    let letters = s.chars().rev().take_while(char::is_ascii_alphabetic).count();
    if letters == 0 {
        return false;
    }
    let before = &s[..s.len() - letters];
    before.ends_with('\\') && !before.ends_with("\\\\")
}

impl Writer {
    /// Appends a token, with a space first when it would run into a preceding control word.
    fn token(&mut self, t: &str) {
        if t.starts_with(|c: char| c.is_ascii_alphabetic()) && ends_in_control_word(&self.out) {
            self.out.push(' ');
        }
        self.out.push_str(t);
    }

    fn space(&mut self) {
        if !self.out.is_empty() && !self.out.ends_with(' ') {
            self.out.push(' ');
        }
    }

    /// `n` as a braced group.
    fn group(&mut self, n: &Node) {
        self.out.push('{');
        self.node(n);
        self.out.push('}');
    }

    /// A node that is one TeX atom as it stands; a script of anything else needs braces.
    fn needs_braces(n: &Node) -> bool {
        !matches!(
            n,
            Node::Symbol(_)
                | Node::Styled { .. }
                | Node::Function(_)
                | Node::Text(_)
                | Node::Delimited { .. }
                | Node::BigOp { .. }
                | Node::Accent { .. }
                | Node::Scripts { .. }
        )
    }

    fn node(&mut self, n: &Node) {
        match n {
            Node::Row(items) => {
                for (k, item) in items.iter().enumerate() {
                    let loud = |x: &Node| matches!(x, Node::Symbol(s) if matches!(s.class, Class::Bin | Class::Rel));
                    let function = |x: &Node| matches!(x, Node::Function(_));
                    if k > 0 && (loud(item) || loud(&items[k - 1]) || function(&items[k - 1]) && !matches!(item, Node::Delimited { .. })) {
                        self.space();
                    }
                    self.node(item);
                }
            }
            Node::Symbol(s) => self.token(&s.latex),
            Node::Styled { alphabet, text } => match alphabet.command() {
                Some(cmd) => {
                    self.token(cmd);
                    self.out.push('{');
                    self.out.push_str(text);
                    self.out.push('}');
                }
                None => self.token(text),
            },
            Node::Function(name) => self.token(name),
            Node::Text(t) => {
                self.token("\\text");
                self.out.push('{');
                self.out.push_str(t);
                self.out.push('}');
            }
            Node::Scripts { base, sub, sup } => {
                if Self::needs_braces(base) {
                    self.group(base);
                } else {
                    self.node(base);
                }
                // A superscript that starts with primes writes them as `'`.
                let (primes, rest) = split_primes(sup.as_deref());
                self.out.push_str(&"'".repeat(primes));
                if let Some(s) = sub {
                    self.out.push('_');
                    self.group(s);
                }
                if let Some(s) = rest {
                    self.out.push('^');
                    self.group(s);
                }
            }
            Node::Fraction { numerator, denominator, style } => {
                self.token(match style {
                    FractionStyle::Plain => "\\frac",
                    FractionStyle::Display => "\\dfrac",
                    FractionStyle::Text => "\\tfrac",
                });
                self.group(numerator);
                self.group(denominator);
            }
            Node::Radical { index, body } => {
                self.token("\\sqrt");
                if let Some(i) = index {
                    self.out.push('[');
                    self.node(i);
                    self.out.push(']');
                }
                self.group(body);
            }
            Node::BigOp { op, sub, sup, limits, forced } => {
                self.token(&op.latex);
                if *forced {
                    self.token(match limits {
                        Limits::Above => "\\limits",
                        Limits::Side => "\\nolimits",
                    });
                }
                if let Some(s) = sub {
                    self.out.push('_');
                    self.group(s);
                }
                if let Some(s) = sup {
                    self.out.push('^');
                    self.group(s);
                }
            }
            Node::Delimited { left, body, right, sizing } => {
                let side = |w: &mut Writer, d: &Option<Sym>, left: bool| {
                    let latex = d.as_ref().map_or(".", |s| s.latex.as_str());
                    match sizing {
                        Sizing::Auto => w.token(if left { "\\left" } else { "\\right" }),
                        Sizing::Manual(k) => {
                            let name = ["\\bigl", "\\Bigl", "\\biggl", "\\Biggl"];
                            let name_r = ["\\bigr", "\\Bigr", "\\biggr", "\\Biggr"];
                            let list = if left { name } else { name_r };
                            w.token(list.get(usize::from(k.saturating_sub(1))).copied().unwrap_or(list[0]));
                        }
                    }
                    w.token(latex);
                };
                side(self, left, true);
                self.node(body);
                side(self, right, false);
            }
            Node::Accent { accent, base } => {
                self.token(accent);
                self.group(base);
            }
            Node::Overline(b) => {
                self.token("\\overline");
                self.group(b);
            }
            Node::Space(s) => self.token(s.latex()),
        }
    }
}

/// The primes at the start of a superscript, and what follows them.
fn split_primes(sup: Option<&Node>) -> (usize, Option<&Node>) {
    fn is_prime(n: &Node) -> bool {
        matches!(n, Node::Symbol(s) if s.latex == "\\prime")
    }
    match sup {
        None => (0, None),
        Some(n) if is_prime(n) => (1, None),
        Some(Node::Row(items)) => {
            let k = items.iter().take_while(|n| is_prime(n)).count();
            if k == 0 {
                (0, sup)
            } else if k == items.len() {
                (k, None)
            } else {
                // The remainder is a row of its own; the caller writes it as the superscript. A row
                // cannot borrow a suffix of `items`, so the remainder is kept when it is one node.
                match items.get(k..) {
                    Some([one]) => (k, Some(one)),
                    _ => (0, sup),
                }
            }
        }
        Some(_) => (0, sup),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(latex: &str, class: Class) -> Node {
        Node::Symbol(Sym { latex: latex.to_owned(), unicode: String::new(), class })
    }

    #[test]
    fn control_words_get_a_space_before_a_letter() {
        let row = Node::Row(vec![sym("\\alpha", Class::Ord), sym("x", Class::Ord), sym("\\beta", Class::Ord), sym("2", Class::Ord)]);
        assert_eq!(row.to_latex(), "\\alpha x\\beta2");
    }

    #[test]
    fn scripts_fractions_and_radicals() {
        let x = sym("x", Class::Ord);
        let s = Node::Scripts { base: Box::new(x.clone()), sub: Some(Box::new(sym("i", Class::Ord))), sup: Some(Box::new(sym("2", Class::Ord))) };
        assert_eq!(s.to_latex(), "x_{i}^{2}");
        let f = Node::Fraction { numerator: Box::new(x.clone()), denominator: Box::new(sym("y", Class::Ord)), style: FractionStyle::Plain };
        assert_eq!(f.to_latex(), "\\frac{x}{y}");
        let r = Node::Radical { index: Some(Box::new(sym("3", Class::Ord))), body: Box::new(x.clone()) };
        assert_eq!(r.to_latex(), "\\sqrt[3]{x}");
        // A script on a fraction needs braces around the fraction.
        let sf = Node::Scripts { base: Box::new(f), sub: None, sup: Some(Box::new(sym("2", Class::Ord))) };
        assert_eq!(sf.to_latex(), "{\\frac{x}{y}}^{2}");
    }

    #[test]
    fn primes_are_written_as_primes() {
        let prime = || sym("\\prime", Class::Ord);
        let f = sym("f", Class::Ord);
        let one = Node::Scripts { base: Box::new(f.clone()), sub: None, sup: Some(Box::new(prime())) };
        assert_eq!(one.to_latex(), "f'");
        let two = Node::Scripts { base: Box::new(f.clone()), sub: None, sup: Some(Box::new(Node::Row(vec![prime(), prime()]))) };
        assert_eq!(two.to_latex(), "f''");
        let more = Node::Scripts { base: Box::new(f), sub: None, sup: Some(Box::new(Node::Row(vec![prime(), sym("2", Class::Ord)]))) };
        assert_eq!(more.to_latex(), "f'^{2}");
    }

    #[test]
    fn delimiters_and_big_operators() {
        let d = |l: &str| Some(Sym { latex: l.to_owned(), unicode: String::new(), class: Class::Open });
        let x = sym("x", Class::Ord);
        let auto = Node::Delimited { left: d("("), body: Box::new(x.clone()), right: d(")"), sizing: Sizing::Auto };
        assert_eq!(auto.to_latex(), "\\left(x\\right)");
        let big = Node::Delimited { left: d("["), body: Box::new(x.clone()), right: None, sizing: Sizing::Manual(2) };
        assert_eq!(big.to_latex(), "\\Bigl[x\\Bigr.");
        let sum = Node::BigOp {
            op: Sym { latex: "\\sum".to_owned(), unicode: String::new(), class: Class::Large },
            sub: Some(Box::new(sym("i", Class::Ord))),
            sup: Some(Box::new(sym("n", Class::Ord))),
            limits: Limits::Side,
            forced: true,
        };
        assert_eq!(sum.to_latex(), "\\sum\\nolimits_{i}^{n}");
    }

    #[test]
    fn binary_operators_and_relations_are_spaced() {
        let row = Node::Row(vec![sym("a", Class::Ord), sym("+", Class::Bin), sym("b", Class::Ord), sym("=", Class::Rel), sym("c", Class::Ord)]);
        assert_eq!(row.to_latex(), "a + b = c");
    }
}
