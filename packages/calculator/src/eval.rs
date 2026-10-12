//! The calculator's expression evaluator: a tokenizer and a precedence-climbing parser over f64.
//!
//! Accepts what people type or paste into a calculator: + − × ÷ (and - x * /), ^ for powers, !
//! factorial, parentheses, implicit multiplication (2(3+4), 2π), functions, constants and
//! percentages. A percentage after + or − is relative, as on a handheld: 200 + 10% is 220, and
//! 200 − 10% is 180; after × or ÷ it is the plain fraction (200 × 10% is 20); alone it is /100.

use std::f64::consts::{E, PI};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Angle {
    Degrees,
    Radians,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum EvalError {
    Empty,
    DivideByZero,
    /// Something the parser cannot make sense of; the text is a short reason.
    Syntax(String),
    /// A function used outside its domain (sqrt of a negative, ln of zero, ...).
    Domain,
    Overflow,
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EvalError::Empty => write!(f, ""),
            EvalError::DivideByZero => write!(f, "Cannot divide by zero"),
            EvalError::Syntax(s) => write!(f, "{s}"),
            EvalError::Domain => write!(f, "Invalid input"),
            EvalError::Overflow => write!(f, "Overflow"),
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
enum Tok {
    Num(f64),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    Bang,
    Percent,
    LParen,
    RParen,
}

fn tokenize(src: &str) -> Result<Vec<Tok>, EvalError> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = vec![];
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\u{a0}' | ',' => i += 1, // thousands separators are ignored
            '0'..='9' | '.' => {
                let start = i;
                let mut text = String::new();
                while i < chars.len() {
                    if chars[i].is_ascii_digit() || chars[i] == '.' {
                        text.push(chars[i]);
                        i += 1;
                    } else if chars[i] == ',' && !text.contains('.') && is_thousands_comma(&chars, i) {
                        i += 1; // 1,234,567: the comma belongs to the number
                    } else {
                        break;
                    }
                }
                // Scientific notation: 1e5, 2.5E-3 (only when digits follow, so "2e" stays 2 × e).
                if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
                    let mut j = i + 1;
                    if j < chars.len() && (chars[j] == '+' || chars[j] == '-') {
                        j += 1;
                    }
                    if j < chars.len() && chars[j].is_ascii_digit() {
                        while j < chars.len() && chars[j].is_ascii_digit() {
                            j += 1;
                        }
                        text.push('e');
                        text.extend(chars[i + 1..j].iter());
                        i = j;
                    }
                }
                let _ = start;
                let n: f64 = text.parse().map_err(|_| EvalError::Syntax(format!("Bad number {text}")))?;
                out.push(Tok::Num(n));
            }
            '+' => {
                out.push(Tok::Plus);
                i += 1;
            }
            '-' | '−' | '–' => {
                out.push(Tok::Minus);
                i += 1;
            }
            '*' | '×' | '·' | 'x' | 'X' if !(c == 'x' || c == 'X') || next_is_operand(&chars, i) => {
                out.push(Tok::Star);
                i += 1;
            }
            '/' | '÷' => {
                out.push(Tok::Slash);
                i += 1;
            }
            '^' => {
                out.push(Tok::Caret);
                i += 1;
            }
            '!' => {
                out.push(Tok::Bang);
                i += 1;
            }
            '%' => {
                out.push(Tok::Percent);
                i += 1;
            }
            '(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            'π' => {
                out.push(Tok::Ident("pi".into()));
                i += 1;
            }
            '√' => {
                out.push(Tok::Ident("sqrt".into()));
                i += 1;
            }
            c if c.is_alphabetic() => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric()) {
                    i += 1;
                }
                out.push(Tok::Ident(chars[start..i].iter().collect::<String>().to_lowercase()));
            }
            other => return Err(EvalError::Syntax(format!("Unexpected “{other}”"))),
        }
    }
    Ok(out)
}

/// A comma inside a number is a thousands separator when exactly three digits follow it.
fn is_thousands_comma(chars: &[char], i: usize) -> bool {
    let digits = |from: usize| chars[from..].iter().take_while(|c| c.is_ascii_digit()).count();
    i > 0 && chars[i - 1].is_ascii_digit() && digits(i + 1) == 3
}

