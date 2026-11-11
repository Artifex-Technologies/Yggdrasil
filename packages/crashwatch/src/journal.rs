//! The crashes whose core is not stored (`Storage=journal` or `none`, a core over `ExternalSizeMax`, a core size limit of
//! 0): systemd-coredump still writes its "process dumped core" entry for each, and journald files it in the crashing user's
//! own journal file (`user-UID.journal`). The watcher follows that one file in this process, through libsystemd's sd-journal
//! (no journalctl child), for that message about this user's own crashes only, from the moment it starts. It opens only the
//! file journald writes now, not the archived ones (each would cost about a megabyte of mapped pages), and is woken by its
//! own inotify watch on that file (main.rs), not by sd-journal's, which wakes for every write of every journal in the folder.
//!
//! Any program of the user can write to that journal, with any message id and fields; journald adds `_EXE` itself, from the
//! writer's /proc entry. So only entries that systemd-coredump wrote count.

use std::ffi::{c_char, c_int, c_void, CString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

#[repr(C)]
struct SdJournal {
    _private: [u8; 0],
}

#[link(name = "systemd")]
extern "C" {
    fn sd_journal_open_files(ret: *mut *mut SdJournal, paths: *const *const c_char, flags: c_int) -> c_int;
    fn sd_journal_close(j: *mut SdJournal);
    fn sd_journal_add_match(j: *mut SdJournal, data: *const c_void, size: usize) -> c_int;
    fn sd_journal_seek_head(j: *mut SdJournal) -> c_int;
    fn sd_journal_seek_tail(j: *mut SdJournal) -> c_int;
    fn sd_journal_previous(j: *mut SdJournal) -> c_int;
    fn sd_journal_next(j: *mut SdJournal) -> c_int;
    fn sd_journal_get_data(j: *mut SdJournal, field: *const c_char, data: *mut *const c_void, length: *mut usize) -> c_int;
}

/// systemd's message id for "process dumped core".
pub const COREDUMP_MESSAGE: &str = "fc2e22bc6ee647b6b90729ab34a250b1";

/// Where systemd-coredump is (the second for systems where /lib is not a link into /usr).
const COREDUMP_EXES: [&str; 2] = ["/usr/lib/systemd/systemd-coredump", "/lib/systemd/systemd-coredump"];

/// A field's value is cut at this many bytes (a program's path is far shorter; nothing here needs more).
const FIELD_MAX: usize = 4096;

fn error(what: &str, r: c_int) -> String {
    format!("{what}: {}", std::io::Error::from_raw_os_error(-r))
}

/// Where journald keeps this computer's journals when they are stored on disk: /var/log/journal/MACHINE-ID (none when the
/// machine id cannot be read).
pub fn default_dir() -> Option<PathBuf> {
    let id = std::fs::read_to_string("/etc/machine-id").ok()?;
    let id = id.trim();
    (id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())).then(|| Path::new("/var/log/journal").join(id))
}

/// The name of the journal file journald writes `uid`'s entries to now (it renames it to user-UID@....journal when full).
pub fn file_name(uid: u32) -> String {
    format!("user-{uid}.journal")
}

/// One journal file, read for systemd-coredump's message about `uid`'s crashes.
pub struct Journal {
    j: *mut SdJournal,
}

impl Journal {
    /// Opens `path`, positioned after its last entry (only what is written from now on is read), or before its first one when
    /// `from_start` (a new file: all of it is new).
    pub fn open(path: &Path, uid: u32, from_start: bool) -> Result<Journal, String> {
        let p = CString::new(path.as_os_str().as_bytes()).map_err(|_| "a journal path with a NUL in it".to_string())?;
        let paths = [p.as_ptr(), std::ptr::null()];
        let mut j: *mut SdJournal = std::ptr::null_mut();
        // SAFETY: a NULL-terminated list of NUL-terminated paths; sd_journal_open_files fills `j` on success
        let r = unsafe { sd_journal_open_files(&mut j, paths.as_ptr(), 0) };
        if r < 0 || j.is_null() {
            return Err(error(&format!("cannot open {}", path.display()), r));
        }
        // from here on Drop closes it
        let journal = Journal { j };
        // matches on different fields must all hold; the two _EXE ones are either
        let exes = COREDUMP_EXES.map(|e| format!("_EXE={e}"));
        for m in [format!("MESSAGE_ID={COREDUMP_MESSAGE}"), format!("COREDUMP_UID={uid}"), exes[0].clone(), exes[1].clone()] {
            // SAFETY: the match's bytes and their length; sd-journal copies them
            let r = unsafe { sd_journal_add_match(j, m.as_ptr().cast(), m.len()) };
            if r < 0 {
                return Err(error("journal match", r));
            }
        }
        // SAFETY: an open journal; the next read starts after the place set here
        unsafe {
            if from_start {
                sd_journal_seek_head(j);
            } else {
                sd_journal_seek_tail(j);
                sd_journal_previous(j);
            }
        }
        Ok(journal)
    }

    /// Hands each matching entry written since the last call to `f`, which reads its fields by name. (sd-journal sees what was
    /// appended to the file without being told.)
    pub fn new_entries(&mut self, mut f: impl FnMut(&dyn Fn(&str) -> Option<String>)) {
        // SAFETY: an open journal; each step moves to the next matching entry
        while unsafe { sd_journal_next(self.j) } > 0 {
            f(&|name| self.field(name));
        }
    }

    /// One field of the current entry (its value, at most FIELD_MAX bytes, as text).
    fn field(&self, name: &str) -> Option<String> {
        let n = CString::new(name).ok()?;
        let mut data: *const c_void = std::ptr::null();
        let mut len: usize = 0;
        // SAFETY: an open journal on an entry; on success `data` points at `len` bytes ("NAME=value") that stay valid until
        // the journal moves, and they are copied before that
        let r = unsafe { sd_journal_get_data(self.j, n.as_ptr(), &mut data, &mut len) };
        if r < 0 || data.is_null() {
            return None;
        }
        // SAFETY: see above
        let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), len) };
        let value = bytes.strip_prefix(name.as_bytes())?.strip_prefix(b"=")?;
        Some(String::from_utf8_lossy(&value[..value.len().min(FIELD_MAX)]).into_owned())
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        // SAFETY: opened by sd_journal_open*, closed once
        unsafe { sd_journal_close(self.j) };
    }
}
