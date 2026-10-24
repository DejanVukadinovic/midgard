//! Broker-neutral messaging boundary for MIDGARD capability contracts (`protocol: mq`).
//!
//! The contract names a *logical destination*; adapters map it to broker constructs.
//!
//! Semantics, by who provides them:
//!
//! | Property | Provided by |
//! |---|---|
//! | `publish` returns only after the broker durably accepted the message, or an error | MIDGARD trait contract (adapters must conform) |
//! | At-least-once delivery, explicit ack, redelivery on nak / ack timeout | MIDGARD trait contract |
//! | Bounded redelivery (`max_deliver`) and dead-letter isolation via [`settle`] | MIDGARD trait contract |
//! | Publish-side de-duplication by `Message::id` | Adapter capability (NATS: bounded `duplicate_window`) |
//! | Durability across broker restart | Adapter capability (NATS: file storage; in-memory: none) |
//! | Publisher/subscriber authorization | Broker ACLs (never headers) |
//! | Idempotent handling of redeliveries; business ordering | Application logic |
//!
//! No global ordering and no exactly-once processing is claimed.

use std::{collections::BTreeMap, time::Duration};

use async_trait::async_trait;

pub mod conformance;
pub mod memory;
#[cfg(feature = "nats")]
pub mod nats;

#[derive(Debug, thiserror::Error)]
pub enum BusError {
    #[error("publish was not acknowledged by the broker: {0}")]
    PublishFailed(String),
    #[error("broker error: {0}")]
    Broker(String),
    #[error("invalid destination `{0}`")]
    InvalidDestination(String),
}

/// A message. `id` is the application-level idempotency key used for publish de-duplication;
/// `correlation_id` and `traceparent` (W3C) propagate request context. Tenant/application
/// identity must come from the authenticated broker identity, not from `headers`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub id: String,
    pub correlation_id: String,
    pub traceparent: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PubAck {
    pub sequence: u64,
    /// The broker recognised `Message::id` and did not store a second copy.
    pub duplicate: bool,
}

#[derive(Debug, Clone)]
pub struct ConsumerConfig {
    /// Durable consumer name: delivery state survives consumer restarts.
    pub name: String,
    /// Destination filter. A trailing `>` matches one or more further tokens.
    pub destination: String,
    pub max_deliver: u32,
    pub ack_wait: Duration,
    /// Redelivery delay after `nak`/timeout, indexed by attempt (last value repeats).
    pub backoff: Vec<Duration>,
    /// Where [`settle`] isolates a message that exhausted `max_deliver`.
    pub dead_letter: Option<String>,
}

#[async_trait]
pub trait Acker: Send + Sync {
    async fn ack(&self) -> Result<(), BusError>;
    async fn nak(&self, delay: Option<Duration>) -> Result<(), BusError>;
    async fn term(&self) -> Result<(), BusError>;
}

pub struct Delivery {
    pub destination: String,
    pub message: Message,
    /// 1 on first delivery.
    pub attempt: u32,
    acker: Box<dyn Acker>,
}

impl Delivery {
    pub fn new(destination: String, message: Message, attempt: u32, acker: Box<dyn Acker>) -> Self {
        Self {
            destination,
            message,
            attempt,
            acker,
        }
    }
    pub async fn ack(self) -> Result<(), BusError> {
        self.acker.ack().await
    }
    pub async fn nak(self, delay: Option<Duration>) -> Result<(), BusError> {
        self.acker.nak(delay).await
    }
    pub async fn term(self) -> Result<(), BusError> {
        self.acker.term().await
    }
}

#[async_trait]
pub trait Consumer: Send {
    /// Next delivery, or `None` if nothing arrived within `wait`.
    async fn next(&mut self, wait: Duration) -> Result<Option<Delivery>, BusError>;
}

#[async_trait]
pub trait MessageBus: Send + Sync {
    async fn publish(&self, destination: &str, msg: Message) -> Result<PubAck, BusError>;
    async fn consumer(&self, cfg: ConsumerConfig) -> Result<Box<dyn Consumer>, BusError>;
}

pub fn backoff_for(cfg: &ConsumerConfig, attempt: u32) -> Option<Duration> {
    let i = (attempt.saturating_sub(1)) as usize;
    cfg.backoff.get(i).or(cfg.backoff.last()).copied()
}

/// Record the outcome of handling a delivery: ack on success; nak (with backoff) on failure;
/// on the final attempt publish to the dead-letter destination (with the failure reason and the
/// original destination) and terminate, so a poison message cannot loop forever.
pub async fn settle(
    bus: &dyn MessageBus,
    cfg: &ConsumerConfig,
    d: Delivery,
    outcome: Result<(), String>,
) -> Result<(), BusError> {
    match outcome {
        Ok(()) => d.ack().await,
        Err(reason) if d.attempt >= cfg.max_deliver => {
            if let Some(dlq) = &cfg.dead_letter {
                let mut m = d.message.clone();
                // Distinct id: the original id would be de-duplicated against the original publish.
                m.id = format!("dlq-{}", m.id);
                m.headers.insert("midgard-dead-reason".into(), reason);
                m.headers
                    .insert("midgard-original-destination".into(), d.destination.clone());
                m.headers
                    .insert("midgard-attempts".into(), d.attempt.to_string());
                bus.publish(dlq, m).await?;
            }
            d.term().await
        }
        Err(_) => {
            let delay = backoff_for(cfg, d.attempt);
            d.nak(delay).await
        }
    }
}

/// Subject-style matching: tokens split on `.`; `*` matches one token, trailing `>` one or more.
pub fn destination_matches(filter: &str, destination: &str) -> bool {
    let f: Vec<&str> = filter.split('.').collect();
    let d: Vec<&str> = destination.split('.').collect();
    for (i, ft) in f.iter().enumerate() {
        if *ft == ">" {
            return i == f.len() - 1 && d.len() > i;
        }
        match d.get(i) {
            Some(dt) if *ft == "*" || ft == dt => {}
            _ => return false,
        }
    }
    f.len() == d.len()
}

/// Logical destinations are plain dotted tokens; wildcards are only for consumer filters.
pub fn valid_publish_destination(d: &str) -> bool {
    !d.is_empty()
        && d.len() <= 255
        && d.split('.').all(|t| {
            !t.is_empty()
                && t.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching() {
        assert!(destination_matches("a.b", "a.b"));
        assert!(!destination_matches("a.b", "a.b.c"));
        assert!(destination_matches("a.>", "a.b.c"));
        assert!(!destination_matches("a.>", "a"));
        assert!(destination_matches("a.*.c", "a.x.c"));
        assert!(!valid_publish_destination("a.>"));
        assert!(!valid_publish_destination("a..b"));
    }
}
