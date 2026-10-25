use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use yggdrasil_weather::card;
use yggdrasil_weather::config::{self, Config};
use yggdrasil_weather::consensus::{self, Hour};
use yggdrasil_weather::locate;
use yggdrasil_weather::nowcast::{self, WatchState};
use yggdrasil_weather::observe::{self, Station};
use yggdrasil_weather::openmeteo::{self, Forecast};
use yggdrasil_weather::report::{self, BarInput};
use yggdrasil_weather::timeutil::{format_local, now_epoch, parse_local};
use yggdrasil_weather::verify::{self, Obs, Record};

const UA: &str = "yggdrasil-weather/0.1";
const MIN_SCORED: usize = 24;
const MAX_STATION_KM: f64 = 60.0;

fn http_get(url: &str) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(12))).build().into();
    let mut resp = agent.get(url).header("User-Agent", UA).call().map_err(|e| e.to_string())?;
    resp.body_mut().read_to_string().map_err(|e| e.to_string())
}

/// `url`'s answer, from the cache when it is younger than `max_age` seconds (0: always asked again). Every answer is cached; when
/// the service cannot be reached, an answer up to six hours old is used instead. The card asks again after each press, so it reads
/// from here and stays quick.
fn get_cached(url: &str, max_age: u64) -> Result<String, String> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    let path = yggdrasil_conf::dirs::cache().join("weather").join(format!("{:016x}.txt", h.finish()));
    let age = std::fs::metadata(&path).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).map(|d| d.as_secs());
    if max_age > 0 && age.is_some_and(|a| a < max_age) {
        if let Ok(t) = std::fs::read_to_string(&path) {
            return Ok(t);
        }
    }
    match http_get(url) {
        Ok(t) => {
            let _ = std::fs::create_dir_all(path.parent().unwrap());
            let tmp = path.with_extension("tmp");
            if std::fs::write(&tmp, &t).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
            Ok(t)
        }
        Err(e) if age.is_some_and(|a| a < 6 * 3600) => std::fs::read_to_string(&path).map_err(|_| e),
        Err(e) => Err(e),
    }
}

/// How long the card and the bar reuse an answer.
const PANEL_CACHE_S: u64 = 600;

type Ensemble = (Vec<String>, Vec<Vec<Option<f64>>>);

struct Fetched {
    forecast: Forecast,
    ensemble: Option<Ensemble>,
    station: Option<(Station, f64)>,
}

fn fetch(cfg: &Config, max_age: u64) -> Result<Fetched, String> {
    let forecast = openmeteo::parse_multi(&get_cached(&openmeteo::forecast_url(cfg.lat, cfg.lon, openmeteo::DEFAULT_MODELS, 3), max_age)?)?;
    // The ensemble and the measured station are extras: their failure must not break the bar.
    let ensemble = get_cached(&openmeteo::ensemble_url(cfg.lat, cfg.lon, 3), max_age).ok().and_then(|t| openmeteo::parse_ensemble(&t).ok());
    let station = get_cached(&observe::metar_url(cfg.lat, cfg.lon), max_age)
        .ok()
        .and_then(|t| observe::parse_metar(&t).ok())
        .and_then(|s| observe::nearest(&s, cfg.lat, cfg.lon, MAX_STATION_KM));
    Ok(Fetched { forecast, ensemble, station })
}

fn history_paths() -> Option<(PathBuf, PathBuf)> {
    let d = config::data_dir()?;
    Some((d.join("records.jsonl"), d.join("observations.jsonl")))
}

fn learned_weights() -> HashMap<String, f64> {
    let Some((rp, op)) = history_paths() else { return HashMap::new() };
    let (records, obs): (Vec<Record>, Vec<Obs>) = (verify::read_jsonl(&rp), verify::read_jsonl(&op));
    verify::weights(&verify::score(&records, &obs, 1800), MIN_SCORED)
}

