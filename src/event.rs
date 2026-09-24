//! Events delivered to the application over the [`crate::PeerConnection`]
//! event channel.

use crate::error::RtcError;
use crate::state::{ConnectionState, DtlsState, IceConnectionState};
use crate::stats::Stats;
use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// The kind of a media track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrackKind {
    /// Opus audio.
    Audio,
    /// H.264 video.
    Video,
}

/// Asynchronous events produced by a [`crate::PeerConnection`].
///
/// The worker thread pushes these onto a bounded queue; the application
/// drains them via [`crate::PeerConnection::poll_event`]. If the application
/// falls behind, the worker drops the oldest media, statistics and non-fatal
/// error events rather than blocking the pipeline; state changes are kept.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Event {
    /// The ICE connection state changed.
    IceConnectionStateChange(IceConnectionState),
    /// The DTLS transport state changed.
    DtlsStateChange(DtlsState),
    #[allow(missing_docs)]
    ConnectionStateChange(ConnectionState),
    /// A freshly gathered local ICE candidate (trickle ICE). The string is the
    /// `a=candidate:` SDP line value.
    LocalCandidate(String),
    #[allow(missing_docs)]
    IceGatheringComplete,
    /// The full DTLS-SRTP transport is established and media can flow.
    Connected,
    /// A new inbound media track was discovered (via its first authenticated
    /// RTP packet).
    TrackAdded(TrackKind, u32),
    /// A decoded inbound audio frame: `(ssrc, pcm_samples, sample_rate,
    /// channels)`. PCM is interleaved `i16`.
    AudioFrame(u32, Vec<i16>, u32, u8),
    /// A reassembled H.264 access unit: `(ssrc, annex_b_bytes,
    /// rtp_timestamp)`. The caller decodes it.
    VideoAccessUnit(u32, Vec<u8>, u32),
    /// Periodic statistics snapshot.
    Stats(Stats),
    #[allow(missing_docs)]
    Failed(RtcError),
    /// A non-fatal error occurred in the pipeline.
    Error(RtcError),
}

impl Event {
    fn is_droppable(&self) -> bool {
        matches!(
            self,
            Self::AudioFrame(..) | Self::VideoAccessUnit(..) | Self::Stats(_) | Self::Error(_)
        )
    }
}

pub(crate) const EVENT_QUEUE_CAPACITY: usize = 1024;

#[derive(Default)]
struct QueueState {
    events: VecDeque<Event>,
    dropped: u64,
    closed: bool,
}

#[derive(Default)]
pub(crate) struct EventQueue {
    state: Mutex<QueueState>,
    ready: Condvar,
}

impl EventQueue {
    fn lock(&self) -> MutexGuard<'_, QueueState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn push(&self, event: Event) {
        let mut state = self.lock();
        if state.events.len() >= EVENT_QUEUE_CAPACITY {
            let victim = state
                .events
                .iter()
                .position(Event::is_droppable)
                .unwrap_or(0);
            state.events.remove(victim);
            state.dropped += 1;
        }
        state.events.push_back(event);
        drop(state);
        self.ready.notify_all();
    }

    pub(crate) fn close(&self) {
        self.lock().closed = true;
        self.ready.notify_all();
    }

    pub(crate) fn try_pop(&self) -> Option<Event> {
        self.lock().events.pop_front()
    }

    pub(crate) fn pop_timeout(&self, timeout: Duration) -> Option<Event> {
        let deadline = Instant::now().checked_add(timeout);
        let mut state = self.lock();
        loop {
            if let Some(event) = state.events.pop_front() {
                return Some(event);
            }
            if state.closed {
                return None;
            }
            let wait = deadline.map_or(Duration::from_secs(3600), |deadline| {
                deadline.saturating_duration_since(Instant::now())
            });
            if wait.is_zero() {
                return None;
            }
            state = self
                .ready
                .wait_timeout(state, wait)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    pub(crate) fn dropped(&self) -> u64 {
        self.lock().dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_queue_drops_oldest_media_and_keeps_state_changes() {
        let queue = EventQueue::default();
        queue.push(Event::Connected);
        for i in 0..EVENT_QUEUE_CAPACITY - 1 {
            queue.push(Event::AudioFrame(
                u32::try_from(i).unwrap(),
                Vec::new(),
                48_000,
                1,
            ));
        }
        queue.push(Event::ConnectionStateChange(ConnectionState::Failed));
        assert_eq!(queue.dropped(), 1);
        assert!(matches!(queue.try_pop(), Some(Event::Connected)));
        assert!(matches!(queue.try_pop(), Some(Event::AudioFrame(1, ..))));
        let mut last = None;
        while let Some(event) = queue.try_pop() {
            last = Some(event);
        }
        assert!(matches!(
            last,
            Some(Event::ConnectionStateChange(ConnectionState::Failed))
        ));
    }

    #[test]
    fn pop_timeout_returns_none_once_closed_and_drained() {
        let queue = EventQueue::default();
        queue.push(Event::IceGatheringComplete);
        queue.close();
        assert!(matches!(
            queue.pop_timeout(Duration::from_secs(5)),
            Some(Event::IceGatheringComplete)
        ));
        let started = Instant::now();
        assert!(queue.pop_timeout(Duration::from_secs(5)).is_none());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn pop_timeout_waits_for_a_push() {
        let queue = std::sync::Arc::new(EventQueue::default());
        let pusher = std::sync::Arc::clone(&queue);
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            pusher.push(Event::Connected);
        });
        assert!(matches!(
            queue.pop_timeout(Duration::from_secs(5)),
            Some(Event::Connected)
        ));
        handle.join().unwrap();
    }
}
