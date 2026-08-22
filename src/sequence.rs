//! Completeness: sequence gaps are the only evidence of loss.
//!
//! A lost record file leaves **no trace in the surviving data**. Nothing that
//! arrives says the missing file existed, no total is visibly short, and no
//! subscriber complains — the usage simply was never billed. The only evidence
//! is a hole in a per-source sequence number, which makes sequence tracking the
//! load-bearing completeness control of a mediation layer rather than a
//! diagnostic nicety.
//!
//! | Source | Sequence carrier | Scope | Space |
//! |---|---|---|---|
//! | 3GPP CDR files (TS 32.297) | running count in the file name | per CGF node | implementation-defined |
//! | TAP (GSMA TD.57) | `fileSequenceNumber` | per sender/recipient TADIG pair | **1 … 99 999** |
//! | RAP (GSMA TD.32) | `rapFileSequenceNumber` | per TAP relationship | 1 … 99 999 |
//! | Records in a file | `localSequenceNumber` | per recording entity | 32-bit |
//!
//! # The wrap is the trap
//!
//! A TAP file sequence runs `99999` then `00001`. Three naive checks break at
//! exactly that point, once per cycle:
//!
//! - `next == previous + 1` reports a 99 998-file gap
//! - `next > previous` reports the entire next cycle as out of order
//! - `next - previous` underflows, or yields a nonsense distance
//!
//! [`SequenceSpace`] therefore carries the modulus, and comparison uses the
//! serial-number arithmetic of **RFC 1982**: a forward distance of more than
//! half the space is read as a backward move instead. That is the only
//! interpretation available without out-of-band knowledge, and it is right for
//! every real gap, because no operator loses half a cycle of files without
//! noticing by other means.
//!
//! RFC 1982 leaves the ordering of two values **exactly** half a space apart
//! undefined, because neither reading has a better claim. This crate resolves
//! it forward — a jump of 49 999 TAP files is read as a gap rather than as a
//! regression — because a hole that is reported and turns out to be a
//! regression costs a resend request, while a regression read as an accepted
//! arrival silently advances the high-water mark past 49 999 files nobody will
//! ever ask for again. The two errors are not symmetric, so the tie is broken
//! toward the one that is recoverable.
//!
//! ```rust
//! use mediation::{SequenceSpace, SequenceValue};
//!
//! let last = SequenceValue::new(SequenceSpace::TAP, 99_999).unwrap();
//! let next = SequenceValue::new(SequenceSpace::TAP, 1).unwrap();
//!
//! // Across the wrap, the successor is the successor — not a 99 998-file gap.
//! assert!(last.is_successor(next));
//! assert_eq!(last.gap_to(next), Some(0));
//! ```
//!
//! # Gaps are identities, not a count
//!
//! [`SequenceTracker::missing`] returns the missing sequence numbers
//! themselves, because the operational response is to request a resend of
//! exactly those files. A boolean would be true and useless; a count would say
//! how bad it is without saying what to ask for.

use std::collections::BTreeMap;
use std::fmt;

// ── SequenceSpace ────────────────────────────────────────────────────────────

/// The range a sequence number wraps within, inclusive at both ends.
///
/// The modulus is part of the value because a sequence number cannot be
/// compared without it: `99999` is the predecessor of `1` in a TAP file
/// sequence and nowhere near it in a 32-bit counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SequenceSpace {
    first: u32,
    last: u32,
}

impl SequenceSpace {
    /// GSMA TAP and RAP file sequences: `00001` … `99999`.
    ///
    /// TD.57 assigns the same range to test and chargeable data, so the two
    /// share a space and a resend of one shifts the other.
    pub const TAP: Self = Self {
        first: 1,
        last: 99_999,
    };

    /// A full 32-bit counter, `0` … `u32::MAX`.
    ///
    /// The right space for a `localSequenceNumber` and for file counters whose
    /// operator has not declared a narrower one.
    pub const U32: Self = Self {
        first: 0,
        last: u32::MAX,
    };

    /// A space from its inclusive bounds.
    ///
    /// `None` when `last` is below `first`, or when the space holds fewer than
    /// two values — a one-value space has no ordering to reason about.
    #[must_use]
    pub const fn new(first: u32, last: u32) -> Option<Self> {
        if last > first {
            Some(Self { first, last })
        } else {
            None
        }
    }

    /// The lowest value in the space.
    #[must_use]
    pub const fn first(self) -> u32 {
        self.first
    }

    /// The highest value in the space.
    #[must_use]
    pub const fn last(self) -> u32 {
        self.last
    }

    /// How many distinct values the space holds.
    #[must_use]
    pub const fn size(self) -> u64 {
        (self.last as u64 - self.first as u64) + 1
    }

    /// `true` when `value` falls inside the space.
    #[must_use]
    pub const fn contains(self, value: u32) -> bool {
        value >= self.first && value <= self.last
    }