/// The place to use: the saved one, or the time zone's main city (when nothing is saved, or that mode is chosen). The time
/// zone place is worked out each time, offline, and never written.
fn load_config() -> Result<Config, String> {
    let path = config::config_path().ok_or("no config directory on this system")?;
    let saved = config::load(&path);
    if let Some(c) = saved.as_ref().filter(|c| c.mode != "timezone") {
        return Ok(c.clone());
    }
    let f = locate::from_timezone().map_err(|e| format!("no place: {e}; choose one in Weather's settings"))?;
    // the time zone's place, with what else was saved (units, the last chosen place)
    let base = saved.unwrap_or(Config { name: String::new(), lat: 0.0, lon: 0.0, units: "metric".into(), mode: "timezone".into(), detected_at: 0, chosen: None });
    Ok(Config { name: f.name, lat: f.lat, lon: f.lon, mode: "timezone".into(), detected_at: 0, ..base })
}

/// Finds the place from nearby Wi-Fi (beaconDB) and saves it as the automatic place, keeping the rest of `base`.
fn detect_and_save(base: Config) -> Result<Config, String> {
    let path = config::config_path().ok_or("no config directory")?;
    let f = locate::from_wifi()?;
    let c = Config { name: f.name, lat: f.lat, lon: f.lon, mode: "auto".into(), detected_at: now_epoch(), ..base };
    config::save(&path, &c).map_err(|e| e.to_string())?;
    Ok(c)
}

/// `yggdrasil-weather locate`: switch to the automatic place and find it now.
fn cmd_locate() -> ExitCode {
    let base = load_config().unwrap_or(Config { name: String::new(), lat: 0.0, lon: 0.0, units: "metric".into(), mode: "auto".into(), detected_at: 0, chosen: None });
    match detect_and_save(base) {
        Ok(c) => {
            println!("found {} ({:.2}, {:.2})", c.name, c.lat, c.lon);
            ExitCode::SUCCESS
        }
        Err(e) => die(&e),
    }
}

fn hours_for(f: &Fetched, weights: &HashMap<String, f64>) -> Vec<Hour> {
    let ens = f.ensemble.as_ref().map(|(t, m)| (t.as_slice(), m.as_slice()));
    consensus::consensus(&f.forecast, ens, weights)
}

fn cmd_bar() -> ExitCode {
    // The bar must always print one valid JSON line, even when something is wrong.
    let fail = |msg: &str| {
        println!("{}", serde_json::json!({"text": "", "tooltip": msg}));
        ExitCode::SUCCESS
    };
    let cfg = match load_config() {
        Ok(c) => c,
        Err(e) => return fail(&e),
    };
    let fetched = match fetch(&cfg, PANEL_CACHE_S) {
        Ok(f) => f,
        Err(e) => return fail(&format!("weather unavailable: {e}")),
    };
    let weights = learned_weights();
    let hours = hours_for(&fetched, &weights);
    let input = BarInput {
        hours: &hours,
        utc_offset_s: fetched.forecast.utc_offset_s,
        units: &cfg.units,
        place: &cfg.name,
        how: cfg.how(),
        observed: fetched.station.as_ref().map(|(s, d)| (s, *d)),
        weighted: !weights.is_empty(),
    };
    println!("{}", report::bar_json(&input));
    ExitCode::SUCCESS
}

/// The rain outlook from now, or None when the nowcast could not be fetched.
fn fetch_nowcast(cfg: &Config, max_age: u64) -> Option<nowcast::Outlook> {
    let steps = nowcast::parse(&get_cached(&nowcast::url(cfg.lat, cfg.lon), max_age).ok()?).ok()?;
    nowcast::outlook(&steps, now_epoch())
}

