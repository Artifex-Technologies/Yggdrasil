//! `--prefetch`: while the person is still choosing, and the computer is online, the packages the install will download are
//! fetched beforehand, so the install itself is faster. The choices come as one line of JSON on standard input, as the plan
//! does (`{"browsers": [...], "extras": [...]}`); the window closes standard input to stop it (or when it is closed itself).
//!
//! What is fetched is what the install would download: the chosen browsers' packages the medium does not have, and the
//! repository packages of the chosen optional parts it lacks (installed at the first login), with their dependencies. First
//! as the install's own `pacman -S` would get them, with the medium's package lists (a copy, in a private database); when the
//! mirrors have moved on, as its `pacman -Syu` fallback would: fresh lists (in the private database only) and the updates too.
//! One package at a time (`pacman -Swdd`, one download at a time, nice and idle I/O), into `DIR/pkg`. Nothing is installed and
//! the live system's own package database is never written. The install copies what is there into the new system's package
//! cache before it installs (`Ctx::use_prefetched`), and pacman checks each package's signature then, as always; anything that
//! is missing or has moved on is downloaded as before. It needs root (the window runs it with pkexec, as the install).
//!
//! Lines: `PREFETCH done total` (as each package is in), `INFO`, `WARN` (not done: the install downloads as usual), `ERROR`,
//! `DONE`; `STOPPED` when standard input closed first.

use std::collections::HashSet;

use crate::catalog;
use crate::conf::Os;
use crate::install::Ctx;
use crate::py::{self, R};
use crate::pyjson::Py;

/// Where it keeps its database, its pacman.conf and the packages (the live system's /run: memory, gone at shutdown).
pub const DIR: &str = "/run/yggdrasil-install/prefetch";
/// Room left free on the live system beside the packages.
const MARGIN: i128 = 256 << 20;

/// The packages the install would download for these choices (before dependencies), less those the medium has (`installed`).
pub fn wanted(choices: &Py, os: &Os, installed: &mut dyn FnMut(&str) -> bool) -> Vec<String> {
    let ids =
        |k: &str| -> Vec<String> { choices.at(k).as_list().unwrap_or(&[]).iter().filter_map(|b| b.as_str().map(str::to_string)).collect() };
    let mut out: Vec<String> = vec![];
    if choices.get("browsers").is_some() {
        let catalog = catalog::entries(os.browsers_file.as_deref(), "browsers");
        out.extend(catalog::browser_plan(&catalog, &ids("browsers")).install);
    }
    if choices.get("extras").is_some() {
        let catalog = catalog::entries(os.extras_file.as_deref(), "extras");
        let comps = catalog::components(os.components_file.as_deref());
        let have: HashSet<String> = comps.values().flat_map(catalog::component_packages).filter(|x| installed(x)).collect::<HashSet<_>>();
        for cid in catalog::extras_plan(&catalog, &ids("extras"), &comps, &have).queue {
            // the repositories' packages only: what comes from the AUR or a recipe is built at the first login
            out.extend(comps.get(&cid).map(|c| catalog::texts(c.at("packages"))).unwrap_or_default());
        }
    }
    let mut seen = HashSet::new();
    out.retain(|x| seen.insert(x.clone()) && !installed(x));
    out
}

/// `pacman.conf` with one download at a time.
pub fn one_download(conf: &str) -> String {
    let mut out = vec![];
    let (mut options, mut set) = (false, false);
    for line in conf.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            if options && !set {
                out.push("ParallelDownloads = 1".to_string());
                set = true;
            }
            options = t == "[options]";
        }
        let key = t.trim_start_matches('#').split('=').next().unwrap_or("").trim();
        if options && key == "ParallelDownloads" {
            if !set {
                out.push("ParallelDownloads = 1".to_string());
                set = true;
            }
            continue;
        }
        out.push(line.to_string());
    }
    if !set {
        if !options {
            out.push("[options]".to_string());
        }
        out.push("ParallelDownloads = 1".to_string());
    }
    out.join("\n") + "\n"
}

