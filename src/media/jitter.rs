//! A per-SSRC jitter buffer: reorder RTP packets, emit in sequence order, and
//! drop packets that arrive after their playout deadline.

use rtp::seq::seq_diff;
use std::collections::BTreeMap;

/// Unwraps 16-bit RTP sequence numbers into a monotonic 64-bit index, tolerant
/// of reordering and rollover.
#[derive(Debug, Default, Clone)]
struct SeqUnwrapper {
    started: bool,
    last: u16,
    roc: u32,
}

impl SeqUnwrapper {
    fn unwrap(&mut self, seq: u16) -> u64 {
        if !self.started {
            self.started = true;
            self.last = seq;
            self.roc = 0;
            return u64::from(seq);
        }
        let last_ext = (i64::from(self.roc) << 16) | i64::from(self.last);
        let diff = i64::from(seq_diff(self.last, seq));
        let ext = (last_ext + diff).max(0) as u64;
        if diff > 0 {
            self.last = seq;
            self.roc = (ext >> 16) as u32;
        }
        ext
    }
}

/// A reordering jitter buffer keyed by extended sequence number.
#[derive(Debug)]
pub struct JitterBuffer {
    target: usize,
    unwrap: SeqUnwrapper,
    entries: BTreeMap<u64, Vec<u8>>,
    next_out: Option<u64>,
    dropped_late: u64,
}

impl JitterBuffer {
    /// Create a buffer that builds up `target` packets of depth before
    /// releasing, to absorb reordering and jitter.
    #[must_use]
    pub fn new(target: usize) -> Self {
        Self {
            target: target.max(1),
            unwrap: SeqUnwrapper::default(),
            entries: BTreeMap::new(),
            next_out: None,
            dropped_late: 0,
        }
    }

    /// Create a buffer sized for `target_ms` of media given `frame_ms`-spaced
    /// packets (e.g. 50 ms / 20 ms for Opus → 2 packets).
    #[must_use]
    pub fn with_target_ms(target_ms: u32, frame_ms: u32) -> Self {
        let frames = (target_ms / frame_ms.max(1)).max(1) as usize;
        Self::new(frames)
    }

    /// Insert a packet (full RTP bytes) with sequence number `seq`. Returns
    /// `false` if the packet arrived after its playout deadline and was
    /// dropped.
    pub fn push(&mut self, seq: u16, data: Vec<u8>) -> bool {
        let ext = self.unwrap.unwrap(seq);
        if let Some(next) = self.next_out {
            if ext < next {
                self.dropped_late += 1;
                return false;
            }
        }
        self.entries.insert(ext, data);
        true
    }

    /// Pop the next in-order packet once the buffer has reached its target
    /// depth. Gaps are skipped (treated as lost) once the buffer is full.
    pub fn pop(&mut self) -> Option<Vec<u8>> {
        if self.entries.len() < self.target {
            return None;
        }
        self.pop_inner()
    }

    /// Flush a packet regardless of depth (used when draining at shutdown).
    pub fn flush_one(&mut self) -> Option<Vec<u8>> {
        self.pop_inner()
    }

    fn pop_inner(&mut self) -> Option<Vec<u8>> {
        loop {
            let &first = self.entries.keys().next()?;
            let expect = self.next_out.unwrap_or(first);
            if first < expect {
                // Stale duplicate/reorder past the deadline.
                self.entries.remove(&first);
                self.dropped_late += 1;
                continue;
            }
            // `first == expect` is in-order; `first > expect` skips a lost gap.
            self.next_out = Some(first + 1);
            return self.entries.remove(&first);
        }
    }

    /// Current buffered packet count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the buffer is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Number of packets dropped for arriving too late.
    #[must_use]
    pub fn dropped_late(&self) -> u64 {
        self.dropped_late
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_order_recovery() {
        let mut jb = JitterBuffer::new(3);
        jb.push(3, vec![3]);
        jb.push(1, vec![1]);
        jb.push(2, vec![2]);
        // pop() releases while at target depth, then flush_one drains the tail.
        assert_eq!(jb.pop(), Some(vec![1]));
        assert_eq!(jb.flush_one(), Some(vec![2]));
        assert_eq!(jb.flush_one(), Some(vec![3]));
    }

    #[test]
    fn late_packet_is_dropped() {
        let mut jb = JitterBuffer::new(1);
        jb.push(10, vec![10]);
        jb.push(11, vec![11]);
        assert_eq!(jb.pop(), Some(vec![10]));
        // 9 arrives after 10 was already played out → dropped.
        assert!(!jb.push(9, vec![9]));
        assert_eq!(jb.dropped_late(), 1);
    }

    #[test]
    fn sequence_wrap() {
        let mut jb = JitterBuffer::new(4);
        jb.push(65534, vec![1]);
        jb.push(65535, vec![2]);
        jb.push(0, vec![3]);
        jb.push(1, vec![4]);
        // Across the 16-bit rollover the unwrapper keeps order: 1,2,3,4.
        assert_eq!(jb.pop(), Some(vec![1]));
        assert_eq!(jb.flush_one(), Some(vec![2]));
        assert_eq!(jb.flush_one(), Some(vec![3]));
        assert_eq!(jb.flush_one(), Some(vec![4]));
    }
}