/// The Yggdrasil panel's bar add-on: always one JSON object, with its card (the weather, or the place and units).
fn cmd_panel() -> ExitCode {
    let st = card::load_state(now_epoch());
    let cfg = match load_config() {
        Ok(c) => c,
        Err(_) => {
            println!("{}", serde_json::json!({"text": "Set weather place", "state": "warn", "icon": "weather-few-clouds-symbolic",
                "help": "Choose your place with Settings below: found automatically, or a city you pick.", "card": card::blocks(None, &st)}));
            return ExitCode::SUCCESS;
        }
    };
    let fetched = fetch(&cfg, PANEL_CACHE_S).ok();
    let weights = learned_weights();
    let hours = fetched.as_ref().map(|f| hours_for(f, &weights)).unwrap_or_default();
    let input = fetched.as_ref().map(|f| BarInput {
        hours: &hours,
        utc_offset_s: f.forecast.utc_offset_s,
        units: &cfg.units,
        place: &cfg.name,
        how: cfg.how(),
        observed: f.station.as_ref().map(|(s, d)| (s, *d)),
        weighted: !weights.is_empty(),
    });
    let outlook = fetch_nowcast(&cfg, PANEL_CACHE_S);
    let sky = get_cached(&openmeteo::current_url(cfg.lat, cfg.lon), PANEL_CACHE_S).ok().and_then(|t| openmeteo::parse_current(&t));
    let mut v = report::panel_json(input.as_ref(), outlook.as_ref(), sky, now_epoch());
    if st.view == "settings" {
        // the settings take the card's room: the weather stays in the pill
        v["lines"] = serde_json::json!([]);
        v.as_object_mut().map(|o| o.remove("help"));
    }
    v["card"] = card::blocks(Some(&cfg), &st);
    println!("{v}");
    ExitCode::SUCCESS
}

/// `yggdrasil-weather press ID [TEXT]`: a press on the card (the panel runs it, then `panel` again).
fn cmd_press(args: &[String]) -> ExitCode {
    let Some(id) = args.first().filter(|i| i.len() <= 64 && i.chars().all(|c| c.is_ascii_alphanumeric() || "._:-".contains(c))) else {
        return die("usage: press ID [TEXT]");
    };
    let now = now_epoch();
    let mut st = card::load_state(now);
    let current = load_config().ok();
    let changed = card::press(id, args.get(1).map(String::as_str), &mut st, current, now, locate::from_timezone, locate::from_wifi, locate::search_places);
    if let Some(c) = changed {
        match config::config_path().map(|p| config::save(&p, &c)) {
            Some(Ok(())) => {}
            Some(Err(e)) => st.status = format!("Could not save: {e}"),
            None => st.status = "No config folder on this system.".into(),
        }
    }
    card::save_state(&st);
    ExitCode::SUCCESS
}

