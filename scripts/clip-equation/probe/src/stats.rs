//! Summary of a list of measured values.

use std::fmt;

#[derive(Clone, Copy, Debug, Default)]
pub struct Summary {
    pub count: usize,
    pub mean: f64,
    pub p50: f64,
    pub p99: f64,
    pub max: f64,
}

impl Summary {
    pub fn of(values: &[f64]) -> Self {
        if values.is_empty() {
            return Self::default();
        }
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        let last = sorted.len() - 1;
        let at = |quantile: f64| sorted[((last as f64) * quantile).round() as usize];
        Self {
            count: sorted.len(),
            mean: sorted.iter().sum::<f64>() / sorted.len() as f64,
            p50: at(0.5),
            p99: at(0.99),
            max: sorted[last],
        }
    }
}

impl fmt::Display for Summary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "mean={:.3} p50={:.3} p99={:.3} max={:.3} n={}",
            self.mean, self.p50, self.p99, self.max, self.count
        )
    }
}
