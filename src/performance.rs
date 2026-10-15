//! Performance tracking for model racing and optimization
//!
//! Tracks historical performance metrics to inform racing decisions

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

// Preserve per-model identity instead of combining overflow into a fake contender.
const MAX_TRACKED_MODELS: usize = 256;

/// Performance statistics for a model
#[derive(Debug, Clone)]
pub struct ModelStats {
    /// Average time to first token (ms)
    pub avg_ttft_ms: f64,
    /// Average tokens per second
    pub avg_tokens_per_sec: f64,
    /// Success rate (0.0 - 1.0)
    pub success_rate: f64,
    /// Total requests tracked
    pub total_requests: u64,
    /// Total successful requests
    pub successful_requests: u64,
    /// Last updated timestamp
    pub last_updated: std::time::Instant,
    /// Successful responses with an actual time-to-first-token measurement.
    ttft_samples: u64,
    /// Successful responses with a finite, nonnegative token-rate measurement.
    tokens_per_sec_samples: u64,
}

impl ModelStats {
    fn new() -> Self {
        Self {
            avg_ttft_ms: 0.0,
            avg_tokens_per_sec: 0.0,
            success_rate: 1.0,
            total_requests: 0,
            successful_requests: 0,
            last_updated: std::time::Instant::now(),
            ttft_samples: 0,
            tokens_per_sec_samples: 0,
        }
    }

    /// Update stats with a new observation
    fn update(&mut self, observation: &PerformanceObservation) {
        self.total_requests += 1;

        if observation.success {
            self.successful_requests += 1;

            // Exponential moving average for TTFT
            let alpha = 0.3; // Weight for new observations
            if let Some(ttft) = observation.ttft {
                let new_ttft = ttft.as_secs_f64() * 1000.0;
                self.avg_ttft_ms = if self.ttft_samples == 0 {
                    new_ttft
                } else {
                    alpha * new_ttft + (1.0 - alpha) * self.avg_ttft_ms
                };
                self.ttft_samples += 1;
            }

            // Update tokens per second
            if let Some(tps) = observation
                .tokens_per_sec
                .filter(|value| value.is_finite() && *value >= 0.0)
            {
                self.avg_tokens_per_sec = if self.tokens_per_sec_samples == 0 {
                    tps
                } else {
                    alpha * tps + (1.0 - alpha) * self.avg_tokens_per_sec
                };
                self.tokens_per_sec_samples += 1;
            }
        }

        // Update success rate
        self.success_rate = self.successful_requests as f64 / self.total_requests as f64;
        self.last_updated = std::time::Instant::now();
    }
}

/// Single performance observation
#[derive(Debug)]
pub struct PerformanceObservation {
    pub model: String,
    pub success: bool,
    pub ttft: Option<Duration>,
    pub tokens_per_sec: Option<f64>,
    pub error: Option<String>,
}

/// Performance tracker for all models
pub struct PerformanceTracker {
    stats: Arc<RwLock<HashMap<String, ModelStats>>>,
}