    /// The number of steps forward from `from` to `to`, wrapping.
    ///
    /// Zero when they are equal, `1` when `to` immediately follows `from`.
    const fn forward_distance(self, from: u32, to: u32) -> u64 {
        let size = self.size();
        let from = (from - self.first) as u64;
        let to = (to - self.first) as u64;
        if to >= from {
            to - from
        } else {
            size - (from - to)
        }
    }
}

impl fmt::Display for SequenceSpace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..={}", self.first, self.last)
    }
}

// ── SequenceValue ────────────────────────────────────────────────────────────

/// A sequence number, together with the space it wraps within.
///
/// Carrying the space means every comparison is wrap-correct by construction,
/// and two values from different spaces cannot be compared at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SequenceValue {
    space: SequenceSpace,
    value: u32,
}

impl SequenceValue {
    /// A sequence number in a space. `None` when the value falls outside it.
    #[must_use]
    pub const fn new(space: SequenceSpace, value: u32) -> Option<Self> {
        if space.contains(value) {
            Some(Self { space, value })
        } else {
            None
        }
    }

    /// The raw value.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.value
    }

    /// The space this value wraps within.
    #[must_use]
    pub const fn space(self) -> SequenceSpace {
        self.space
    }

    /// The next value, wrapping at the end of the space.
    #[must_use]
    pub const fn successor(self) -> Self {
        Self {
            space: self.space,
            value: if self.value >= self.space.last {
                self.space.first
            } else {
                self.value + 1
            },
        }
    }

    /// How many values are missing between `self` and `next`, exclusive.
    ///
    /// `Some(0)` when `next` immediately follows. `None` when the two are in
    /// different spaces, or when `next` is *behind* `self` under RFC 1982
    /// ordering — that is a regression, not a gap, and reporting it as a gap of
    /// nearly a full cycle is the classic wrap bug.
    ///
    /// Two values exactly half a space apart are the case RFC 1982 leaves
    /// undefined. They are read **forward**, for the reason given in the module
    /// documentation: a hole that turns out to be a regression costs a resend
    /// request, while a regression read as an arrival silently abandons half a
    /// cycle of files.
    ///
    /// ```rust
    /// use mediation::{SequenceSpace, SequenceValue};
    ///
    /// let at = |v| SequenceValue::new(SequenceSpace::TAP, v).unwrap();
    ///
    /// assert_eq!(at(7).gap_to(at(8)), Some(0));   // consecutive
    /// assert_eq!(at(7).gap_to(at(10)), Some(2));  // 8 and 9 are missing
    /// assert_eq!(at(7).gap_to(at(7)), None);      // a repeat is not a gap
    /// assert_eq!(at(10).gap_to(at(7)), None);     // a regression is not a gap
    /// ```
    #[must_use]
    pub const fn gap_to(self, next: Self) -> Option<u64> {
        if !self.same_space(next) {
            return None;
        }
        let forward = self.space.forward_distance(self.value, next.value);
        if forward == 0 || forward > self.space.size() / 2 {
            return None;
        }
        Some(forward - 1)
    }

    /// `true` when `next` is the immediate successor, wrap included.
    #[must_use]
    pub const fn is_successor(self, next: Self) -> bool {
        matches!(self.gap_to(next), Some(0))
    }

    /// `true` when `next` sorts *before* `self` under RFC 1982 ordering.
    ///
    /// The wrap-safe replacement for `next < self`, which is wrong at exactly
    /// the point it matters.
    #[must_use]
    pub const fn is_regression(self, next: Self) -> bool {
        if !self.same_space(next) {
            return false;
        }
        let forward = self.space.forward_distance(self.value, next.value);
        forward != 0 && forward > self.space.size() / 2
    }

    /// `true` when both values wrap within the same space.
    #[must_use]
    pub const fn same_space(self, other: Self) -> bool {
        self.space.first == other.space.first && self.space.last == other.space.last
    }
}

impl fmt::Display for SequenceValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.value)
    }
}

// ── SequenceTracker ──────────────────────────────────────────────────────────

/// What happened when a sequence number arrived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SequenceObservation {
    /// The first value seen from this source; nothing can be concluded yet.
    First,
    /// The immediate successor of the previous high-water mark.
    InOrder,
    /// A forward jump: the listed values have not arrived.
    ///
    /// Carries the identities rather than a count, because the operational
    /// response is a resend request for exactly these — and it carries the
    /// **exact** count beside them, because the list is bounded and the count
    /// is not. A jump of four billion names ten thousand files and counts all
    /// four billion; a tracker that only knew what it could list would report a
    /// source as complete while most of it was missing.
    Gap {
        /// The lowest skipped identities, ascending, bounded by the caller's
        /// listing limit.
        missing: Vec<u32>,
        /// Exactly how many identities were skipped. May exceed
        /// `missing.len()`, in which case the list is truncated and the count
        /// is not.
        missing_count: u64,
    },
    /// A value behind the high-water mark that fills a known hole.
    FilledGap,
    /// A value behind the high-water mark that was already seen.
    Duplicate,
    /// A value behind the high-water mark that was never expected.
    ///
    /// Either a stale resend from before tracking began, or a value from a
    /// different source misrouted into this one.
    Unexpected,
    /// The value does not belong to this tracker's space.
    OutOfSpace,
}

