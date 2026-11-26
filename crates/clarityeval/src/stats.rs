//! Statistics for the report (docs/CLARITYEVAL.md section 13): cluster
//! bootstrap confidence intervals with a fixed seed, percentiles and Wilson
//! intervals. No external crates, so results are identical on every machine.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Default bootstrap seed. Recorded in every report.
pub const BOOTSTRAP_SEED: u64 = 20_261_204;
/// Default number of bootstrap resamples (section 13).
pub const BOOTSTRAP_RESAMPLES: usize = 10_000;

/// SplitMix64: small, fast and fully specified, so a seed reproduces exactly.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (n > 0), without modulo bias worth caring about at these sizes.
    pub fn below(&mut self, n: usize) -> usize {
        ((self.next_u64() >> 11) as f64 / (1u64 << 53) as f64 * n as f64) as usize % n
    }
}

/// A point estimate with a 95% interval and the number of observations behind it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Estimate {
    pub value: Option<f64>,
    pub lo: Option<f64>,
    pub hi: Option<f64>,
    /// Observations (presentations, sections or procedures, as labelled).
    pub n: usize,
    /// Resampling clusters (base plans).
    pub clusters: usize,
}

impl Estimate {
    pub fn empty() -> Self {
        Estimate {
            value: None,
            lo: None,
            hi: None,
            n: 0,
            clusters: 0,
        }
    }

    /// `85% [70, 95]`.
    pub fn fmt_pct(&self) -> String {
        match (self.value, self.lo, self.hi) {
            (Some(v), Some(lo), Some(hi)) => {
                format!("{:.0}% [{:.0}, {:.0}]", v * 100.0, lo * 100.0, hi * 100.0)
            }
            (Some(v), _, _) => format!("{:.0}%", v * 100.0),
            _ => "n/a".into(),
        }
    }

    /// `12.5 [4.0, 20.0]`, scaled and with `digits` decimals.
    pub fn fmt(&self, scale: f64, digits: usize) -> String {
        match (self.value, self.lo, self.hi) {
            (Some(v), Some(lo), Some(hi)) => format!(
                "{:.d$} [{:.d$}, {:.d$}]",
                v * scale,
                lo * scale,
                hi * scale,
                d = digits
            ),
            (Some(v), _, _) => format!("{:.d$}", v * scale, d = digits),
            _ => "n/a".into(),
        }
    }
}

/// Linear-interpolation percentile of sorted values, `p` in 0..=100.
pub fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let pos = (p / 100.0) * (sorted.len() as f64 - 1.0);
    let (lo, hi) = (pos.floor() as usize, pos.ceil() as usize);
    let frac = pos - lo as f64;
    Some(sorted[lo] + (sorted[hi] - sorted[lo]) * frac)
}

/// One observation: its cluster (base plan) and a numerator/denominator pair.
/// Means use denominator 1; pooled ratios (citations, sections) use real counts.
#[derive(Debug, Clone, PartialEq)]
pub struct Obs {
    pub cluster: String,
    pub num: f64,
    pub den: f64,
}

impl Obs {
    pub fn mean(cluster: &str, value: f64) -> Obs {
        Obs {
            cluster: cluster.into(),
            num: value,
            den: 1.0,
        }
    }

    pub fn ratio(cluster: &str, num: f64, den: f64) -> Obs {
        Obs {
            cluster: cluster.into(),
            num,
            den,
        }
    }
}

fn group(obs: &[Obs]) -> Vec<(f64, f64, usize)> {
    let mut by: BTreeMap<&str, (f64, f64, usize)> = BTreeMap::new();
    for o in obs {
        let e = by.entry(o.cluster.as_str()).or_default();
        e.0 += o.num;
        e.1 += o.den;
        e.2 += 1;
    }
    by.into_values().collect()
}

