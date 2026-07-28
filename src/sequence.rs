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

use std::collections::BTreeSet;
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
    /// response is a resend request for exactly these.
    Gap {
        /// The sequence numbers that were skipped over.
        missing: Vec<u32>,
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
}

/// Tracks one source's sequence numbers and reports what is missing.
///
/// One tracker per **scope**, and the scope is part of the standard: a TAP
/// sequence is per sender/recipient TADIG pair, a CDR file counter is per CGF
/// node, a `localSequenceNumber` is per recording entity. Sharing a tracker
/// across scopes manufactures gaps that do not exist.
///
/// Holds only the outstanding holes, not every value ever seen, so memory is
/// proportional to what is actually missing rather than to throughput.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SequenceTracker {
    space: SequenceSpace,
    /// The most recent value under RFC 1982 ordering, once anything has arrived.
    high_water: Option<u32>,
    /// Values skipped over and not yet filled.
    missing: BTreeSet<u32>,
}

impl SequenceTracker {
    /// A tracker over a sequence space, having seen nothing.
    #[must_use]
    pub const fn new(space: SequenceSpace) -> Self {
        Self {
            space,
            high_water: None,
            missing: BTreeSet::new(),
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

    /// The sequence numbers known to be missing, in ascending order.
    #[must_use]
    pub fn missing(&self) -> Vec<u32> {
        self.missing.iter().copied().collect()
    }

    /// How many values are known to be missing.
    #[must_use]
    pub fn missing_count(&self) -> usize {
        self.missing.len()
    }

    /// `true` when nothing is outstanding.
    ///
    /// The precondition for releasing a batch: a source with an unexplained
    /// hole has unknown completeness, and billing over unknown completeness
    /// invoices an amount nobody can prove is right.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.missing.is_empty()
    }

    /// Record an arrival and classify it.
    ///
    /// A gap larger than `max_gap` is still reported, but the missing
    /// identities are truncated to that many — an unbounded list would let a
    /// single corrupt sequence number allocate the whole space. The truncation
    /// is visible in the returned [`SequenceObservation::Gap`] because the list
    /// is shorter than the jump, and callers that need the exact set should
    /// raise the bound deliberately.
    pub fn observe(&mut self, value: u32, max_gap: usize) -> SequenceObservation {
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
            return if self.missing.remove(&value) {
                SequenceObservation::FilledGap
            } else {
                SequenceObservation::Unexpected
            };
        }

        // Forward. Anything skipped over becomes a hole.
        //
        // The distance is computed directly rather than through `gap_to`, whose
        // `None` cases — equal, and backward — are both already excluded above.
        // Reading that `None` as "no gap" would hide a hole, which is the one
        // direction this module must never fail in.
        self.high_water = Some(value);
        let skipped = self.space.forward_distance(previous.get(), value) - 1;
        if skipped == 0 {
            return SequenceObservation::InOrder;
        }

        let mut missing = Vec::new();
        let mut cursor = previous.successor();
        for _ in 0..skipped.min(max_gap as u64) {
            missing.push(cursor.get());
            self.missing.insert(cursor.get());
            cursor = cursor.successor();
        }
        SequenceObservation::Gap { missing }
    }

    /// Record an arrival with a default bound of 10 000 missing identities.
    pub fn observe_default(&mut self, value: u32) -> SequenceObservation {
        self.observe(value, 10_000)
    }

    /// Forget holes older than the resend window, accepting them as lost.
    ///
    /// Removes and returns the missing values that are more than `beyond` steps
    /// behind the high-water mark. A hole a partner will never fill must stop
    /// blocking every future batch, and abandoning it is a decision the caller
    /// makes explicitly rather than one the tracker makes quietly.
    pub fn abandon_older_than(&mut self, beyond: u64) -> Vec<u32> {
        let Some(high) = self.high_water else {
            return Vec::new();
        };
        let abandoned: Vec<u32> = self
            .missing
            .iter()
            .copied()
            .filter(|v| self.space.forward_distance(*v, high) > beyond)
            .collect();
        for value in &abandoned {
            self.missing.remove(value);
        }
        abandoned
    }
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::{SequenceSpace, SequenceValue};
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
                missing: vec![12, 13, 14]
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
                missing: vec![99_999, 1]
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

    /// A corrupt sequence number must not be able to allocate the whole space.
    #[test]
    fn the_missing_list_is_bounded() {
        let mut tracker = SequenceTracker::new(SequenceSpace::TAP);
        tracker.observe_default(1);
        let observation = tracker.observe(40_000, 100);
        match observation {
            SequenceObservation::Gap { missing } => assert_eq!(missing.len(), 100),
            other => panic!("expected a gap, got {other:?}"),
        }
        assert_eq!(tracker.missing_count(), 100);
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
        assert!(tracker.abandon_older_than(100).is_empty());
        assert!(!tracker.is_complete());

        // Ten steps back does reach them.
        let abandoned = tracker.abandon_older_than(10);
        assert_eq!(abandoned, vec![2, 3]);
        assert!(tracker.is_complete());
    }
}