impl SequenceObservation {
    /// `true` when the observation revealed newly missing files.
    #[must_use]
    pub fn opened_a_gap(&self) -> bool {
        matches!(self, Self::Gap { .. })
    }

    /// How many identities the observation revealed as missing.
    ///
    /// Zero for every observation but a gap, and exact for a gap even where the
    /// listed identities were truncated.
    #[must_use]
    pub const fn missing_count(&self) -> u64 {
        match self {
            Self::Gap { missing_count, .. } => *missing_count,
            _ => 0,
        }
    }

    /// `true` when a gap named fewer identities than it counted.
    #[must_use]
    pub fn missing_is_truncated(&self) -> bool {
        match self {
            Self::Gap {
                missing,
                missing_count,
            } => *missing_count > missing.len() as u64,
            _ => false,
        }
    }
}

/// Tracks one source's sequence numbers and reports what is missing.
///
/// One tracker per **scope**, and the scope is part of the standard: a TAP
/// sequence is per sender/recipient TADIG pair, a CDR file counter is per CGF
/// node, a `localSequenceNumber` is per recording entity. Sharing a tracker
/// across scopes manufactures gaps that do not exist.
///
/// # Holes are stored as runs, so the count is always exact
///
/// A jump from file 7 to file 4 000 000 007 leaves four billion holes. Storing
/// them one per value is not an option — it is a four-billion-element
/// allocation from one corrupt field — but *forgetting* the ones that do not
/// fit is worse: [`is_complete`](Self::is_complete) would then read `true` for
/// a source that is missing almost everything, and completeness is the one
/// thing this module exists to be right about.
///
/// So a run of consecutive holes costs the same as a single hole. Memory is
/// proportional to the number of **discontiguous** holes, the count is exact
/// at every size, and only the *listing* of identities is ever bounded.
///
/// # Persistence
///
/// A tracker is caller-owned state that must survive a restart, so it is
/// round-trippable: [`snapshot`](Self::snapshot) hands out everything it holds
/// and [`restore`](Self::restore) rebuilds it. With the `serde` feature the
/// tracker serialises directly, through that same snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SequenceTracker {
    space: SequenceSpace,
    /// The most recent value under RFC 1982 ordering, once anything has arrived.
    high_water: Option<u32>,
    /// Outstanding holes as runs of consecutive **space offsets**, keyed by the
    /// run's first offset and valued by its length. Offsets rather than raw
    /// values so a run that crosses the wrap point is two entries with ordinary
    /// arithmetic rather than one entry with a special case.
    holes: BTreeMap<u64, u64>,
    /// The sum of every run's length, maintained incrementally so
    /// [`missing_count`](Self::missing_count) is O(1) at any scale.
    missing_count: u64,
}

/// Everything a [`SequenceTracker`] holds, for persisting and restoring it.
///
/// The runs are half-open in **space offsets** — `(first, count)` pairs where
/// `first` is `value - space.first()` — because that is the form the tracker
/// stores and converting on the way out would make a snapshot lossy for a run
/// that crosses the wrap point.
///
/// Use [`SequenceTracker::missing_runs`] for the same information as raw
/// sequence values, which is what a resend request needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SequenceSnapshot {
    /// The space the tracker follows.
    pub space: SequenceSpace,
    /// The high-water mark, if anything has arrived.
    pub high_water: Option<u32>,
    /// Outstanding holes as `(first offset, length)` runs, ascending and
    /// non-overlapping.
    pub holes: Vec<(u64, u64)>,
}

impl SequenceTracker {
    /// The default bound on how many missing identities one observation lists.
    ///
    /// It bounds the **list**, never the tracker's own count — see
    /// [`observe`](Self::observe).
    pub const DEFAULT_MAX_LISTED: usize = 10_000;

    /// A tracker over a sequence space, having seen nothing.
    #[must_use]
    pub const fn new(space: SequenceSpace) -> Self {
        Self {
            space,
            high_water: None,
            holes: BTreeMap::new(),
            missing_count: 0,
        }
    }

    /// The space this tracker follows.
    #[must_use]
    pub const fn space(&self) -> SequenceSpace {
        self.space
    }

    /// The highest value seen so far, under wrap-aware ordering.
    #[must_use]
    pub const fn high_water(&self) -> Option<u32> {
        self.high_water
    }