impl PerformanceTracker {
    pub fn new() -> Self {
        Self {
            stats: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Record a performance observation
    pub fn record(&self, observation: PerformanceObservation) {
        let mut stats = self.stats.write().unwrap();
        if stats.len() >= MAX_TRACKED_MODELS && !stats.contains_key(&observation.model) {
            return;
        }
        let model_stats = stats
            .entry(observation.model.clone())
            .or_insert_with(ModelStats::new);
        model_stats.update(&observation);
    }

    /// Get stats for a specific model
    pub fn get_stats(&self, model: &str) -> Option<ModelStats> {
        let stats = self.stats.read().unwrap();
        stats.get(model).cloned()
    }

    /// Get all stats
    pub fn get_all_stats(&self) -> HashMap<String, ModelStats> {
        self.stats.read().unwrap().clone()
    }

    /// Get the fastest model after at least five successful TTFT measurements.
    pub fn get_fastest_model(&self) -> Option<String> {
        let stats = self.stats.read().unwrap();

        stats
            .iter()
            .filter(|(_, s)| s.ttft_samples >= 5)
            .min_by(|(model_a, a), (model_b, b)| {
                a.avg_ttft_ms
                    .total_cmp(&b.avg_ttft_ms)
                    .then_with(|| model_a.cmp(model_b))
            })
            .map(|(model, _)| model.clone())
    }

    /// Rank models with at least three successful TTFT measurements.
    pub fn get_ranked_models(&self) -> Vec<(String, ModelStats)> {
        let stats = self.stats.read().unwrap();

        let mut models: Vec<_> = stats
            .iter()
            .filter(|(_, s)| s.ttft_samples >= 3)
            .map(|(model, stats)| (model.clone(), stats.clone()))
            .collect();

        // Sort by composite score: TTFT (50%) + success rate (30%) + TPS (20%)
        models.sort_by(|(model_a, a), (model_b, b)| {
            let score_a = a.avg_ttft_ms * 0.5
                + (1.0 - a.success_rate) * 10000.0 * 0.3
                + (1.0 / a.avg_tokens_per_sec.max(1.0)) * 0.2;
            let score_b = b.avg_ttft_ms * 0.5
                + (1.0 - b.success_rate) * 10000.0 * 0.3
                + (1.0 / b.avg_tokens_per_sec.max(1.0)) * 0.2;
            score_a
                .total_cmp(&score_b)
                .then_with(|| model_a.cmp(model_b))
        });

        models
    }
}

impl Default for PerformanceTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observe(tracker: &PerformanceTracker, model: &str, success: bool, ttft: Option<Duration>) {
        tracker.record(PerformanceObservation {
            model: model.to_string(),
            success,
            ttft,
            tokens_per_sec: None,
            error: None,
        });
    }

    #[test]
    fn unknown_or_failed_ttft_never_outranks_measured_samples() {
        let tracker = PerformanceTracker::new();
        for _ in 0..5 {
            observe(&tracker, "unknown", true, None);
            observe(&tracker, "failed", false, Some(Duration::ZERO));
            observe(&tracker, "measured", true, Some(Duration::from_millis(10)));
        }
        assert_eq!(tracker.get_fastest_model().as_deref(), Some("measured"));
        let ranked: Vec<_> = tracker
            .get_ranked_models()
            .into_iter()
            .map(|(model, _)| model)
            .collect();
        assert_eq!(ranked, vec!["measured"]);
    }

    #[test]
    fn minimum_sample_counts_require_measured_ttft() {
        let tracker = PerformanceTracker::new();
        for _ in 0..5 {
            observe(&tracker, "sparse", true, None);
        }
        observe(&tracker, "sparse", true, Some(Duration::from_millis(1)));
        assert!(tracker.get_fastest_model().is_none());
        assert!(tracker.get_ranked_models().is_empty());
    }

    #[test]
    fn equal_scores_have_stable_model_order() {
        let tracker = PerformanceTracker::new();
        let expected: Vec<_> = (0..20).map(|i| format!("model-{i:02}")).collect();
        for model in expected.iter().rev() {
            for _ in 0..5 {
                observe(&tracker, model, true, Some(Duration::from_millis(10)));
            }
        }
        assert_eq!(tracker.get_fastest_model(), expected.first().cloned());
        let ranked: Vec<_> = tracker
            .get_ranked_models()
            .into_iter()
            .map(|(model, _)| model)
            .collect();
        assert_eq!(ranked, expected);
    }

    #[test]
    fn ttft_keeps_submillisecond_precision_and_zero_samples() {
        let tracker = PerformanceTracker::new();
        observe(
            &tracker,
            "fractional",
            true,
            Some(Duration::from_micros(500)),
        );
        assert_eq!(tracker.get_stats("fractional").unwrap().avg_ttft_ms, 0.5);
        observe(&tracker, "zero", true, Some(Duration::ZERO));
        observe(&tracker, "zero", true, Some(Duration::from_millis(1)));
        assert!((tracker.get_stats("zero").unwrap().avg_ttft_ms - 0.3).abs() < 1e-10);
    }

    #[test]
    fn nonfinite_or_negative_token_rates_do_not_poison_stats() {
        let tracker = PerformanceTracker::new();
        for tokens_per_sec in [f64::NAN, f64::INFINITY, -1.0, 10.0] {
            tracker.record(PerformanceObservation {
                model: "measured".to_string(),
                success: true,
                ttft: Some(Duration::from_millis(10)),
                tokens_per_sec: Some(tokens_per_sec),
                error: None,
            });
        }
        assert_eq!(
            tracker.get_stats("measured").unwrap().avg_tokens_per_sec,
            10.0
        );
    }

    #[test]
    fn distinct_models_are_bounded_and_existing_models_keep_updating() {
        let tracker = PerformanceTracker::new();
        for i in 0..256 {
            observe(&tracker, &format!("model-{i}"), true, None);
        }
        observe(&tracker, "overflow", true, None);
        observe(&tracker, "model-0", false, None);
        assert_eq!(tracker.get_all_stats().len(), 256);
        assert!(tracker.get_stats("overflow").is_none());
        let existing = tracker.get_stats("model-0").unwrap();
        assert_eq!(existing.total_requests, 2);
        assert_eq!(existing.success_rate, 0.5);
    }

    #[test]
    fn test_performance_tracking() {
        let tracker = PerformanceTracker::new();

        // Record some observations
        tracker.record(PerformanceObservation {
            model: "llama-3.1-70b".to_string(),
            success: true,
            ttft: Some(Duration::from_millis(1500)),
            tokens_per_sec: Some(25.0),
            error: None,
        });

        tracker.record(PerformanceObservation {
            model: "mistral-7b".to_string(),
            success: true,
            ttft: Some(Duration::from_millis(800)),
            tokens_per_sec: Some(40.0),
            error: None,
        });

        // Check stats
        let stats = tracker.get_stats("llama-3.1-70b");
        assert!(stats.is_some());
        let stats = stats.unwrap();
        assert_eq!(stats.total_requests, 1);
        assert_eq!(stats.avg_ttft_ms, 1500.0);
    }

    #[test]
    fn test_fastest_model() {
        let tracker = PerformanceTracker::new();

        // Need at least 5 observations for each
        for _ in 0..5 {
            tracker.record(PerformanceObservation {
                model: "slow-model".to_string(),
                success: true,
                ttft: Some(Duration::from_millis(2000)),
                tokens_per_sec: Some(20.0),
                error: None,
            });

            tracker.record(PerformanceObservation {
                model: "fast-model".to_string(),
                success: true,
                ttft: Some(Duration::from_millis(800)),
                tokens_per_sec: Some(40.0),
                error: None,
            });
        }

        let fastest = tracker.get_fastest_model();
        assert_eq!(fastest, Some("fast-model".to_string()));
    }
}
