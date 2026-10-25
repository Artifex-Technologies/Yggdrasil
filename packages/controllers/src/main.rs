//! yggdrasil-controllers: game controllers in the top bar.
//!
//!   yggdrasil-controllers daemon      keeps the bar's file up to date, puts saved light bar colours back when a controller connects,
//!                                     and notifies: connected, disconnected, battery low, charged (a user service)
//!   yggdrasil-controllers --bar       prints what the bar shows (the same JSON as the file)
//!   yggdrasil-controllers press ID    a press on the card: rumble:N, disconnect:MAC, light:NAME, settings

use gamepads::{Pad, Settings};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

/// The light bar colours on the card.
const SWATCHES: [(&str, &str, [u8; 3]); 6] = [
    ("purple", "Purple", [124, 84, 214]),
    ("blue", "Blue", [0, 80, 255]),
    ("red", "Red", [255, 24, 24]),
    ("green", "Green", [0, 200, 70]),
    ("white", "White", [255, 255, 255]),
    ("off", "Off", [0, 0, 0]),
];

fn bar_path() -> PathBuf {
    yggdrasil_conf::dirs::state().join("controllers/bar.json")
}

/// A controller's key between runs: its Bluetooth address or serial, else its name and event device.
fn key(p: &Pad) -> String {
    if p.uniq.is_empty() { format!("{}@{}", p.name, p.event.as_ref().map(|e| e.display().to_string()).unwrap_or_default()) } else { p.uniq.clone() }
}

/// What the top bar shows: the lowest battery, a line per controller, and the card's controls. No controller: no pill.
fn bar(pads: &[Pad], s: &Settings) -> Value {
    if pads.is_empty() {
        return json!({"state": "off", "text": ""});
    }
    let lowest = pads.iter().filter_map(|p| p.battery.as_ref()).filter(|b| !b.charging() && !b.full()).filter_map(|b| b.estimate()).min();
    let any_battery = pads.iter().filter_map(|p| p.battery.as_ref()).filter_map(|b| b.estimate()).min();
    let charging = pads.iter().filter_map(|p| p.battery.as_ref()).any(|b| b.charging());
    let text = match (lowest, any_battery) {
        (Some(p), _) => format!("{p}%"),
        (None, Some(p)) if charging => format!("{p}% · charging"),
        (None, Some(p)) => format!("{p}%"),
        _ => String::new(),
    };
    let state = match lowest {
        Some(p) if p <= 10 => "alert",
        Some(p) if p <= s.low_at => "warn",
        _ => "ok",
    };
    let lines: Vec<Value> = pads
        .iter()
        .map(|p| {
            let mut l = json!({"title": p.title(), "note": format!("{} · {}", p.connection.label(), p.name)});
            match &p.battery {
                Some(b) => {
                    l["value"] = json!(b.describe());
                    if let Some(e) = b.estimate() {
                        l["percent"] = json!(e);
                    }
                    l["alert"] = json!(!b.charging() && b.estimate().is_some_and(|e| e <= s.low_at));
                }
                None => l["value"] = json!("No battery reported"),
            }
            l
        })
        .collect();
    let mut card: Vec<Value> = vec![];
    for (i, p) in pads.iter().enumerate() {
        let rumble = if pads.len() > 1 { format!("Rumble {}", i + 1) } else { "Rumble test".into() };
        let mut row = vec![json!({"id": format!("rumble:{i}"), "label": rumble})];
        if let Some(mac) = p.mac() {
            row.push(json!({"id": format!("disconnect:{mac}"), "label": "Disconnect"}));
        }
        card.push(json!({"buttons": row}));
    }
    if pads.iter().any(|p| p.leds.has_lightbar() && p.leds.writable) {
        let current = s.lightbar.clone().unwrap_or_default();
        card.push(json!({"section": "Light bar"}));
        let swatches: Vec<Value> = SWATCHES.iter().map(|(id, label, c)| json!({"id": format!("light:{id}"), "label": label, "accent": current == gamepads::color_hex(*c)})).collect();
        card.push(json!({"buttons": swatches, "columns": 3}));
    } else if pads.iter().any(|p| p.leds.has_lightbar()) {
        card.push(json!({"text": "The light bar can be set once Labyrinth's controller rule is installed (reinstall the plugin).", "dim": true}));
    }
    card.push(json!({"buttons": [{"id": "settings", "label": "Bluetooth settings", "close": true}]}));
    json!({"text": text, "state": state, "lines": lines, "card": card})
}