    /// The sequence numbers known to be missing, ascending, at most `limit` of
    /// them.
    ///
    /// The count is unbounded and the list is not, so ask
    /// [`missing_count`](Self::missing_count) how many there really are.
    #[must_use]
    pub fn missing_upto(&self, limit: usize) -> Vec<u32> {
        let mut out = Vec::new();
        'runs: for (first, len) in &self.holes {
            for step in 0..*len {
                if out.len() >= limit {
                    break 'runs;
                }
                out.push(self.value_at(first + step));
            }
        }
        out
    }

    /// The sequence numbers known to be missing, ascending, at most
    /// [`DEFAULT_MAX_LISTED`](Self::DEFAULT_MAX_LISTED) of them.
    #[must_use]
    pub fn missing(&self) -> Vec<u32> {
        self.missing_upto(Self::DEFAULT_MAX_LISTED)
    }

    /// The outstanding holes as inclusive `(first, last)` runs of sequence
    /// values, ascending.
    ///
    /// The compact form, and the one a resend request should quote: four
    /// billion missing files are one run here and would be four billion entries
    /// in [`missing`](Self::missing). A run that crosses the space's wrap point
    /// appears as two.
    #[must_use]
    pub fn missing_runs(&self) -> Vec<(u32, u32)> {
        self.holes
            .iter()
            .map(|(first, len)| (self.value_at(*first), self.value_at(first + len - 1)))
            .collect()
    }

    /// How many values are known to be missing. Always exact.
    #[must_use]
    pub const fn missing_count(&self) -> u64 {
        self.missing_count
    }

    /// How many discontiguous holes the tracker is carrying.
    ///
    /// The tracker's memory cost, and a useful signal of its own: one run of a
    /// thousand is a lost file batch, a thousand runs of one is a source that
    /// is dropping records at random.
    #[must_use]
    pub fn hole_count(&self) -> usize {
        self.holes.len()
    }

    /// `true` when nothing is outstanding.
    ///
    /// The precondition for releasing a batch: a source with an unexplained
    /// hole has unknown completeness, and billing over unknown completeness
    /// invoices an amount nobody can prove is right.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.missing_count == 0
    }

    /// `true` when `value` is one of the outstanding holes.
    #[must_use]
    pub fn is_missing(&self, value: u32) -> bool {
        self.space.contains(value) && self.run_containing(self.offset_of(value)).is_some()
    }

    /// Record an arrival and classify it.
    ///
    /// `max_listed` bounds the identities the returned
    /// [`Gap`](SequenceObservation::Gap) *names*. It does **not** bound what
    /// the tracker remembers: the exact count is always kept, and
    /// [`is_complete`](Self::is_complete) is therefore never wrong in the
    /// direction that would let an incomplete source be billed.
    pub fn observe(&mut self, value: u32, max_listed: usize) -> SequenceObservation {
        let Some(arriving) = SequenceValue::new(self.space, value) else {
            return SequenceObservation::OutOfSpace;
        };

        let Some(previous) = self.high_water else {
            self.high_water = Some(value);
            return SequenceObservation::First;
        };
        // `previous` came from this tracker's own space, so it is always valid.
        let previous = SequenceValue {
            space: self.space,
            value: previous,
        };

        if previous.get() == value {
            return SequenceObservation::Duplicate;
        }

        if previous.is_regression(arriving) {
            // Behind the high-water mark: it either fills a hole or is stale.
            return if self.take_hole(self.offset_of(value)) {
                SequenceObservation::FilledGap
            } else {
                SequenceObservation::Unexpected
            };
        }

        // Forward. Everything skipped over becomes a hole — all of it, however
        // many. The distance is computed directly rather than through `gap_to`,
        // whose `None` cases — equal, and backward — are both already excluded
        // above. Reading that `None` as "no gap" would hide a hole, which is
        // the one direction this module must never fail in.
        self.high_water = Some(value);
        let skipped = self.space.forward_distance(previous.get(), value) - 1;
        if skipped == 0 {
            return SequenceObservation::InOrder;
        }

        let from = self.offset_of(previous.successor().get());
        self.add_run(from, skipped);

        let mut missing = Vec::with_capacity(skipped.min(max_listed as u64) as usize);
        for step in 0..skipped.min(max_listed as u64) {
            missing.push(self.value_at((from + step) % self.space.size()));
        }
        SequenceObservation::Gap {
            missing,
            missing_count: skipped,
        }
    }

    /// Record an arrival, listing at most
    /// [`DEFAULT_MAX_LISTED`](Self::DEFAULT_MAX_LISTED) missing identities.
    pub fn observe_default(&mut self, value: u32) -> SequenceObservation {
        self.observe(value, Self::DEFAULT_MAX_LISTED)
    }

    /// Forget holes older than the resend window, accepting them as lost.
    ///
    /// Removes the missing values that are more than `beyond` steps behind the
    /// high-water mark, and returns how many were abandoned. A hole a partner
    /// will never fill must stop blocking every future batch, and abandoning it
    /// is a decision the caller makes explicitly rather than one the tracker
    /// makes quietly.
    ///
    /// A count rather than a list: the set may be enormous, and the values that
    /// were given up on are recoverable from a snapshot taken before the call.
    /// [`missing_runs`](Self::missing_runs) names what is left.
    pub fn abandon_older_than(&mut self, beyond: u64) -> u64 {
        let Some(high) = self.high_water else {
            return 0;
        };
        // A value is abandoned when its forward distance to the high-water mark
        // exceeds `beyond`. Distance decreases as the value approaches the
        // mark, so within one run the abandoned part is always its head.
        let mut abandoned = 0u64;
        let mut kept: BTreeMap<u64, u64> = BTreeMap::new();
        for (first, len) in std::mem::take(&mut self.holes) {
            let mut start = first;
            let mut remaining = len;
            while remaining > 0 && self.space.forward_distance(self.value_at(start), high) > beyond
            {
                start += 1;
                remaining -= 1;
                abandoned += 1;
            }
            if remaining > 0 {
                kept.insert(start, remaining);
            }
        }
        self.holes = kept;
        self.missing_count -= abandoned;
        abandoned
    }

    /// Everything the tracker holds, for persisting it.
    #[must_use]
    pub fn snapshot(&self) -> SequenceSnapshot {
        SequenceSnapshot {
            space: self.space,
            high_water: self.high_water,
            holes: self.holes.iter().map(|(k, v)| (*k, *v)).collect(),
        }
    }

    /// Rebuild a tracker from a snapshot.
    ///
    /// `None` when the snapshot does not describe a tracker this type could
    /// have produced: a high-water mark outside the space, a run that starts or
    /// ends outside it, a zero-length run, or runs that overlap. Restoring a
    /// contradictory snapshot would put the completeness control into a state
    /// its own invariants do not cover, which is worse than refusing.
    #[must_use]
    pub fn restore(snapshot: SequenceSnapshot) -> Option<Self> {
        let SequenceSnapshot {
            space,
            high_water,
            holes,
        } = snapshot;
        if let Some(high) = high_water
            && !space.contains(high)
        {
            return None;
        }
        let size = space.size();
        let mut map = BTreeMap::new();
        let mut count = 0u64;
        let mut previous_end = 0u64;
        for (index, (first, len)) in holes.into_iter().enumerate() {
            if len == 0 || first >= size || len > size - first {
                return None;
            }
            if index > 0 && first < previous_end {
                return None;
            }
            previous_end = first + len;
            count = count.checked_add(len)?;
            if map.insert(first, len).is_some() {
                return None;
            }
        }
        Some(Self {
            space,
            high_water,
            holes: map,
            missing_count: count,
        })
    }

    // ── Run arithmetic ───────────────────────────────────────────────────────

    /// A value's offset from the space's first value.
    const fn offset_of(&self, value: u32) -> u64 {
        (value - self.space.first()) as u64
    }

    /// The value at an offset from the space's first value.
    const fn value_at(&self, offset: u64) -> u32 {
        self.space.first() + offset as u32
    }

    /// The run containing `offset`, as its key.
    fn run_containing(&self, offset: u64) -> Option<u64> {
        self.holes
            .range(..=offset)
            .next_back()
            .filter(|(first, len)| offset < *first + *len)
            .map(|(first, _)| *first)
    }

    /// Mark `len` consecutive offsets missing, starting at `from`, wrapping.
    ///
    /// Splits at the wrap point so every stored run is linear in offsets, and
    /// merges with any run it touches so a source that loses files in adjacent
    /// batches ends up with one hole rather than a chain of them.
    fn add_run(&mut self, from: u64, len: u64) {
        let size = self.space.size();
        if from + len <= size {
            self.add_linear_run(from, len);
        } else {
            let head = size - from;
            self.add_linear_run(from, head);
            self.add_linear_run(0, len - head);
        }
    }

    /// Mark a run that does not cross the wrap point.
    fn add_linear_run(&mut self, from: u64, len: u64) {
        if len == 0 {
            return;
        }
        let (mut start, mut end) = (from, from + len);

        // Absorb the run that starts at or before `start` and reaches it, then
        // every run that begins inside the widened span. Both are removals from
        // the map, so the count is recomputed from what survives rather than
        // adjusted term by term.
        if let Some(previous) = self
            .holes
            .range(..=start)
            .next_back()
            .filter(|(first, len)| *first + *len >= start)
            .map(|(first, len)| (*first, *len))
        {
            self.holes.remove(&previous.0);
            self.missing_count -= previous.1;
            start = previous.0;
            end = end.max(previous.0 + previous.1);
        }
        let overlapping: Vec<u64> = self
            .holes
            .range(start..=end)
            .map(|(first, _)| *first)
            .collect();
        for first in overlapping {
            let absorbed = self.holes.remove(&first).unwrap_or(0);
            self.missing_count -= absorbed;
            end = end.max(first + absorbed);
        }

        self.holes.insert(start, end - start);
        self.missing_count += end - start;
    }

    /// Remove one offset from the holes, `true` when it was there.
    fn take_hole(&mut self, offset: u64) -> bool {
        let Some(first) = self.run_containing(offset) else {
            return false;
        };
        let len = self.holes.remove(&first).unwrap_or(0);
        let end = first + len;
        self.missing_count -= len;
        if offset > first {
            self.holes.insert(first, offset - first);
            self.missing_count += offset - first;
        }
        if offset + 1 < end {
            self.holes.insert(offset + 1, end - offset - 1);
            self.missing_count += end - offset - 1;
        }
        true
    }
}

