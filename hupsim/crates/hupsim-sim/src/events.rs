//! Deterministic future-event queue (min-heap on scheduled time, FIFO within
//! ties via a monotonically increasing sequence number).

use hupsim_core::ids::{PatientId, RoomId};
use hupsim_core::patient::Patient;
use serde::{Deserialize, Serialize};
use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SimEventKind {
    Admission {
        room: RoomId,
        patient: Box<Patient>,
    },
    Discharge {
        room: RoomId,
        /// When set, the discharge only fires if this patient is still in the
        /// room. Guards room-addressed events against the patient having been
        /// moved/discharged in the meantime (EP-10: boarding transfers and
        /// concurrent flow processes make that a real interleaving).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect_patient: Option<PatientId>,
    },
    Transfer {
        from: RoomId,
        to: RoomId,
    },
    AcuityChange {
        room: RoomId,
        instability: f32,
    },
    /// Escape hatch for future model-specific events.
    Custom {
        tag: String,
        #[serde(default)]
        payload: serde_json::Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledEvent {
    pub at_min: f64,
    /// Tie-breaker preserving schedule order; assigned by the queue.
    pub seq: u64,
    pub kind: SimEventKind,
}

// Equality is deliberately (at_min, seq) only — NOT the payload — because
// `Ord`'s contract requires `a == b` exactly when `cmp == Equal`, and the heap
// orders solely on time+sequence. Do not use `==` to compare event *contents*
// across queues/files; two distinct events from different queues can share
// (at_min, seq).
impl PartialEq for ScheduledEvent {
    fn eq(&self, other: &Self) -> bool {
        self.seq == other.seq && self.at_min.total_cmp(&other.at_min) == Ordering::Equal
    }
}
impl Eq for ScheduledEvent {}

impl PartialOrd for ScheduledEvent {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ScheduledEvent {
    fn cmp(&self, other: &Self) -> Ordering {
        self.at_min
            .total_cmp(&other.at_min)
            .then(self.seq.cmp(&other.seq))
    }
}

#[derive(Debug, Default)]
pub struct EventQueue {
    heap: BinaryHeap<Reverse<ScheduledEvent>>,
    next_seq: u64,
}

impl EventQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn schedule(&mut self, at_min: f64, kind: SimEventKind) {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.heap.push(Reverse(ScheduledEvent { at_min, seq, kind }));
    }

    /// Pop every event due at or before `now_min`, in time-then-schedule order.
    pub fn pop_due(&mut self, now_min: f64) -> Vec<ScheduledEvent> {
        let mut due = Vec::new();
        while let Some(Reverse(ev)) = self.heap.peek() {
            if ev.at_min <= now_min {
                due.push(self.heap.pop().unwrap().0);
            } else {
                break;
            }
        }
        due
    }

    pub fn len(&self) -> usize {
        self.heap.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    pub fn clear(&mut self) {
        self.heap.clear();
    }

    /// Next scheduled time, if any (for UI display).
    pub fn peek_time(&self) -> Option<f64> {
        self.heap.peek().map(|Reverse(e)| e.at_min)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pops_in_time_order_with_fifo_ties() {
        let mut q = EventQueue::new();
        q.schedule(
            10.0,
            SimEventKind::Custom {
                tag: "b".into(),
                payload: serde_json::Value::Null,
            },
        );
        q.schedule(
            5.0,
            SimEventKind::Custom {
                tag: "a".into(),
                payload: serde_json::Value::Null,
            },
        );
        q.schedule(
            10.0,
            SimEventKind::Custom {
                tag: "c".into(),
                payload: serde_json::Value::Null,
            },
        );
        let due = q.pop_due(10.0);
        let tags: Vec<_> = due
            .iter()
            .map(|e| match &e.kind {
                SimEventKind::Custom { tag, .. } => tag.clone(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(tags, vec!["a", "b", "c"]);
        assert!(q.is_empty());
    }

    #[test]
    fn future_events_stay_queued() {
        let mut q = EventQueue::new();
        q.schedule(
            100.0,
            SimEventKind::Discharge {
                room: "r".into(),
                expect_patient: None,
            },
        );
        assert!(q.pop_due(99.9).is_empty());
        assert_eq!(q.len(), 1);
        assert_eq!(q.peek_time(), Some(100.0));
    }
}
