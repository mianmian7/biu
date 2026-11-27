//! Play queue state machine (Order / Loop / Single / Shuffle).

use biu_core::models::{PlayMode, TrackItem};

/// Result of [`PlayQueue::remove`].
#[derive(Debug, Clone, PartialEq)]
pub struct Removed {
    pub item: TrackItem,
    /// The removed item was the current track; the caller should start `current()` (if any).
    pub was_current: bool,
}

#[derive(Debug, Clone)]
pub struct PlayQueue {
    items: Vec<TrackItem>,
    current_index: Option<usize>,
    mode: PlayMode,
    /// Shuffle permutation of item indices; `shuffle_order[shuffle_pos] == current_index`.
    shuffle_order: Vec<usize>,
    shuffle_pos: usize,
    rng: u64,
}

impl Default for PlayQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl PlayQueue {
    /// Empty queue with a time-based shuffle seed.
    pub fn new() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        Self::with_seed(seed)
    }

    /// Empty queue with a fixed shuffle seed (deterministic shuffle order).
    pub fn with_seed(seed: u64) -> Self {
        Self {
            items: Vec::new(),
            current_index: None,
            mode: PlayMode::default(),
            shuffle_order: Vec::new(),
            shuffle_pos: 0,
            rng: seed | 1,
        }
    }

    pub fn items(&self) -> &[TrackItem] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn current_index(&self) -> Option<usize> {
        self.current_index
    }

    pub fn current(&self) -> Option<&TrackItem> {
        self.current_index.and_then(|i| self.items.get(i))
    }

    pub fn mode(&self) -> PlayMode {
        self.mode
    }

    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.items.iter().position(|t| t.id == id)
    }

    /// Replace the queue and select `start_index` (clamped).
    pub fn set_queue(&mut self, items: Vec<TrackItem>, start_index: usize) -> Option<&TrackItem> {
        self.items = items;
        self.current_index = if self.items.is_empty() {
            None
        } else {
            Some(start_index.min(self.items.len() - 1))
        };
        self.rebuild_shuffle();
        self.current()
    }

    /// Select `track` if already queued, otherwise insert it right after the current track.
    pub fn play_or_insert(&mut self, track: TrackItem) -> &TrackItem {
        let idx = match self.index_of(&track.id) {
            Some(i) => {
                self.items[i] = track;
                i
            }
            None => {
                let at = self.current_index.map_or(0, |c| c + 1).min(self.items.len());
                self.items.insert(at, track);
                at
            }
        };
        self.current_index = Some(idx);
        self.rebuild_shuffle();
        &self.items[idx]
    }

    /// Insert `track` right after the current track (moving it there if already queued).
    pub fn insert_next(&mut self, track: TrackItem) {
        if let Some(existing) = self.index_of(&track.id) {
            if Some(existing) == self.current_index {
                self.items[existing] = track;
                return;
            }
            self.items.remove(existing);
            if let Some(cur) = self.current_index
                && existing < cur
            {
                self.current_index = Some(cur - 1);
            }
        }
        let at = self.current_index.map_or(0, |c| c + 1).min(self.items.len());
        self.items.insert(at, track);
        if self.current_index.is_none() {
            self.current_index = Some(0);
        }
        self.rebuild_shuffle();
    }

    /// Append tracks not already in the queue. Returns how many were added.
    pub fn append(&mut self, tracks: impl IntoIterator<Item = TrackItem>) -> usize {
        let mut added = 0;
        for t in tracks {
            if self.index_of(&t.id).is_none() {
                self.items.push(t);
                added += 1;
            }
        }
        if added > 0 {
            self.extend_shuffle();
        }
        added
    }

    /// Remove the item at `index`.
    ///
    /// Removing before the current track shifts the index; removing the current track selects
    /// the track that slid into its place (wrapping to 0 when it was last).
    pub fn remove(&mut self, index: usize) -> Option<Removed> {
        if index >= self.items.len() {
            return None;
        }
        let item = self.items.remove(index);
        let was_current = self.current_index == Some(index);
        self.current_index = match self.current_index {
            _ if self.items.is_empty() => None,
            Some(c) if index < c => Some(c - 1),
            Some(c) if index == c => Some(if c >= self.items.len() { 0 } else { c }),
            other => other,
        };
        self.rebuild_shuffle();
        Some(Removed { item, was_current })
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.current_index = None;
        self.shuffle_order.clear();
        self.shuffle_pos = 0;
    }

    /// Advance to the next track.
    ///
    /// `auto_advance` is true when the previous track ended naturally: `Single` then repeats,
    /// and `Order` stops (returns `None`) at the end of the list.
    pub fn next_track(&mut self, auto_advance: bool) -> Option<&TrackItem> {
        let len = self.items.len();
        if len == 0 {
            return None;
        }
        let Some(cur) = self.current_index else {
            let first = if self.mode == PlayMode::Shuffle {
                self.shuffle_pos = 0;
                self.shuffle_order.first().copied().unwrap_or(0)
            } else {
                0
            };
            self.current_index = Some(first);
            return self.current();
        };
        let next = match self.mode {
            PlayMode::Single if auto_advance => cur,
            PlayMode::Order => {
                if cur + 1 >= len {
                    return None;
                }
                cur + 1
            }
            PlayMode::Loop | PlayMode::Single => (cur + 1) % len,
            PlayMode::Shuffle => {
                if self.shuffle_pos + 1 >= self.shuffle_order.len() {
                    self.new_shuffle_cycle(Some(cur));
                } else {
                    self.shuffle_pos += 1;
                }
                self.shuffle_order[self.shuffle_pos]
            }
        };
        self.current_index = Some(next);
        self.current()
    }

    /// Go back one track (`Order` stops at the first track and returns `None`).
    pub fn prev_track(&mut self) -> Option<&TrackItem> {
        let len = self.items.len();
        if len == 0 {
            return None;
        }
        let Some(cur) = self.current_index else {
            self.current_index = Some(len - 1);
            return self.current();
        };
        let prev = match self.mode {
            PlayMode::Order => {
                if cur == 0 {
                    return None;
                }
                cur - 1
            }
            PlayMode::Loop | PlayMode::Single => (cur + len - 1) % len,
            PlayMode::Shuffle => {
                self.shuffle_pos = if self.shuffle_pos == 0 {
                    self.shuffle_order.len() - 1
                } else {
                    self.shuffle_pos - 1
                };
                self.shuffle_order[self.shuffle_pos]
            }
        };
        self.current_index = Some(prev);
        self.current()
    }

    pub fn jump_to(&mut self, index: usize) -> Option<&TrackItem> {
        if index >= self.items.len() {
            return None;
        }
        self.current_index = Some(index);
        if self.mode == PlayMode::Shuffle {
            if let Some(p) = self.shuffle_order.iter().position(|&i| i == index) {
                self.shuffle_pos = p;
            } else {
                self.rebuild_shuffle();
            }
        }
        self.current()
    }

    pub fn set_mode(&mut self, mode: PlayMode) {
        if mode != self.mode {
            self.mode = mode;
            if mode == PlayMode::Shuffle {
                self.rebuild_shuffle();
            }
        }
    }

    /// Cycle Loop -> Order -> Single -> Shuffle -> Loop.
    pub fn toggle_mode(&mut self) -> PlayMode {
        self.set_mode(self.mode.next());
        self.mode
    }

    /// The upcoming shuffle order starting at the current track (for diagnostics / UI).
    pub fn shuffle_order(&self) -> &[usize] {
        &self.shuffle_order
    }

    // -- shuffle internals ----------------------------------------------------------------

    fn next_rand(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn fisher_yates(&mut self, v: &mut [usize]) {
        for i in (1..v.len()).rev() {
            let j = (self.next_rand() % (i as u64 + 1)) as usize;
            v.swap(i, j);
        }
    }

    /// New permutation with the current track first.
    fn rebuild_shuffle(&mut self) {
        let mut rest: Vec<usize> = (0..self.items.len())
            .filter(|&i| Some(i) != self.current_index)
            .collect();
        self.fisher_yates(&mut rest);
        self.shuffle_order.clear();
        self.shuffle_order.extend(self.current_index);
        self.shuffle_order.extend(rest);
        self.shuffle_pos = 0;
    }

    /// Insert newly appended indices at random spots in the not-yet-played part of the cycle.
    fn extend_shuffle(&mut self) {
        let known = self.shuffle_order.len();
        for idx in known..self.items.len() {
            let lo = (self.shuffle_pos + 1).min(self.shuffle_order.len());
            let span = (self.shuffle_order.len() - lo) as u64 + 1;
            let at = lo + (self.next_rand() % span) as usize;
            self.shuffle_order.insert(at, idx);
        }
    }

    /// Start a fresh cycle after exhausting the permutation, avoiding an immediate repeat.
    fn new_shuffle_cycle(&mut self, last: Option<usize>) {
        let mut order: Vec<usize> = (0..self.items.len()).collect();
        self.fisher_yates(&mut order);
        if order.len() > 1 && order.first().copied() == last {
            let j = 1 + (self.next_rand() % (order.len() as u64 - 1)) as usize;
            order.swap(0, j);
        }
        self.shuffle_order = order;
        self.shuffle_pos = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use biu_core::models::TrackSource;
    use std::collections::HashSet;

    fn t(id: &str) -> TrackItem {
        TrackItem {
            id: id.into(),
            title: id.to_uppercase(),
            artist: "a".into(),
            artist_mid: None,
            cover: String::new(),
            duration_secs: 1.0,
            source: TrackSource::BiliAudio { sid: 1 },
        }
    }

    fn q(ids: &[&str], start: usize, mode: PlayMode) -> PlayQueue {
        let mut q = PlayQueue::with_seed(42);
        q.set_mode(mode);
        q.set_queue(ids.iter().map(|i| t(i)).collect(), start);
        q
    }

    fn id(t: Option<&TrackItem>) -> Option<String> {
        t.map(|t| t.id.clone())
    }

    #[test]
    fn empty_queue_edges() {
        let mut q = PlayQueue::with_seed(1);
        for mode in [
            PlayMode::Loop,
            PlayMode::Order,
            PlayMode::Single,
            PlayMode::Shuffle,
        ] {
            q.set_mode(mode);
            assert!(q.current().is_none());
            assert!(q.next_track(true).is_none());
            assert!(q.next_track(false).is_none());
            assert!(q.prev_track().is_none());
            assert!(q.jump_to(0).is_none());
            assert!(q.remove(0).is_none());
        }
        assert!(q.set_queue(vec![], 3).is_none());
        q.clear();
        assert!(q.is_empty());
    }

    #[test]
    fn order_mode_stops_at_ends() {
        let mut q = q(&["a", "b", "c"], 0, PlayMode::Order);
        assert_eq!(id(q.next_track(true)), Some("b".into()));
        assert_eq!(id(q.next_track(false)), Some("c".into()));
        assert_eq!(id(q.next_track(true)), None);
        assert_eq!(q.current_index(), Some(2), "end of list keeps current");
        assert_eq!(id(q.prev_track()), Some("b".into()));
        assert_eq!(id(q.prev_track()), Some("a".into()));
        assert_eq!(id(q.prev_track()), None);
    }

    #[test]
    fn loop_mode_wraps() {
        let mut q = q(&["a", "b", "c"], 2, PlayMode::Loop);
        assert_eq!(id(q.next_track(true)), Some("a".into()));
        assert_eq!(id(q.prev_track()), Some("c".into()));
        assert_eq!(id(q.next_track(false)), Some("a".into()));
    }

    #[test]
    fn single_mode_repeats_on_auto_but_moves_on_manual() {
        let mut q = q(&["a", "b"], 0, PlayMode::Single);
        assert_eq!(id(q.next_track(true)), Some("a".into()));
        assert_eq!(id(q.next_track(true)), Some("a".into()));
        assert_eq!(id(q.next_track(false)), Some("b".into()));
        assert_eq!(id(q.next_track(false)), Some("a".into()));
        assert_eq!(id(q.prev_track()), Some("b".into()));
    }

    #[test]
    fn shuffle_cycles_without_repeats_and_is_deterministic() {
        let ids: Vec<String> = (0..10).map(|i| format!("t{i}")).collect();
        let refs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
        let mut a = q(&refs, 3, PlayMode::Shuffle);
        let mut b = q(&refs, 3, PlayMode::Shuffle);
        let mut seen = HashSet::new();
        seen.insert(a.current_index().unwrap());
        for _ in 0..9 {
            let ia = a.next_track(true).map(|t| t.id.clone());
            let ib = b.next_track(true).map(|t| t.id.clone());
            assert_eq!(ia, ib, "same seed => same order");
            assert!(seen.insert(a.current_index().unwrap()), "no repeats in a cycle");
        }
        assert_eq!(seen.len(), 10);
        // Next cycle starts without repeating the last track back-to-back.
        let last = a.current_index();
        a.next_track(true);
        assert_ne!(a.current_index(), last);
        // prev walks back through history.
        let cur = a.current_index();
        a.next_track(false);
        a.prev_track();
        assert_eq!(a.current_index(), cur);
    }

    #[test]
    fn shuffle_append_and_single_item() {
        let mut q = q(&["only"], 0, PlayMode::Shuffle);
        assert_eq!(id(q.next_track(true)), Some("only".into()));
        assert_eq!(id(q.prev_track()), Some("only".into()));
        assert_eq!(q.append([t("x"), t("y"), t("only")]), 2);
        let mut seen = HashSet::new();
        for _ in 0..3 {
            seen.insert(q.next_track(true).unwrap().id.clone());
        }
        assert_eq!(seen.len(), 3);
    }

    #[test]
    fn remove_before_current_after_and_current() {
        let mut q = q(&["a", "b", "c", "d"], 2, PlayMode::Loop);
        // after current
        let r = q.remove(3).unwrap();
        assert_eq!((r.item.id.as_str(), r.was_current), ("d", false));
        assert_eq!(id(q.current()), Some("c".into()));
        // before current
        let r = q.remove(0).unwrap();
        assert!(!r.was_current);
        assert_eq!(q.current_index(), Some(1));
        assert_eq!(id(q.current()), Some("c".into()));
        // current (last) => wraps to first
        let r = q.remove(1).unwrap();
        assert!(r.was_current);
        assert_eq!(id(q.current()), Some("b".into()));
        // current in the middle => next slides in
        let mut q2 = q_mid();
        let r = q2.remove(1).unwrap();
        assert!(r.was_current);
        assert_eq!(id(q2.current()), Some("c".into()));
        // remove the last remaining
        let r = q.remove(0).unwrap();
        assert!(r.was_current);
        assert!(q.current().is_none() && q.is_empty());
        assert!(q.remove(0).is_none());
    }

    fn q_mid() -> PlayQueue {
        q(&["a", "b", "c"], 1, PlayMode::Order)
    }

    #[test]
    fn play_or_insert_and_jump() {
        let mut q = q(&["a", "b"], 0, PlayMode::Order);
        assert_eq!(q.play_or_insert(t("z")).id, "z");
        assert_eq!(q.current_index(), Some(1));
        assert_eq!(
            q.items().iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["a", "z", "b"]
        );
        assert_eq!(q.play_or_insert(t("b")).id, "b");
        assert_eq!(q.len(), 3);
        assert_eq!(id(q.jump_to(0)), Some("a".into()));
        assert!(q.jump_to(9).is_none());
        let mut empty = PlayQueue::with_seed(7);
        assert_eq!(empty.play_or_insert(t("x")).id, "x");
        assert_eq!(empty.current_index(), Some(0));
    }

    #[test]
    fn toggle_mode_cycles() {
        let mut q = PlayQueue::with_seed(3);
        assert_eq!(q.mode(), PlayMode::Loop);
        assert_eq!(q.toggle_mode(), PlayMode::Order);
        assert_eq!(q.toggle_mode(), PlayMode::Single);
        assert_eq!(q.toggle_mode(), PlayMode::Shuffle);
        assert_eq!(q.toggle_mode(), PlayMode::Loop);
    }

    #[test]
    fn insert_next_places_after_current_and_moves_existing() {
        let mut q = q(&["a", "b", "c"], 0, PlayMode::Order);
        q.insert_next(t("x"));
        assert_eq!(
            q.items().iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["a", "x", "b", "c"]
        );
        assert_eq!(q.current_index(), Some(0));

        // Moving an existing later item to next preserves current_index.
        q.insert_next(t("c"));
        assert_eq!(
            q.items().iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["a", "c", "x", "b"]
        );
        assert_eq!(q.current_index(), Some(0));

        // Moving an existing earlier item shifts current_index properly.
        q.jump_to(2); // "x"
        q.insert_next(t("a"));
        assert_eq!(
            q.items().iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["c", "x", "a", "b"]
        );
        assert_eq!(id(q.current()), Some("x".into()));
    }
}