/// "x" is a multiplication sign when it sits between operands (3 x 4), not the start of a name.
fn next_is_operand(chars: &[char], i: usize) -> bool {
    let prev = chars[..i].iter().rev().find(|c| !c.is_whitespace());
    let next = chars[i + 1..].iter().find(|c| !c.is_whitespace());
    let prev_ok = matches!(prev, Some(c) if c.is_ascii_digit() || *c == ')' || *c == '.' || *c == '%' || *c == '!');
    let next_ok = matches!(next, Some(c) if c.is_ascii_digit() || *c == '(' || *c == '.' || *c == '-' || c.is_alphabetic() || *c == 'π');
    prev_ok && next_ok
}

struct Parser<'a> {
    toks: &'a [Tok],
    pos: usize,
    angle: Angle,
}

/// A parsed value remembers whether it was written as a percentage (for the + and − rule).
#[derive(Clone, Copy, Debug)]
struct Val {
    v: f64,
    percent: bool,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == Some(t) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    // expr := term (('+'|'-') term)*
    fn expr(&mut self) -> Result<Val, EvalError> {
        let mut left = self.term()?;
        loop {
            let plus = match self.peek() {
                Some(Tok::Plus) => true,
                Some(Tok::Minus) => false,
                _ => break,
            };
            self.pos += 1;
            let right = self.term()?;
            // A percentage on the right of + or − is a share of the left operand.
            let r = if right.percent { left.v * right.v / 100.0 } else { right.v };
            left = Val { v: if plus { left.v + r } else { left.v - r }, percent: false };
        }
        Ok(left)
    }

    // term := unary (('*'|'/'|implicit) unary)*
    fn term(&mut self) -> Result<Val, EvalError> {
        let mut left = self.unary()?;
        loop {
            match self.peek() {
                Some(Tok::Star) => {
                    self.pos += 1;
                    let r = self.unary()?;
                    left = Val { v: left.v * as_fraction(r), percent: false };
                }
                Some(Tok::Slash) => {
                    self.pos += 1;
                    let r = self.unary()?;
                    let d = as_fraction(r);
                    if d == 0.0 {
                        return Err(EvalError::DivideByZero);
                    }
                    left = Val { v: left.v / d, percent: false };
                }
                // Implicit multiplication: 2(3+4), 2π, (1+2)(3+4), 3sin(30)
                Some(Tok::LParen) | Some(Tok::Num(_)) | Some(Tok::Ident(_)) => {
                    let r = self.unary()?;
                    left = Val { v: left.v * as_fraction(r), percent: false };
                }
                _ => break,
            }
        }
        Ok(left)
    }

    // unary := ('-'|'+') unary | power
    fn unary(&mut self) -> Result<Val, EvalError> {
        if self.eat(&Tok::Minus) {
            let v = self.unary()?;
            return Ok(Val { v: -v.v, percent: v.percent });
        }
        if self.eat(&Tok::Plus) {
            return self.unary();
        }
        self.power()
    }

    // power := postfix ('^' unary)?   (right associative, and binds tighter than unary minus on its left)
    fn power(&mut self) -> Result<Val, EvalError> {
        let base = self.postfix()?;
        if self.eat(&Tok::Caret) {
            let exp = self.unary()?;
            let v = base.v.powf(exp.v);
            if v.is_nan() {
                return Err(EvalError::Domain);
            }
            if v.is_infinite() {
                return Err(EvalError::Overflow);
            }
            return Ok(Val { v, percent: false });
        }
        Ok(base)
    }

    // postfix := atom ('!' | '%')*
    fn postfix(&mut self) -> Result<Val, EvalError> {
        let mut v = self.atom()?;
        loop {
            if self.eat(&Tok::Bang) {
                v = Val { v: factorial(v.v)?, percent: false };
            } else if self.eat(&Tok::Percent) {
                v = Val { v: v.v, percent: true };
            } else {
                break;
            }
        }
        Ok(v)
    }