/// Writes the bar's file when it changed (the panel reads it within a second).
fn write_bar(v: &Value) {
    let p = bar_path();
    let text = v.to_string();
    if std::fs::read_to_string(&p).ok().as_deref() == Some(text.as_str()) {
        return;
    }
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, &text).is_ok() {
        let _ = std::fs::rename(tmp, p);
    }
}

fn notify(title: &str, body: &str) {
    let _ = Command::new("notify-send").args(["-a", "Controllers", "-i", "input-gaming", title, body]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map(|mut c| {
        std::thread::spawn(move || c.wait());
    });
}

fn press(id: &str) -> Result<(), String> {
    let pads = gamepads::list();
    let mut s = Settings::load();
    if let Some(n) = id.strip_prefix("rumble:") {
        let pad = n.parse::<usize>().ok().and_then(|i| pads.get(i)).ok_or("no such controller")?;
        gamepads::rumble(pad, 600)?;
    } else if let Some(mac) = id.strip_prefix("disconnect:") {
        if !pads.iter().any(|p| p.mac() == Some(mac)) {
            return Err("no such controller".into());
        }
        gamepads::disconnect(mac)?;
    } else if let Some(name) = id.strip_prefix("light:") {
        let (_, _, c) = SWATCHES.iter().find(|(n, _, _)| *n == name).ok_or("no such colour")?;
        s.lightbar = Some(gamepads::color_hex(*c));
        // one colour for all: a controller's own colour gives way
        for p in s.per_pad.values_mut() {
            p.lightbar = None;
        }
        s.save().map_err(|e| e.to_string())?;
        for p in &pads {
            let _ = gamepads::set_lightbar(p, *c);
        }
    } else if id == "settings" {
        Command::new("yggdrasil-settings").arg("bluetooth").stdin(Stdio::null()).spawn().map_err(|e| e.to_string())?;
    } else {
        return Err(format!("unknown press {id:?}"));
    }
    write_bar(&bar(&gamepads::list(), &s));
    Ok(())
}

/// What the daemon remembers about a controller between looks.
#[derive(Default)]
struct Seen {
    title: String,
    warned_low: bool,
    told_full: bool,
}

fn daemon() -> ExitCode {
    let mut seen: BTreeMap<String, Seen> = BTreeMap::new();
    let mut first = true;
    loop {
        let s = Settings::load();
        let pads = gamepads::list();
        for p in &pads {
            let k = key(p);
            let battery = p.battery.as_ref();
            if !seen.contains_key(&k) {
                // a controller came: the kernel reset its lights, so the saved ones go back on
                s.apply(p);
                if s.notify && !first {
                    let b = battery.map(|b| format!(" · {}", b.describe())).unwrap_or_default();
                    notify(&format!("{} connected", p.title()), &format!("{}{b}", p.connection.label()));
                }
                seen.insert(k.clone(), Seen { title: p.title(), ..Default::default() });
            }
            let e = seen.get_mut(&k).unwrap();
            if let Some(b) = battery {
                let low = !b.charging() && !b.full() && b.estimate().is_some_and(|x| x <= s.low_at);
                if low && !e.warned_low && s.notify {
                    notify(&format!("{} battery low", p.title()), &format!("{} left. Plug it in to charge.", b.describe()));
                }
                e.warned_low = low || (e.warned_low && !b.charging());
                let full = b.full() || (b.charging() && b.percent == Some(100));
                if full && !e.told_full && s.notify && !first {
                    notify(&format!("{} charged", p.title()), "The battery is full.");
                }
                e.told_full = full || (e.told_full && b.charging());
            }
        }
        let gone: Vec<String> = seen.keys().filter(|k| !pads.iter().any(|p| &key(p) == *k)).cloned().collect();
        for k in gone {
            if let Some(e) = seen.remove(&k) {
                if s.notify {
                    notify(&format!("{} disconnected", e.title), "");
                }
            }
        }
        write_bar(&bar(&pads, &s));
        first = false;
        // sysfs reads only; a little more often while a controller is connected (its battery, its lights)
        std::thread::sleep(Duration::from_secs(if pads.is_empty() { 10 } else { 5 }));
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--bar") => {
            println!("{}", bar(&gamepads::list(), &Settings::load()));
            ExitCode::SUCCESS
        }
        Some("press") if args.len() == 2 => match press(&args[1]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("yggdrasil-controllers: {e}");
                ExitCode::from(1)
            }
        },
        Some("daemon") => daemon(),
        _ => {
            eprintln!("usage: yggdrasil-controllers daemon | --bar | press rumble:N|disconnect:MAC|light:NAME|settings");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gamepads::{Battery, Connection, Leds, Model};

    fn pad(pct: Option<u8>, status: &str, bt: bool) -> Pad {
        Pad {
            name: "Wireless Controller".into(),
            model: Model::DualShock4,
            connection: if bt { Connection::Bluetooth } else { Connection::Usb },
            uniq: if bt { "a4:ae:12:00:11:22".into() } else { String::new() },
            vendor: 0x054c,
            product: 0x09cc,
            driver: "playstation".into(),
            event: None,
            battery: pct.map(|p| Battery { percent: Some(p), level: None, status: status.into() }),
            leds: Leds { lightbar: Some("/x".into()), rgb: None, players: vec![], writable: true },
        }
    }

    #[test]
    fn no_controller_no_pill() {
        assert_eq!(bar(&[], &Settings::default())["state"], "off");
    }

    #[test]
    fn the_pill_shows_the_lowest_battery_and_warns() {
        let s = Settings::default();
        let v = bar(&[pad(Some(80), "Discharging", true), pad(Some(15), "Discharging", false)], &s);
        assert_eq!((v["text"].as_str(), v["state"].as_str()), (Some("15%"), Some("warn")));
        assert_eq!(bar(&[pad(Some(8), "Discharging", true)], &s)["state"], "alert");
        let c = bar(&[pad(Some(40), "Charging", true)], &s);
        assert_eq!((c["text"].as_str(), c["state"].as_str()), (Some("40% · charging"), Some("ok")));
        assert_eq!(c["lines"][0]["value"], "Charging 40%");
        assert_eq!(c["lines"][0]["title"], "DualShock 4 (PS4)");
    }

    #[test]
    fn the_card_offers_what_each_controller_can_do() {
        let mut s = Settings::default();
        s.lightbar = Some("#7c54d6".into());
        let v = bar(&[pad(Some(80), "Discharging", true), pad(None, "", false)], &s);
        let card = v["card"].as_array().unwrap();
        assert_eq!(card[0]["buttons"][1]["id"], "disconnect:a4:ae:12:00:11:22");
        assert_eq!(card[1]["buttons"].as_array().unwrap().len(), 1, "no disconnect over USB");
        assert_eq!(card[1]["buttons"][0]["label"], "Rumble 2");
        assert_eq!(card[2]["section"], "Light bar");
        assert_eq!(card[3]["buttons"][0]["accent"], true, "the colour in use is marked");
        assert_eq!(card.last().unwrap()["buttons"][0]["id"], "settings");
        assert_eq!(v["lines"][1]["value"], "No battery reported");
    }
}
