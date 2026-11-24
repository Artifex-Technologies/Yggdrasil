//! Colours for code blocks: comments, strings, numbers and each language's keywords, found by a small scanner of our own (no
//! grammar files). A language it does not know is shown plain.

use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Plain,
    Keyword,
    Str,
    Comment,
    Number,
}

struct Lang {
    /// a comment to the end of the line
    line: &'static [&'static str],
    /// a comment between these
    block: Option<(&'static str, &'static str)>,
    quotes: &'static [char],
    keywords: &'static [&'static str],
}

const SHELL: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac", "in", "function", "return", "local",
    "export", "readonly", "set", "unset", "exit", "break", "continue", "source", "sudo", "echo", "printf", "cd", "shift", "trap",
];
const RUST: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn", "for", "if", "impl",
    "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait",
    "true", "type", "unsafe", "use", "where", "while",
];
const PYTHON: &[&str] = &[
    "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else", "except", "False", "finally",
    "for", "from", "global", "if", "import", "in", "is", "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "return", "True",
    "try", "while", "with", "yield",
];
const C_LIKE: &[&str] = &[
    "auto",
    "break",
    "case",
    "char",
    "const",
    "continue",
    "default",
    "do",
    "double",
    "else",
    "enum",
    "extern",
    "float",
    "for",
    "goto",
    "if",
    "int",
    "long",
    "return",
    "short",
    "signed",
    "sizeof",
    "static",
    "struct",
    "switch",
    "typedef",
    "union",
    "unsigned",
    "void",
    "volatile",
    "while",
    "class",
    "public",
    "private",
    "protected",
    "namespace",
    "new",
    "delete",
    "template",
    "this",
    "true",
    "false",
    "nullptr",
    "bool",
];
const JS: &[&str] = &[
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "default",
    "delete",
    "do",
    "else",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "let",
    "new",
    "null",
    "return",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "undefined",
    "var",
    "void",
    "while",
    "yield",
    "interface",
    "type",
];
const LUA: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in", "local", "nil", "not", "or", "repeat",
    "return", "then", "true", "until", "while",
];
const GO: &[&str] = &[
    "break",
    "case",
    "chan",
    "const",
    "continue",
    "default",
    "defer",
    "else",
    "fallthrough",
    "for",
    "func",
    "go",
    "goto",
    "if",
    "import",
    "interface",
    "map",
    "package",
    "range",
    "return",
    "select",
    "struct",
    "switch",
    "type",
    "var",
    "nil",
    "true",
    "false",
];
const DATA: &[&str] = &["true", "false", "null", "yes", "no", "on", "off"];

fn lang(name: &str) -> Option<Lang> {
    let l = |line, block, quotes, keywords| Some(Lang { line, block, quotes, keywords });
    match name.to_ascii_lowercase().as_str() {
        "sh" | "bash" | "shell" | "zsh" | "console" | "shellsession" => l(&["#"], None, &['"', '\''], SHELL),
        "rs" | "rust" => l(&["//"], Some(("/*", "*/")), &['"'], RUST),
        "py" | "python" => l(&["#"], None, &['"', '\''], PYTHON),
        "c" | "h" | "cpp" | "c++" | "hpp" | "cc" | "java" | "cs" => l(&["//"], Some(("/*", "*/")), &['"', '\''], C_LIKE),
        "js" | "javascript" | "ts" | "typescript" | "jsx" | "tsx" => l(&["//"], Some(("/*", "*/")), &['"', '\'', '`'], JS),
        "lua" => l(&["--"], None, &['"', '\''], LUA),
        "go" => l(&["//"], Some(("/*", "*/")), &['"', '`'], GO),
        "toml" | "ini" | "conf" | "desktop" | "yaml" | "yml" => l(&["#", ";"], None, &['"', '\''], DATA),
        "json" | "jsonc" => l(&["//"], None, &['"'], DATA),
        _ => None,
    }
}