/// Pooled ratio sum(num)/sum(den) with a cluster-bootstrap 95% percentile interval.
/// Resampling draws whole clusters with replacement (section 13).
pub fn cluster_bootstrap(obs: &[Obs], resamples: usize, seed: u64) -> Estimate {
    let clusters = group(obs);
    let total_den: f64 = clusters.iter().map(|c| c.1).sum();
    if clusters.is_empty() || total_den <= 0.0 {
        return Estimate {
            n: obs.len(),
            clusters: clusters.len(),
            ..Estimate::empty()
        };
    }
    let value = clusters.iter().map(|c| c.0).sum::<f64>() / total_den;
    let mut rng = Rng::new(seed);
    let k = clusters.len();
    let mut stats = Vec::with_capacity(resamples);
    for _ in 0..resamples {
        let (mut num, mut den) = (0.0, 0.0);
        for _ in 0..k {
            let c = &clusters[rng.below(k)];
            num += c.0;
            den += c.1;
        }
        if den > 0.0 {
            stats.push(num / den);
        }
    }
    stats.sort_by(f64::total_cmp);
    Estimate {
        value: Some(value),
        lo: percentile(&stats, 2.5),
        hi: percentile(&stats, 97.5),
        n: obs.len(),
        clusters: k,
    }
}

/// Paired difference of means (a minus b) over clusters present in both,
/// with a paired cluster-bootstrap interval: each resample draws the same
/// clusters for both systems.
pub fn paired_difference(a: &[Obs], b: &[Obs], resamples: usize, seed: u64) -> Estimate {
    let index = |obs: &[Obs]| {
        let mut m: BTreeMap<String, (f64, f64)> = BTreeMap::new();
        for o in obs {
            let e = m.entry(o.cluster.clone()).or_default();
            e.0 += o.num;
            e.1 += o.den;
        }
        m
    };
    let (ia, ib) = (index(a), index(b));
    let shared: Vec<((f64, f64), (f64, f64))> = ia
        .iter()
        .filter_map(|(k, va)| ib.get(k).map(|vb| (*va, *vb)))
        .collect();
    let n = a.len().min(b.len());
    if shared.is_empty() {
        return Estimate {
            n,
            ..Estimate::empty()
        };
    }
    let diff = |rows: &mut dyn Iterator<Item = ((f64, f64), (f64, f64))>| {
        let (mut an, mut ad, mut bn, mut bd) = (0.0, 0.0, 0.0, 0.0);
        for (x, y) in rows {
            an += x.0;
            ad += x.1;
            bn += y.0;
            bd += y.1;
        }
        (ad > 0.0 && bd > 0.0).then(|| an / ad - bn / bd)
    };
    let value = diff(&mut shared.iter().copied());
    let mut rng = Rng::new(seed);
    let k = shared.len();
    let mut stats = Vec::with_capacity(resamples);
    for _ in 0..resamples {
        let mut draw = (0..k).map(|_| shared[rng.below(k)]);
        if let Some(d) = diff(&mut draw) {
            stats.push(d);
        }
    }
    stats.sort_by(f64::total_cmp);
    Estimate {
        value,
        lo: percentile(&stats, 2.5),
        hi: percentile(&stats, 97.5),
        n,
        clusters: k,
    }
}

