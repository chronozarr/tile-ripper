//! Pure physical-range policy. Transport and Zarr interpretation live elsewhere.
use crate::{fail, Result};
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Policy {
    Individual,
    Adjacent,
    Fixed,
    Cost,
}
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Config {
    pub policy: Policy,
    pub max_span: u64,
    pub max_gap: u64,
    pub max_overfetch_ratio: f64,
    pub latency_seconds: f64,
    pub bandwidth_bytes_second: f64,
    pub concurrency: usize,
}
fn number(name: &str, default: f64) -> Result<f64> {
    match std::env::var(name) {
        Ok(v) => v
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite() && *n > 0.)
            .ok_or_else(|| format!("{name} must be finite and positive").into()),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(_) => fail(&format!("invalid {name}")),
    }
}
impl Config {
    pub fn from_env(concurrency: usize) -> Result<Self> {
        let policy = match std::env::var("TILE_RIPPER_ZARR_COALESCE").as_deref() {
            Ok("individual") => Policy::Individual,
            Ok("adjacent") => Policy::Adjacent,
            Ok("fixed") => Policy::Fixed,
            Ok("cost") | Err(std::env::VarError::NotPresent) => Policy::Cost,
            _ => {
                return fail(
                    "TILE_RIPPER_ZARR_COALESCE must be individual, adjacent, fixed or cost",
                )
            }
        };
        let fixed = policy == Policy::Fixed;
        Ok(Self {
            policy,
            max_span: number(
                "TILE_RIPPER_ZARR_MAX_SPAN",
                if fixed { 16777216. } else { 4194304. },
            )?
            .min(usize::MAX as f64) as u64,
            max_gap: if policy == Policy::Adjacent {
                0
            } else {
                number(
                    "TILE_RIPPER_ZARR_MAX_GAP",
                    if fixed { 1048576. } else { 65536. },
                )?
                .min(usize::MAX as f64) as u64
            },
            max_overfetch_ratio: number(
                "TILE_RIPPER_ZARR_OVERFETCH_RATIO",
                if fixed { 1e6 } else { 1. },
            )?,
            latency_seconds: number("TILE_RIPPER_ZARR_LATENCY_MS", 2.)? / 1000.,
            bandwidth_bytes_second: number("TILE_RIPPER_ZARR_BANDWIDTH_MBPS", 32.)? * 1_000_000.,
            concurrency: concurrency.max(1),
        })
    }
    pub fn with_concurrency(&self, concurrency: usize) -> Self {
        Self {
            concurrency: concurrency.max(1),
            ..self.clone()
        }
    }
    /// Input order is irrelevant; callers retain their logical delivery order.
    pub fn groups(&self, spans: &[(u64, u64, Vec<usize>)], cap: u64) -> Vec<Group> {
        let mut sorted = spans.to_vec();
        sorted.sort_by_key(|s| s.0);
        let mut groups: Vec<Group> = Vec::new();
        for (start, len, logical) in sorted {
            let Some(end) = start.checked_add(len).filter(|_| len > 0) else {
                continue;
            };
            if let Some(last) = groups.last_mut() {
                let selected = last.selected + len;
                let span = end.saturating_sub(last.start);
                let overfetch = span.saturating_sub(selected);
                let gap = start.saturating_sub(last.end);
                let beneficial = self.policy != Policy::Cost
                    || gap as f64 / self.bandwidth_bytes_second
                        < self.latency_seconds / self.concurrency as f64;
                if self.policy != Policy::Individual
                    && start >= last.end
                    && gap <= self.max_gap
                    && span <= self.max_span.min(cap)
                    && overfetch as f64 <= selected as f64 * self.max_overfetch_ratio
                    && beneficial
                {
                    last.end = end;
                    last.selected = selected;
                    last.members.push(logical);
                    continue;
                }
            }
            groups.push(Group {
                start,
                end,
                selected: len,
                members: vec![logical],
            });
        }
        groups
    }
}
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Group {
    pub start: u64,
    pub end: u64,
    pub selected: u64,
    pub members: Vec<Vec<usize>>,
}
impl Group {
    pub fn len(&self) -> usize {
        (self.end - self.start) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> Config {
        Config {
            policy: Policy::Cost,
            max_span: 1024,
            max_gap: 100,
            max_overfetch_ratio: 1.,
            latency_seconds: 0.008,
            bandwidth_bytes_second: 1e6,
            concurrency: 4,
        }
    }
    #[test]
    fn physical_order_and_sparse_gaps() {
        let spans = vec![(130, 30, vec![0]), (0, 100, vec![3]), (1000, 10, vec![1])];
        let g = config().groups(&spans, 1024);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].members, vec![vec![3], vec![0]]);
        assert_eq!(g[0].len(), 160);
        assert_eq!(g[0].selected, 130);
    }
    #[test]
    fn costs_and_hard_caps() {
        let spans = vec![(0, 10, vec![0]), (20, 10, vec![1])];
        assert_eq!(config().groups(&spans, 29).len(), 2);
        let mut c = config();
        c.bandwidth_bytes_second = 100.;
        assert_eq!(c.groups(&spans, 100).len(), 2);
        c.policy = Policy::Adjacent;
        c.max_gap = 0;
        assert_eq!(c.groups(&spans, 100).len(), 2);
        assert_eq!(
            c.groups(&[(0, 10, vec![1]), (10, 10, vec![0])], 100).len(),
            1
        );
        c.policy = Policy::Individual;
        assert_eq!(
            c.groups(&[(0, 10, vec![1]), (10, 10, vec![0])], 100).len(),
            2
        );
    }
    #[test]
    fn overlaps_and_overfetch_not_merged() {
        assert_eq!(
            config()
                .groups(&[(0, 10, vec![0]), (5, 10, vec![1])], 100)
                .len(),
            2
        );
        assert_eq!(
            config()
                .groups(&[(0, 1, vec![0]), (20, 1, vec![1])], 100)
                .len(),
            2
        );
    }
}
