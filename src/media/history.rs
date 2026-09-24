use std::collections::VecDeque;

pub(crate) const AUDIO_HISTORY: usize = 128;
pub(crate) const VIDEO_HISTORY: usize = 512;
pub(crate) const MAX_RETRANSMIT_PER_NACK: usize = 64;

pub(crate) struct SendHistory {
    packets: VecDeque<(u16, Vec<u8>)>,
    capacity: usize,
}

impl SendHistory {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            packets: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub(crate) fn record(&mut self, seq: u16, protected: &[u8]) {
        if self.packets.len() == self.capacity {
            self.packets.pop_front();
        }
        self.packets.push_back((seq, protected.to_vec()));
    }

    pub(crate) fn get(&self, seq: u16) -> Option<&[u8]> {
        self.packets
            .iter()
            .rev()
            .find(|(s, _)| *s == seq)
            .map(|(_, bytes)| bytes.as_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_the_most_recent_packets() {
        let mut history = SendHistory::new(4);
        for seq in 0..10u16 {
            history.record(seq, &[u8::try_from(seq).unwrap(); 3]);
        }
        assert!(history.get(5).is_none());
        assert_eq!(history.get(6), Some(&[6u8, 6, 6][..]));
        assert_eq!(history.get(9), Some(&[9u8, 9, 9][..]));
    }

    #[test]
    fn a_wrapped_sequence_number_returns_the_newest_packet() {
        let mut history = SendHistory::new(8);
        history.record(1, &[1]);
        history.record(1, &[2]);
        assert_eq!(history.get(1), Some(&[2u8][..]));
    }
}