/// Wilson score 95% interval for `k` successes in `n` trials.
pub fn wilson(k: usize, n: usize) -> Option<(f64, f64)> {
    if n == 0 {
        return None;
    }
    let z = 1.959_963_984_540_054_f64;
    let (k, n) = (k as f64, n as f64);
    let p = k / n;
    let denom = 1.0 + z * z / n;
    let centre = (p + z * z / (2.0 * n)) / denom;
    let half = z * ((p * (1.0 - p) / n + z * z / (4.0 * n * n)).sqrt()) / denom;
    Some(((centre - half).max(0.0), (centre + half).min(1.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_is_reproducible_and_in_range() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        for _ in 0..1000 {
            let x = a.below(13);
            assert_eq!(x, b.below(13));
            assert!(x < 13);
        }
        // SplitMix64 reference value for seed 0.
        assert_eq!(Rng::new(0).next_u64(), 0xE220_A839_7B1D_CDAF);
    }

    #[test]
    fn percentiles_interpolate() {
        let v = [1.0, 2.0, 3.0, 4.0];
        assert_eq!(percentile(&v, 50.0), Some(2.5));
        assert_eq!(percentile(&v, 0.0), Some(1.0));
        assert_eq!(percentile(&v, 100.0), Some(4.0));
        assert_eq!(percentile(&[], 50.0), None);
    }

    #[test]
    fn constant_data_has_a_degenerate_interval() {
        let obs: Vec<Obs> = (0..8).map(|i| Obs::mean(&format!("c{i}"), 2.0)).collect();
        let e = cluster_bootstrap(&obs, 500, 1);
        assert_eq!(e.value, Some(2.0));
        assert_eq!((e.lo, e.hi), (Some(2.0), Some(2.0)));
        assert_eq!((e.n, e.clusters), (8, 8));
    }

    #[test]
    fn interval_covers_the_mean_and_is_seeded() {
        let obs: Vec<Obs> = (0..20)
            .map(|i| Obs::mean(&format!("c{i}"), f64::from(i % 5)))
            .collect();
        let a = cluster_bootstrap(&obs, 2000, 42);
        let b = cluster_bootstrap(&obs, 2000, 42);
        assert_eq!(a, b);
        let (v, lo, hi) = (a.value.unwrap(), a.lo.unwrap(), a.hi.unwrap());
        assert!((v - 2.0).abs() < 1e-12);
        assert!(lo < v && v < hi, "{lo} {v} {hi}");
        assert!(hi - lo < 1.6);
    }

    #[test]
    fn clusters_resample_together() {
        // Two observations in one cluster count as one draw.
        let obs = vec![
            Obs::mean("a", 1.0),
            Obs::mean("a", 1.0),
            Obs::mean("b", 0.0),
        ];
        let e = cluster_bootstrap(&obs, 1000, 3);
        assert_eq!(e.clusters, 2);
        assert!((e.value.unwrap() - 2.0 / 3.0).abs() < 1e-12);
        assert_eq!(e.lo, Some(0.0));
        assert_eq!(e.hi, Some(1.0));
    }

    #[test]
    fn pooled_ratio_weights_by_denominator() {
        let obs = vec![Obs::ratio("a", 9.0, 10.0), Obs::ratio("b", 0.0, 1.0)];
        let e = cluster_bootstrap(&obs, 100, 3);
        assert!((e.value.unwrap() - 9.0 / 11.0).abs() < 1e-12);
        assert!(
            cluster_bootstrap(&[Obs::ratio("a", 0.0, 0.0)], 10, 1)
                .value
                .is_none()
        );
    }

    #[test]
    fn paired_difference_uses_shared_clusters() {
        let a: Vec<Obs> = (0..10).map(|i| Obs::mean(&format!("c{i}"), 3.0)).collect();
        let mut b: Vec<Obs> = (0..10).map(|i| Obs::mean(&format!("c{i}"), 1.0)).collect();
        b.push(Obs::mean("only-b", 100.0));
        let d = paired_difference(&a, &b, 500, 9);
        assert_eq!(d.value, Some(2.0));
        assert_eq!((d.lo, d.hi), (Some(2.0), Some(2.0)));
        assert_eq!(d.clusters, 10);
    }

    #[test]
    fn wilson_matches_reference_values() {
        // 18/20: about 0.699 to 0.972.
        let (lo, hi) = wilson(18, 20).unwrap();
        assert!((lo - 0.699).abs() < 0.002, "{lo}");
        assert!((hi - 0.972).abs() < 0.002, "{hi}");
        assert_eq!(wilson(0, 0), None);
        let (lo, _) = wilson(0, 10).unwrap();
        assert_eq!(lo, 0.0);
    }
}
