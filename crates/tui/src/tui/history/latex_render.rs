//! LaTeX math expression rendering for the TUI transcript.
//! Renders `$...$` (inline) and `$$...$$` (display) math expressions using
//! Unicode approximations for terminal display.
use ratatui::style::{Color, Modifier, Style};
use std::sync::OnceLock;
#[allow(dead_code)]
fn math_style() -> Style {
    Style::default()
        .fg(Color::Rgb(80, 180, 230))
        .add_modifier(Modifier::ITALIC)
}
#[allow(dead_code)]
fn math_display_style() -> Style {
    Style::default()
        .fg(Color::Rgb(60, 160, 220))
        .add_modifier(Modifier::BOLD)
}
/// Private: find the start of math delimiter ($, $$, \(, \[) in text.
fn find_math_start(text: &str) -> Option<usize> {
    let b = text.as_bytes();
    for (idx, &byte) in b.iter().enumerate() {
        if byte == b'$' {
            return Some(idx);
        }
        if byte == b'\\' && idx + 1 < b.len() {
            if b[idx + 1] == b'(' || b[idx + 1] == b'[' {
                return Some(idx);
            }
        }
    }
    None
}
/// Private: if text starts with a math delimiter, find closing delimiter and return (end_pos, is_display).
fn find_math_end(text: &str) -> Option<(usize, bool)> {
    let b = text.as_bytes();
    if b.starts_with(b"$$") {
        for i in 2..b.len().saturating_sub(1) {
            if b[i] == b'{' {
                continue;
            }
            if b[i..].starts_with(b"$$") {
                return Some((i, true));
            }
        }
    } else if b.starts_with(b"$") && !b.starts_with(b"$$") {
        for (j, &byte) in b[1..].iter().enumerate() {
            if byte == b'{' {
                continue;
            }
            if byte == b'$' {
                return Some((j + 1, false));
            }
        }
    } else if b.starts_with(b"\\[") {
        for i in 2..b.len().saturating_sub(1) {
            if b[i] == b'{' {
                continue;
            }
            if b[i..].starts_with(b"\\]") {
                return Some((i, true));
            }
        }
    } else if b.starts_with(b"\\(") {
        for i in 2..b.len().saturating_sub(1) {
            if b[i] == b'{' {
                continue;
            }
            if b[i..].starts_with(b"\\)") {
                return Some((i, false));
            }
        }
    }
    None
}
fn math_delim_offset(text: &str) -> usize {
    let b = text.as_bytes();
    if b.starts_with(b"$$") || b.starts_with(b"\\[") || b.starts_with(b"\\(") {
        2
    } else {
        1
    }
}
/// Replace math delimiters with plain Unicode, preserving everything else.
pub fn render_latex_in_text(text: &str) -> String {
    let mut result = String::new();
    let mut i = 0;
    while i < text.len() {
        let remaining = &text[i..];
        if let Some((end, _is_display)) = find_math_end(remaining) {
            let offset = math_delim_offset(remaining);
            let inner = &remaining[offset..end];
            result.push_str(&render_latex_to_string(inner));
            let close_len: usize = if remaining.as_bytes().get(end..end + 2) == Some(b"\\]")
                || remaining.as_bytes().get(end..end + 2) == Some(b"$$")
            {
                2
            } else if remaining.as_bytes().get(end..end + 1) == Some(b"$") {
                1
            } else if remaining.as_bytes().get(end..end + 2) == Some(b"\\)") {
                2
            } else {
                0
            };
            i += end + close_len;
        } else {
            let skip = find_math_start(remaining).unwrap_or(remaining.len());
            if skip == 0 {
                // Unmatched opening delimiter ($, $$, \(, \[) during streaming:
                // push it as plain text so the loop can advance.
                result.push(remaining.chars().next().unwrap_or('$'));
                i += remaining.chars().next().map(|c| c.len_utf8()).unwrap_or(1);
            } else {
                result.push_str(&remaining[..skip]);
                i += skip;
                if skip == remaining.len() {
                    break;
                }
            }
        }
    }
    result
}
fn render_latex_to_string(latex: &str) -> String {
    let mut out = String::new();
    let mut chars = latex.trim().chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                let mut cmd = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_ascii_alphabetic() {
                        cmd.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if cmd.is_empty() {
                    if let Some(&c) = chars.peek() {
                        match c {
                            '{' | '}' | '$' | '%' | '#' | '&' | '_' | ' ' => {
                                chars.next();
                            }
                            _ => {
                                out.push(c);
                                chars.next();
                            }
                        }
                    }
                    continue;
                }
                match cmd.as_str() {
                    "frac" => {
                        let n = render_latex_to_string(&read_braced(&mut chars));
                        let d = render_latex_to_string(&read_braced(&mut chars));
                        out.push_str(&format!("({}/{})", n, d));
                    }
                    "sqrt" => {
                        let _n = read_optional_sqrt_root(&mut chars);
                        let r = render_latex_to_string(&read_braced(&mut chars));
                        out.push_str(&format!("\u{221a}({})", r));
                    }
                    "sum" => {
                        out.push('\u{2211}');
                        skip_limits(&mut chars);
                    }
                    "prod" => {
                        out.push('\u{220f}');
                        skip_limits(&mut chars);
                    }
                    "int" => {
                        out.push('\u{222b}');
                        skip_limits(&mut chars);
                    }
                    "iint" => {
                        out.push('\u{222c}');
                    }
                    "iiint" => {
                        out.push('\u{222d}');
                    }
                    "oint" => {
                        out.push('\u{222e}');
                    }
                    "lim" => {
                        out.push_str("lim");
                        skip_limits(&mut chars);
                    }
                    "sin" | "cos" | "tan" | "cot" | "sec" | "csc" | "log" | "ln" | "lg" | "exp"
                    | "det" | "dim" | "ker" | "hom" | "max" | "min" | "sup" | "inf" | "arg"
                    | "deg" | "mod" | "gcd" | "lcm" => out.push_str(&cmd),
                    "to" | "rightarrow" => out.push('\u{2192}'),
                    "leftarrow" => out.push('\u{2190}'),
                    "Rightarrow" => out.push('\u{21d2}'),
                    "Leftarrow" => out.push('\u{21d0}'),
                    "Leftrightarrow" | "iff" => out.push('\u{21d4}'),
                    "mapsto" => out.push('\u{21a6}'),
                    "longrightarrow" => out.push('\u{27f6}'),
                    "Longrightarrow" => out.push('\u{27f9}'),
                    "uparrow" => out.push('\u{2191}'),
                    "downarrow" => out.push('\u{2193}'),
                    "Uparrow" => out.push('\u{21d1}'),
                    "Downarrow" => out.push('\u{21d3}'),
                    _ => {
                        if let Some(u) = SYMBOLS.get_or_init(build_symbols).get(cmd.as_str()) {
                            out.push_str(u);
                        }
                    }
                }
            }
            '_' => {
                let sub = read_optional_arg(&mut chars);
                append_subscript(&render_latex_to_string(&sub), &mut out);
            }
            '^' => {
                let sup = read_optional_arg(&mut chars);
                append_superscript(&render_latex_to_string(&sup), &mut out);
            }
            '{' | '}' => {}
            ' ' => {
                if !out.ends_with(' ') {
                    out.push(' ');
                }
            }
            '\n' => {
                if !out.ends_with(' ') {
                    out.push(' ');
                }
            }
            _ => out.push(ch),
        }
    }
    out.trim().to_string()
}
fn read_braced(chars: &mut std::iter::Peekable<std::str::Chars>) -> String {
    let mut s = String::new();
    let mut depth: u32 = 0;
    if chars.next_if_eq(&'{').is_some() {
        depth = 1;
    }
    while let Some(&c) = chars.peek() {
        match c {
            '{' => {
                depth += 1;
                s.push(c);
                chars.next();
            }
            '}' => {
                depth = depth.saturating_sub(1);
                chars.next();
                if depth == 0 {
                    break;
                }
                s.push('}');
            }
            _ => {
                s.push(c);
                chars.next();
            }
        }
    }
    s
}
fn read_optional_arg(chars: &mut std::iter::Peekable<std::str::Chars>) -> String {
    if chars.peek() == Some(&'{') {
        read_braced(chars)
    } else {
        let mut s = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_alphanumeric() || c == '+' || c == '-' {
                s.push(c);
                chars.next();
            } else {
                break;
            }
        }
        s
    }
}
fn read_optional_sqrt_root(chars: &mut std::iter::Peekable<std::str::Chars>) -> String {
    if chars.peek() == Some(&'[') {
        chars.next();
        let mut s = String::new();
        while let Some(&c) = chars.peek() {
            if c == ']' {
                chars.next();
                break;
            }
            s.push(c);
            chars.next();
        }
        s
    } else {
        String::new()
    }
}
fn skip_limits(chars: &mut std::iter::Peekable<std::str::Chars>) {
    if chars.peek() == Some(&'_') {
        chars.next();
        let _ = read_optional_arg(chars);
    }
    if chars.peek() == Some(&'^') {
        chars.next();
        let _ = read_optional_arg(chars);
    }
}
fn append_superscript(s: &str, out: &mut String) {
    for c in s.chars() {
        out.push(match c {
            '0' => '\u{2070}',
            '1' => '\u{00b9}',
            '2' => '\u{00b2}',
            '3' => '\u{00b3}',
            '4' => '\u{2074}',
            '5' => '\u{2075}',
            '6' => '\u{2076}',
            '7' => '\u{2077}',
            '8' => '\u{2078}',
            '9' => '\u{2079}',
            '+' => '\u{207a}',
            '-' => '\u{207b}',
            '=' => '\u{207c}',
            'n' => '\u{207f}',
            'i' => '\u{2071}',
            _ => c,
        });
    }
}
fn append_subscript(s: &str, out: &mut String) {
    for c in s.chars() {
        out.push(match c {
            '0' => '\u{2080}',
            '1' => '\u{2081}',
            '2' => '\u{2082}',
            '3' => '\u{2083}',
            '4' => '\u{2084}',
            '5' => '\u{2085}',
            '6' => '\u{2086}',
            '7' => '\u{2087}',
            '8' => '\u{2088}',
            '9' => '\u{2089}',
            '+' => '\u{208a}',
            '-' => '\u{208b}',
            '=' => '\u{208c}',
            'a' => '\u{2090}',
            'e' => '\u{2091}',
            'h' => '\u{2095}',
            'i' => '\u{1d62}',
            'k' => '\u{2096}',
            'l' => '\u{2097}',
            'm' => '\u{2098}',
            'n' => '\u{2099}',
            'o' => '\u{2092}',
            'p' => '\u{209a}',
            'r' => '\u{1d63}',
            's' => '\u{209b}',
            't' => '\u{209c}',
            'u' => '\u{1d64}',
            'v' => '\u{1d65}',
            'x' => '\u{2093}',
            _ => c,
        });
    }
}
type SymbolMap = std::collections::HashMap<&'static str, &'static str>;
fn build_symbols() -> SymbolMap {
    let mut m = SymbolMap::new();
    for (k, v) in [
        ("alpha", "\u{03b1}"),
        ("beta", "\u{03b2}"),
        ("gamma", "\u{03b3}"),
        ("delta", "\u{03b4}"),
        ("epsilon", "\u{03b5}"),
        ("zeta", "\u{03b6}"),
        ("eta", "\u{03b7}"),
        ("theta", "\u{03b8}"),
        ("iota", "\u{03b9}"),
        ("kappa", "\u{03ba}"),
        ("lambda", "\u{03bb}"),
        ("mu", "\u{03bc}"),
        ("nu", "\u{03bd}"),
        ("xi", "\u{03be}"),
        ("pi", "\u{03c0}"),
        ("rho", "\u{03c1}"),
        ("sigma", "\u{03c3}"),
        ("tau", "\u{03c4}"),
        ("upsilon", "\u{03c5}"),
        ("phi", "\u{03c6}"),
        ("chi", "\u{03c7}"),
        ("psi", "\u{03c8}"),
        ("omega", "\u{03c9}"),
        ("varepsilon", "\u{03b5}"),
        ("vartheta", "\u{03d1}"),
        ("varphi", "\u{03c6}"),
    ] {
        m.insert(k, v);
    }
    for (k, v) in [
        ("Gamma", "\u{0393}"),
        ("Delta", "\u{0394}"),
        ("Theta", "\u{0398}"),
        ("Lambda", "\u{039b}"),
        ("Xi", "\u{039e}"),
        ("Pi", "\u{03a0}"),
        ("Sigma", "\u{03a3}"),
        ("Upsilon", "\u{03a5}"),
        ("Phi", "\u{03a6}"),
        ("Psi", "\u{03a8}"),
        ("Omega", "\u{03a9}"),
    ] {
        m.insert(k, v);
    }
    for (k, v) in [
        ("infty", "\u{221e}"),
        ("partial", "\u{2202}"),
        ("nabla", "\u{2207}"),
        ("ell", "\u{2113}"),
        ("hbar", "\u{210f}"),
        ("Im", "\u{2111}"),
        ("Re", "\u{211c}"),
        ("emptyset", "\u{2205}"),
        ("aleph", "\u{2135}"),
        ("angle", "\u{2220}"),
        ("perp", "\u{22a5}"),
        ("parallel", "\u{2225}"),
        ("prime", "\u{2032}"),
        ("surd", "\u{221a}"),
        ("top", "\u{22a4}"),
    ] {
        m.insert(k, v);
    }
    for (k, v) in [
        ("in", "\u{2208}"),
        ("notin", "\u{2209}"),
        ("ni", "\u{220b}"),
        ("subset", "\u{2282}"),
        ("supset", "\u{2283}"),
        ("subseteq", "\u{2286}"),
        ("supseteq", "\u{2287}"),
        ("cup", "\u{222a}"),
        ("cap", "\u{2229}"),
        ("vee", "\u{2228}"),
        ("wedge", "\u{2227}"),
        ("oplus", "\u{2295}"),
        ("otimes", "\u{2297}"),
        ("odot", "\u{2299}"),
        ("forall", "\u{2200}"),
        ("exists", "\u{2203}"),
        ("neg", "\u{00ac}"),
        ("sim", "\u{223c}"),
        ("simeq", "\u{2243}"),
        ("cong", "\u{2245}"),
        ("approx", "\u{2248}"),
        ("neq", "\u{2260}"),
        ("ne", "\u{2260}"),
        ("equiv", "\u{2261}"),
        ("le", "\u{2264}"),
        ("ge", "\u{2265}"),
        ("leq", "\u{2264}"),
        ("geq", "\u{2265}"),
        ("ll", "\u{226a}"),
        ("gg", "\u{226b}"),
        ("times", "\u{00d7}"),
        ("div", "\u{00f7}"),
        ("pm", "\u{00b1}"),
        ("mp", "\u{2213}"),
        ("cdot", "\u{00b7}"),
        ("ast", "\u{2217}"),
        ("circ", "\u{2218}"),
        ("bullet", "\u{2022}"),
        ("cdots", "\u{2026}"),
        ("ldots", "\u{2026}"),
        ("vdots", "\u{22ee}"),
        ("ddots", "\u{22f1}"),
    ] {
        m.insert(k, v);
    }
    for (k, v) in [
        ("mathbb{R}", "\u{211d}"),
        ("mathbb{C}", "\u{2102}"),
        ("mathbb{N}", "\u{2115}"),
        ("mathbb{Q}", "\u{211a}"),
        ("mathbb{Z}", "\u{2124}"),
        ("mathbb{P}", "\u{2119}"),
        ("mathbb{H}", "\u{210d}"),
        ("mathbb{F}", "\u{1d53b}"),
    ] {
        m.insert(k, v);
    }
    for (k, v) in [
        ("mathcal{L}", "\u{2112}"),
        ("mathcal{H}", "\u{210b}"),
        ("mathcal{R}", "\u{211b}"),
    ] {
        m.insert(k, v);
    }
    m
}
static SYMBOLS: OnceLock<SymbolMap> = OnceLock::new();
#[allow(dead_code)]
pub fn lookup_symbol(cmd: &str) -> Option<&'static str> {
    SYMBOLS.get_or_init(build_symbols).get(cmd).copied()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_superscript() {
        assert_eq!(render_latex_to_string("x^2"), "x\u{00b2}");
    }
    #[test]
    fn test_subscript() {
        assert_eq!(render_latex_to_string("x_1"), "x\u{2081}");
    }
    #[test]
    fn test_blackboard() {
        assert_eq!(render_latex_to_string(r"\mathbb{R}"), "\u{211d}");
    }
    #[test]
    fn test_infty() {
        assert_eq!(render_latex_to_string(r"\infty"), "\u{221e}");
    }
    #[test]
    fn test_inline_dollar() {
        let r = render_latex_in_text(r"text $x^2$ more");
        assert_eq!(r, "text x\u{00b2} more");
    }
    #[test]
    fn test_display_bracket() {
        let r = render_latex_in_text(r"text \[x^2\] more");
        assert_eq!(r, "text x\u{00b2} more");
    }
}
