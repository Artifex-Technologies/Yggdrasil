//! labyrinth-appusage: how long each app has been running, for Settings > Apps ("sort by usage").
//!
//!   labyrinth-appusage            run until stopped (the user service labyrinth-appusage.service does)
//!   labyrinth-appusage --once     one sample, as the Python script took it: 30 seconds for every app with a running scope
//!                                 (asked of systemctl), then exit (for tests)
//!   labyrinth-appusage --scopes   the scopes the service sees in the cgroup tree, each with its app, then exit (for checks)
//!
//! Any other arguments are ignored, as the script ignored them.

use std::collections::BTreeSet;

use labyrinth_appusage::{desktop_ids, match_unit, running_scopes, sample, usage_path, watch, INTERVAL};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--once") {
        match sample(INTERVAL as i64, &desktop_ids(), &running_scopes(), &usage_path()) {
            Ok(_) => std::process::exit(0),
            Err(e) => {
                eprintln!("labyrinth-appusage: {e}");
                std::process::exit(1);
            }
        }
    }
    if args.iter().any(|a| a == "--scopes") {
        let Some(root) = watch::cgroup_root() else {
            eprintln!("labyrinth-appusage: no cgroup tree of this user's systemd");
            std::process::exit(1);
        };
        let ids: BTreeSet<String> = desktop_ids();
        for s in watch::scan(&root, |_| {}) {
            println!("{s}\t{}", match_unit(&s, &ids).unwrap_or_default());
        }
        std::process::exit(0);
    }
    std::process::exit(watch::run());
}
