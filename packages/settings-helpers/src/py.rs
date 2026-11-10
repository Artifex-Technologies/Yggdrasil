//! How the Python scripts met the outside, so the ports meet it the same way: text files read in universal-newline mode (a lone
//! \r ends a line too) and strictly as UTF-8, commands run with their output captured as text, one line from standard input,
//! and what happened where a script stopped with an error (a traceback and exit code 1): a Crash.

use std::ffi::OsStr;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};

/// Where a script would have stopped with a traceback: its exit code is 1, its message is on standard error.
#[derive(Debug, Clone, PartialEq)]
pub struct Crash(pub String);

impl From<io::Error> for Crash {
    fn from(e: io::Error) -> Crash {
        Crash(oserror(&e, None))
    }
}

pub type R<T> = Result<T, Crash>;

/// str(OSError) as Python writes it: "[Errno 2] No such file or directory: 'path'".
pub fn oserror(e: &io::Error, path: Option<&str>) -> String {
    let code = e.raw_os_error().unwrap_or(0);
    let text = if code != 0 { strerror(code) } else { e.to_string() };
    match path {
        Some(p) => format!("[Errno {code}] {text}: {}", pycompat::repr(p)),
        None => format!("[Errno {code}] {text}"),
    }
}

/// The C library's text for an error number (what Python puts in an OSError).
pub fn strerror(code: i32) -> String {
    unsafe { std::ffi::CStr::from_ptr(libc::strerror(code)).to_string_lossy().into_owned() }
}

/// Text read in universal-newline mode: \r\n and a lone \r become \n.
pub fn universal(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// open(path, encoding="utf-8").read() in universal-newline mode. Ok(None): an OSError (the scripts caught those);
/// Err: the text is not UTF-8 (a UnicodeDecodeError, which they did not catch).
pub fn read_text(path: impl AsRef<Path>) -> R<Option<String>> {
    let bytes = match std::fs::read(path.as_ref()) {
        Ok(b) => b,
        Err(_) => return Ok(None),
    };
    match String::from_utf8(bytes) {
        Ok(t) => Ok(Some(universal(&t))),
        Err(e) => Err(Crash(format!("UnicodeDecodeError: {}: {e}", path.as_ref().display()))),
    }
}

/// The lines of a text as `for line in f` gives them: each with its \n (the last one may have none).
pub fn lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// What subprocess.run(..., capture_output=True, text=True) gives back.
#[derive(Debug, Default, Clone)]
pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// subprocess.run(argv, input=stdin, capture_output=True, text=True). A program that cannot be started is a Crash (Python
/// raised FileNotFoundError there). The program gets no standard input when none is given (the script's programs inherited
/// the helper's own, which they never read).
pub fn run<S: AsRef<OsStr>>(argv: &[S], stdin: Option<&str>) -> R<Output> {
    let mut c = Command::new(&argv[0]);
    c.args(&argv[1..]).stdout(Stdio::piped()).stderr(Stdio::piped());
    c.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    let shown = argv[0].as_ref().to_string_lossy().into_owned();
    let mut child = c.spawn().map_err(|e| Crash(oserror(&e, Some(&shown))))?;
    if let Some(text) = stdin {
        let mut si = child.stdin.take().unwrap();
        let text = text.to_string();
        // written from a thread, so a program that writes a lot before it reads cannot block us both
        std::thread::spawn(move || {
            let _ = si.write_all(text.as_bytes());
        });
    }
    let out = child.wait_with_output()?;
    use std::os::unix::process::ExitStatusExt;
    let code = out.status.code().unwrap_or_else(|| -(out.status.signal().unwrap_or(1)));
    Ok(Output { code, stdout: universal(&String::from_utf8_lossy(&out.stdout)), stderr: universal(&String::from_utf8_lossy(&out.stderr)) })
}

/// sys.stdin.readline(): up to and with the first \n (or to the end), decoded strictly as UTF-8 (a Crash otherwise).
pub fn read_stdin_line() -> R<String> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    let mut stdin = io::stdin().lock();
    loop {
        match stdin.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                line.push(byte[0]);
                if byte[0] == b'\n' {
                    break;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    String::from_utf8(line).map_err(|_| Crash("UnicodeDecodeError: 'utf-8' codec can't decode standard input".into()))
}

/// An environment variable as os.environ.get(name, default) gives it (set but empty stays empty).
pub fn env_or(name: &str, default: &str) -> String {
    std::env::var_os(name).map(|v| v.to_string_lossy().into_owned()).unwrap_or_else(|| default.to_string())
}

/// bool(os.environ.get(name)): set and not empty.
pub fn env_set(name: &str) -> bool {
    std::env::var_os(name).map(|v| !v.is_empty()).unwrap_or(false)
}

/// os.path.join(a, b) for a b that does not start with a slash.
pub fn join(a: &str, b: &str) -> String {
    if b.starts_with('/') {
        b.to_string()
    } else if a.is_empty() || a.ends_with('/') {
        format!("{a}{b}")
    } else {
        format!("{a}/{b}")
    }
}

/// os.path.dirname(p)
pub fn dirname(p: &str) -> String {
    match p.rfind('/') {
        None => String::new(),
        Some(i) => {
            let head = &p[..i + 1];
            let trimmed = head.trim_end_matches('/');
            if trimmed.is_empty() {
                head.to_string()
            } else {
                trimmed.to_string()
            }
        }
    }
}

/// os.path.basename(p)
pub fn basename(p: &str) -> &str {
    match p.rfind('/') {
        None => p,
        Some(i) => &p[i + 1..],
    }
}

/// text[:n] in characters.
pub fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// The last n characters (text[-n:]).
pub fn last_chars(s: &str, n: usize) -> String {
    let count = s.chars().count();
    s.chars().skip(count.saturating_sub(n)).collect()
}

/// Prints what a script's traceback would have ended with and gives its exit code.
pub fn crashed(prog: &str, c: &Crash) -> i32 {
    eprintln!("{prog}: unexpected error (the Python script stopped here with a traceback): {}", c.0);
    1
}
