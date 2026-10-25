//! The top bar's Authenticator card, drawn by the panel (docs/INTEGRATIONS.md, "Cards you can use"): a search box, the matching
//! accounts with their codes (a click copies one), and what to do when the codes cannot be read. The panel runs `--bar` when the card
//! opens and after each press, and `press ID [TEXT]` for the card's controls. What is typed is kept in the runtime folder only
//! (memory-backed, gone at logout), never on disk; the output holds codes, as the window shows them, never seeds.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use super::{explain, needs_the_app, parse_codes, run_helper, search, spaced, Entry, CLEAR_AFTER, PROTON_APP};

/// Accounts listed at most; typing narrows the rest.
const SHOWN: usize = 8;

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn state_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join("yggdrasil-authenticator"))
}

fn query() -> String {
    state_dir().and_then(|d| std::fs::read_to_string(d.join("query")).ok()).unwrap_or_default()
}

fn set_query(q: &str) {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    use std::io::Write;
    let Some(d) = state_dir() else { return };
    let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&d);
    let q: String = q.chars().filter(|c| !c.is_control()).take(100).collect();
    if let Ok(mut f) = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(d.join("query")) {
        let _ = f.write_all(q.as_bytes());
    }
}

/// A short, stable id for an entry the panel accepts (letters and digits): the helper's own ids can be long and hold other characters.
fn key(id: &str) -> String {
    // FNV-1a, 64 bits
    let mut h: u64 = 0xcbf29ce484222325;
    for b in id.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn codes(demo: bool, prompt: bool) -> Result<Vec<Entry>, String> {
    let mut args = vec!["codes"];
    if prompt {
        args.push("--allow-prompt");
    }
    if demo {
        args.push("--demo");
    }
    parse_codes(&run_helper(&args))
}

/// The bar-item JSON for a helper answer (`codes`) and what was typed (`q`), at `t`.
pub fn build(codes: Result<Vec<Entry>, String>, q: &str, t: u64) -> Value {
    match codes {
        Ok(entries) => {
            let shown = search(&entries, q);
            let mut card = vec![json!({"field": {"id": "search", "hint": "Search accounts", "value": q, "submit": "copy-first"}})];
            if entries.is_empty() {
                card.push(json!({"text": "No accounts in Proton Authenticator yet.", "dim": true}));
            } else if shown.is_empty() {
                card.push(json!({"text": "No account matches.", "dim": true}));
            } else {
                let rows: Vec<Value> = shown
                    .iter()
                    .take(SHOWN)
                    .map(|e| {
                        let (title, who) = if e.issuer.is_empty() { (e.account.as_str(), "") } else { (e.issuer.as_str(), e.account.as_str()) };
                        let next = format!("next in {} s", e.remaining.max(1));
                        let note = if who.is_empty() { next } else { format!("{who} · {next}") };
                        json!({"id": format!("copy:{}", key(&e.id)), "title": title, "note": note, "value": spaced(&e.code), "close": true})
                    })
                    .collect();
                card.push(json!({"list": rows}));
                if shown.len() > SHOWN {
                    card.push(json!({"text": format!("{} more: type to narrow", shown.len() - SHOWN), "dim": true}));
                }
            }
            // the codes roll over together: read again at the next rollover while the card is open
            let next = entries.iter().map(|e| e.remaining.max(1)).min().unwrap_or(30);
            json!({"text": "", "state": "ok", "card": card, "refresh_at": t + next,
                   "help": format!("A click copies the code; the clipboard clears in {CLEAR_AFTER} s.")})
        }
        Err(code) => {
            let why = if code == "HELPER_MISSING" { "The helper (pa-helper) is not installed; reinstall the plugin from Agora." } else { explain(&code) };
            let mut buttons = vec![];
            if needs_the_app(&code) {
                buttons.push(json!({"id": "open-app", "label": "Open Proton Authenticator", "close": true}));
            }
            buttons.push(json!({"id": "retry", "label": "Try again"}));
            let card = vec![json!({"text": why}), json!({"buttons": buttons})];
            // after Try again the keyring may be asking for its password: look again shortly
            json!({"text": "", "state": "ok", "card": card, "refresh_at": t + 3})
        }
    }
}

/// `--bar`: the card as the panel draws it.
pub fn card(demo: bool) -> String {
    build(codes(demo, false), &query(), now()).to_string()
}

/// `press ID [TEXT]`: the card's controls.
pub fn press(id: &str, text: Option<&str>, demo: bool) {
    match id {
        "search" => set_query(text.unwrap_or("")),
        "copy-first" => {
            if let Ok(es) = codes(demo, false) {
                if let Some(e) = search(&es, &query()).first() {
                    copy(&e.id, demo);
                }
            }
        }
        "open-app" => {
            let _ = std::process::Command::new(PROTON_APP).stdin(std::process::Stdio::null()).spawn();
        }
        "retry" => {
            // lets the keyring ask for its password; not waited for (the panel is waiting on this press)
            let mut c = std::process::Command::new(super::helper());
            c.args(["codes", "--allow-prompt"]);
            if demo {
                c.arg("--demo");
            }
            let _ = c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
        }
        _ => {
            if let Some(k) = id.strip_prefix("copy:") {
                if let Ok(es) = codes(demo, false) {
                    if let Some(e) = es.iter().find(|e| key(&e.id) == k) {
                        copy(&e.id, demo);
                    }
                }
            }
        }
    }
}

/// Copies through the helper (marked sensitive, cleared after CLEAR_AFTER seconds), then forgets what was typed.
fn copy(entry_id: &str, demo: bool) {
    let secs = CLEAR_AFTER.to_string();
    let mut args = vec!["copy", entry_id, "--clear-after", secs.as_str()];
    if demo {
        args.push("--demo");
    }
    let _ = run_helper(&args);
    set_query("");
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANSWER: &str = r#"{"ok":true,"protocol":1,"now":1700000000,"entries":[
        {"id":"a1","issuer":"GitHub","account":"ada@example.com","kind":"totp","code":"123456","nextCode":"654321","period":30,"digits":6,"remaining":12},
        {"id":"b/2+=","issuer":"Proton","account":"ada@proton.me","kind":"totp","code":"11112222","period":30,"digits":8,"remaining":7}]}"#;

    #[test]
    fn the_card_lists_codes_and_reads_again_at_the_rollover() {
        let v = build(parse_codes(&ANSWER.replace('\n', "")), "", 1000);
        assert_eq!(v["text"], "");
        assert_eq!(v["refresh_at"], 1007);
        assert_eq!(v["card"][0]["field"]["submit"], "copy-first");
        let rows = v["card"][1]["list"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["value"], "123 456");
        assert_eq!(rows[1]["value"], "1111 2222");
        assert_eq!(rows[0]["note"], "ada@example.com · next in 12 s");
        // ids the panel accepts, whatever the helper's ids hold
        let id = rows[1]["id"].as_str().unwrap();
        assert!(id.starts_with("copy:") && id.chars().all(|c| c.is_ascii_alphanumeric() || c == ':') && id.len() <= 64);
        assert_eq!(id, format!("copy:{}", key("b/2+=")));
        // codes only: no seeds or next codes in what the panel gets
        assert!(!v.to_string().contains("654321"));
    }

    #[test]
    fn typing_narrows_the_list() {
        let v = build(parse_codes(&ANSWER.replace('\n', "")), "proton", 1000);
        assert_eq!(v["card"][0]["field"]["value"], "proton");
        assert_eq!(v["card"][1]["list"].as_array().unwrap().len(), 1);
        let none = build(parse_codes(&ANSWER.replace('\n', "")), "gitlab", 1000);
        assert_eq!(none["card"][1]["text"], "No account matches.");
    }

    #[test]
    fn errors_explain_and_offer_a_retry() {
        let v = build(Err("KEYRING_LOCKED".into()), "", 1000);
        assert!(v["card"][0]["text"].as_str().unwrap().contains("keyring is locked"));
        assert_eq!(v["card"][1]["buttons"][0]["id"], "retry");
        assert_eq!(v["refresh_at"], 1003);
    }
}
