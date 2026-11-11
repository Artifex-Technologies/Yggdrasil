//! What the program asks of the computer besides files: running programs (as subprocess.run with captured text output did),
//! following unsquashfs's progress, sleeping, random bytes, the account database, who it runs as, and its two outputs.
//! `Real` is the computer; the tests use stand-ins that write down what they were asked.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};

use crate::py::{decode, oserror, universal, Fail, R};

/// What subprocess.run(..., capture_output=True, text=True) gave back.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub trait System {
    /// subprocess.run(argv, input=stdin, capture_output=True, text=True): without `stdin` the program shares this one's (read
    /// to its end by then).
    fn run(&mut self, argv: &[String], stdin: Option<&str>) -> R<Output>;
    /// A program whose standard output is followed line by line: `line` gets each as it comes, and what it gives back is
    /// printed there and then (as out() prints); then its exit code and standard error.
    fn stream(
        &mut self,
        argv: &[String],
        line: &mut dyn FnMut(&str) -> R<Option<String>>,
    ) -> R<(i32, String)>;
    fn sleep(&mut self, seconds: u64);
    /// os.urandom(n)
    fn random(&mut self, n: usize) -> Vec<u8>;
    /// Whether pwd.getpwnam(login) finds an account.
    fn account_exists(&mut self, login: &str) -> bool;
    fn euid(&mut self) -> u32;
    /// print(line, flush=True)
    fn out(&mut self, line: &str) -> R<()>;
    /// A line on standard error (where the script's tracebacks went).
    fn err(&mut self, line: &str);
}

/// The computer itself.
pub struct Real;

/// A program as Python started it: an argument with a NUL in it is a ValueError before anything runs.
fn command(argv: &[String]) -> R<Command> {
    if argv.iter().any(|a| a.contains('\0')) {
        return Err(Fail::Crash("embedded null byte".into()));
    }
    let mut c = Command::new(&argv[0]);
    c.args(&argv[1..]);
    Ok(c)
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .unwrap_or_else(|| -status.signal().unwrap_or(1))
}

impl System for Real {
    fn run(&mut self, argv: &[String], stdin: Option<&str>) -> R<Output> {
        let mut c = command(argv)?;
        c.stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::inherit()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        let mut child = c.spawn().map_err(|e| oserror(&e, &argv[0]))?;
        if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
            let text = text.to_string();
            // from a thread, so a program that writes a lot before it reads cannot block us both; one that never reads is fine
            std::thread::spawn(move || {
                let _ = pipe.write_all(text.as_bytes());
            });
        }
        let out = child
            .wait_with_output()
            .map_err(|e| oserror(&e, &argv[0]))?;
        let stdout = universal(&decode(&out.stdout)?);
        let stderr = universal(&decode(&out.stderr)?);
        Ok(Output {
            code: exit_code(out.status),
            stdout,
            stderr,
        })
    }

    fn stream(
        &mut self,
        argv: &[String],
        line: &mut dyn FnMut(&str) -> R<Option<String>>,
    ) -> R<(i32, String)> {
        let mut c = command(argv)?;
        c.stdin(Stdio::inherit())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn().map_err(|e| oserror(&e, &argv[0]))?;
        let mut err_pipe = child.stderr.take().unwrap();
        let errors = std::thread::spawn(move || {
            let mut b = vec![];
            let _ = err_pipe.read_to_end(&mut b);
            b
        });
        let mut out = BufReader::new(child.stdout.take().unwrap());
        // lines as a text file in universal-newline mode gives them: ending at \n, \r\n or a lone \r
        let mut buf = vec![];
        loop {
            let mut byte = [0u8; 1];
            let n = out.read(&mut byte).map_err(|e| oserror(&e, &argv[0]))?;
            let end = n == 0;
            if !end {
                match byte[0] {
                    b'\n' => {}
                    b'\r' => {
                        if out
                            .fill_buf()
                            .map(|b| b.first() == Some(&b'\n'))
                            .unwrap_or(false)
                        {
                            out.consume(1);
                        }
                    }
                    b => {
                        buf.push(b);
                        continue;
                    }
                }
                buf.push(b'\n');
            }
            if !buf.is_empty() {
                if let Some(said) = line(&decode(&buf)?)? {
                    self.out(&said)?;
                }
                buf.clear();
            }
            if end {
                break;
            }
        }
        let err = universal(&decode(&errors.join().unwrap_or_default())?);
        let status = child.wait().map_err(|e| oserror(&e, &argv[0]))?;
        Ok((exit_code(status), err))
    }

    fn sleep(&mut self, seconds: u64) {
        std::thread::sleep(std::time::Duration::from_secs(seconds));
    }

    fn random(&mut self, n: usize) -> Vec<u8> {
        let mut b = vec![0u8; n];
        let mut got = 0;
        while got < n {
            let r = unsafe { libc::getrandom(b[got..].as_mut_ptr().cast(), n - got, 0) };
            if r > 0 {
                got += r as usize;
            } else if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                // never a key file of zeros
                panic!("no random bytes: {}", std::io::Error::last_os_error());
            }
        }
        b
    }

    fn account_exists(&mut self, login: &str) -> bool {
        let Ok(name) = std::ffi::CString::new(login) else {
            return false;
        };
        unsafe { !libc::getpwnam(name.as_ptr()).is_null() }
    }

    fn euid(&mut self) -> u32 {
        unsafe { libc::geteuid() }
    }

    fn out(&mut self, line: &str) -> R<()> {
        let mut o = std::io::stdout().lock();
        writeln!(o, "{line}").and_then(|_| o.flush()).map_err(|e| {
            Fail::Crash(match e.raw_os_error() {
                Some(code) => format!("[Errno {code}] {}", crate::py::strerror(code)),
                None => e.to_string(),
            })
        })
    }

    fn err(&mut self, line: &str) {
        let _ = writeln!(std::io::stderr(), "{line}");
    }
}
