//! Lock-free sample transport between the decode worker and the real-time output callback.
//!
//! `SampleRing` is a single-producer / single-consumer ring of `f32` samples stored as
//! `AtomicU32` bit patterns (no `unsafe`). The consumer (audio callback) never takes a lock.
//! The producer side carries an *epoch* plus timestamp markers behind a small mutex that only
//! the worker and the control thread touch, which gives:
//!  * atomic "flush + invalidate in-flight producers" on seek / new track, and
//!  * accurate playback position (interpolated from markers at the consumer's read head).
//!
//! A flush moves `flush_to` past samples the consumer may be copying *right now*. To stop a
//! producer from immediately refilling (and tearing) those slots, `pop` publishes a lower bound
//! of its read range in `reading` before it samples `flush_to`, and producers never write past
//! `reading + cap`. Both sides use `SeqCst` for this store/load pair (Dekker-style), so either
//! the producer sees the in-flight read, or the consumer sees the new `flush_to` and skips the
//! flushed range.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

const MAX_MARKERS: usize = 1024;
/// `reading` value while no `pop` is in flight.
const NOT_READING: u64 = u64::MAX;

pub(crate) struct SampleRing {
    buf: Box<[AtomicU32]>,
    cap: u64,
    /// Total samples consumed (consumer-owned).
    head: AtomicU64,
    /// Total samples produced (producer-owned, guarded by `producer`).
    tail: AtomicU64,
    /// Everything before this sample index has been discarded by a flush.
    flush_to: AtomicU64,
    /// Lower bound of the slots an in-flight `pop` may be reading, or `NOT_READING`.
    reading: AtomicU64,
    producer: Mutex<ProducerState>,
}

