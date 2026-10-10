//! Combine several independent forecasts into one number plus an honest uncertainty.
//!
//! What this does and does not do: averaging independent models usually beats any
//! single one, and their disagreement is a real signal of uncertainty. It cannot know
//! more than the models do. Accuracy weighting (verify.rs) only helps once there is
//! measured history for the location.

use crate::openmeteo::Forecast;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    High,
    Medium,
    Low,
}

impl Confidence {
    pub fn label(self) -> &'static str {
        match self {
            Confidence::High => "high",
            Confidence::Medium => "medium",
            Confidence::Low => "low",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Hour {
    pub time: String,
    /// Weighted mean of the models' temperatures (deg C).
    pub mean: f64,
    pub low: f64,
    pub high: f64,
    pub n_models: usize,
    /// (p10, p50, p90) over ensemble members when available.
    pub ensemble: Option<(f64, f64, f64)>,
    pub precip_prob: Option<f64>,
    pub confidence: Confidence,
}

/// Linear-interpolated percentile of an already sorted slice; `p` in 0..=1.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    debug_assert!(!sorted.is_empty());
    if sorted.len() == 1 {
        return sorted[0];
    }
    let pos = p.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let (lo, hi) = (pos.floor() as usize, pos.ceil() as usize);
    sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo as f64)
}

/// Heuristic labels, not calibrated probabilities: the band is how far apart the models
/// (and the ensemble's 10th-90th percentile range) are, in degrees C.
pub fn confidence(spread_c: f64) -> Confidence {
    if spread_c <= 2.0 {
        Confidence::High
    } else if spread_c <= 4.0 {
        Confidence::Medium
    } else {
        Confidence::Low
    }
}

