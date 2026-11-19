//! Per-mod performance: load time, memory and per-tick cost, with regression
//! detection against the previous run.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The numbers kept for one mod.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ModPerformance {
    pub load_ms: f64,
    pub memory_bytes: u64,
    pub tick_total_us: u64,
    pub tick_count: u64,
}

impl ModPerformance {
    pub fn average_tick_us(&self) -> f64 {
        if self.tick_count == 0 {
            0.0
        } else {
            self.tick_total_us as f64 / self.tick_count as f64
        }
    }
}

/// A row the performance panel draws.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub id: String,
    pub load_ms: f64,
    pub memory_bytes: u64,
    pub average_tick_us: f64,
    pub regression: Option<f64>,
}

/// The panel's data.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PerformancePanel {
    pub mods: BTreeMap<String, ModPerformance>,
    /// The previous run's averages, for regression detection.
    #[serde(default)]
    pub previous_tick_us: BTreeMap<String, f64>,
}

/// The default regression threshold: 25% slower than last run.
pub const REGRESSION_PERCENT: f64 = 25.0;

impl PerformancePanel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_load(&mut self, id: &str, milliseconds: f64) {
        self.mods.entry(id.to_owned()).or_default().load_ms += milliseconds;
    }

    pub fn record_memory(&mut self, id: &str, bytes: u64) {
        self.mods.entry(id.to_owned()).or_default().memory_bytes = bytes;
    }

    pub fn record_tick(&mut self, id: &str, microseconds: u64) {
        let entry = self.mods.entry(id.to_owned()).or_default();
        entry.tick_total_us = entry.tick_total_us.saturating_add(microseconds);
        entry.tick_count = entry.tick_count.saturating_add(1);
    }

    /// Remembers this run's averages as the baseline for the next one.
    pub fn commit_baseline(&mut self) {
        self.previous_tick_us = self
            .mods
            .iter()
            .map(|(id, perf)| (id.clone(), perf.average_tick_us()))
            .collect();
    }

    /// Rows sorted by id, with a regression percentage when one is detected.
    pub fn rows(&self) -> Vec<Row> {
        self.mods
            .iter()
            .map(|(id, perf)| {
                let average = perf.average_tick_us();
                let regression = self
                    .previous_tick_us
                    .get(id)
                    .filter(|previous| **previous > 0.0)
                    .map(|previous| (average - previous) / previous * 100.0)
                    .filter(|percent| *percent > REGRESSION_PERCENT);
                Row {
                    id: id.clone(),
                    load_ms: perf.load_ms,
                    memory_bytes: perf.memory_bytes,
                    average_tick_us: average,
                    regression,
                }
            })
            .collect()
    }

    /// Rows that got slower than the threshold.
    pub fn regressions(&self) -> Vec<Row> {
        self.rows()
            .into_iter()
            .filter(|row| row.regression.is_some())
            .collect()
    }

    /// Total memory across mods.
    pub fn memory_total(&self) -> u64 {
        self.mods.values().map(|m| m.memory_bytes).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_averages() {
        let mut panel = PerformancePanel::new();
        panel.record_load("a", 2.0);
        panel.record_load("a", 3.0);
        panel.record_memory("a", 1024);
        panel.record_tick("a", 10);
        panel.record_tick("a", 20);
        let row = &panel.rows()[0];
        assert_eq!(row.id, "a");
        assert_eq!(row.load_ms, 5.0);
        assert_eq!(row.memory_bytes, 1024);
        assert_eq!(row.average_tick_us, 15.0);
        assert_eq!(panel.memory_total(), 1024);
        assert!(panel.regressions().is_empty());
    }

    #[test]
    fn a_slower_mod_is_a_regression() {
        let mut panel = PerformancePanel::new();
        panel.record_tick("a", 100);
        panel.commit_baseline();
        for _ in 0..10 {
            panel.record_tick("a", 500);
            panel.record_tick("a", 500);
        }
        let rows = panel.regressions();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].regression.unwrap() > REGRESSION_PERCENT);
        // A mod with no baseline is never a regression.
        panel.record_tick("new", 1);
        assert!(panel.regressions().iter().all(|r| r.id == "a"));
    }

    #[test]
    fn the_panel_round_trips() {
        let mut panel = PerformancePanel::new();
        panel.record_tick("a", 12);
        let text = serde_json::to_string(&panel).unwrap();
        let back: PerformancePanel = serde_json::from_str(&text).unwrap();
        assert_eq!(panel, back);
    }
}
