//! Adapter conformance suite: every `MessageBus` adapter must pass `run_all`.
//! `prefix` must be unique per run and covered by the adapter's provisioned destinations.

use std::{collections::BTreeMap, time::Duration};

use crate::{ConsumerConfig, Message, MessageBus, settle};

const WAIT: Duration = Duration::from_millis(1500);

pub fn msg(id: &str) -> Message {
    Message {
        id: id.into(),
        correlation_id: format!("corr-{id}"),
        traceparent: Some("00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".into()),
        headers: BTreeMap::new(),
        payload: format!("{{\"id\":\"{id}\",\"value\":\"v\"}}").into_bytes(),
    }
}

pub fn cfg(prefix: &str, name: &str, dest: &str) -> ConsumerConfig {
    ConsumerConfig {
        name: format!("{prefix}-{name}"),
        destination: dest.into(),
        max_deliver: 3,
        ack_wait: Duration::from_millis(400),
        backoff: vec![Duration::from_millis(50)],
        dead_letter: Some(format!("{prefix}.dlq")),
    }
}

pub async fn run_all(bus: &dyn MessageBus, prefix: &str) {
    publish_ack_and_dedupe(bus, prefix).await;
    ack_removes_message(bus, prefix).await;
    nak_redelivers(bus, prefix).await;
    unacked_message_is_redelivered(bus, prefix).await;
    poison_message_is_isolated(bus, prefix).await;
}

async fn publish_ack_and_dedupe(bus: &dyn MessageBus, p: &str) {
    let d = format!("{p}.dedupe");
    let a = bus.publish(&d, msg("m1")).await.expect("publish acked");
    assert!(!a.duplicate);
    let b = bus.publish(&d, msg("m1")).await.expect("duplicate publish acked");
    assert!(b.duplicate, "same message id must be de-duplicated");
    assert!(bus.publish(&format!("{p}.>"), msg("m2")).await.is_err(), "wildcards are not publishable");
    let mut c = bus.consumer(cfg(p, "dedupe", &d)).await.unwrap();
    let got = c.next(WAIT).await.unwrap().expect("delivered");
    assert_eq!(got.message.id, "m1");
    assert_eq!(got.message.correlation_id, "corr-m1");
    assert_eq!(got.message.traceparent.as_deref(), Some("00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"));
    assert_eq!(got.attempt, 1);
    got.ack().await.unwrap();
    assert!(c.next(Duration::from_millis(300)).await.unwrap().is_none(), "only one copy stored");
}

async fn ack_removes_message(bus: &dyn MessageBus, p: &str) {
    let d = format!("{p}.ack");
    bus.publish(&d, msg("a1")).await.unwrap();
    let mut c = bus.consumer(cfg(p, "ack", &d)).await.unwrap();
    c.next(WAIT).await.unwrap().expect("delivered").ack().await.unwrap();
    assert!(c.next(Duration::from_millis(700)).await.unwrap().is_none(), "acked message must not return");
}

async fn nak_redelivers(bus: &dyn MessageBus, p: &str) {
    let d = format!("{p}.nak");
    bus.publish(&d, msg("n1")).await.unwrap();
    let mut c = bus.consumer(cfg(p, "nak", &d)).await.unwrap();
    let first = c.next(WAIT).await.unwrap().expect("delivered");
    assert_eq!(first.attempt, 1);
    first.nak(Some(Duration::from_millis(50))).await.unwrap();
    let again = c.next(WAIT).await.unwrap().expect("redelivered after nak");
    assert_eq!((again.message.id.as_str(), again.attempt), ("n1", 2));
    again.ack().await.unwrap();
}

async fn unacked_message_is_redelivered(bus: &dyn MessageBus, p: &str) {
    let d = format!("{p}.crash");
    bus.publish(&d, msg("c1")).await.unwrap();
    let mut c = bus.consumer(cfg(p, "crash", &d)).await.unwrap();
    let first = c.next(WAIT).await.unwrap().expect("delivered");
    drop(first); // consumer "crashes" before acking
    let again = c.next(Duration::from_secs(3)).await.unwrap().expect("redelivered after ack_wait");
    assert_eq!((again.message.id.as_str(), again.attempt), ("c1", 2));
    again.ack().await.unwrap();
}

async fn poison_message_is_isolated(bus: &dyn MessageBus, p: &str) {
    let d = format!("{p}.poison");
    let cf = cfg(p, "poison", &d);
    bus.publish(&d, msg("x1")).await.unwrap();
    let mut c = bus.consumer(cf.clone()).await.unwrap();
    let mut attempts = 0;
    while let Some(del) = c.next(Duration::from_secs(2)).await.unwrap() {
        attempts += 1;
        settle(bus, &cf, del, Err("cannot parse".into())).await.unwrap();
        if attempts > cf.max_deliver {
            break;
        }
    }
    assert_eq!(attempts, cf.max_deliver, "bounded redelivery");
    let mut dlq = bus.consumer(cfg(p, "dlq", &format!("{p}.dlq"))).await.unwrap();
    let dead = dlq.next(WAIT).await.unwrap().expect("poison message isolated in dead-letter destination");
    assert_eq!(dead.message.headers.get("midgard-dead-reason").map(String::as_str), Some("cannot parse"));
    assert_eq!(dead.message.headers.get("midgard-original-destination"), Some(&d));
    dead.ack().await.unwrap();
}
