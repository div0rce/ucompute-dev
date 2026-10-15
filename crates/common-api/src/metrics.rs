//! Prometheus metrics (F-09-03). Only counts and timings, never prompt or completion text.

use common_runtime::{Timings, Usage};
use prometheus::{
    Encoder, Gauge, Histogram, HistogramOpts, IntCounter, IntCounterVec, IntGauge, Opts, Registry, TextEncoder,
};

#[derive(Clone)]
pub struct Metrics {
    pub registry: Registry,
    pub requests: IntCounterVec,
    pub rejected: IntCounterVec,
    pub tokens_generated: IntCounter,
    pub prompt_tokens: IntCounter,
    pub ttft: Histogram,
    pub decode_tps: Gauge,
    pub queue_depth: IntGauge,
    pub inflight: IntGauge,
    /// `common_remote_requests_total{peer}`: requests this device sent to a paired device (Mode B),
    /// by the peer's short label. Counts only, never text.
    pub remote_requests: IntCounterVec,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::new();
        let requests = IntCounterVec::new(
            Opts::new(
                "common_requests_total",
                "HTTP requests by matched route and status code.",
            ),
            &["route", "status"],
        )
        .expect("static metric");
        let rejected = IntCounterVec::new(
            Opts::new(
                "common_requests_rejected_total",
                "Requests turned away by admission control, by reason.",
            ),
            &["reason"],
        )
        .expect("static metric");
        // Pre-create both reasons so the series exist at 0 (a label vec with no children is not exported).
        for r in [
            crate::admission::Rejected::QueueFull,
            crate::admission::Rejected::Timeout,
        ] {
            rejected.with_label_values(&[r.reason()]).reset();
        }
        let tokens_generated =
            IntCounter::new("common_tokens_generated_total", "Completion tokens generated.").expect("static metric");
        let prompt_tokens =
            IntCounter::new("common_prompt_tokens_total", "Prompt tokens evaluated.").expect("static metric");
        let ttft = Histogram::with_opts(
            HistogramOpts::new("common_ttft_seconds", "Time to first token.")
                .buckets(vec![0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0]),
        )
        .expect("static metric");
        let decode_tps = Gauge::new(
            "common_decode_tokens_per_second",
            "Decode speed of the most recent completed request.",
        )
        .expect("static metric");
        let queue_depth =
            IntGauge::new("common_queue_depth", "Requests waiting for a generation slot.").expect("static metric");
        let inflight = IntGauge::new("common_inflight", "Generations currently running.").expect("static metric");
        let remote_requests = IntCounterVec::new(
            Opts::new(
                "common_remote_requests_total",
                "Requests sent to a paired device for remote execution, by peer short label.",
            ),
            &["peer"],
        )
        .expect("static metric");
        for m in [
            Box::new(requests.clone()) as Box<dyn prometheus::core::Collector>,
            Box::new(rejected.clone()),
            Box::new(tokens_generated.clone()),
            Box::new(prompt_tokens.clone()),
            Box::new(ttft.clone()),
            Box::new(decode_tps.clone()),
            Box::new(queue_depth.clone()),
            Box::new(inflight.clone()),
            Box::new(remote_requests.clone()),
        ] {
            registry.register(m).expect("unique metric names");
        }
        Self {
            registry,
            requests,
            rejected,
            tokens_generated,
            prompt_tokens,
            ttft,
            decode_tps,
            queue_depth,
            inflight,
            remote_requests,
        }
    }

    /// Record a finished generation.
    pub fn record_done(&self, usage: &Usage, timings: &Timings) {
        self.tokens_generated.inc_by(u64::from(usage.completion_tokens));
        self.prompt_tokens.inc_by(u64::from(usage.prompt_tokens));
        if let Some(ms) = timings.time_to_first_token_ms {
            self.ttft.observe(ms / 1000.0);
        }
        if let Some(tps) = timings.decode_tokens_per_second {
            self.decode_tps.set(tps);
        }
    }

    /// Prometheus text exposition, with the live queue gauges filled in.
    pub fn render(&self, queued: usize, inflight: usize) -> String {
        self.queue_depth.set(queued as i64);
        self.inflight.set(inflight as i64);
        let mut buf = Vec::new();
        // Encoding into a Vec cannot fail; an empty page is the only fallback anyway.
        let _ = TextEncoder::new().encode(&self.registry.gather(), &mut buf);
        String::from_utf8(buf).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_renders() {
        let m = Metrics::new();
        m.requests.with_label_values(&["/health", "200"]).inc();
        m.remote_requests.with_label_values(&["abcd-ef01-2345"]).inc();
        m.record_done(
            &Usage {
                prompt_tokens: 10,
                completion_tokens: 32,
            },
            &Timings {
                time_to_first_token_ms: Some(12.0),
                decode_tokens_per_second: Some(400.0),
                ..Default::default()
            },
        );
        let text = m.render(2, 1);
        for needle in [
            "common_requests_total{route=\"/health\",status=\"200\"} 1",
            "common_tokens_generated_total 32",
            "common_prompt_tokens_total 10",
            "common_ttft_seconds_bucket{le=\"0.025\"} 1",
            "common_ttft_seconds_count 1",
            "common_decode_tokens_per_second 400",
            "common_queue_depth 2",
            "common_inflight 1",
            "# TYPE common_requests_rejected_total counter",
            "common_requests_rejected_total{reason=\"queue_full\"} 0",
            "common_requests_rejected_total{reason=\"queue_timeout\"} 0",
            "common_remote_requests_total{peer=\"abcd-ef01-2345\"} 1",
        ] {
            assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
        }
    }
}