struct ProducerState {
    epoch: u64,
    /// `(sample_index, source_secs)` pairs, ascending.
    markers: VecDeque<(u64, f64)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PushResult {
    /// The producer's epoch is outdated (seek / new track happened); nothing was written.
    Stale,
    /// Number of samples written (may be 0 when the ring is full).
    Pushed(usize),
}

impl SampleRing {
    /// `capacity` in samples (rounded up to an even number).
    pub fn new(capacity: usize) -> Self {
        let cap = (capacity.max(2) + 1) & !1;
        Self {
            buf: (0..cap).map(|_| AtomicU32::new(0)).collect(),
            cap: cap as u64,
            head: AtomicU64::new(0),
            tail: AtomicU64::new(0),
            flush_to: AtomicU64::new(0),
            reading: AtomicU64::new(NOT_READING),
            producer: Mutex::new(ProducerState {
                epoch: 0,
                markers: VecDeque::new(),
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ProducerState> {
        self.producer.lock().unwrap_or_else(|p| p.into_inner())
    }

    #[inline]
    fn effective_head(&self) -> u64 {
        self.head
            .load(Ordering::Acquire)
            .max(self.flush_to.load(Ordering::SeqCst))
    }

    /// Buffered samples not yet consumed.
    pub fn len(&self) -> usize {
        let tail = self.tail.load(Ordering::Acquire);
        tail.saturating_sub(self.effective_head()) as usize
    }

    pub fn epoch(&self) -> u64 {
        self.lock().epoch
    }

    /// Discard all buffered audio, invalidate current producers, and anchor the position clock
    /// at `secs`. Returns the new epoch producers must use.
    pub fn reset(&self, secs: f64) -> u64 {
        let mut st = self.lock();
        st.epoch += 1;
        let t = self.tail.load(Ordering::Acquire);
        self.flush_to.store(t, Ordering::SeqCst);
        st.markers.clear();
        st.markers.push_back((t, secs));
        st.epoch
    }

    /// Producer: write as many whole stereo frames of `samples` as fit.
    ///
    /// `secs_start`/`secs_end` are the source times of `samples[0]` and the end of `samples`.
    pub fn push(&self, epoch: u64, samples: &[f32], secs_start: f64, secs_end: f64) -> PushResult {
        let mut st = self.lock();
        if st.epoch != epoch {
            return PushResult::Stale;
        }
        // Slots from an in-flight read are still occupied even if a flush skipped past them.
        let reading = self.reading.load(Ordering::SeqCst);
        let head = self.effective_head();
        let tail = self.tail.load(Ordering::Relaxed);
        let reserved = head.min(reading).min(tail);
        let free = self.cap.saturating_sub(tail - reserved);
        let n = (free.min(samples.len() as u64) as usize) & !1;
        if n == 0 {
            return PushResult::Pushed(0);
        }
        for (i, &s) in samples[..n].iter().enumerate() {
            let slot = ((tail + i as u64) % self.cap) as usize;
            self.buf[slot].store(s.to_bits(), Ordering::Relaxed);
        }
        let new_tail = tail + n as u64;
        self.tail.store(new_tail, Ordering::Release);
        let secs = secs_start + (secs_end - secs_start) * (n as f64 / samples.len() as f64);
        st.markers.push_back((new_tail, secs));
        Self::prune(&mut st.markers, head);
        PushResult::Pushed(n)
    }

    /// Drop markers fully behind `head`, keeping one at or before it for interpolation.
    fn prune(markers: &mut VecDeque<(u64, f64)>, head: u64) {
        while markers.len() >= 2 && markers[1].0 <= head {
            markers.pop_front();
        }
        while markers.len() > MAX_MARKERS {
            markers.remove(1);
        }
    }

    /// Consumer (real-time safe): read up to `out.len()` samples (whole frames), returns count.
    pub fn pop(&self, out: &mut [f32]) -> usize {
        self.pop_inner(out, || {})
    }

    /// `pop` with a hook between sizing the read and copying it (deterministic race tests).
    #[inline]
    fn pop_inner(&self, out: &mut [f32], before_copy: impl FnOnce()) -> usize {
        // Publish (before sampling `flush_to`) that slots from our raw head on may be read.
        let own_head = self.head.load(Ordering::Relaxed);
        self.reading.store(own_head, Ordering::SeqCst);
        let head = own_head.max(self.flush_to.load(Ordering::SeqCst));
        let tail = self.tail.load(Ordering::Acquire);
        let avail = tail.saturating_sub(head);
        let n = (avail.min(out.len() as u64) as usize) & !1;
        before_copy();
        for (i, o) in out[..n].iter_mut().enumerate() {
            let slot = ((head + i as u64) % self.cap) as usize;
            *o = f32::from_bits(self.buf[slot].load(Ordering::Relaxed));
        }
        // Always advance (even by 0) so the raw head catches up with a flush and the
        // `reading` bound published next time does not hold back producers.
        self.head.store(head + n as u64, Ordering::Release);
        self.reading.store(NOT_READING, Ordering::Release);
        n
    }

    /// Source-time position of the sample the consumer will play next.
    pub fn position(&self) -> Option<f64> {
        let mut st = self.lock();
        let head = self.effective_head();
        Self::prune(&mut st.markers, head);
        let &(a, sa) = st.markers.front()?;
        match st.markers.get(1) {
            Some(&(b, sb)) if head >= a && b > a => {
                let t = ((head - a) as f64 / (b - a) as f64).clamp(0.0, 1.0);
                Some(sa + (sb - sa) * t)
            }
            _ => Some(sa),
        }
    }
}

/// Lock-free tap of the most recent mono output samples, for the spectrum analyzer.
pub(crate) struct TapRing {
    buf: Box<[AtomicU32]>,
    write: AtomicUsize,
}

impl TapRing {
    pub fn new(capacity: usize) -> Self {
        Self {
            buf: (0..capacity.max(1)).map(|_| AtomicU32::new(0)).collect(),
            write: AtomicUsize::new(0),
        }
    }

    /// Writer (real-time callback).
    #[inline]
    pub fn push(&self, s: f32) {
        let w = self.write.load(Ordering::Relaxed);
        self.buf[w % self.buf.len()].store(s.to_bits(), Ordering::Relaxed);
        self.write.store(w.wrapping_add(1), Ordering::Release);
    }

    /// Total samples ever written (wrapping).
    pub fn written(&self) -> usize {
        self.write.load(Ordering::Acquire)
    }

    /// Copy the latest `out.len()` samples (oldest first).
    pub fn latest(&self, out: &mut [f32]) {
        let w = self.write.load(Ordering::Acquire);
        let cap = self.buf.len();
        let n = out.len().min(cap);
        for (i, o) in out[..n].iter_mut().enumerate() {
            let idx = w.wrapping_sub(n).wrapping_add(i) % cap;
            *o = f32::from_bits(self.buf[idx].load(Ordering::Relaxed));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_pop_wraps_and_tracks_position() {
        let r = SampleRing::new(8);
        let e = r.reset(10.0);
        assert_eq!(
            r.push(e, &[1.0, 2.0, 3.0, 4.0], 10.0, 11.0),
            PushResult::Pushed(4)
        );
        assert_eq!(r.position(), Some(10.0));
        let mut out = [0.0; 2];
        assert_eq!(r.pop(&mut out), 2);
        assert_eq!(out, [1.0, 2.0]);
        assert!((r.position().unwrap() - 10.5).abs() < 1e-9);
        // 6 free => only 6 of 8 fit.
        let data: Vec<f32> = (0..8).map(|i| 10.0 + i as f32).collect();
        assert_eq!(r.push(e, &data, 11.0, 12.0), PushResult::Pushed(6));
        let mut all = [0.0; 8];
        assert_eq!(r.pop(&mut all), 8);
        assert_eq!(all, [3.0, 4.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0]);
        assert!((r.position().unwrap() - 11.75).abs() < 1e-9);
    }

    #[test]
    fn reset_flushes_and_invalidates_stale_producers() {
        let r = SampleRing::new(16);
        let e1 = r.reset(0.0);
        r.push(e1, &[1.0; 8], 0.0, 1.0);
        let e2 = r.reset(5.0);
        assert_eq!(r.len(), 0);
        assert_eq!(r.push(e1, &[9.0; 2], 1.0, 2.0), PushResult::Stale);
        assert_eq!(r.push(e2, &[7.0; 4], 5.0, 6.0), PushResult::Pushed(4));
        let mut out = [0.0; 16];
        assert_eq!(r.pop(&mut out), 4);
        assert_eq!(&out[..4], &[7.0; 4]);
        assert_eq!(r.position(), Some(6.0));
    }

    #[test]
    fn reset_during_in_flight_pop_does_not_overwrite_read_slots() {
        let r = SampleRing::new(8);
        let e1 = r.reset(0.0);
        let old: Vec<f32> = (1..=8).map(|i| i as f32).collect();
        assert_eq!(r.push(e1, &old, 0.0, 1.0), PushResult::Pushed(8));
        let mut out = [0.0; 8];
        let mut during = None;
        // The callback has sized its read of slots 0..8; a seek resets and the new producer
        // immediately tries to refill the whole ring before the callback copies.
        let n = r.pop_inner(&mut out, || {
            let e2 = r.reset(5.0);
            during = Some((e2, r.push(e2, &[9.0; 8], 5.0, 6.0)));
        });
        assert_eq!(n, 8);
        assert_eq!(
            out.to_vec(),
            old,
            "in-flight read was torn by the new producer"
        );
        let (e2, pushed) = during.unwrap();
        assert_eq!(pushed, PushResult::Pushed(0));
        // Once the read finishes the producer gets the full ring.
        assert_eq!(r.push(e2, &[9.0; 8], 5.0, 6.0), PushResult::Pushed(8));
        let mut out = [0.0; 8];
        assert_eq!(r.pop(&mut out), 8);
        assert_eq!(out, [9.0; 8]);
    }

    #[test]
    fn stale_head_after_reset_does_not_block_producer() {
        let r = SampleRing::new(8);
        let e1 = r.reset(0.0);
        r.push(e1, &[1.0; 8], 0.0, 1.0);
        let e2 = r.reset(1.0);
        // Consumer idle (no pop in flight): the flushed range is free immediately.
        assert_eq!(r.push(e2, &[2.0; 8], 1.0, 2.0), PushResult::Pushed(8));
        let mut out = [0.0; 8];
        assert_eq!(r.pop(&mut out), 8);
        assert_eq!(out, [2.0; 8]);
    }

    #[test]
    fn tap_returns_latest_samples() {
        let t = TapRing::new(4);
        for i in 0..6 {
            t.push(i as f32);
        }
        let mut out = [0.0; 3];
        t.latest(&mut out);
        assert_eq!(out, [3.0, 4.0, 5.0]);
        assert_eq!(t.written(), 6);
    }
}