    fn atom(&mut self) -> Result<Val, EvalError> {
        match self.peek().cloned() {
            Some(Tok::Num(n)) => {
                self.pos += 1;
                Ok(Val { v: n, percent: false })
            }
            Some(Tok::LParen) => {
                self.pos += 1;
                let v = self.expr()?;
                // A missing ")" at the end is forgiven while typing: sin(30 shows its value.
                if self.peek().is_some() && !self.eat(&Tok::RParen) {
                    return Err(EvalError::Syntax("Missing )".into()));
                }
                Ok(Val { v: v.v, percent: false })
            }
            Some(Tok::Ident(name)) => {
                self.pos += 1;
                if let Some(c) = constant(&name) {
                    return Ok(Val { v: c, percent: false });
                }
                if !is_function(&name) {
                    return Err(EvalError::Syntax(format!("Unknown “{name}”")));
                }
                // A function takes a parenthesised argument, or the next factor: sqrt 9, sin30.
                let arg = if self.peek() == Some(&Tok::LParen) { self.atom()? } else { self.power()? };
                Ok(Val { v: apply(&name, arg.v, self.angle)?, percent: false })
            }
            Some(t) => Err(EvalError::Syntax(match t {
                Tok::RParen => "Unexpected )".into(),
                _ => "Incomplete expression".into(),
            })),
            None => Err(EvalError::Syntax("Incomplete expression".into())),
        }
    }
}

fn as_fraction(v: Val) -> f64 {
    if v.percent {
        v.v / 100.0
    } else {
        v.v
    }
}

fn constant(name: &str) -> Option<f64> {
    match name {
        "pi" => Some(PI),
        "e" => Some(E),
        _ => None,
    }
}

fn is_function(name: &str) -> bool {
    matches!(
        name,
        "sin" | "cos" | "tan" | "asin" | "acos" | "atan" | "sinh" | "cosh" | "tanh" | "ln" | "log" | "lg" | "log2" | "sqrt" | "cbrt" | "abs" | "exp" | "fact" | "floor" | "ceil" | "round"
    )
}

fn factorial(x: f64) -> Result<f64, EvalError> {
    if x < 0.0 || x.fract() != 0.0 {
        return Err(EvalError::Domain);
    }
    if x > 170.0 {
        return Err(EvalError::Overflow);
    }
    Ok((1..=x as u64).fold(1.0, |a, b| a * b as f64))
}

fn apply(name: &str, x: f64, angle: Angle) -> Result<f64, EvalError> {
    let to_rad = |v: f64| if angle == Angle::Degrees { v.to_radians() } else { v };
    let from_rad = |v: f64| if angle == Angle::Degrees { v.to_degrees() } else { v };
    let r = match name {
        "sin" => clean(to_rad(x).sin(), x, angle, 180.0),
        "cos" => clean(to_rad(x).cos(), x + 90.0, angle, 180.0),
        "tan" => {
            // tan is undefined at 90°, 270°...
            if angle == Angle::Degrees && ((x - 90.0) % 180.0).abs() < 1e-9 {
                return Err(EvalError::Domain);
            }
            to_rad(x).tan()
        }
        "asin" => {
            if !(-1.0..=1.0).contains(&x) {
                return Err(EvalError::Domain);
            }
            from_rad(x.asin())
        }
        "acos" => {
            if !(-1.0..=1.0).contains(&x) {
                return Err(EvalError::Domain);
            }
            from_rad(x.acos())
        }
        "atan" => from_rad(x.atan()),
        "sinh" => x.sinh(),
        "cosh" => x.cosh(),
        "tanh" => x.tanh(),
        "ln" => {
            if x <= 0.0 {
                return Err(EvalError::Domain);
            }
            x.ln()
        }
        "log" | "lg" => {
            if x <= 0.0 {
                return Err(EvalError::Domain);
            }
            x.log10()
        }
        "log2" => {
            if x <= 0.0 {
                return Err(EvalError::Domain);
            }
            x.log2()
        }
        "sqrt" => {
            if x < 0.0 {
                return Err(EvalError::Domain);
            }
            x.sqrt()
        }
        "cbrt" => x.cbrt(),
        "abs" => x.abs(),
        "exp" => x.exp(),
        "fact" => factorial(x)?,
        "floor" => x.floor(),
        "ceil" => x.ceil(),
        "round" => x.round(),
        _ => return Err(EvalError::Syntax(format!("Unknown “{name}”"))),
    };
    if r.is_nan() {
        Err(EvalError::Domain)
    } else if r.is_infinite() {
        Err(EvalError::Overflow)
    } else {
        Ok(r)
    }
}

/// sin(180°) is exactly 0 and cos(90°) exactly 0, not 1.2e-16: snap results at multiples of 180°.
fn clean(v: f64, deg_arg: f64, angle: Angle, period: f64) -> f64 {
    if angle == Angle::Degrees && (deg_arg % period).abs() < 1e-9 {
        0.0
    } else {
        v
    }
}

