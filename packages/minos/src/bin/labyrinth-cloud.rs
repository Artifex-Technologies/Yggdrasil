//! labyrinth-cloud: link local folders to cloud folders and verify they match.
//!
//!   labyrinth-cloud link NAME LOCAL_DIR REMOTE     e.g. link docs ~/Documents/Github "proton:My files/Documents/Github"
//!   labyrinth-cloud links
//!   labyrinth-cloud verify [NAME]                  exit 0 only if every file was matched by hash
//!   labyrinth-cloud unlink NAME
//!
//! Read-only: verification never changes either side. Needs `rclone` on PATH.

use std::path::PathBuf;
use std::process::ExitCode;

use minos_core::cloud::{self, Link, Problem};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = cloud::links_path() else {
        eprintln!("no config directory on this system");
        return ExitCode::from(2);
    };
    let mut links = cloud::load_links(&path);

    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["link", name, local, remote] => {
            let local = PathBuf::from(local);
            if !local.is_dir() {
                eprintln!("{} is not a folder", local.display());
                return ExitCode::from(2);
            }
            links.retain(|l| l.name != *name);
            links.push(Link { name: name.to_string(), local, remote: remote.to_string() });
            if let Err(e) = cloud::save_links(&path, &links) {
                eprintln!("cannot save links: {e}");
                return ExitCode::from(2);
            }
            println!("linked {name}");
            ExitCode::SUCCESS
        }
        ["unlink", name] => {
            links.retain(|l| l.name != *name);
            match cloud::save_links(&path, &links) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("cannot save links: {e}");
                    ExitCode::from(2)
                }
            }
        }
        ["links"] => {
            for l in &links {
                let state = cloud::last_report(&l.name).map(|r| r.summary()).unwrap_or_else(|| "not verified".into());
                println!("{}\t{} <-> {}\t{}", l.name, l.local.display(), l.remote, state);
            }
            ExitCode::SUCCESS
        }
        ["verify"] | ["verify", _] => {
            let wanted = args.get(1);
            let todo: Vec<&Link> = links.iter().filter(|l| wanted.is_none_or(|w| &l.name == w)).collect();
            if todo.is_empty() {
                eprintln!("no matching link (see `labyrinth-cloud links`)");
                return ExitCode::from(2);
            }
            let mut all_ok = true;
            for l in todo {
                match cloud::verify_link(l) {
                    Ok(r) => {
                        println!("{}: {}", l.name, r.summary());
                        for p in r.problems.iter().take(50) {
                            match p {
                                Problem::MissingLocal(f) => println!("  missing locally:   {f}"),
                                Problem::MissingRemote(f) => println!("  not uploaded yet:  {f}"),
                                Problem::SizeDiffers(f) => println!("  size differs:      {f}"),
                                Problem::HashDiffers(f) => println!("  CONTENT differs:   {f}"),
                            }
                        }
                        if r.problems.len() > 50 {
                            println!("  ... and {} more", r.problems.len() - 50);
                        }
                        all_ok &= r.is_fully_verified();
                    }
                    Err(e) => {
                        eprintln!("{}: {e}", l.name);
                        all_ok = false;
                    }
                }
            }
            if all_ok { ExitCode::SUCCESS } else { ExitCode::from(1) }
        }
        _ => {
            eprintln!("usage: labyrinth-cloud link NAME LOCAL REMOTE | links | verify [NAME] | unlink NAME");
            ExitCode::from(2)
        }
    }
}