/// Run every 10 minutes (a systemd user timer): one notification when rain is about to start.
fn cmd_watch() -> ExitCode {
    let mut cfg = match load_config() {
        Ok(c) => c,
        Err(e) => return die(&e),
    };
    // the automatic place is found again every few hours (a laptop travels); if that fails the last place stays
    if cfg.mode == "auto" && now_epoch() - cfg.detected_at > 3 * 3600 {
        match detect_and_save(cfg.clone()) {
            Ok(c) => cfg = c,
            Err(e) => eprintln!("yggdrasil-weather: keeping {}: {e}", cfg.name),
        }
    }
    let Some(o) = fetch_nowcast(&cfg, 300) else { return die("no nowcast (offline, or Open-Meteo did not answer)") };
    let Some(path) = config::data_dir().map(|d| d.join("nowcast-state.json")) else { return die("no data directory") };
    let mut st: WatchState = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let before = st.clone();
    if let Some((title, body)) = nowcast::decide(&o, now_epoch(), &mut st) {
        // no -A: notify-send waits for an answer when it offers actions
        let sent = std::process::Command::new("notify-send")
            .args(["-a", "Weather", "-i", "weather-showers-symbolic", &title, &body])
            .stdin(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !sent {
            return die("notify-send failed");
        }
        println!("{title}: {body}");
    }
    if st != before {
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        if let Err(e) = std::fs::write(&path, serde_json::to_string(&st).unwrap()) {
            return die(&e.to_string());
        }
    }
    ExitCode::SUCCESS
}

fn cmd_forecast(n_hours: usize) -> ExitCode {
    let cfg = match load_config() {
        Ok(c) => c,
        Err(e) => return die(&e),
    };
    let f = match fetch(&cfg, PANEL_CACHE_S) {
        Ok(f) => f,
        Err(e) => return die(&e),
    };
    let weights = learned_weights();
    let hours = hours_for(&f, &weights);
    let Some(start) = report::current_index(&hours, f.forecast.utc_offset_s, now_epoch()) else { return die("no forecast data") };
    println!("{}  ({})", cfg.name, if weights.is_empty() { "models weighted equally" } else { "weighted by measured accuracy" });
    println!("{:<17} {:>6} {:>13} {:>9} {:>6}  agreement", "time", "temp", "models", "ens +/-", "rain");
    for h in hours.iter().skip(start).take(n_hours).step_by(3) {
        let u = |c: f64| format!("{:.0}", report::to_units(c, &cfg.units));
        let half = |a: f64, b: f64| format!("{:.0}", (report::to_units(b, &cfg.units) - report::to_units(a, &cfg.units)) / 2.0);
        let ens = h.ensemble.map(|(a, _, b)| half(a, b)).unwrap_or_else(|| "-".into());
        println!(
            "{:<17} {:>6} {:>13} {:>9} {:>5}%  {}",
            h.time.replace('T', " "),
            u(h.mean),
            format!("{} to {}", u(h.low), u(h.high)),
            ens,
            h.precip_prob.map(|p| format!("{p:.0}")).unwrap_or_else(|| "-".into()),
            h.confidence.label()
        );
    }
    if let Some((s, km)) = &f.station {
        println!("measured at {} ({:.0} km away): {:.1} deg C", s.id, km, s.temp_c);
    } else {
        println!("no weather station within {MAX_STATION_KM:.0} km for measured readings");
    }
    ExitCode::SUCCESS
}

fn cmd_set_location(args: &[String]) -> ExitCode {
    let Some(path) = config::config_path() else { return die("no config directory") };
    let base = load_config().unwrap_or(Config { name: String::new(), lat: 0.0, lon: 0.0, units: "metric".into(), mode: "manual".into(), detected_at: 0, chosen: None });
    let cfg = if args.first().map(String::as_str) == Some("--coords") {
        let (Some(lat), Some(lon)) = (args.get(1).and_then(|s| s.parse().ok()), args.get(2).and_then(|s| s.parse().ok())) else {
            return die("usage: set-location --coords LAT LON [NAME]");
        };
        let name = if args.len() > 3 { args[3..].join(" ") } else { format!("{lat:.2}, {lon:.2}") };
        Config { name: name.clone(), lat, lon, mode: "manual".into(), detected_at: 0, chosen: Some(config::Chosen { name, lat, lon }), ..base }
    } else {
        let query = args.join(" ");
        if query.trim().is_empty() {
            return die("usage: set-location CITY  |  set-location --coords LAT LON [NAME]");
        }
        let places = match http_get(&openmeteo::geocode_url(&query)).and_then(|t| openmeteo::parse_places(&t)) {
            Ok(p) => p,
            Err(e) => return die(&e),
        };
        let Some(p) = places.first() else { return die(&format!("no place found for {query:?}")) };
        for (i, other) in places.iter().enumerate().skip(1) {
            eprintln!("(other match {i}: {} at {:.3},{:.3}; use --coords to pick it)", other.label(), other.latitude, other.longitude);
        }
        Config { name: p.label(), lat: p.latitude, lon: p.longitude, mode: "manual".into(), detected_at: 0, chosen: Some(config::Chosen { name: p.label(), lat: p.latitude, lon: p.longitude }), ..base }
    };
    match config::save(&path, &cfg) {
        Ok(()) => {
            println!("location set to {} ({:.3}, {:.3})", cfg.name, cfg.lat, cfg.lon);
            ExitCode::SUCCESS
        }
        Err(e) => die(&e.to_string()),
    }
}

fn cmd_units(u: &str) -> ExitCode {
    let (Some(path), Ok(mut c)) = (config::config_path(), load_config()) else { return die("set a location first") };
    if u != "metric" && u != "us" {
        return die("units must be metric or us");
    }
    c.units = u.into();
    match config::save(&path, &c) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => die(&e.to_string()),
    }
}

