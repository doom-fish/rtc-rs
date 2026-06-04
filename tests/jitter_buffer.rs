//! The jitter buffer reorders packets, releases them in sequence once it has
//! reached its target depth, and counts packets that arrive too late.

use rtc::media::JitterBuffer;

fn pkt(n: u8) -> Vec<u8> {
    vec![n]
}

#[test]
fn reorders_then_releases_in_order() {
    let mut jb = JitterBuffer::new(3);

    // Arrive out of order: 1, 3, 2.
    assert!(jb.push(1, pkt(1)));
    assert!(jb.push(3, pkt(3)));
    assert_eq!(jb.pop(), None, "below target depth, nothing released yet");
    assert!(jb.push(2, pkt(2)));

    // At target depth, pop releases the oldest in order; the tail is drained
    // with flush_one (which ignores the depth gate).
    assert_eq!(jb.pop(), Some(pkt(1)));
    assert_eq!(jb.flush_one(), Some(pkt(2)));
    assert_eq!(jb.flush_one(), Some(pkt(3)));
    assert_eq!(jb.flush_one(), None);
}

#[test]
fn skips_gaps_once_full() {
    let mut jb = JitterBuffer::new(2);
    // Sequence 5 is lost; 4, 6, 7 arrive.
    jb.push(4, pkt(4));
    jb.push(6, pkt(6));
    jb.push(7, pkt(7));

    assert_eq!(jb.pop(), Some(pkt(4)));
    // The gap at 5 is skipped; next in-order is 6.
    assert_eq!(jb.pop(), Some(pkt(6)));
    assert_eq!(jb.flush_one(), Some(pkt(7)));
}

#[test]
fn counts_late_arrivals() {
    let mut jb = JitterBuffer::new(1);
    jb.push(10, pkt(10));
    assert_eq!(jb.pop(), Some(pkt(10)));
    // 9 is now past the playout deadline.
    assert!(!jb.push(9, pkt(9)));
    assert_eq!(jb.dropped_late(), 1);
}