/// The text split into coloured runs (byte ranges, in order, covering all of it); one plain run for a language not known.
pub fn spans(language: &str, text: &str) -> Vec<(Kind, Range<usize>)> {
    let Some(l) = lang(language) else { return vec![(Kind::Plain, 0..text.len())] };
    let mut out: Vec<(Kind, Range<usize>)> = vec![];
    let mut push = |k: Kind, r: Range<usize>| {
        if r.is_empty() {
            return;
        }
        match out.last_mut() {
            Some((lk, lr)) if *lk == k && lr.end == r.start => lr.end = r.end,
            _ => out.push((k, r)),
        }
    };
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let rest = &text[i..];
        if let Some(c) = l.line.iter().find(|c| rest.starts_with(**c)) {
            // `#` in a shell word (a#b) or a Rust attribute (#[..]) is not a comment
            let word_before = i > 0 && !b[i - 1].is_ascii_whitespace() && *c == "#";
            if !word_before {
                let end = rest.find('\n').map(|n| i + n).unwrap_or(b.len());
                push(Kind::Comment, i..end);
                i = end;
                continue;
            }
        }
        if let Some((open, close)) = l.block {
            if let Some(after) = rest.strip_prefix(open) {
                let end = after.find(close).map(|n| i + open.len() + n + close.len()).unwrap_or(b.len());
                push(Kind::Comment, i..end);
                i = end;
                continue;
            }
        }
        let ch = rest.chars().next().unwrap_or(' ');
        if l.quotes.contains(&ch) {
            let mut j = i + ch.len_utf8();
            while j < b.len() {
                let c = text[j..].chars().next().unwrap_or(' ');
                if c == '\\' {
                    j += 1 + text[j + 1..].chars().next().map(char::len_utf8).unwrap_or(0);
                    continue;
                }
                j += c.len_utf8();
                if c == ch || (c == '\n' && ch != '`') {
                    break;
                }
            }
            push(Kind::Str, i..j.min(b.len()));
            i = j.min(b.len());
            continue;
        }
        if ch.is_ascii_digit() && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')) {
            let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '_')).map(|n| i + n).unwrap_or(b.len());
            push(Kind::Number, i..end);
            i = end;
            continue;
        }
        if ch.is_alphabetic() || ch == '_' {
            let end = rest.find(|c: char| !(c.is_alphanumeric() || c == '_')).map(|n| i + n).unwrap_or(b.len());
            let word = &text[i..end];
            push(if l.keywords.contains(&word) { Kind::Keyword } else { Kind::Plain }, i..end);
            i = end;
            continue;
        }
        let end = i + ch.len_utf8();
        push(Kind::Plain, i..end);
        i = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(lang: &str, text: &str) -> Vec<(Kind, String)> {
        spans(lang, text).into_iter().filter(|(k, _)| *k != Kind::Plain).map(|(k, r)| (k, text[r].to_string())).collect()
    }

    #[test]
    fn rust_keywords_strings_numbers_and_comments() {
        let k = kinds("rust", "fn main() { let s = \"a \\\"b\\\"\"; // done\n let n = 42; /* x */ }");
        assert_eq!(
            k,
            [
                (Kind::Keyword, "fn".into()),
                (Kind::Keyword, "let".into()),
                (Kind::Str, "\"a \\\"b\\\"\"".into()),
                (Kind::Comment, "// done".into()),
                (Kind::Keyword, "let".into()),
                (Kind::Number, "42".into()),
                (Kind::Comment, "/* x */".into()),
            ]
        );
    }

    #[test]
    fn a_shell_comment_is_only_a_word_of_its_own() {
        let k = kinds("bash", "echo a#b  # note\nif [ \"$x\" ]; then exit 1; fi");
        assert!(k.contains(&(Kind::Comment, "# note".into())), "{k:?}");
        assert!(!k.iter().any(|(_, t)| t.contains("a#b")));
        assert!(k.contains(&(Kind::Keyword, "then".into())) && k.contains(&(Kind::Str, "\"$x\"".into())));
    }

    #[test]
    fn the_runs_cover_the_text_and_an_unknown_language_is_plain() {
        for (lang, text) in [("toml", "a = \"b\" # c\n[x]\nn = 3"), ("lua", "local x = 'é' -- ç"), ("nope", "anything")] {
            let s = spans(lang, text);
            assert_eq!(s.first().map(|(_, r)| r.start), Some(0));
            assert_eq!(s.last().map(|(_, r)| r.end), Some(text.len()));
            assert!(s.windows(2).all(|w| w[0].1.end == w[1].1.start), "{lang}: {s:?}");
        }
        assert_eq!(spans("nope", "x = 1"), [(Kind::Plain, 0..5)]);
        // an unclosed string or comment ends with the text
        assert_eq!(kinds("python", "x = 'open"), [(Kind::Str, "'open".to_string())]);
    }
}
