//! The command lines as Python 3.14's argparse parsed them, for the few kinds of argument the tools have (a value with or
//! without choices, a switch, one positional, -h/--help): long options shortened to any unique beginning, --opt=value,
//! "--", the same error messages (usage and "PROG: error: ..." on standard error, exit code 2) and the same help, wrapped
//! to the terminal's width (COLUMNS, else the terminal on standard output, else 80) and coloured as argparse colours it
//! (when standard output is a terminal, or FORCE_COLOR / PYTHON_COLORS=1; never with NO_COLOR or TERM=dumb).
//!
//! The help texts are wrapped at spaces only: they hold no hyphens (textwrap would also break after those).

use std::io::Write;

pub enum Kind {
    /// -h/--help: prints the help and exits
    Help,
    /// store_true
    Switch,
    /// one value (nargs=None), optionally from a list of choices, with a default
    Value {
        choices: Option<&'static [&'static str]>,
        default: Option<&'static str>,
    },
}

pub struct Arg {
    /// option strings; none for a positional
    pub flags: &'static [&'static str],
    pub dest: &'static str,
    pub kind: Kind,
    pub help: Option<&'static str>,
}

pub struct Parser {
    pub prog: String,
    pub description: &'static str,
    pub args: Vec<Arg>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    None,
    Str(String),
    Bool(bool),
}

