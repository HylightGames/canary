//! Session sequence gate: each message applied at most once.
//!
//! The reliable ordered stream already delivers bytes in order, but sessions
//! must survive reconnects, resyncs, and (later) additional lanes where the
//! same logical message can legitimately arrive twice. [`SequenceGate`]
//! requires strictly increasing [`NetSequence`](crate::ids::NetSequence)
//! values: duplicates and reordered values are rejected, never applied twice
//! (ADR 0027, point 10).
//!
//! A forward jump (gap) is accepted at this layer. Detecting a missing base
//! sequence and forcing a full resync is the delta layer's job (WP3); the
//! gate's contract is only "never apply twice", not "never skip".

use crate::error::NetError;
use crate::ids::NetSequence;

/// Tracks the highest accepted session sequence and rejects replays.
#[derive(Debug, Clone, Copy, Default)]
pub struct SequenceGate {
    /// Highest sequence accepted so far, if any message has been accepted.
    last_accepted: Option<NetSequence>,
}

impl SequenceGate {
    /// A gate that has accepted nothing yet; the first message of any
    /// sequence is welcome.
    #[must_use]
    pub fn new() -> Self {
        Self {
            last_accepted: None,
        }
    }

    /// Highest sequence accepted so far, or `None` before the first message.
    #[must_use]
    pub fn last_accepted(&self) -> Option<NetSequence> {
        self.last_accepted
    }

    /// Checks `sequence` against the gate, advancing it on success.
    ///
    /// Accepts strictly increasing values (gaps allowed — see module docs).
    /// An equal value fails as [`NetError::DuplicateSequence`]; a lower value
    /// fails as [`NetError::ReorderedSequence`]. Rejected values do not
    /// advance the gate.
    pub fn check(&mut self, sequence: NetSequence) -> Result<(), NetError> {
        match self.last_accepted {
            None => {
                self.last_accepted = Some(sequence);
                Ok(())
            }
            Some(last) if sequence.0 > last.0 => {
                self.last_accepted = Some(sequence);
                Ok(())
            }
            Some(last) if sequence.0 == last.0 => Err(NetError::DuplicateSequence {
                sequence: sequence.0,
            }),
            Some(last) => Err(NetError::ReorderedSequence {
                sequence: sequence.0,
                last_accepted: last.0,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_order_sequences_advance_the_gate() {
        let mut gate = SequenceGate::new();
        assert_eq!(gate.last_accepted(), None);
        gate.check(NetSequence(1)).expect("first");
        gate.check(NetSequence(2)).expect("next");
        assert_eq!(gate.last_accepted(), Some(NetSequence(2)));
    }

    #[test]
    fn duplicate_sequence_rejected_and_gate_holds() {
        let mut gate = SequenceGate::new();
        gate.check(NetSequence(5)).expect("first");
        assert!(matches!(
            gate.check(NetSequence(5)),
            Err(NetError::DuplicateSequence { sequence: 5 })
        ));
        assert_eq!(gate.last_accepted(), Some(NetSequence(5)));
        gate.check(NetSequence(6)).expect("gate still advances");
    }

    #[test]
    fn reordered_sequence_rejected_and_gate_holds() {
        let mut gate = SequenceGate::new();
        gate.check(NetSequence(10)).expect("first");
        assert!(matches!(
            gate.check(NetSequence(7)),
            Err(NetError::ReorderedSequence {
                sequence: 7,
                last_accepted: 10
            })
        ));
        assert_eq!(gate.last_accepted(), Some(NetSequence(10)));
    }

    #[test]
    fn forward_gap_accepted_for_delta_layer_resync() {
        // Skips are the delta layer's resync trigger, not the gate's
        // rejection: the gate only promises never-apply-twice.
        let mut gate = SequenceGate::new();
        gate.check(NetSequence(4)).expect("first");
        gate.check(NetSequence(9)).expect("gap accepted");
        assert_eq!(gate.last_accepted(), Some(NetSequence(9)));
    }
}