/// `weights` maps a model name to its weight (missing = 1.0). `ensemble` is
/// (times, one temperature series per member), matched to the forecast by time string.
pub fn consensus(f: &Forecast, ensemble: Option<(&[String], &[Vec<Option<f64>>])>, weights: &HashMap<String, f64>) -> Vec<Hour> {
    let ens_index: HashMap<&str, usize> = ensemble
        .map(|(t, _)| t.iter().enumerate().map(|(i, s)| (s.as_str(), i)).collect())
        .unwrap_or_default();
    let mut out = Vec::new();
    for (i, time) in f.times.iter().enumerate() {
        let mut vals: Vec<(f64, f64)> = vec![]; // (temp, weight)
        let mut precip: Vec<(f64, f64)> = vec![];
        for m in &f.models {
            let w = weights.get(&m.source).copied().unwrap_or(1.0).max(0.0);
            if let Some(Some(t)) = m.temp_c.get(i) {
                vals.push((*t, w));
            }
            if let Some(Some(p)) = m.precip_prob.get(i) {
                precip.push((*p, w));
            }
        }
        if vals.is_empty() {
            continue;
        }
        let wsum: f64 = vals.iter().map(|v| v.1).sum();
        let mean = if wsum > 0.0 { vals.iter().map(|v| v.0 * v.1).sum::<f64>() / wsum } else { vals.iter().map(|v| v.0).sum::<f64>() / vals.len() as f64 };
        let low = vals.iter().map(|v| v.0).fold(f64::INFINITY, f64::min);
        let high = vals.iter().map(|v| v.0).fold(f64::NEG_INFINITY, f64::max);

        let ens = ensemble.and_then(|(_, members)| {
            let idx = *ens_index.get(time.as_str())?;
            let mut v: Vec<f64> = members.iter().filter_map(|m| m.get(idx).copied().flatten()).collect();
            if v.len() < 3 {
                return None;
            }
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            Some((percentile(&v, 0.1), percentile(&v, 0.5), percentile(&v, 0.9)))
        });
        let mut spread = high - low;
        if let Some((p10, _, p90)) = ens {
            spread = spread.max(p90 - p10);
        }
        let pw: f64 = precip.iter().map(|v| v.1).sum();
        let precip_prob = if precip.is_empty() {
            None
        } else if pw > 0.0 {
            Some(precip.iter().map(|v| v.0 * v.1).sum::<f64>() / pw)
        } else {
            Some(precip.iter().map(|v| v.0).sum::<f64>() / precip.len() as f64)
        };
        out.push(Hour { time: time.clone(), mean, low, high, n_models: vals.len(), ensemble: ens, precip_prob, confidence: confidence(spread) });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openmeteo::Series;

    fn fc(models: Vec<(&str, Vec<f64>)>) -> Forecast {
        let n = models[0].1.len();
        Forecast {
            times: (0..n).map(|i| format!("2026-10-10T{i:02}:00")).collect(),
            utc_offset_s: 0,
            models: models
                .into_iter()
                .map(|(s, t)| Series { source: s.into(), temp_c: t.iter().map(|x| Some(*x)).collect(), precip_prob: vec![Some(20.0); n] })
                .collect(),
        }
    }

    #[test]
    fn percentiles() {
        let v = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(percentile(&v, 0.0), 1.0);
        assert_eq!(percentile(&v, 0.5), 3.0);
        assert_eq!(percentile(&v, 1.0), 5.0);
        assert!((percentile(&v, 0.1) - 1.4).abs() < 1e-9);
        assert_eq!(percentile(&[7.0], 0.9), 7.0);
    }

    #[test]
    fn equal_weights_is_the_plain_mean_and_range() {
        let h = &consensus(&fc(vec![("a", vec![10.0]), ("b", vec![12.0]), ("c", vec![14.0])]), None, &HashMap::new())[0];
        assert_eq!(h.mean, 12.0);
        assert_eq!((h.low, h.high, h.n_models), (10.0, 14.0, 3));
        assert_eq!(h.confidence, Confidence::Medium); // exactly 4 degrees apart; Low starts above 4
    }

    #[test]
    fn confidence_bands() {
        assert_eq!(confidence(1.0), Confidence::High);
        assert_eq!(confidence(2.0), Confidence::High);
        assert_eq!(confidence(3.0), Confidence::Medium);
        assert_eq!(confidence(4.1), Confidence::Low);
    }

    #[test]
    fn weights_shift_the_mean_but_not_the_range() {
        let mut w = HashMap::new();
        w.insert("a".to_string(), 3.0);
        let h = &consensus(&fc(vec![("a", vec![10.0]), ("b", vec![14.0])]), None, &w)[0];
        assert_eq!(h.mean, 11.0); // (10*3 + 14*1) / 4
        assert_eq!((h.low, h.high), (10.0, 14.0));
    }

    #[test]
    fn zero_weights_fall_back_to_plain_mean() {
        let mut w = HashMap::new();
        w.insert("a".to_string(), 0.0);
        w.insert("b".to_string(), 0.0);
        assert_eq!(consensus(&fc(vec![("a", vec![10.0]), ("b", vec![14.0])]), None, &w)[0].mean, 12.0);
    }

    #[test]
    fn missing_values_are_skipped_and_empty_hours_dropped() {
        let mut f = fc(vec![("a", vec![10.0, 0.0]), ("b", vec![14.0, 0.0])]);
        f.models[0].temp_c[1] = None;
        f.models[1].temp_c[1] = None;
        let hs = consensus(&f, None, &HashMap::new());
        assert_eq!(hs.len(), 1);
        f.models[1].temp_c[0] = None;
        assert_eq!(consensus(&f, None, &HashMap::new())[0].n_models, 1);
    }

    #[test]
    fn ensemble_widens_uncertainty() {
        let f = fc(vec![("a", vec![10.0]), ("b", vec![10.5])]); // models agree
        let times = vec!["2026-10-10T00:00".to_string()];
        let members: Vec<Vec<Option<f64>>> = (0..10).map(|i| vec![Some(4.0 + i as f64 * 1.5)]).collect(); // 4..17.5
        let h = &consensus(&f, Some((&times, &members)), &HashMap::new())[0];
        let (p10, p50, p90) = h.ensemble.unwrap();
        assert!(p10 < p50 && p50 < p90);
        assert_eq!(h.confidence, Confidence::Low); // ensemble disagrees even though the models agree
    }

    #[test]
    fn too_few_members_are_ignored() {
        let f = fc(vec![("a", vec![10.0])]);
        let times = vec!["2026-10-10T00:00".to_string()];
        let members = vec![vec![Some(1.0)], vec![Some(2.0)]];
        assert!(consensus(&f, Some((&times, &members)), &HashMap::new())[0].ensemble.is_none());
    }
}