pub struct Values(Vec<(&'static str, Val)>);

impl Values {
    pub fn str(&self, dest: &str) -> Option<&str> {
        match self.0.iter().find(|(d, _)| *d == dest).map(|(_, v)| v) {
            Some(Val::Str(s)) => Some(s),
            _ => None,
        }
    }
    pub fn flag(&self, dest: &str) -> bool {
        matches!(
            self.0.iter().find(|(d, _)| *d == dest).map(|(_, v)| v),
            Some(Val::Bool(true))
        )
    }
}

/// (action index or None for an unknown option, option string, sep, explicit argument)
type Tuple = (Option<usize>, String, Option<String>, Option<String>);

/// take_action: an argument and its values
type Take<'a> = dyn FnMut(&Parser, usize, &[String], &mut Vec<usize>) -> Result<(), Stop> + 'a;

impl Parser {
    /// A parser with -h/--help first, as argparse adds it.
    pub fn new(prog: String, description: &'static str, mut args: Vec<Arg>) -> Parser {
        args.insert(
            0,
            Arg {
                flags: &["-h", "--help"],
                dest: "help",
                kind: Kind::Help,
                help: Some("show this help message and exit"),
            },
        );
        Parser {
            prog,
            description,
            args,
        }
    }

    /// The program's name as argparse takes it: the last part of how it was started.
    pub fn prog_from_argv0() -> String {
        let a0 = std::env::args_os()
            .next()
            .map(|a| a.to_string_lossy().into_owned())
            .unwrap_or_default();
        crate::py::basename(&a0).to_string()
    }

    fn option_strings(&self) -> Vec<(&'static str, usize)> {
        self.args
            .iter()
            .enumerate()
            .flat_map(|(i, a)| a.flags.iter().map(move |f| (*f, i)))
            .collect()
    }

    fn lookup(&self, s: &str) -> Option<usize> {
        self.option_strings()
            .into_iter()
            .find(|(f, _)| *f == s)
            .map(|(_, i)| i)
    }

    fn parse_optional(&self, arg: &str) -> Option<Vec<Tuple>> {
        if !arg.starts_with('-') {
            return None;
        }
        if let Some(i) = self.lookup(arg) {
            return Some(vec![(Some(i), arg.to_string(), None, None)]);
        }
        if arg.chars().count() == 1 {
            return None;
        }
        if let Some((opt, explicit)) = arg.split_once('=') {
            if let Some(i) = self.lookup(opt) {
                return Some(vec![(
                    Some(i),
                    opt.to_string(),
                    Some("=".into()),
                    Some(explicit.to_string()),
                )]);
            }
        }
        let tuples = self.option_tuples(arg);
        if !tuples.is_empty() {
            return Some(tuples);
        }
        // -\.?\d: a negative number is a positional (no option looks like one)
        let mut rest = arg[1..].chars();
        let mut c = rest.next();
        if c == Some('.') {
            c = rest.next();
        }
        if c.is_some_and(|c| pycompat::decimal(c).is_some()) {
            return None;
        }
        if arg.contains(' ') {
            return None;
        }
        Some(vec![(None, arg.to_string(), None, None)])
    }

    fn option_tuples(&self, s: &str) -> Vec<Tuple> {
        let mut out = vec![];
        let second = s.chars().nth(1);
        let (prefix, sep, explicit) = match s.split_once('=') {
            Some((p, e)) => (p, Some("=".to_string()), Some(e.to_string())),
            None => (s, None, None),
        };
        if second == Some('-') {
            for (f, i) in self.option_strings() {
                if f.starts_with(prefix) {
                    out.push((Some(i), f.to_string(), sep.clone(), explicit.clone()));
                }
            }
        } else {
            let short: String = s.chars().take(2).collect();
            let short_explicit: String = s.chars().skip(2).collect();
            for (f, i) in self.option_strings() {
                if f == short {
                    out.push((
                        Some(i),
                        f.to_string(),
                        Some(String::new()),
                        Some(short_explicit.clone()),
                    ));
                } else if f.starts_with(prefix) {
                    out.push((Some(i), f.to_string(), sep.clone(), explicit.clone()));
                }
            }
        }
        out
    }

    fn name(&self, i: usize) -> String {
        let a = &self.args[i];
        if a.flags.is_empty() {
            a.dest.to_string()
        } else {
            a.flags.join("/")
        }
    }

    fn takes_value(&self, i: usize) -> bool {
        matches!(self.args[i].kind, Kind::Value { .. })
    }

    /// The parsed values, or the exit code after the help or an error was printed.
    pub fn parse(&self, argv: &[String]) -> Result<Values, i32> {
        match self.parse_inner(argv) {
            Ok(v) => Ok(v),
            Err(Stop::Help) => {
                let mut out = std::io::stdout();
                let _ = out.write_all(self.format_help().as_bytes());
                let _ = out.flush();
                Err(0)
            }
            Err(Stop::Error(msg)) => {
                let mut err = std::io::stderr();
                let _ = err.write_all(self.format_usage().as_bytes());
                let _ = err.write_all(format!("{}: error: {}\n", self.prog, msg).as_bytes());
                Err(2)
            }
        }
    }

    fn parse_inner(&self, args: &[String]) -> Result<Values, Stop> {
        let mut values: Vec<(&'static str, Val)> = self
            .args
            .iter()
            .filter(|a| !matches!(a.kind, Kind::Help))
            .map(|a| {
                let v = match a.kind {
                    Kind::Switch => Val::Bool(false),
                    Kind::Value {
                        default: Some(d), ..
                    } => Val::Str(d.to_string()),
                    _ => Val::None,
                };
                (a.dest, v)
            })
            .collect();
        // the pattern: O an option, A an argument, - the first "--"
        let mut pattern = String::new();
        let mut options: std::collections::BTreeMap<usize, Vec<Tuple>> = Default::default();
        let mut dashdash = false;
        for (i, a) in args.iter().enumerate() {
            if dashdash {
                pattern.push('A');
            } else if a == "--" {
                pattern.push('-');
                dashdash = true;
            } else if let Some(t) = self.parse_optional(a) {
                options.insert(i, t);
                pattern.push('O');
            } else {
                pattern.push('A');
            }
        }
        let pat: Vec<char> = pattern.chars().collect();
        let mut seen: Vec<usize> = vec![];
        let mut extras: Vec<String> = vec![];
        let mut positionals: Vec<usize> = (0..self.args.len())
            .filter(|&i| self.args[i].flags.is_empty())
            .collect();

        let mut take =
            |this: &Parser, i: usize, vals: &[String], seen: &mut Vec<usize>| -> Result<(), Stop> {
                seen.push(i);
                let a = &this.args[i];
                match a.kind {
                    Kind::Help => Err(Stop::Help),
                    Kind::Switch => {
                        set(&mut values, a.dest, Val::Bool(true));
                        Ok(())
                    }
                    Kind::Value { choices, .. } => {
                        let v = vals[0].clone();
                        if let Some(ch) = choices {
                            if !ch.contains(&v.as_str()) {
                                let list: Vec<String> =
                                    ch.iter().map(|c| pycompat::repr(c)).collect();
                                return Err(Stop::Error(format!(
                                    "argument {}: invalid choice: {} (choose from {})",
                                    this.name(i),
                                    pycompat::repr(&v),
                                    list.join(", ")
                                )));
                            }
                        }
                        set(&mut values, a.dest, Val::Str(v));
                        Ok(())
                    }
                }
            };

        // a positional takes -*A-* (the "--" itself is dropped)
        let consume_positionals = |start: usize,
                                   positionals: &mut Vec<usize>,
                                   seen: &mut Vec<usize>,
                                   take: &mut Take|
         -> Result<usize, Stop> {
            let Some(&p) = positionals.first() else {
                return Ok(start);
            };
            let mut j = start;
            while j < pat.len() && pat[j] == '-' {
                j += 1;
            }
            if j >= pat.len() || pat[j] != 'A' {
                return Ok(start);
            }
            j += 1;
            while j < pat.len() && pat[j] == '-' {
                j += 1;
            }
            let mut vals: Vec<String> = args[start..j].to_vec();
            if pat[start..j].contains(&'-') {
                if let Some(k) = vals.iter().position(|v| v == "--") {
                    vals.remove(k);
                }
            }
            take(self, p, &vals, seen)?;
            positionals.remove(0);
            Ok(j)
        };

        let mut start = 0usize;
        let max_option = options
            .keys()
            .next_back()
            .map(|&m| m as isize)
            .unwrap_or(-1);
        while (start as isize) <= max_option {
            let mut next = start;
            while (next as isize) <= max_option && !options.contains_key(&next) {
                next += 1;
            }
            if start != next {
                let end = consume_positionals(start, &mut positionals, &mut seen, &mut take)?;
                if end > start {
                    start = end;
                    continue;
                }
                start = end;
            }
            if !options.contains_key(&start) {
                extras.extend(args[start..next].iter().cloned());
                start = next;
            }
            start = self.consume_optional(
                start,
                args,
                &pat,
                &options,
                &mut extras,
                &mut seen,
                &mut take,
            )?;
        }
        let stop = consume_positionals(start, &mut positionals, &mut seen, &mut take)?;
        extras.extend(args[stop..].iter().cloned());
        let missing: Vec<String> = (0..self.args.len())
            .filter(|&i| self.args[i].flags.is_empty() && !seen.contains(&i))
            .map(|i| self.name(i))
            .collect();
        if !missing.is_empty() {
            return Err(Stop::Error(format!(
                "the following arguments are required: {}",
                missing.join(", ")
            )));
        }
        if !extras.is_empty() {
            return Err(Stop::Error(format!(
                "unrecognized arguments: {}",
                extras.join(" ")
            )));
        }
        Ok(Values(values))
    }

    #[allow(clippy::too_many_arguments)]
    fn consume_optional(
        &self,
        start: usize,
        args: &[String],
        pat: &[char],
        options: &std::collections::BTreeMap<usize, Vec<Tuple>>,
        extras: &mut Vec<String>,
        seen: &mut Vec<usize>,
        take: &mut Take,
    ) -> Result<usize, Stop> {
        let tuples = &options[&start];
        if tuples.len() > 1 {
            let names: Vec<&str> = tuples.iter().map(|t| t.1.as_str()).collect();
            return Err(Stop::Error(format!(
                "ambiguous option: {} could match {}",
                args[start],
                names.join(", ")
            )));
        }
        let (mut action, mut option_string, mut sep, mut explicit) = tuples[0].clone();
        let mut todo: Vec<(usize, Vec<String>)> = vec![];
        let stop;
        loop {
            let Some(a) = action else {
                extras.push(args[start].clone());
                return Ok(start + 1);
            };
            if let Some(ex) = explicit.clone() {
                let count = if self.takes_value(a) { 1 } else { 0 };
                let second_is_dash = option_string.chars().nth(1) == Some('-');
                if count == 0 && !second_is_dash && !ex.is_empty() {
                    if sep.as_deref().is_some_and(|s| !s.is_empty()) || ex.starts_with('-') {
                        return Err(Stop::Error(format!(
                            "argument {}: ignored explicit argument {}",
                            self.name(a),
                            pycompat::repr(&ex)
                        )));
                    }
                    todo.push((a, vec![]));
                    let first = ex.chars().next().unwrap();
                    option_string = format!("-{first}");
                    if let Some(next) = self.lookup(&option_string) {
                        action = Some(next);
                        let rest: String = ex.chars().skip(1).collect();
                        if rest.is_empty() {
                            sep = None;
                            explicit = None;
                        } else if let Some(r) = rest.strip_prefix('=') {
                            sep = Some("=".into());
                            explicit = Some(r.to_string());
                        } else {
                            sep = Some(String::new());
                            explicit = Some(rest);
                        }
                    } else {
                        extras.push(format!("-{ex}"));
                        stop = start + 1;
                        break;
                    }
                } else if count == 1 {
                    stop = start + 1;
                    todo.push((a, vec![ex]));
                    break;
                } else {
                    return Err(Stop::Error(format!(
                        "argument {}: ignored explicit argument {}",
                        self.name(a),
                        pycompat::repr(&ex)
                    )));
                }
            } else {
                let s = start + 1;
                let count = if self.takes_value(a) {
                    if pat.get(s) != Some(&'A') {
                        return Err(Stop::Error(format!(
                            "argument {}: expected one argument",
                            self.name(a)
                        )));
                    }
                    1
                } else {
                    0
                };
                stop = s + count;
                todo.push((a, args[s..stop].to_vec()));
                break;
            }
        }
        for (a, vals) in todo {
            take(self, a, &vals, seen)?;
        }
        Ok(stop)
    }

    // ---------------------------------------------------------------- help and usage

    pub fn format_usage(&self) -> String {
        let f = Fmt::new(&self.prog);
        let usage = f.usage(self);
        finish(&usage)
    }

    pub fn format_help(&self) -> String {
        let f = Fmt::new(&self.prog);
        let mut root = String::new();
        root.push_str(&f.usage(self));
        root.push_str(&f.text(self.description));
        // the longest invocation (indented by 2 inside a section) sets the help column
        let max_len = self
            .args
            .iter()
            .enumerate()
            .map(|(i, _)| f.invocation(self, i).1.chars().count() + 2)
            .max()
            .unwrap_or(0);
        let positionals: Vec<usize> = (0..self.args.len())
            .filter(|&i| self.args[i].flags.is_empty())
            .collect();
        let optionals: Vec<usize> = (0..self.args.len())
            .filter(|&i| !self.args[i].flags.is_empty())
            .collect();
        for (heading, items) in [
            ("positional arguments", positionals),
            ("options", optionals),
        ] {
            if items.is_empty() {
                continue;
            }
            let body: String = items.iter().map(|&i| f.action(self, i, max_len)).collect();
            root.push_str(&format!(
                "\n{}{}:{}\n{}\n",
                f.t.heading, heading, f.t.reset, body
            ));
        }
        finish(&format!("\n{root}\n"))
    }
}

fn set(values: &mut [(&'static str, Val)], dest: &str, v: Val) {
    if let Some(slot) = values.iter_mut().find(|(d, _)| *d == dest) {
        slot.1 = v;
    }
}

enum Stop {
    Help,
    Error(String),
}

/// HelpFormatter.format_help's end: runs of blank lines become one, no blank lines around.
fn finish(s: &str) -> String {
    let mut out = String::new();
    let mut newlines = 0;
    for c in s.chars() {
        if c == '\n' {
            newlines += 1;
            continue;
        }
        if newlines > 0 {
            out.push_str(if newlines >= 2 { "\n\n" } else { "\n" });
            newlines = 0;
        }
        out.push(c);
    }
    let out = out.trim_start_matches('\n');
    format!("{out}\n")
}

struct Theme {
    usage: &'static str,
    prog: &'static str,
    heading: &'static str,
    summary_long_option: &'static str,
    summary_short_option: &'static str,
    summary_label: &'static str,
    summary_action: &'static str,
    long_option: &'static str,
    short_option: &'static str,
    label: &'static str,
    action: &'static str,
    reset: &'static str,
}

const COLOURS: Theme = Theme {
    usage: "\x1b[1;34m",
    prog: "\x1b[1;35m",
    heading: "\x1b[1;34m",
    summary_long_option: "\x1b[36m",
    summary_short_option: "\x1b[32m",
    summary_label: "\x1b[33m",
    summary_action: "\x1b[32m",
    long_option: "\x1b[1;36m",
    short_option: "\x1b[1;32m",
    label: "\x1b[1;33m",
    action: "\x1b[1;32m",
    reset: "\x1b[0m",
};

const PLAIN: Theme = Theme {
    usage: "",
    prog: "",
    heading: "",
    summary_long_option: "",
    summary_short_option: "",
    summary_label: "",
    summary_action: "",
    long_option: "",
    short_option: "",
    label: "",
    action: "",
    reset: "",
};

fn decolor(s: &str) -> String {
    let mut out = s.to_string();
    for code in [
        "\x1b[1;34m",
        "\x1b[1;35m",
        "\x1b[36m",
        "\x1b[32m",
        "\x1b[33m",
        "\x1b[1;36m",
        "\x1b[1;32m",
        "\x1b[1;33m",
        "\x1b[0m",
    ] {
        out = out.replace(code, "");
    }
    out
}

fn len(s: &str) -> i64 {
    decolor(s).chars().count() as i64
}

/// shutil.get_terminal_size().columns
fn terminal_columns() -> i64 {
    if let Some(n) = std::env::var("COLUMNS")
        .ok()
        .and_then(|v| pycompat::int(&v))
    {
        if n > 0 {
            return n.min(i64::MAX as i128) as i64;
        }
    }
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) } == 0 && ws.ws_col > 0 {
        ws.ws_col as i64
    } else {
        80
    }
}

/// _colorize.can_colorize() for standard output
fn can_colorize() -> bool {
    let env = |k: &str| std::env::var(k).ok();
    match env("PYTHON_COLORS").as_deref() {
        Some("0") => return false,
        Some("1") => return true,
        _ => {}
    }
    if env("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    if env("FORCE_COLOR").is_some_and(|v| !v.is_empty()) {
        return true;
    }
    if env("TERM").as_deref() == Some("dumb") {
        return false;
    }
    unsafe { libc::isatty(1) == 1 }
}

struct Fmt {
    prog: String,
    width: i64,
    max_help_position: i64,
    t: &'static Theme,
}

impl Fmt {
    fn new(prog: &str) -> Fmt {
        let width = terminal_columns() - 2;
        Fmt {
            prog: prog.to_string(),
            width,
            max_help_position: 24.min((width - 20).max(4)),
            t: if can_colorize() { &COLOURS } else { &PLAIN },
        }
    }

    fn metavar(a: &Arg) -> String {
        match a.kind {
            Kind::Value {
                choices: Some(ch), ..
            } => format!("{{{}}}", ch.join(",")),
            _ if a.flags.is_empty() => a.dest.to_string(),
            _ => a.dest.to_uppercase(),
        }
    }

    /// _format_usage
    fn usage(&self, p: &Parser) -> String {
        let t = self.t;
        let prefix = "usage: ";
        let mut opt_parts = vec![];
        let mut pos_parts = vec![];
        for a in &p.args {
            if a.flags.is_empty() {
                pos_parts.push(format!(
                    "{}{}{}",
                    t.summary_action,
                    Self::metavar(a),
                    t.reset
                ));
                continue;
            }
            let o = a.flags[0];
            let colour = if o.chars().count() > 2 {
                t.summary_long_option
            } else {
                t.summary_short_option
            };
            let part = match a.kind {
                Kind::Value { .. } => format!(
                    "{colour}{o} {}{}{}",
                    t.summary_label,
                    Self::metavar(a),
                    t.reset
                ),
                _ => format!("{colour}{o}{}", t.reset),
            };
            opt_parts.push(format!("[{part}]"));
        }
        let prog = self.prog.clone();
        let parts: Vec<String> = opt_parts.iter().chain(pos_parts.iter()).cloned().collect();
        let mut usage = std::iter::once(prog.clone())
            .chain(parts.iter().cloned())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let text_width = self.width;
        if prefix.len() as i64 + len(&usage) > text_width {
            let get_lines = |parts: &[String], indent: &str, prefix: Option<&str>| -> Vec<String> {
                let mut lines: Vec<String> = vec![];
                let mut line: Vec<String> = vec![];
                let indent_len = indent.chars().count() as i64;
                let mut line_len = match prefix {
                    Some(p) => p.chars().count() as i64 - 1,
                    None => indent_len - 1,
                };
                for part in parts {
                    let part_len = len(part);
                    if line_len + 1 + part_len > text_width && !line.is_empty() {
                        lines.push(format!("{indent}{}", line.join(" ")));
                        line.clear();
                        line_len = indent_len - 1;
                    }
                    line.push(part.clone());
                    line_len += part_len + 1;
                }
                if !line.is_empty() {
                    lines.push(format!("{indent}{}", line.join(" ")));
                }
                if prefix.is_some() && !lines.is_empty() {
                    lines[0] = lines[0].chars().skip(indent_len as usize).collect();
                }
                lines
            };
            let prog_len = len(&prog);
            let lines: Vec<String>;
            if (prefix.len() as i64 + prog_len) as f64 <= 0.75 * text_width as f64 {
                let indent = " ".repeat((prefix.len() as i64 + prog_len + 1) as usize);
                let first: Vec<String> = std::iter::once(prog.clone())
                    .chain(opt_parts.iter().cloned())
                    .collect();
                if !opt_parts.is_empty() {
                    let mut l = get_lines(&first, &indent, Some(prefix));
                    l.extend(get_lines(&pos_parts, &indent, None));
                    lines = l;
                } else if !pos_parts.is_empty() {
                    let first: Vec<String> = std::iter::once(prog.clone())
                        .chain(pos_parts.iter().cloned())
                        .collect();
                    lines = get_lines(&first, &indent, Some(prefix));
                } else {
                    lines = vec![prog.clone()];
                }
            } else {
                let indent = " ".repeat(prefix.len());
                let all: Vec<String> = opt_parts.iter().chain(pos_parts.iter()).cloned().collect();
                let mut l = get_lines(&all, &indent, None);
                if l.len() > 1 {
                    l = get_lines(&opt_parts, &indent, None);
                    l.extend(get_lines(&pos_parts, &indent, None));
                }
                let mut with_prog = vec![prog.clone()];
                with_prog.extend(l);
                lines = with_prog;
            }
            usage = lines.join("\n");
        }
        let rest = usage
            .strip_prefix(prog.as_str())
            .unwrap_or(&usage)
            .to_string();
        format!(
            "{}{prefix}{}{}{prog}{}{rest}\n\n",
            t.usage, t.reset, t.prog, t.reset
        )
    }

    /// _format_text: the description, filled to the width
    fn text(&self, text: &str) -> String {
        let width = (self.width).max(11);
        let words = normalise(text);
        format!("{}\n\n", wrap(&words, width).join("\n"))
    }

    /// (coloured, plain) invocation of an argument
    fn invocation(&self, p: &Parser, i: usize) -> (String, String) {
        let t = self.t;
        let a = &p.args[i];
        if a.flags.is_empty() {
            let m = Self::metavar(a);
            return (format!("{}{m}{}", t.action, t.reset), m);
        }
        let coloured: Vec<String> = a
            .flags
            .iter()
            .map(|f| {
                if f.chars().count() > 2 {
                    format!("{}{f}{}", t.long_option, t.reset)
                } else {
                    format!("{}{f}{}", t.short_option, t.reset)
                }
            })
            .collect();
        let plain = a.flags.join(", ");
        match a.kind {
            Kind::Value { .. } => {
                let m = Self::metavar(a);
                (
                    format!("{} {}{m}{}", coloured.join(", "), t.label, t.reset),
                    format!("{plain} {m}"),
                )
            }
            _ => (coloured.join(", "), plain),
        }
    }

    /// _format_action, inside a section (indent 2)
    fn action(&self, p: &Parser, i: usize, max_len: usize) -> String {
        let indent = 2i64;
        let help_position = (max_len as i64 + 2).min(self.max_help_position);
        let help_width = (self.width - help_position).max(11);
        let action_width = help_position - indent - 2;
        let (header_c, header) = self.invocation(p, i);
        let help = p.args[i].help;
        let mut out = String::new();
        let indent_first = match help {
            None => {
                return format!("{}{header_c}\n", " ".repeat(indent as usize));
            }
            Some(_) if header.chars().count() as i64 <= action_width => {
                let pad = action_width as usize - header.chars().count();
                out.push_str(&format!(
                    "{}{header_c}{}  ",
                    " ".repeat(indent as usize),
                    " ".repeat(pad)
                ));
                0
            }
            Some(_) => {
                out.push_str(&format!("{}{header_c}\n", " ".repeat(indent as usize)));
                help_position
            }
        };
        let help = help.unwrap();
        if !help.trim().is_empty() {
            let lines = wrap(&normalise(help), help_width);
            out.push_str(&format!(
                "{}{}\n",
                " ".repeat(indent_first as usize),
                lines[0]
            ));
            for l in &lines[1..] {
                out.push_str(&format!("{}{l}\n", " ".repeat(help_position as usize)));
            }
        } else if !out.ends_with('\n') {
            out.push('\n');
        }
        out
    }
}

/// re.sub(r'\s+', ' ', text, flags=re.ASCII).strip()
fn normalise(text: &str) -> String {
    text.split([' ', '\t', '\n', '\r', '\x0b', '\x0c'])
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// textwrap.wrap(text, width) for text of words and single spaces without hyphens
fn wrap(text: &str, width: i64) -> Vec<String> {
    // the chunks: words and the spaces between them, reversed so the next one is last
    let mut chunks: Vec<String> = vec![];
    for (k, w) in text.split(' ').enumerate() {
        if k > 0 {
            chunks.push(" ".into());
        }
        chunks.push(w.to_string());
    }
    chunks.reverse();
    let clen = |s: &str| s.chars().count() as i64;
    let mut lines: Vec<String> = vec![];
    while !chunks.is_empty() {
        let mut cur: Vec<String> = vec![];
        let mut cur_len = 0i64;
        if chunks.last().unwrap().trim().is_empty() && !lines.is_empty() {
            chunks.pop();
        }
        while let Some(c) = chunks.last() {
            let l = clen(c);
            if cur_len + l <= width {
                cur_len += l;
                cur.push(chunks.pop().unwrap());
            } else {
                break;
            }
        }
        // a word longer than the line: as much of it as fits
        if let Some(c) = chunks.pop_if(|c| clen(c) > width) {
            let space_left = if width < 1 { 1 } else { width - cur_len };
            let head: String = c.chars().take(space_left as usize).collect();
            let tail: String = c.chars().skip(space_left as usize).collect();
            cur.push(head);
            chunks.push(tail);
        }
        if cur.last().is_some_and(|c| c.trim().is_empty()) {
            cur.pop();
        }
        if !cur.is_empty() {
            lines.push(cur.concat());
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping_follows_textwrap() {
        assert_eq!(wrap("one two three", 7), vec!["one two", "three"]);
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
        assert_eq!(wrap("a abcdefghij", 4), vec!["a ab", "cdef", "ghij"]);
        assert_eq!(finish("\nusage: x\n\n\n\nbody\n\n"), "usage: x\n\nbody\n");
    }
}
