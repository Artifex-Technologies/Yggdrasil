//! Learn which forecast source is actually right for this place.
//!
//! Forecasts are recorded when issued; measured temperatures (observe.rs) are recorded
//! as they arrive. Later each prediction is compared with what was measured at the
//! target time. Sources with a smaller average error get more weight in the consensus.
//!
//! Honest limits: it needs days of data to mean anything, the "truth" is the nearest
//! airport's thermometer rather than your yard, it scores temperature only, and it
//! cannot make a model better than it is; it only picks which to lean on.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Record {
    pub source: String,
    pub issued: i64,
    pub target: i64,
    pub predicted_c: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Obs {
    pub epoch: i64,
    pub temp_c: f64,
    pub station: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Score {
    pub n: usize,
    /// Mean absolute error, deg C.
    pub mae: f64,
    /// Mean signed error (predicted - observed); positive = runs warm.
    pub bias: f64,
}

/// Compare each prediction with the observation closest to its target time, if one is
/// within `tolerance_s`. Predictions are only scored against observations at or after
/// they were issued.
pub fn score(records: &[Record], obs: &[Obs], tolerance_s: i64) -> HashMap<String, Score> {
    let mut sums: HashMap<String, (usize, f64, f64)> = HashMap::new();
    for r in records {
        let best = obs
            .iter()
            .filter(|o| o.epoch >= r.issued)
            .map(|o| ((o.epoch - r.target).abs(), o))
            .filter(|(gap, _)| *gap <= tolerance_s)
            .min_by_key(|(gap, _)| *gap);
        if let Some((_, o)) = best {
            let e = r.predicted_c - o.temp_c;
            let s = sums.entry(r.source.clone()).or_insert((0, 0.0, 0.0));
            s.0 += 1;
            s.1 += e.abs();
            s.2 += e;
        }
    }
    sums.into_iter().map(|(k, (n, abs, signed))| (k, Score { n, mae: abs / n as f64, bias: signed / n as f64 })).collect()
}

/// Weight = 1 / (MAE + 0.5), for sources with at least `min_n` scored predictions,
/// scaled so the scored sources average 1.0. Everything else keeps the neutral weight 1.0,
/// so a source is never penalized (or favored) on thin evidence.
pub fn weights(scores: &HashMap<String, Score>, min_n: usize) -> HashMap<String, f64> {
    let eligible: Vec<(&String, f64)> = scores.iter().filter(|(_, s)| s.n >= min_n).map(|(k, s)| (k, 1.0 / (s.mae + 0.5))).collect();
    if eligible.len() < 2 {
        return HashMap::new(); // nothing to compare against
    }
    let mean = eligible.iter().map(|e| e.1).sum::<f64>() / eligible.len() as f64;
    eligible.into_iter().map(|(k, w)| (k.clone(), w / mean)).collect()
}

pub fn append_jsonl<T: Serialize>(path: &Path, items: &[T]) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    for i in items {
        writeln!(f, "{}", serde_json::to_string(i).map_err(std::io::Error::other)?)?;
    }
    Ok(())
}

/// Unreadable lines are skipped, so one corrupt line cannot disable the history.
pub fn read_jsonl<T: for<'de> Deserialize<'de>>(path: &Path) -> Vec<T> {
    std::fs::read_to_string(path)
        .map(|t| t.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(source: &str, target: i64, p: f64) -> Record {
        Record { source: source.into(), issued: 0, target, predicted_c: p }
    }
    fn ob(epoch: i64, t: f64) -> Obs {
        Obs { epoch, temp_c: t, station: "X".into() }
    }

    #[test]
    fn scores_mae_and_bias() {
        let obs = vec![ob(3600, 10.0), ob(7200, 12.0)];
        let recs = vec![rec("good", 3600, 10.5), rec("good", 7200, 11.5), rec("warm", 3600, 13.0), rec("warm", 7200, 15.0)];
        let s = score(&recs, &obs, 900);
        assert_eq!(s["good"].n, 2);
        assert!((s["good"].mae - 0.5).abs() < 1e-9);
        assert!(s["good"].bias.abs() < 1e-9);
        assert!((s["warm"].mae - 3.0).abs() < 1e-9);
        assert!((s["warm"].bias - 3.0).abs() < 1e-9);
    }

    #[test]
    fn no_observation_within_tolerance_means_not_scored() {
        let s = score(&[rec("a", 3600, 10.0)], &[ob(9000, 10.0)], 900);
        assert!(s.is_empty());
    }

    #[test]
    fn observations_before_the_forecast_was_issued_do_not_count() {
        let r = Record { source: "a".into(), issued: 5000, target: 5100, predicted_c: 10.0 };
        assert!(score(&[r], &[ob(4900, 10.0)], 900).is_empty());
    }

    #[test]
    fn better_source_gets_more_weight_and_weights_average_one() {
        let mut s = HashMap::new();
        s.insert("good".to_string(), Score { n: 50, mae: 0.5, bias: 0.0 });
        s.insert("bad".to_string(), Score { n: 50, mae: 3.0, bias: 0.0 });
        let w = weights(&s, 24);
        assert!(w["good"] > w["bad"]);
        assert!(((w["good"] + w["bad"]) / 2.0 - 1.0).abs() < 1e-9);
    }

    #[test]
    fn thin_evidence_changes_nothing() {
        let mut s = HashMap::new();
        s.insert("a".to_string(), Score { n: 3, mae: 0.1, bias: 0.0 });
        s.insert("b".to_string(), Score { n: 50, mae: 3.0, bias: 0.0 });
        assert!(weights(&s, 24).is_empty()); // only one eligible source: nothing to compare
        s.insert("c".to_string(), Score { n: 50, mae: 1.0, bias: 0.0 });
        let w = weights(&s, 24);
        assert!(!w.contains_key("a")); // thin source keeps the neutral default
        assert!(w["c"] > w["b"]);
    }

    #[test]
    fn jsonl_round_trip_and_corruption_tolerance() {
        let dir = std::env::temp_dir().join(format!("lw-verify-{}", std::process::id()));
        let f = dir.join("r.jsonl");
        let _ = std::fs::remove_dir_all(&dir);
        append_jsonl(&f, &[rec("a", 1, 2.0)]).unwrap();
        append_jsonl(&f, &[rec("b", 3, 4.0)]).unwrap();
        std::fs::OpenOptions::new().append(true).open(&f).unwrap().write_all(b"{garbage\n").unwrap();
        let back: Vec<Record> = read_jsonl(&f);
        assert_eq!(back.len(), 2);
        assert_eq!(back[1].source, "b");
        assert!(read_jsonl::<Record>(&dir.join("missing.jsonl")).is_empty());
    }
}