impl fmt::Display for SequenceTracker {
    /// The high-water mark and what is outstanding, in one line.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.high_water {
            None => f.write_str("nothing seen"),
            Some(high) => write!(
                f,
                "high water {high}, {} missing in {} hole(s)",
                self.missing_count,
                self.holes.len()
            ),
        }
    }
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::{SequenceSnapshot, SequenceSpace, SequenceTracker, SequenceValue};
    use serde::de;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// Field names are part of the public representation and covered by semver.
    #[derive(Serialize, Deserialize)]
    #[serde(rename = "SequenceSpace")]
    struct SpaceRepr {
        first: u32,
        last: u32,
    }

    impl Serialize for SequenceSpace {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            SpaceRepr {
                first: self.first(),
                last: self.last(),
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for SequenceSpace {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = SpaceRepr::deserialize(deserializer)?;
            SequenceSpace::new(r.first, r.last).ok_or_else(|| {
                serde::de::Error::custom("a sequence space needs at least two values")
            })
        }
    }

    #[derive(Serialize, Deserialize)]
    #[serde(rename = "SequenceValue")]
    struct ValueRepr {
        space: SequenceSpace,
        value: u32,
    }

    impl Serialize for SequenceValue {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            ValueRepr {
                space: self.space(),
                value: self.get(),
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for SequenceValue {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = ValueRepr::deserialize(deserializer)?;
            SequenceValue::new(r.space, r.value)
                .ok_or_else(|| serde::de::Error::custom("value outside its sequence space"))
        }
    }

    /// A tracker is caller-owned state that has to survive a restart, so it
    /// serialises through its snapshot: the space it follows, how far it has
    /// got, and the runs of files it is still waiting for. Runs rather than
    /// values, so a source missing four billion files is three numbers.
    ///
    /// Field names are part of the public representation and covered by semver.
    #[derive(Serialize, Deserialize)]
    #[serde(rename = "SequenceTracker")]
    struct TrackerRepr {
        space: SequenceSpace,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        high_water: Option<u32>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        holes: Vec<(u64, u64)>,
    }

    impl Serialize for SequenceTracker {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let snapshot = self.snapshot();
            TrackerRepr {
                space: snapshot.space,
                high_water: snapshot.high_water,
                holes: snapshot.holes,
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for SequenceTracker {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = TrackerRepr::deserialize(deserializer)?;
            SequenceTracker::restore(SequenceSnapshot {
                space: r.space,
                high_water: r.high_water,
                holes: r.holes,
            })
            .ok_or_else(|| de::Error::custom("a self-consistent sequence tracker"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tap(value: u32) -> SequenceValue {
        SequenceValue::new(SequenceSpace::TAP, value).expect("in space")
    }

    #[test]
    fn the_tap_space_matches_the_published_range() {
        assert_eq!(SequenceSpace::TAP.first(), 1);
        assert_eq!(SequenceSpace::TAP.last(), 99_999);
        assert_eq!(SequenceSpace::TAP.size(), 99_999);
        assert!(!SequenceSpace::TAP.contains(0));
        assert!(SequenceSpace::TAP.contains(99_999));
        assert!(!SequenceSpace::TAP.contains(100_000));
    }

    /// The bug this module exists to prevent, in all three of its forms.
    #[test]
    fn the_wrap_is_not_a_gap_a_regression_or_an_underflow() {
        let last = tap(99_999);
        let first = tap(1);

        assert!(last.is_successor(first), "the wrap is a successor");
        assert_eq!(last.gap_to(first), Some(0), "and not a 99 998-file gap");
        assert!(!last.is_regression(first), "nor a backward move");

        // The naive checks it replaces, spelled out.
        assert_ne!(last.get() + 1, first.get());
        assert!(first.get() < last.get());
    }

    #[test]
    fn gaps_across_the_wrap_count_correctly() {
        assert_eq!(tap(99_998).gap_to(tap(2)), Some(2)); // 99999 and 1 missing
        assert_eq!(tap(99_999).gap_to(tap(3)), Some(2)); // 1 and 2 missing
        assert_eq!(tap(5).gap_to(tap(8)), Some(2)); // 6 and 7 missing
    }

    #[test]
    fn a_repeat_and_a_regression_are_neither_gaps_nor_successors() {
        assert_eq!(tap(7).gap_to(tap(7)), None);
        assert_eq!(tap(10).gap_to(tap(7)), None);
        assert!(tap(10).is_regression(tap(7)));
        assert!(!tap(7).is_regression(tap(10)));
    }

    #[test]
    fn values_from_different_spaces_do_not_compare() {
        let a = tap(5);
        let b = SequenceValue::new(SequenceSpace::U32, 5).unwrap();
        assert!(!a.same_space(b));
        assert_eq!(a.gap_to(b), None);
        assert!(!a.is_successor(b));
        assert!(!a.is_regression(b));
    }

    #[test]
    fn a_value_outside_its_space_cannot_be_constructed() {
        assert!(SequenceValue::new(SequenceSpace::TAP, 0).is_none());
        assert!(SequenceValue::new(SequenceSpace::TAP, 100_000).is_none());
        assert!(SequenceSpace::new(5, 5).is_none());
        assert!(SequenceSpace::new(9, 3).is_none());
    }

    #[test]
    fn a_successor_wraps_at_the_end_of_the_space() {
        assert_eq!(tap(99_999).successor().get(), 1);
        assert_eq!(tap(1).successor().get(), 2);
        let top = SequenceValue::new(SequenceSpace::U32, u32::MAX).unwrap();
        assert_eq!(top.successor().get(), 0);
    }

    // ── Tracker ──────────────────────────────────────────────────────────────

    #[test]
    fn a_tracker_reports_the_identities_of_missing_files() {
        let mut tracker = SequenceTracker::new(SequenceSpace::TAP);

        assert_eq!(tracker.observe_default(10), SequenceObservation::First);
        assert_eq!(tracker.observe_default(11), SequenceObservation::InOrder);
        assert_eq!(
            tracker.observe_default(15),
            SequenceObservation::Gap {
                missing: vec![12, 13, 14],
                missing_count: 3,
            }
        );

        assert_eq!(tracker.missing(), vec![12, 13, 14]);
        assert!(!tracker.is_complete());
        assert_eq!(tracker.high_water(), Some(15));
    }

    /// A resend closes the hole, and only then is the source complete again.
    #[test]
    fn a_resend_fills_the_gap_it_was_requested_for() {
        let mut tracker = SequenceTracker::new(SequenceSpace::TAP);
        tracker.observe_default(1);
        tracker.observe_default(4);
        assert_eq!(tracker.missing(), vec![2, 3]);

        assert_eq!(tracker.observe_default(3), SequenceObservation::FilledGap);
        assert_eq!(tracker.missing(), vec![2]);
        assert!(!tracker.is_complete());

        assert_eq!(tracker.observe_default(2), SequenceObservation::FilledGap);
        assert!(tracker.is_complete());
    }

    #[test]
    fn a_repeat_and_a_stale_value_are_told_apart() {
        let mut tracker = SequenceTracker::new(SequenceSpace::TAP);
        tracker.observe_default(10);
        tracker.observe_default(11);

        assert_eq!(tracker.observe_default(11), SequenceObservation::Duplicate);
        // 5 was never expected: tracking began at 10.
        assert_eq!(tracker.observe_default(5), SequenceObservation::Unexpected);
        assert!(tracker.is_complete());
    }

    /// The whole point, end to end: a source that wraps mid-stream stays
    /// complete and does not manufacture a hole.
    #[test]
    fn a_source_that_wraps_stays_complete() {
        let mut tracker = SequenceTracker::new(SequenceSpace::TAP);
        for value in [99_997, 99_998, 99_999, 1, 2, 3] {
            let observation = tracker.observe_default(value);
            assert!(
                matches!(
                    observation,
                    SequenceObservation::First | SequenceObservation::InOrder
                ),
                "{value} was classified as {observation:?}"
            );
        }
        assert!(tracker.is_complete());
        assert_eq!(tracker.high_water(), Some(3));
    }

    #[test]
    fn a_gap_that_straddles_the_wrap_names_the_right_files() {
        let mut tracker = SequenceTracker::new(SequenceSpace::TAP);
        tracker.observe_default(99_998);
        assert_eq!(
            tracker.observe_default(2),
            SequenceObservation::Gap {
                missing: vec![99_999, 1],
                missing_count: 2,
            }
        );
        assert_eq!(tracker.missing(), vec![1, 99_999]);
    }

    #[test]
    fn an_out_of_space_value_is_rejected_rather_than_tracked() {
        let mut tracker = SequenceTracker::new(SequenceSpace::TAP);
        tracker.observe_default(5);
        assert_eq!(
            tracker.observe_default(200_000),
            SequenceObservation::OutOfSpace
        );
        assert_eq!(tracker.high_water(), Some(5));
        assert!(tracker.is_complete());
    }

    /// A corrupt sequence number must not be able to allocate the whole space —
    /// and must not be able to make an incomplete source look complete either.
    /// The list is bounded; the count never is.
    #[test]
    fn the_missing_list_is_bounded_but_the_count_is_exact() {
        let mut tracker = SequenceTracker::new(SequenceSpace::TAP);
        tracker.observe_default(1);
        let observation = tracker.observe(40_000, 100);
        match observation {
            SequenceObservation::Gap {
                missing,
                missing_count,
            } => {
                assert_eq!(missing.len(), 100, "the list is capped");
                assert_eq!(missing_count, 39_998, "the count is not");
            }
            other => panic!("expected a gap, got {other:?}"),
        }
        // The whole hole is remembered, not just the hundred that were named:
        // a tracker that forgot the rest would call this source complete after
        // a hundred resends while 39 898 files were still missing.
        assert_eq!(tracker.missing_count(), 39_998);
        assert_eq!(tracker.hole_count(), 1, "one run, not 39 998 entries");
        assert_eq!(tracker.missing_runs(), vec![(2, 39_999)]);
        assert!(!tracker.is_complete());
    }

    /// The pathological case the run representation exists for: the largest
    /// forward jump RFC 1982 will read as a gap — just under half a 32-bit
    /// space — must cost one run, must be counted exactly, and must not be able
    /// to allocate two billion entries on the way.
    #[test]
    fn the_largest_readable_gap_costs_one_run() {
        let mut tracker = SequenceTracker::new(SequenceSpace::U32);
        tracker.observe_default(0);

        let arriving = 2_000_000_000u32;
        let observation = tracker.observe(arriving, 4);
        let expected = u64::from(arriving) - 1;

        assert_eq!(observation.missing_count(), expected);
        assert!(observation.missing_is_truncated());
        assert_eq!(tracker.hole_count(), 1, "two billion holes, one run");
        assert_eq!(tracker.missing_count(), expected);
        assert_eq!(tracker.missing_upto(3), vec![1, 2, 3]);
        assert_eq!(tracker.missing_runs(), vec![(1, arriving - 1)]);
    }

    /// The other half of RFC 1982: a jump of *more* than half the space is a
    /// regression, not a gap, so it opens nothing at all.
    #[test]
    fn a_jump_past_half_the_space_is_a_regression_not_a_gap() {
        let mut tracker = SequenceTracker::new(SequenceSpace::U32);
        tracker.observe_default(0);
        assert_eq!(
            tracker.observe_default(u32::MAX),
            SequenceObservation::Unexpected
        );
        assert!(tracker.is_complete());
        assert_eq!(tracker.high_water(), Some(0));
    }

    /// Filling a value in the middle of a run splits it, and the count stays
    /// exact on both sides.
    #[test]
    fn filling_the_middle_of_a_run_splits_it() {
        let mut tracker = SequenceTracker::new(SequenceSpace::TAP);
        tracker.observe_default(1);
        tracker.observe_default(10); // 2..=9 missing
        assert_eq!(tracker.hole_count(), 1);
        assert_eq!(tracker.missing_count(), 8);

        assert_eq!(tracker.observe_default(6), SequenceObservation::FilledGap);
        assert_eq!(tracker.hole_count(), 2);
        assert_eq!(tracker.missing_count(), 7);
        assert_eq!(tracker.missing_runs(), vec![(2, 5), (7, 9)]);
        assert!(tracker.is_missing(5));
        assert!(!tracker.is_missing(6));
    }

    /// Two holes that grow into each other become one run, and the count is not
    /// double-charged for the join.
    #[test]
    fn adjacent_runs_merge() {
        let mut tracker = SequenceTracker::new(SequenceSpace::TAP);
        tracker.observe_default(1);
        tracker.observe_default(5); // 2..=4
        tracker.observe_default(4); // fills 4, leaving 2..=3
        tracker.observe_default(9); // 6..=8
        assert_eq!(tracker.missing_runs(), vec![(2, 3), (6, 8)]);

        // 5 arrives late? No — 5 was already seen. Fill the join instead.
        tracker.observe_default(2);
        tracker.observe_default(3);
        assert_eq!(tracker.missing_runs(), vec![(6, 8)]);
        assert_eq!(tracker.missing_count(), 3);
    }

    /// A tracker is state a restart must not lose.
    #[test]
    fn a_tracker_round_trips_through_its_snapshot() {
        let mut tracker = SequenceTracker::new(SequenceSpace::TAP);
        tracker.observe_default(99_998);
        tracker.observe_default(4); // a hole straddling the wrap
        tracker.observe_default(2);

        let restored = SequenceTracker::restore(tracker.snapshot()).expect("self-consistent");
        assert_eq!(restored, tracker);
        assert_eq!(restored.missing_runs(), tracker.missing_runs());
        assert_eq!(restored.missing_count(), tracker.missing_count());
    }

    #[test]
    fn a_contradictory_snapshot_is_refused_rather_than_restored() {
        let mut snapshot = SequenceTracker::new(SequenceSpace::TAP).snapshot();
        snapshot.high_water = Some(200_000);
        assert!(SequenceTracker::restore(snapshot).is_none());

        let mut overlapping = SequenceTracker::new(SequenceSpace::TAP).snapshot();
        overlapping.holes = vec![(0, 5), (3, 5)];
        assert!(SequenceTracker::restore(overlapping).is_none());
    }

    /// A hole a partner will never fill must stop blocking future batches, and
    /// abandoning it is the caller's explicit decision.
    #[test]
    fn abandoning_a_hole_is_explicit_and_reported() {
        let mut tracker = SequenceTracker::new(SequenceSpace::TAP);
        tracker.observe_default(1);
        tracker.observe_default(4); // 2 and 3 missing
        for value in 5..=20 {
            tracker.observe_default(value);
        }
        assert_eq!(tracker.missing(), vec![2, 3]);

        // Nothing older than 100 steps back yet.
        assert_eq!(tracker.abandon_older_than(100), 0);
        assert!(!tracker.is_complete());

        // Ten steps back does reach them.
        assert_eq!(tracker.abandon_older_than(10), 2);
        assert!(tracker.is_complete());
    }
}
