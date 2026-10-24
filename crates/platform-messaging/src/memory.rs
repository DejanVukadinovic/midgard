//! Deterministic in-memory adapter: API behaviour only (no durability, no ACLs).

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;

use crate::{
    Acker, BusError, Consumer, ConsumerConfig, Delivery, Message, MessageBus, PubAck, backoff_for,
    destination_matches, valid_publish_destination,
};

struct Stored {
    destination: String,
    msg: Message,
}

struct CState {
    cfg: ConsumerConfig,
    cursor: usize,
    /// seq -> (ack deadline, attempt)
    in_flight: HashMap<usize, (Instant, u32)>,
    /// (ready_at, seq, attempts so far)
    redeliver: VecDeque<(Instant, usize, u32)>,
    exhausted: BTreeSet<usize>,
}

#[derive(Default)]
struct State {
    log: Vec<Stored>,
    ids: BTreeMap<String, u64>,
    consumers: HashMap<String, CState>,
}

#[derive(Clone, Default)]
pub struct MemoryBus {
    st: Arc<Mutex<State>>,
}

impl MemoryBus {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sequences that exhausted `max_deliver` without being acked (adapter-visible failure).
    pub fn exhausted(&self, consumer: &str) -> Vec<u64> {
        let s = self.st.lock().unwrap();
        s.consumers
            .get(consumer)
            .map(|c| c.exhausted.iter().map(|q| *q as u64 + 1).collect())
            .unwrap_or_default()
    }
}

#[async_trait]
impl MessageBus for MemoryBus {
    async fn publish(&self, destination: &str, msg: Message) -> Result<PubAck, BusError> {
        if !valid_publish_destination(destination) {
            return Err(BusError::InvalidDestination(destination.into()));
        }
        let mut s = self.st.lock().unwrap();
        if let Some(seq) = s.ids.get(&msg.id) {
            return Ok(PubAck {
                sequence: *seq,
                duplicate: true,
            });
        }
        s.log.push(Stored {
            destination: destination.into(),
            msg: msg.clone(),
        });
        let seq = s.log.len() as u64;
        s.ids.insert(msg.id, seq);
        Ok(PubAck {
            sequence: seq,
            duplicate: false,
        })
    }

    async fn consumer(&self, cfg: ConsumerConfig) -> Result<Box<dyn Consumer>, BusError> {
        let mut s = self.st.lock().unwrap();
        s.consumers.entry(cfg.name.clone()).or_insert(CState {
            cfg: cfg.clone(),
            cursor: 0,
            in_flight: HashMap::new(),
            redeliver: VecDeque::new(),
            exhausted: BTreeSet::new(),
        });
        Ok(Box::new(MemConsumer {
            name: cfg.name,
            st: self.st.clone(),
        }))
    }
}

struct MemConsumer {
    name: String,
    st: Arc<Mutex<State>>,
}

impl MemConsumer {
    fn poll(&self) -> Option<Delivery> {
        let mut guard = self.st.lock().unwrap();
        let State { log, consumers, .. } = &mut *guard;
        let c = consumers.get_mut(&self.name)?;
        let now = Instant::now();
        // Ack timeouts become redeliveries (or exhaustion).
        let expired: Vec<(usize, u32)> = c
            .in_flight
            .iter()
            .filter(|(_, (dl, _))| *dl <= now)
            .map(|(q, (_, a))| (*q, *a))
            .collect();
        for (seq, attempt) in expired {
            c.in_flight.remove(&seq);
            requeue(c, seq, attempt, now, None);
        }
        let mut pick = None;
        if let Some(i) = c.redeliver.iter().position(|(at, _, _)| *at <= now) {
            let (_, seq, attempts) = c.redeliver.remove(i).unwrap();
            pick = Some((seq, attempts + 1));
        } else {
            while c.cursor < log.len() {
                let seq = c.cursor;
                c.cursor += 1;
                if destination_matches(&c.cfg.destination, &log[seq].destination) {
                    pick = Some((seq, 1));
                    break;
                }
            }
        }
        let (seq, attempt) = pick?;
        c.in_flight.insert(seq, (now + c.cfg.ack_wait, attempt));
        Some(Delivery::new(
            log[seq].destination.clone(),
            log[seq].msg.clone(),
            attempt,
            Box::new(MemAcker {
                name: self.name.clone(),
                seq,
                st: self.st.clone(),
            }),
        ))
    }
}

fn requeue(c: &mut CState, seq: usize, attempt: u32, now: Instant, delay: Option<Duration>) {
    if attempt >= c.cfg.max_deliver {
        c.exhausted.insert(seq);
    } else {
        let d = delay
            .or_else(|| backoff_for(&c.cfg, attempt))
            .unwrap_or_default();
        c.redeliver.push_back((now + d, seq, attempt));
    }
}

#[async_trait]
impl Consumer for MemConsumer {
    async fn next(&mut self, wait: Duration) -> Result<Option<Delivery>, BusError> {
        let end = Instant::now() + wait;
        loop {
            if let Some(d) = self.poll() {
                return Ok(Some(d));
            }
            if Instant::now() >= end {
                return Ok(None);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

struct MemAcker {
    name: String,
    seq: usize,
    st: Arc<Mutex<State>>,
}

#[async_trait]
impl Acker for MemAcker {
    async fn ack(&self) -> Result<(), BusError> {
        let mut s = self.st.lock().unwrap();
        if let Some(c) = s.consumers.get_mut(&self.name) {
            c.in_flight.remove(&self.seq);
        }
        Ok(())
    }
    async fn nak(&self, delay: Option<Duration>) -> Result<(), BusError> {
        let mut s = self.st.lock().unwrap();
        if let Some(c) = s.consumers.get_mut(&self.name)
            && let Some((_, attempt)) = c.in_flight.remove(&self.seq)
        {
            requeue(c, self.seq, attempt, Instant::now(), delay);
        }
        Ok(())
    }
    async fn term(&self) -> Result<(), BusError> {
        self.ack().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn memory_adapter_conforms() {
        crate::conformance::run_all(&MemoryBus::new(), "mem").await;
    }

    #[tokio::test]
    async fn exhausted_unacked_messages_are_reported() {
        let bus = MemoryBus::new();
        bus.publish("t.x", crate::conformance::msg("e1"))
            .await
            .unwrap();
        let mut cf = crate::conformance::cfg("t", "e", "t.x");
        cf.max_deliver = 2;
        cf.ack_wait = Duration::from_millis(20);
        let mut c = bus.consumer(cf).await.unwrap();
        while c.next(Duration::from_millis(300)).await.unwrap().is_some() {}
        assert_eq!(bus.exhausted("t-e"), vec![1]);
    }
}
