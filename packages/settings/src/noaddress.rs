//! A Wi-Fi network that joins but gives out no address (YGG-76): NetworkManager then reports "IP configuration could not be
//! reserved" (the device's ip-config-unavailable) or, as the join ends, "The base network connection was interrupted", and
//! tries again by itself. A phone's hotspot does it when its mobile signal is weak. Settings counts such joins in a row for
//! each network: the first says the network gave no address, the second says so plainly with what to check.

use std::collections::HashMap;
use std::sync::Mutex;

static FAILS: Mutex<Option<HashMap<String, u32>>> = Mutex::new(None);

/// Whether NetworkManager's error is a join that got no address.
pub fn no_address(err: &str) -> bool {
    let e = err.to_lowercase();
    e.contains("ip configuration could not be reserved")
        || e.contains("base network connection was interrupted")
        || e.contains("ip-config-unavailable")
}

/// What to say when joining `name` failed with `err`, the `n`th time in a row with no address.
pub fn message(name: &str, err: &str, n: u32) -> String {
    if !no_address(err) {
        return err.to_string();
    }
    if n >= 2 {
        format!(
            "Joined “{name}”, but it is not giving out an address. If it is a phone's hotspot, its mobile signal may be too \
             weak: check the phone's signal (or move it nearer a window), then try again."
        )
    } else {
        format!("“{name}” joined but gave no address this time; trying again may work.")
    }
}

/// Runs a join of `name` (`f`: the nmcli call) and explains its failure, counting the joins in a row that got no address
/// (a join that works, or fails another way, starts the count again).
pub fn join(name: &str, f: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    let r = f();
    let mut g = FAILS.lock().unwrap_or_else(|e| e.into_inner());
    let fails = g.get_or_insert_with(HashMap::new);
    match r {
        Err(e) if no_address(&e) => {
            let n = fails.entry(name.to_string()).or_default();
            *n += 1;
            Err(message(name, &e, *n))
        }
        other => {
            fails.remove(name);
            other
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NM: &str = "Error: Connection activation failed: IP configuration could not be reserved (no available address, timeout, etc.).";

    #[test]
    fn the_second_join_with_no_address_says_what_to_check() {
        let bad = || Err::<(), String>(NM.to_string());
        let first = join("Phone", bad).unwrap_err();
        assert!(first.contains("gave no address this time"), "{first}");
        let second = join("Phone", bad).unwrap_err();
        assert!(second.starts_with("Joined “Phone”, but it is not giving out an address") && second.contains("mobile signal"), "{second}");
        // another network counts on its own; a join that works starts again
        assert!(join("Cafe", bad).unwrap_err().contains("this time"));
        assert!(join("Phone", || Ok(())).is_ok());
        assert!(join("Phone", bad).unwrap_err().contains("this time"));
        // another error passes through unchanged, and resets the count
        assert_eq!(
            join("Phone", || Err("Error: Secrets were required, but not provided.".into())).unwrap_err(),
            "Error: Secrets were required, but not provided."
        );
        assert!(join("Phone", bad).unwrap_err().contains("this time"));
        assert!(no_address("Error: Connection activation failed: The base network connection was interrupted."));
        assert!(!no_address("Error: No network with SSID 'x' found."));
    }
}