/// Run hourly (a systemd timer): store what each model predicts for the next 24 hours and
/// what the nearest station actually measured. Scoring needs days of this.
fn cmd_record() -> ExitCode {
    let cfg = match load_config() {
        Ok(c) => c,
        Err(e) => return die(&e),
    };
    let Some((rp, op)) = history_paths() else { return die("no data directory") };
    let f = match fetch(&cfg, 0) {
        Ok(f) => f,
        Err(e) => return die(&e),
    };
    let now = now_epoch();
    let off = f.forecast.utc_offset_s;
    let mut records = vec![];
    for m in &f.forecast.models {
        for (i, t) in f.forecast.times.iter().enumerate() {
            let (Some(target), Some(Some(p))) = (parse_local(t, off), m.temp_c.get(i)) else { continue };
            if target > now && target <= now + 24 * 3600 {
                records.push(Record { source: m.source.clone(), issued: now, target, predicted_c: *p });
            }
        }
    }
    if let Err(e) = verify::append_jsonl(&rp, &records) {
        return die(&e.to_string());
    }
    let mut stored_obs = false;
    if let Some((s, _)) = &f.station {
        let known: Vec<Obs> = verify::read_jsonl(&op);
        if !known.iter().any(|o| o.epoch == s.obs_epoch && o.station == s.id) {
            let _ = verify::append_jsonl(&op, &[Obs { epoch: s.obs_epoch, temp_c: s.temp_c, station: s.id.clone() }]);
            stored_obs = true;
        }
    }
    prune(&rp, now - 30 * 86_400);
    println!("recorded {} predictions{}", records.len(), if stored_obs { " and 1 measurement" } else { "" });
    ExitCode::SUCCESS
}

/// Keep the history file bounded: drop predictions whose target is older than `cutoff`.
fn prune(path: &std::path::Path, cutoff: i64) {
    if std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) < 4_000_000 {
        return;
    }
    let keep: Vec<Record> = verify::read_jsonl::<Record>(path).into_iter().filter(|r| r.target >= cutoff).collect();
    let tmp = path.with_extension("jsonl.tmp");
    let _ = std::fs::remove_file(&tmp);
    if verify::append_jsonl(&tmp, &keep).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

fn cmd_score() -> ExitCode {
    let Some((rp, op)) = history_paths() else { return die("no data directory") };
    let (records, obs): (Vec<Record>, Vec<Obs>) = (verify::read_jsonl(&rp), verify::read_jsonl(&op));
    let scores = verify::score(&records, &obs, 1800);
    let weights = verify::weights(&scores, MIN_SCORED);
    println!("{} predictions and {} measurements on file", records.len(), obs.len());
    if scores.is_empty() {
        println!("nothing scored yet: needs predictions whose time has passed and a measurement near that time.\nRun `yggdrasil-weather record` hourly (the systemd timer does this).");
        return ExitCode::SUCCESS;
    }
    println!("{:<16} {:>6} {:>9} {:>9} {:>8}", "source", "n", "MAE (C)", "bias (C)", "weight");
    let mut rows: Vec<_> = scores.iter().collect();
    rows.sort_by(|a, b| a.1.mae.partial_cmp(&b.1.mae).unwrap());
    for (k, s) in rows {
        let w = weights.get(k).map(|w| format!("{w:.2}")).unwrap_or_else(|| format!("1.00 (<{MIN_SCORED} samples)"));
        println!("{:<16} {:>6} {:>9.2} {:>+9.2} {:>8}", k, s.n, s.mae, s.bias, w);
    }
    ExitCode::SUCCESS
}

fn die(msg: &str) -> ExitCode {
    eprintln!("yggdrasil-weather: {msg}");
    ExitCode::from(1)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("bar") => cmd_bar(),
        Some("panel") => cmd_panel(),
        Some("press") => cmd_press(&args[1..]),
        Some("watch") => cmd_watch(),
        Some("forecast") => cmd_forecast(args.get(1).and_then(|s| s.parse().ok()).unwrap_or(24)),
        Some("set-location") => cmd_set_location(&args[1..]),
        Some("units") if args.len() == 2 => cmd_units(&args[1]),
        Some("record") => cmd_record(),
        Some("locate") => cmd_locate(),
        Some("score") => cmd_score(),
        _ => {
            eprintln!("usage: yggdrasil-weather bar | panel | press ID [TEXT] | watch | forecast [HOURS] | set-location CITY | set-location --coords LAT LON [NAME] | units metric|us | locate | record | score");
            ExitCode::from(2)
        }
    }
}

#[allow(dead_code)]
fn _keep(_: &str) -> String {
    format_local(0, 0)
}