/// `pacman -Sp --print-format '%r/%n %s'`: each package and its size in bytes.
pub fn listed(text: &str) -> Vec<(String, i128)> {
    text.lines()
        .filter_map(|l| {
            let (name, size) = l.trim().split_once(' ')?;
            (name.contains('/') && !name.starts_with('-')).then(|| (name.to_string(), size.trim().parse().unwrap_or(0)))
        })
        .collect()
}

enum Got {
    All,
    /// A download failed (the medium's lists name versions the mirrors no longer have, or the network went).
    Failed,
    Stopped,
}

impl Ctx<'_> {
    fn pacman(&self, dir: &str, args: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = ["nice", "-n", "19", "ionice", "-c", "3", "pacman", "--noconfirm"].iter().map(|s| s.to_string()).collect();
        for (k, p) in [("--config", "pacman.conf"), ("--dbpath", "db"), ("--cachedir", "pkg"), ("--logfile", "pacman.log")] {
            v.push(k.into());
            v.push(py::join(dir, p));
        }
        v.extend(args.iter().map(|s| s.to_string()));
        v
    }

    /// The private database: the live system's installed list as it is (linked, never written), and its package lists copied
    /// the first time (the medium's, as the install starts from them).
    fn prefetch_db(&mut self, dir: &str) -> R<()> {
        let db = py::join(dir, "db");
        py::makedirs(&py::join(&db, "sync"))?;
        py::makedirs(&py::join(dir, "pkg"))?;
        let local = py::join(&db, "local");
        if !py::exists(&local) {
            py::symlink(&py::join(&self.host.pacman_db, "local"), &local)?;
        }
        let sync = py::join(&self.host.pacman_db, "sync");
        if py::listdir(&py::join(&db, "sync"))?.is_empty() && py::isdir(&sync) {
            for f in py::listdir(&sync)? {
                if f.ends_with(".db") {
                    std::fs::copy(py::join(&sync, &f), py::join(&py::join(&db, "sync"), &f)).map_err(|e| py::oserror(&e, &f))?;
                }
            }
        }
        let conf = py::read_text(&self.host.pacman_conf).unwrap_or_default();
        py::write_text(&py::join(dir, "pacman.conf"), &one_download(&conf))
    }

    /// Room on the live system for `bytes` more.
    fn room_for(&mut self, dir: &str, bytes: i128) -> R<bool> {
        let r = self.sys.run(&["df", "--output=avail", "-B1", dir].map(String::from), None)?;
        let avail: i128 = r.stdout.lines().nth(1).and_then(|l| l.trim().parse().ok()).unwrap_or(0);
        Ok(r.code == 0 && bytes + MARGIN <= avail)
    }

    fn get_each(&mut self, dir: &str, list: &[(String, i128)], stop: &dyn Fn() -> bool) -> R<Got> {
        self.say("PREFETCH", &format!("0 {}", list.len()))?;
        for (i, (name, _)) in list.iter().enumerate() {
            if stop() {
                return Ok(Got::Stopped);
            }
            if self.sys.run(&self.pacman(dir, &["-Swdd", name]), None)?.code != 0 {
                return Ok(Got::Failed);
            }
            self.say("PREFETCH", &format!("{} {}", i + 1, list.len()))?;
        }
        Ok(Got::All)
    }

    /// The whole --prefetch run (see the module's text); the exit code.
    pub fn prefetch(&mut self, choices: &Py, dir: &str, stop: &dyn Fn() -> bool) -> R<i32> {
        if self.sys.euid() != 0 {
            self.say("ERROR", "getting packages ready needs root")?;
            return Ok(1);
        }
        let names = {
            let Ctx { os, sys, .. } = self;
            let mut have = |x: &str| sys.run(&["pacman".to_string(), "-Q".into(), x.into()], None).is_ok_and(|r| r.code == 0);
            wanted(choices, os, &mut have)
        };
        if names.is_empty() {
            self.say("PREFETCH", "0 0")?;
            self.say("DONE", "")?;
            return Ok(0);
        }
        self.prefetch_db(dir)?;
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        // as the install's pacman -S would, with the medium's lists
        let mut args = vec!["-Sp", "--print-format", "%r/%n %s"];
        args.extend(&names);
        let first = self.sys.run(&self.pacman(dir, &args), None)?;
        let mut got = Got::Failed;
        if first.code == 0 {
            let list = listed(&first.stdout);
            if !self.room_for(dir, list.iter().map(|x| x.1).sum())? {
                self.say("WARN", "not enough room on the live system to get the packages ready; the install downloads them")?;
                return Ok(3);
            }
            got = self.get_each(dir, &list, stop)?;
        }
        if matches!(got, Got::Failed) {
            // as its pacman -Syu fallback would: fresh lists (in the private database) and the updates as well
            if stop() {
                got = Got::Stopped;
            } else if self.sys.run(&self.pacman(dir, &["-Sy"]), None)?.code != 0 {
                self.say("WARN", "the package mirrors could not be reached; the install downloads as usual")?;
                return Ok(3);
            } else {
                let mut args = vec!["-Sup", "--print-format", "%r/%n %s"];
                args.extend(&names);
                let r = self.sys.run(&self.pacman(dir, &args), None)?;
                let list = listed(&r.stdout);
                if r.code != 0 {
                    self.say("WARN", "the packages could not be worked out; the install downloads as usual")?;
                    return Ok(3);
                }
                if !self.room_for(dir, list.iter().map(|x| x.1).sum())? {
                    self.say("WARN", "not enough room on the live system to get the packages ready; the install downloads them")?;
                    return Ok(3);
                }
                got = self.get_each(dir, &list, stop)?;
            }
        }
        match got {
            Got::All => {
                self.say("DONE", "")?;
                Ok(0)
            }
            Got::Stopped => {
                self.say("STOPPED", "")?;
                Ok(0)
            }
            Got::Failed => {
                self.say("WARN", "a package could not be downloaded; the install downloads the rest as usual")?;
                Ok(3)
            }
        }
    }

    /// Before the install's own pacman: the packages fetched beforehand go into the new system's package cache, where pacman
    /// takes them (checking each one's signature) instead of downloading them again.
    pub fn use_prefetched(&mut self) -> R<()> {
        let from = py::join(&self.host.prefetch, "pkg");
        if !py::isdir(&from) {
            return Ok(());
        }
        let to = format!("{}/var/cache/pacman/pkg", self.os.target);
        let files: Vec<String> = py::listdir(&from)?.into_iter().filter(|f| f.contains(".pkg.tar.") && !f.ends_with(".part")).collect();
        if files.is_empty() {
            return Ok(());
        }
        py::makedirs(&to)?;
        let mut n = 0;
        for f in &files {
            let src = py::join(&from, f);
            // plain files only: never a link out of the folder
            if std::fs::symlink_metadata(&src).is_ok_and(|m| m.file_type().is_file()) {
                std::fs::copy(&src, py::join(&to, f)).map_err(|e| py::oserror(&e, f))?;
                n += usize::from(!f.ends_with(".sig"));
            }
        }
        self.say(
            "INFO",
            &if n == 1 { "1 package was fetched beforehand".to_string() } else { format!("{n} packages were fetched beforehand") },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_download_at_a_time() {
        assert_eq!(
            one_download("[options]\nParallelDownloads = 5\nSigLevel = Required\n[core]\nInclude = x\n"),
            "[options]\nParallelDownloads = 1\nSigLevel = Required\n[core]\nInclude = x\n"
        );
        assert_eq!(one_download("[options]\n#ParallelDownloads = 5\n[core]\n"), "[options]\nParallelDownloads = 1\n[core]\n");
        assert_eq!(one_download("[options]\nColor\n[core]\n"), "[options]\nColor\nParallelDownloads = 1\n[core]\n");
        assert_eq!(one_download("[options]\nColor"), "[options]\nColor\nParallelDownloads = 1\n");
        assert_eq!(one_download(""), "[options]\nParallelDownloads = 1\n");
        assert_eq!(
            one_download("[core]\nParallelDownloads = 9\n"),
            "[core]\nParallelDownloads = 9\n[options]\nParallelDownloads = 1\n",
            "only in [options]"
        );
    }

    #[test]
    fn pacmans_list_with_sizes() {
        assert_eq!(
            listed("extra/firefox 72000000\ncore/nss 1500000\n\n:: noise\n"),
            [("extra/firefox".to_string(), 72_000_000), ("core/nss".to_string(), 1_500_000)]
        );
    }
}
