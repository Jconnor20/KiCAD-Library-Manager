//! Minimal KiCad s-expression reader/writer. Every token is kept, so syntax we
//! don't understand survives a round trip untouched.

use anyhow::{Result, bail};

#[derive(Clone, Debug, PartialEq)]
pub enum Sexpr {
    /// Unquoted token: keyword, number, `yes`/`no`.
    Atom(String),
    /// Quoted string.
    Str(String),
    List(Vec<Sexpr>),
}

use Sexpr::*;

impl Sexpr {
    /// `(head items...)`
    pub fn list(head: &str, items: Vec<Sexpr>) -> Sexpr {
        let mut v = vec![Atom(head.into())];
        v.extend(items);
        List(v)
    }

    pub fn str(s: &str) -> Sexpr {
        Str(s.into())
    }

    pub fn text(&self) -> Option<&str> {
        match self {
            Atom(s) | Str(s) => Some(s),
            List(_) => None,
        }
    }

    pub fn items(&self) -> &[Sexpr] {
        match self {
            List(v) => v,
            _ => &[],
        }
    }

    pub fn items_mut(&mut self) -> &mut Vec<Sexpr> {
        match self {
            List(v) => v,
            _ => panic!("items_mut on a non-list"),
        }
    }

    pub fn head(&self) -> Option<&str> {
        match self.items().first() {
            Some(Atom(a)) => Some(a),
            _ => None,
        }
    }

    /// Text of the i-th argument after the head.
    pub fn arg(&self, i: usize) -> Option<&str> {
        self.items().get(i + 1).and_then(Sexpr::text)
    }

    pub fn set_arg(&mut self, i: usize, value: Sexpr) {
        self.items_mut()[i + 1] = value;
    }

    pub fn children<'a>(&'a self, head: &'a str) -> impl Iterator<Item = &'a Sexpr> {
        self.items().iter().filter(move |c| c.head() == Some(head))
    }

    pub fn child(&self, head: &str) -> Option<&Sexpr> {
        self.items().iter().find(|c| c.head() == Some(head))
    }
}

pub fn parse(src: &str) -> Result<Sexpr> {
    let mut stack: Vec<Vec<Sexpr>> = vec![vec![]];
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '(' => stack.push(vec![]),
            ')' => {
                if stack.len() < 2 {
                    bail!("unbalanced ')'");
                }
                let done = stack.pop().unwrap();
                stack.last_mut().unwrap().push(List(done));
            }
            '"' => {
                let mut s = String::new();
                loop {
                    match chars.next() {
                        Some('\\') => match chars.next() {
                            Some('n') => s.push('\n'),
                            Some(e) => s.push(e),
                            None => bail!("unterminated string"),
                        },
                        Some('"') => break,
                        Some(ch) => s.push(ch),
                        None => bail!("unterminated string"),
                    }
                }
                stack.last_mut().unwrap().push(Str(s));
            }
            c if c.is_whitespace() => {}
            _ => {
                let mut s = String::from(c);
                while let Some(&n) = chars.peek() {
                    if n.is_whitespace() || n == '(' || n == ')' || n == '"' {
                        break;
                    }
                    s.push(n);
                    chars.next();
                }
                stack.last_mut().unwrap().push(Atom(s));
            }
        }
    }
    if stack.len() != 1 {
        bail!("unbalanced '('");
    }
    let mut root = stack.pop().unwrap();
    if root.len() != 1 {
        bail!("expected exactly one top-level expression, found {}", root.len());
    }
    Ok(root.pop().unwrap())
}

/// Writes in KiCad's own layout: atom-only lists inline, nested lists on new tab-indented lines.
pub fn to_string(x: &Sexpr) -> String {
    let mut out = String::new();
    write(x, 0, &mut out);
    out.push('\n');
    out
}

fn write(x: &Sexpr, depth: usize, out: &mut String) {
    match x {
        Atom(a) => out.push_str(a),
        Str(s) => {
            out.push('"');
            for c in s.chars() {
                match c {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    _ => out.push(c),
                }
            }
            out.push('"');
        }
        List(v) => {
            out.push('(');
            let mut broken = false;
            for (i, c) in v.iter().enumerate() {
                if broken || matches!(c, List(_)) {
                    broken = true;
                    out.push('\n');
                    out.extend(std::iter::repeat_n('\t', depth + 1));
                } else if i > 0 {
                    out.push(' ');
                }
                write(c, depth + 1, out);
            }
            if broken {
                out.push('\n');
                out.extend(std::iter::repeat_n('\t', depth));
            }
            out.push(')');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_keeps_everything() {
        let src = r#"(kicad_symbol_lib (version 20251024) (symbol "A\"B" (pin_names (offset 1.016)) (unknown_thing yes) (property "Value" "line1\nline2\\x")))"#;
        let a = parse(src).unwrap();
        let b = parse(&to_string(&a)).unwrap();
        assert_eq!(a, b);
        let sym = a.child("symbol").unwrap();
        assert_eq!(sym.arg(0), Some("A\"B"));
        assert_eq!(sym.child("property").unwrap().arg(1), Some("line1\nline2\\x"));
    }

    #[test]
    fn rejects_unbalanced() {
        assert!(parse("(a (b)").is_err());
        assert!(parse("(a))").is_err());
        assert!(parse("(a \"open)").is_err());
    }

    /// Round-trips a real KiCad library when one is installed.
    #[test]
    fn round_trip_installed_kicad_library() {
        let Some(p) = ["C:/Program Files/KiCad/10.0/share/kicad/symbols/Device.kicad_sym", "/usr/share/kicad/symbols/Device.kicad_sym"]
            .iter()
            .map(std::path::Path::new)
            .find(|p| p.exists())
        else {
            return;
        };
        let a = parse(&std::fs::read_to_string(p).unwrap()).unwrap();
        assert_eq!(a, parse(&to_string(&a)).unwrap());
    }
}
