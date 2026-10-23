//! NATS JetStream adapter (the initial MQ backend).
//!
//! Mapping: logical destination -> subject (identical string); one JetStream stream with
//! file storage covers a set of subject patterns; a MIDGARD consumer -> a durable explicit-ack
//! pull consumer; `Message::id` -> `Nats-Msg-Id` (de-duplicated within `duplicate_window`).
//!
//! Stream and consumer *provisioning* needs JetStream admin permissions; a runtime identity
//! should only publish / consume (`provision = false`, consumers pre-created by deployment).

use std::{sync::Arc, time::Duration};

use async_nats::{
    HeaderMap,
    header::IntoHeaderName,
    jetstream::{
        self, AckKind,
        consumer::{AckPolicy, pull},
        stream,
    },
};
use async_trait::async_trait;
use futures_util::StreamExt;
use tokio::sync::Mutex;

use crate::{
    Acker, BusError, Consumer, ConsumerConfig, Delivery, Message, MessageBus, PubAck,
    valid_publish_destination,
};

const H_CORRELATION: &str = "Midgard-Correlation-Id";
const H_TRACEPARENT: &str = "traceparent";
const H_CUSTOM_PREFIX: &str = "Midgard-H-";

#[derive(Debug, Clone)]
pub struct NatsConfig {
    pub url: String,
    pub user: Option<String>,
    pub password: Option<String>,
    pub stream: String,
    /// Subject patterns the stream captures, e.g. `example.>`.
    pub subjects: Vec<String>,
    pub duplicate_window: Duration,
    /// Create/update the stream (needs JetStream admin permissions).
    pub provision: bool,
}

/// JetStream consumer names may not contain `.`, `*`, `>` or whitespace.
fn consumer_name(n: &str) -> String {
    n.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect()
}

pub struct NatsBus {
    provision: bool,
    js: jetstream::Context,
    stream: jetstream::stream::Stream,
}

fn broker<E: std::fmt::Display>(e: E) -> BusError {
    BusError::Broker(e.to_string())
}

impl NatsBus {
    pub async fn connect(cfg: NatsConfig) -> Result<Self, BusError> {
        let opts = match (&cfg.user, &cfg.password) {
            (Some(u), Some(p)) => async_nats::ConnectOptions::with_user_and_password(u.clone(), p.clone()),
            _ => async_nats::ConnectOptions::new(),
        };
        let client = opts.connect(&cfg.url).await.map_err(broker)?;
        let js = jetstream::new(client);
        let stream = if cfg.provision {
            js.get_or_create_stream(stream::Config {
                name: cfg.stream.clone(),
                subjects: cfg.subjects.clone(),
                storage: stream::StorageType::File,
                retention: stream::RetentionPolicy::Limits,
                duplicate_window: cfg.duplicate_window,
                ..Default::default()
            })
            .await
            .map_err(broker)?
        } else {
            js.get_stream(&cfg.stream).await.map_err(broker)?
        };
        Ok(Self { provision: cfg.provision, js, stream })
    }
}

#[async_trait]
impl MessageBus for NatsBus {
    async fn publish(&self, destination: &str, msg: Message) -> Result<PubAck, BusError> {
        if !valid_publish_destination(destination) {
            return Err(BusError::InvalidDestination(destination.into()));
        }
        let mut h = HeaderMap::new();
        h.insert(async_nats::header::NATS_MESSAGE_ID, msg.id.as_str());
        h.insert(H_CORRELATION, msg.correlation_id.as_str());
        if let Some(t) = &msg.traceparent {
            h.insert(H_TRACEPARENT, t.as_str());
        }
        for (k, v) in &msg.headers {
            h.insert(format!("{H_CUSTOM_PREFIX}{k}"), v.as_str());
        }
        // Both awaits matter: the first sends, the second waits for the durable JetStream ack.
        let ack = self
            .js
            .publish_with_headers(destination.to_string(), h, msg.payload.into())
            .await
            .map_err(|e| BusError::PublishFailed(e.to_string()))?
            .await
            .map_err(|e| BusError::PublishFailed(e.to_string()))?;
        Ok(PubAck { sequence: ack.sequence, duplicate: ack.duplicate })
    }

    async fn consumer(&self, cfg: ConsumerConfig) -> Result<Box<dyn Consumer>, BusError> {
        if !self.provision {
            // Runtime identities cannot create consumers; deployment pre-creates them.
            let c = self.stream.get_consumer(&consumer_name(&cfg.name)).await.map_err(broker)?;
            return Ok(Box::new(NatsConsumer { c }));
        }
        let c = self
            .stream
            .get_or_create_consumer(
                &consumer_name(&cfg.name),
                pull::Config {
                    durable_name: Some(consumer_name(&cfg.name)),
                    filter_subject: cfg.destination.clone(),
                    ack_policy: AckPolicy::Explicit,
                    max_deliver: i64::from(cfg.max_deliver),
                    ack_wait: cfg.ack_wait,
                    backoff: cfg.backoff.clone(),
                    ..Default::default()
                },
            )
            .await
            .map_err(broker)?;
        Ok(Box::new(NatsConsumer { c }))
    }
}

struct NatsConsumer {
    c: jetstream::consumer::PullConsumer,
}

#[async_trait]
impl Consumer for NatsConsumer {
    async fn next(&mut self, wait: Duration) -> Result<Option<Delivery>, BusError> {
        let mut batch = self
            .c
            .fetch()
            .max_messages(1)
            .expires(wait.max(Duration::from_millis(10)))
            .messages()
            .await
            .map_err(broker)?;
        let Some(m) = batch.next().await else {
            return Ok(None);
        };
        let m = m.map_err(broker)?;
        let attempt = m.info().map_err(broker)?.delivered.max(1) as u32;
        let hdr = |k: async_nats::HeaderName| {
            m.headers
                .as_ref()
                .and_then(|h| h.get(k))
                .map(|v| v.as_str().to_string())
        };
        let mut headers = std::collections::BTreeMap::new();
        if let Some(h) = &m.headers {
            for (k, v) in h.iter() {
                if let Some(name) = AsRef::<str>::as_ref(k).strip_prefix(H_CUSTOM_PREFIX)
                    && let Some(v) = v.first()
                {
                    headers.insert(name.to_string(), v.as_str().to_string());
                }
            }
        }
        let message = Message {
            id: hdr(async_nats::header::NATS_MESSAGE_ID).unwrap_or_default(),
            correlation_id: hdr(IntoHeaderName::into_header_name(H_CORRELATION)).unwrap_or_default(),
            traceparent: hdr(IntoHeaderName::into_header_name(H_TRACEPARENT)),
            headers,
            payload: m.payload.to_vec(),
        };
        let destination = m.subject.to_string();
        Ok(Some(Delivery::new(
            destination,
            message,
            attempt,
            Box::new(NatsAcker(Arc::new(Mutex::new(m)))),
        )))
    }
}

struct NatsAcker(Arc<Mutex<jetstream::Message>>);

#[async_trait]
impl Acker for NatsAcker {
    async fn ack(&self) -> Result<(), BusError> {
        self.0.lock().await.double_ack().await.map_err(broker)
    }
    async fn nak(&self, delay: Option<Duration>) -> Result<(), BusError> {
        self.0.lock().await.ack_with(AckKind::Nak(delay)).await.map_err(broker)
    }
    async fn term(&self) -> Result<(), BusError> {
        self.0.lock().await.ack_with(AckKind::Term).await.map_err(broker)
    }
}
