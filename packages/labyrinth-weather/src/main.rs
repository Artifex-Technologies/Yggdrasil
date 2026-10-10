use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use labyrinth_weather::config::{self, Config};
use labyrinth_weather::consensus::{self, Hour};
use labyrinth_weather::observe::{self, Station};
use labyrinth_weather::openmeteo::{self, Forecast};
use labyrinth_weather::report::{self, BarInput};
use labyrinth_weather::timeutil::{format_local, now_epoch, parse_local};
use labyrinth_weather::verify::{self, Obs, Record};

const UA: &str = "labyrinth-weather/0.1 (Labyrinth OS desktop widget)";
const MIN_SCORED: usize = 24;
const MAX_STATION_KM: f64 = 60.0;

fn http_get(url: &str) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(12))).build().into();
    let mut resp = agent.get(url).header("User-Agent", UA).call().map_err(|e| e.to_string())?;
    resp.body_mut().read_to_string().map_err(|e| e.to_string())
}

type Ensemble = (Vec<String>, Vec<Vec<Option<f64>>>);

struct Fetched {
    forecast: Forecast,
    ensemble: Option<Ensemble>,
    station: Option<(Station, f64)>,
}

fn fetch(cfg: &Config) -> Result<Fetched, String> {
    let forecast = openmeteo::parse_multi(&http_get(&openmeteo::forecast_url(cfg.lat, cfg.lon, openmeteo::DEFAULT_MODELS, 3))?)?;
    // The ensemble and the measured station are extras: their failure must not break the bar.
    let ensemble = http_get(&openmeteo::ensemble_url(cfg.lat, cfg.lon, 3)).ok().and_then(|t| openmeteo::parse_ensemble(&t).ok());
    let station = http_get(&observe::metar_url(cfg.lat, cfg.lon))
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

fn load_config() -> Result<Config, String> {
    let path = config::config_path().ok_or("no config directory on this system")?;
    config::load(&path).ok_or_else(|| "no location set; run: labyrinth-weather set-location \"City name\"".to_string())
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
    let fetched = match fetch(&cfg) {
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
        observed: fetched.station.as_ref().map(|(s, d)| (s, *d)),
        weighted: !weights.is_empty(),
    };
    println!("{}", report::bar_json(&input));
    ExitCode::SUCCESS
}

fn cmd_forecast(n_hours: usize) -> ExitCode {
    let cfg = match load_config() {
        Ok(c) => c,
        Err(e) => return die(&e),
    };
    let f = match fetch(&cfg) {
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
    let units = load_config().map(|c| c.units).unwrap_or_else(|_| "metric".into());
    let cfg = if args.first().map(String::as_str) == Some("--coords") {
        let (Some(lat), Some(lon)) = (args.get(1).and_then(|s| s.parse().ok()), args.get(2).and_then(|s| s.parse().ok())) else {
            return die("usage: set-location --coords LAT LON [NAME]");
        };
        let name = if args.len() > 3 { args[3..].join(" ") } else { format!("{lat:.2}, {lon:.2}") };
        Config { name, lat, lon, units }
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
        Config { name: p.label(), lat: p.latitude, lon: p.longitude, units }
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
    let f = match fetch(&cfg) {
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
        println!("nothing scored yet: needs predictions whose time has passed and a measurement near that time.\nRun `labyrinth-weather record` hourly (the systemd timer does this).");
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
    eprintln!("labyrinth-weather: {msg}");
    ExitCode::from(1)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("bar") => cmd_bar(),
        Some("forecast") => cmd_forecast(args.get(1).and_then(|s| s.parse().ok()).unwrap_or(24)),
        Some("set-location") => cmd_set_location(&args[1..]),
        Some("units") if args.len() == 2 => cmd_units(&args[1]),
        Some("record") => cmd_record(),
        Some("score") => cmd_score(),
        _ => {
            eprintln!("usage: labyrinth-weather bar | forecast [HOURS] | set-location CITY | set-location --coords LAT LON [NAME] | units metric|us | record | score");
            ExitCode::from(2)
        }
    }
}

#[allow(dead_code)]
fn _keep(_: &str) -> String {
    format_local(0, 0)
}