/// Evaluate an expression.
pub fn eval(src: &str, angle: Angle) -> Result<f64, EvalError> {
    let toks = tokenize(src)?;
    if toks.is_empty() {
        return Err(EvalError::Empty);
    }
    let mut p = Parser { toks: &toks, pos: 0, angle };
    let v = p.expr()?;
    if p.pos < toks.len() {
        return Err(EvalError::Syntax(match &toks[p.pos] {
            Tok::RParen => "Unexpected )".to_string(),
            _ => "Incomplete expression".to_string(),
        }));
    }
    if v.v.is_infinite() {
        return Err(EvalError::Overflow);
    }
    if v.v.is_nan() {
        return Err(EvalError::Domain);
    }
    Ok(if v.percent { v.v / 100.0 } else { v.v })
}

// ---------------------------------------------------------------- formatting

/// A result as text: 12 significant decimals (hiding float noise such as 0.30000000000000004),
/// trailing zeros trimmed, and scientific notation for very large or small values.
pub fn format_number(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    let a = v.abs();
    if a >= 1e15 || a < 1e-9 {
        let s = format!("{:.10e}", v);
        let (m, e) = s.split_once('e').unwrap();
        let m = if m.contains('.') { m.trim_end_matches('0').trim_end_matches('.') } else { m };
        return format!("{m}e{e}");
    }
    // 15 significant digits overall, then trim.
    let digits_before = if a >= 1.0 { a.log10().floor() as i32 + 1 } else { 1 };
    let decimals = (15 - digits_before).clamp(0, 12) as usize;
    let s = format!("{:.*}", decimals, v);
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

/// The same with thousands separators in the integer part: 1234567.5 → 1,234,567.5
pub fn group_thousands(s: &str) -> String {
    if s.contains('e') {
        return s.to_string();
    }
    let (neg, rest) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s),
    };
    let (int, frac) = match rest.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (rest, None),
    };
    let mut out = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    format!("{}{}{}", if neg { "-" } else { "" }, out, frac.map(|f| format!(".{f}")).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(s: &str) -> f64 {
        eval(s, Angle::Degrees).unwrap_or_else(|e| panic!("{s}: {e:?}"))
    }
    fn txt(s: &str) -> String {
        format_number(ev(s))
    }

    #[test]
    fn arithmetic_and_precedence() {
        assert_eq!(ev("2+3*4"), 14.0);
        assert_eq!(ev("(2+3)*4"), 20.0);
        assert_eq!(ev("10-2-3"), 5.0);
        assert_eq!(ev("100/10/5"), 2.0);
        assert_eq!(ev("2^3^2"), 512.0); // right associative
        assert_eq!(ev("-2^2"), -4.0); // minus applies after the power
        assert_eq!(ev("2*-3"), -6.0);
        assert_eq!(ev("--4"), 4.0);
    }

    #[test]
    fn symbols_and_separators() {
        assert_eq!(ev("6×7"), 42.0);
        assert_eq!(ev("84÷2"), 42.0);
        assert_eq!(ev("9−4"), 5.0);
        assert_eq!(ev("3 x 4"), 12.0);
        assert_eq!(ev("1,234 + 1"), 1235.0);
        assert_eq!(ev("1,234,567.5"), 1234567.5);
        assert_eq!(ev("2,5"), 10.0); // not three digits after the comma: separate numbers
        assert_eq!(ev("1e3"), 1000.0);
    }

    #[test]
    fn long_equations() {
        assert_eq!(txt("1+2+3+4+5+6+7+8+9+10+11+12+13+14+15+16+17+18+19+20"), "210");
        assert_eq!(txt("((((1+2)*3)-4)*5)/6+7"), "11.166666666667");
        assert_eq!(txt("12.5*8 + 19.99*3 - 4.25 + 7/8"), "156.595");
    }

    #[test]
    fn percentages_follow_calculator_rules() {
        assert_eq!(ev("200+10%"), 220.0);
        assert_eq!(ev("200-10%"), 180.0);
        assert_eq!(ev("200*10%"), 20.0);
        assert_eq!(ev("50%"), 0.5);
        assert_eq!(txt("100+8.25%"), "108.25"); // sales tax
        assert_eq!(txt("84.99*1.0825"), "92.001675");
        assert_eq!(ev("200/50%"), 400.0);
    }

    #[test]
    fn float_noise_is_hidden() {
        assert_eq!(txt("0.1+0.2"), "0.3");
        assert_eq!(txt("1.15*100"), "115");
        assert_eq!(txt("0.1*3"), "0.3");
        assert_eq!(txt("1/3"), "0.333333333333");
        assert_eq!(txt("2/3"), "0.666666666667");
    }

    #[test]
    fn implicit_multiplication() {
        assert_eq!(ev("2(3+4)"), 14.0);
        assert_eq!(ev("(1+2)(3+4)"), 21.0);
        assert!((ev("2π") - 6.283185307179586).abs() < 1e-12);
        assert_eq!(txt("3sin(30)"), "1.5");
    }

    #[test]
    fn functions_in_degrees_and_radians() {
        assert_eq!(txt("sin(30)"), "0.5");
        assert_eq!(txt("sin(90)"), "1");
        assert_eq!(txt("cos(60)"), "0.5");
        assert_eq!(txt("cos(90)"), "0");
        assert_eq!(txt("sin(180)"), "0");
        assert_eq!(txt("tan(45)"), "1");
        assert_eq!(txt("atan(1)"), "45");
        assert_eq!(txt("asin(1)"), "90");
        let rad = eval("sin(pi/2)", Angle::Radians).unwrap();
        assert!((rad - 1.0).abs() < 1e-12);
        assert_eq!(txt("sqrt(144)"), "12");
        assert_eq!(txt("√9"), "3");
        assert_eq!(txt("ln(e)"), "1");
        assert_eq!(txt("log(1000)"), "3");
        assert_eq!(txt("abs(-7.5)"), "7.5");
        assert_eq!(txt("10^3"), "1000");
        assert_eq!(txt("2^0.5*2^0.5"), "2");
    }

    #[test]
    fn factorial_and_limits() {
        assert_eq!(ev("5!"), 120.0);
        assert_eq!(ev("0!"), 1.0);
        assert_eq!(ev("3!+2"), 8.0);
        assert!(matches!(eval("171!", Angle::Degrees), Err(EvalError::Overflow)));
        assert!(matches!(eval("2.5!", Angle::Degrees), Err(EvalError::Domain)));
        assert!(matches!(eval("(-1)!", Angle::Degrees), Err(EvalError::Domain)));
    }

    #[test]
    fn errors_are_specific() {
        assert!(matches!(eval("1/0", Angle::Degrees), Err(EvalError::DivideByZero)));
        assert!(matches!(eval("5/(2-2)", Angle::Degrees), Err(EvalError::DivideByZero)));
        assert!(matches!(eval("sqrt(-4)", Angle::Degrees), Err(EvalError::Domain)));
        assert!(matches!(eval("ln(0)", Angle::Degrees), Err(EvalError::Domain)));
        assert!(matches!(eval("tan(90)", Angle::Degrees), Err(EvalError::Domain)));
        assert!(matches!(eval("", Angle::Degrees), Err(EvalError::Empty)));
        assert!(matches!(eval("2+", Angle::Degrees), Err(EvalError::Syntax(_))));
        assert!(matches!(eval("2)", Angle::Degrees), Err(EvalError::Syntax(_))));
        assert!(matches!(eval("foo(2)", Angle::Degrees), Err(EvalError::Syntax(_))));
        assert!(matches!(eval("2 $ 3", Angle::Degrees), Err(EvalError::Syntax(_))));
    }

    #[test]
    fn unclosed_parenthesis_is_forgiven_while_typing() {
        assert_eq!(ev("(2+3"), 5.0);
        assert_eq!(txt("sin(30"), "0.5");
    }

    #[test]
    fn scientific_notation_and_constants() {
        assert_eq!(ev("1e3"), 1000.0);
        assert_eq!(ev("2.5e-1"), 0.25);
        assert_eq!(txt("2e"), "5.436563656918"); // 2 × e, not an exponent
        assert!((ev("e") - std::f64::consts::E).abs() < 1e-15);
    }

    #[test]
    fn number_formatting() {
        assert_eq!(format_number(0.0), "0");
        assert_eq!(format_number(1234.5), "1234.5");
        assert_eq!(format_number(-0.000123), "-0.000123");
        assert_eq!(format_number(1e20), "1e20");
        assert_eq!(format_number(1.5e-12), "1.5e-12");
        assert_eq!(format_number(123456789012345.0), "123456789012345");
        assert_eq!(group_thousands("1234567.891"), "1,234,567.891");
        assert_eq!(group_thousands("-1000"), "-1,000");
        assert_eq!(group_thousands("999"), "999");
        assert_eq!(group_thousands("1e20"), "1e20");
    }
}
