use anyhow::{Context, Result};
use prometheus::{Encoder, IntCounterVec, IntGauge, Registry, TextEncoder};

#[derive(Clone)]
pub struct Metrics {
    registry: Registry,
    received_total: IntCounterVec,
    forwarded_total: IntCounterVec,
    dropped_total: IntCounterVec,
    queue_depth: IntGauge,
    dlq_depth: IntGauge,
}

impl Metrics {
    pub fn new() -> Result<Self> {
        let registry = Registry::new();

        let received_total = IntCounterVec::new(
            prometheus::Opts::new(
                "webhook_relay_events_received_total",
                "Total webhook events received by relay.",
            ),
            &["source"],
        )
        .context("create received_total metric")?;

        let forwarded_total = IntCounterVec::new(
            prometheus::Opts::new(
                "webhook_relay_events_forwarded_total",
                "Total webhook events successfully forwarded to OpenClaw.",
            ),
            &["source"],
        )
        .context("create forwarded_total metric")?;

        let dropped_total = IntCounterVec::new(
            prometheus::Opts::new(
                "webhook_relay_events_dropped_total",
                "Total webhook events dropped before forwarding.",
            ),
            &["source", "reason"],
        )
        .context("create dropped_total metric")?;

        let queue_depth = IntGauge::new("webhook_relay_queue_depth", "Pending queue depth.")
            .context("create queue_depth metric")?;
        let dlq_depth = IntGauge::new("webhook_relay_dlq_depth", "DLQ depth.")
            .context("create dlq_depth metric")?;

        registry
            .register(Box::new(received_total.clone()))
            .context("register received_total")?;
        registry
            .register(Box::new(forwarded_total.clone()))
            .context("register forwarded_total")?;
        registry
            .register(Box::new(dropped_total.clone()))
            .context("register dropped_total")?;
        registry
            .register(Box::new(queue_depth.clone()))
            .context("register queue_depth")?;
        registry
            .register(Box::new(dlq_depth.clone()))
            .context("register dlq_depth")?;

        Ok(Self {
            registry,
            received_total,
            forwarded_total,
            dropped_total,
            queue_depth,
            dlq_depth,
        })
    }

    pub fn inc_received(&self, source: &str) {
        self.received_total.with_label_values(&[source]).inc();
    }

    pub fn inc_forwarded(&self, source: &str) {
        self.forwarded_total.with_label_values(&[source]).inc();
    }

    pub fn inc_dropped(&self, source: &str, reason: &str) {
        self.dropped_total
            .with_label_values(&[source, reason])
            .inc();
    }

    pub fn set_queue_depth(&self, count: usize) {
        self.queue_depth.set(count as i64);
    }

    pub fn set_dlq_depth(&self, count: usize) {
        self.dlq_depth.set(count as i64);
    }

    pub fn render(&self) -> Result<String> {
        let metric_families = self.registry.gather();
        let encoder = TextEncoder::new();
        let mut buffer = Vec::new();
        encoder
            .encode(&metric_families, &mut buffer)
            .context("encode metrics")?;
        String::from_utf8(buffer).context("metrics text is valid utf-8")
    }
}
