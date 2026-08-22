//! Splitting one session across tariff-time boundaries.
//!
//! A session that starts before a boundary and ends after it is two pieces of
//! usage priced differently, and deciding where the split falls is mediation's
//! job — the rating layer prices what it is handed, and cannot recover a split
//! that was never made.
//!
//! This module apportions **usage**. It never prices, and it holds no calendar:
//! a tariff boundary is an instant a caller supplies, because when peak begins
//! is a commercial decision that varies by tariff, market and day.
//!
//! # Splitting time is exact; splitting volume is not
//!
//! The distinction this module exists to keep visible. A session's wall-clock
//! seconds on each side of a boundary are known — they follow from the two
//! instants. **When the octets flowed is not known.** A record reports one
//! total for the whole session, and nothing in it says whether the download
//! happened before the boundary or after.
//!
//! So a volume split is an *assumption*, and [`Apportionment`] records which
//! one was used. It is labelled on every result rather than mentioned in a
//! comment, because an estimate that reaches a bill unlabelled is
//! indistinguishable from a measurement.
//!
//! The network offers a measured alternative: an element closes a service data
//! container at a tariff change and reports the volume on each side.
//! [`split_measured`] uses those. Prefer it wherever the containers are there.
//!
//! # Apportionment conserves
//!
//! Splitting 1 000 octets across three unequal slices by integer division loses
//! octets to truncation, and a mediation layer that loses octets has broken its
//! own conservation law. [`apportion`] uses the largest-remainder method, so
//! **the parts always sum to exactly the whole** — the remainder is distributed
//! rather than discarded, and which slice receives it is deterministic.
//!
//! ```rust
//! use mediation::apportion;
//!
//! // Naive integer division would give 333 + 333 + 333 and lose an octet.
//! let parts = apportion(1_000, &[1, 1, 1]);
//! assert_eq!(parts, vec![334, 333, 333]);
//! assert_eq!(parts.iter().sum::<u64>(), 1_000);
//! ```

use std::fmt;

use time::OffsetDateTime;

use crate::cdrtime::{CdrTimestamp, Seconds};
use crate::container::ChangeCondition;
use crate::record::UsageRecord;
use crate::volume::{Octets, VolumePair};

// ── Apportionment ────────────────────────────────────────────────────────────

/// How a slice's quantities were arrived at.
///
/// Carried on every [`UsageSplit`] so an estimate never reaches a rating layer
/// wearing the same face as a measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Apportionment {
    /// The network reported the quantity for this span.
    ///
    /// Either the containers carried it, or no boundary fell inside the session
    /// so nothing needed dividing.
    Measured,
    /// The quantity was divided in proportion to elapsed time.
    ///
    /// An assumption: a record says how much flowed, never when. Sound for a
    /// steady stream, wrong for a burst that happened entirely on one side.
    ProRata,
}

impl Apportionment {
    /// Every variant, for exhaustive iteration in tests and mappings.
    pub const ALL: &'static [Self] = &[Self::Measured, Self::ProRata];

    /// The canonical code of each variant, in [`ALL`](Self::ALL) order.
    pub const CODES: &'static [&'static str] = &["MEASURED", "PRO_RATA"];

    /// `true` when the quantities were reported rather than estimated.
    #[must_use]
    pub const fn is_measured(self) -> bool {
        matches!(self, Self::Measured)
    }

    /// The canonical string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Measured => "MEASURED",
            Self::ProRata => "PRO_RATA",
        }
    }
}

impl fmt::Display for Apportionment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── Largest-remainder apportionment ──────────────────────────────────────────

/// Divide `total` across `weights` so the parts sum to exactly `total`.
///
/// The largest-remainder method: each slice takes its floor share, and the
/// units left over by truncation go one each to the slices with the largest
/// fractional remainders, ties broken by position. Naive integer division would
/// discard them, and a mediation layer that discards octets has broken the
/// conservation law it exists to uphold.
///
/// Returns one part per weight, in the same order.
///
/// **Weights summing to zero divide the total equally**, by the same
/// largest-remainder rule. That case is a real one — a session whose open and
/// close instants are identical still carries the octets it reported — and the
/// alternative of returning nothing would delete usage, which is the one thing
/// this module exists to prevent. Only an *empty* `weights` returns nothing,
/// because there is no slice to put anything on.
///
/// ```rust
/// use mediation::apportion;
///
/// let parts = apportion(100, &[7, 2, 1]);
/// assert_eq!(parts, vec![70, 20, 10]);
/// assert_eq!(parts.iter().sum::<u64>(), 100);
///
/// // The remainder is distributed, never dropped.
/// let awkward = apportion(10, &[1, 1, 1]);
/// assert_eq!(awkward, vec![4, 3, 3]);
/// assert_eq!(awkward.iter().sum::<u64>(), 10);
///
/// // A zero-length session still carries what it reported.
/// assert_eq!(apportion(1_000, &[0]), vec![1_000]);
/// assert_eq!(apportion(10, &[0, 0, 0]), vec![4, 3, 3]);
/// ```
#[must_use]
pub fn apportion(total: u64, weights: &[u64]) -> Vec<u64> {
    if weights.is_empty() {
        return Vec::new();
    }
    let sum: u128 = weights.iter().map(|w| u128::from(*w)).sum();
    // No proportion to divide by, but the total still has to land somewhere:
    // divide it equally. Returning zeros would delete usage, which is the
    // failure this whole module exists to prevent.
    let equal = sum == 0;
    let sum = if equal { weights.len() as u128 } else { sum };

    // u128 throughout: `total * weight` overflows u64 for realistic octet
    // counts long before either factor does.
    let total = u128::from(total);
    let mut parts: Vec<u64> = Vec::with_capacity(weights.len());
    let mut remainders: Vec<(u128, usize)> = Vec::with_capacity(weights.len());
    let mut allocated: u128 = 0;

    for (index, weight) in weights.iter().enumerate() {
        let weight = if equal { 1 } else { u128::from(*weight) };
        let scaled = total * weight;
        let floor = scaled / sum;
        allocated += floor;
        remainders.push((scaled % sum, index));
        parts.push(floor as u64);
    }

    // Descending remainder, ascending index — so the result is the same however
    // the caller's slices happen to be ordered internally.
    remainders.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut left = total - allocated;
    for (_, index) in remainders {
        if left == 0 {
            break;
        }
        parts[index] += 1;
        left -= 1;
    }
    parts
}

// ── Slices ───────────────────────────────────────────────────────────────────

/// One piece of a session, between two instants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeSlice {
    /// When the slice began.
    pub start: OffsetDateTime,
    /// When the slice ended.
    pub end: OffsetDateTime,
    /// Wall-clock seconds in the slice. Exact — it follows from the instants.
    pub wall_clock: Seconds,
    /// Chargeable seconds attributed to the slice, where the record had a
    /// duration to divide.
    pub duration: Option<Seconds>,
    /// Volume attributed to the slice, where the record had one to divide.
    pub volume: Option<VolumePair>,
    /// Events attributed to the slice, where the record had a count to divide.
    pub events: Option<u32>,
}

impl fmt::Display for TimeSlice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} to {} ({} s)", self.start, self.end, self.wall_clock)
    }
}

/// A session divided at tariff boundaries, and how the quantities were arrived
/// at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageSplit {
    slices: Vec<TimeSlice>,
    basis: Apportionment,
}

impl UsageSplit {
    /// The slices, in time order.
    #[must_use]
    pub fn slices(&self) -> &[TimeSlice] {
        &self.slices
    }

    /// How many slices the session became.
    #[must_use]
    pub fn len(&self) -> usize {
        self.slices.len()
    }

    /// Always `false` — a split has at least one slice.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }

    /// How the quantities were arrived at.
    ///
    /// [`Measured`](Apportionment::Measured) when nothing was divided: either
    /// the containers reported each span, or no boundary fell inside the
    /// session at all.
    #[must_use]
    pub const fn basis(&self) -> Apportionment {
        self.basis
    }

    /// `true` when the session was not divided.
    #[must_use]
    pub fn is_whole(&self) -> bool {
        self.slices.len() == 1
    }

    /// The slices' volumes summed back up.
    ///
    /// `None` when **any** slice carried no volume, and `None` when the sum
    /// overflows. All or nothing, deliberately: adding up the slices that
    /// happen to have a figure and presenting the result as the session's total
    /// reports a number short by exactly the slices that did not — which is the
    /// same mistake as summing a partial container set and calling it a total.
    #[must_use]
    pub fn total_volume(&self) -> Option<VolumePair> {
        let mut total = VolumePair::ZERO;
        for slice in &self.slices {
            total = total.checked_add(slice.volume?)?;
        }
        (!self.slices.is_empty()).then_some(total)
    }

    /// The slices' chargeable durations summed back up.
    ///
    /// `None` when **any** slice carried none, for the reason given on
    /// [`total_volume`](Self::total_volume), and `None` on overflow.
    #[must_use]
    pub fn total_duration(&self) -> Option<Seconds> {
        let mut total = Seconds::ZERO;
        for slice in &self.slices {
            total = total.checked_add(slice.duration?)?;
        }
        (!self.slices.is_empty()).then_some(total)
    }

    /// The slices' event counts summed back up.
    ///
    /// `None` when any slice carried none, or the sum overflows.
    #[must_use]
    pub fn total_events(&self) -> Option<u32> {
        let mut total: u32 = 0;
        for slice in &self.slices {
            total = total.checked_add(slice.events?)?;
        }
        (!self.slices.is_empty()).then_some(total)
    }

    /// Whether the split gave back exactly what the record held.
    ///
    /// The conservation law applied to one session: apportionment must not
    /// create or destroy usage. For a [`split_session`] result `false` is a
    /// defect, not a rounding artefact — [`apportion`] is exact by
    /// construction, so a mismatch means something else went wrong.
    ///
    /// For a [`split_measured`] result it means something different and equally
    /// worth knowing: the containers do not add up to the record's own total,
    /// which is the same disagreement
    /// [`ContainerSet::agrees_with`](crate::ContainerSet::agrees_with) reports.
    #[must_use]
    pub fn conserves(&self, record: &UsageRecord) -> bool {
        self.total_volume() == record.volume
            && self.total_duration() == record.duration
            && self.total_events() == record.events
    }
}

// ── Errors ───────────────────────────────────────────────────────────────────

/// Why a session could not be split.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SplitError {
    /// The record has no close time, so its span is unknown.
    ///
    /// An open session cannot be divided: the last slice would have no end, and
    /// assuming one would invent usage.
    NoCloseTime,
    /// The record closed before it opened.
    NegativeSpan,
    /// A container carried no close time, so a measured split is impossible.
    ContainerNotTimed,
    /// A container closed before the band it belongs to began.
    ///
    /// Containers are in record order, which is chronological. One that closes
    /// earlier than its predecessor is a data fault, and clamping the span to
    /// zero would report a band that ran no time at all — a plausible number
    /// for something that did not happen.
    ContainersOutOfOrder,
    /// A container set held no containers to split.
    NoContainers,
    /// The record carries an event count, and containers do not divide one.
    ///
    /// Refused rather than dropped. A container reports volume and time, never
    /// events, so a measured split has nothing to attribute them by — and a
    /// result that silently omitted them would not conserve, while claiming on
    /// its face to be a measurement.
    EventsNotContained,
    /// A quantity did not fit the type it had to be written back into.
    Overflow,
}

impl SplitError {
    /// The canonical string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoCloseTime => "the record has no close time",
            Self::NegativeSpan => "the record closed before it opened",
            Self::ContainerNotTimed => "a container carried no close time",
            Self::ContainersOutOfOrder => "a container closed before its band began",
            Self::NoContainers => "there are no containers to split",
            Self::EventsNotContained => "containers cannot divide an event count",
            Self::Overflow => "a quantity overflowed",
        }
    }
}

impl fmt::Display for SplitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::error::Error for SplitError {}

// ── Pro-rata split ───────────────────────────────────────────────────────────

/// Divide a session at the boundaries that fall inside it, pro rata.
///
/// Boundaries are instants the caller supplies — from their own tariff
/// calendar, in their own time zone. Those outside the session are ignored,
/// duplicates collapse, and order does not matter, so a caller may hand over a
/// whole day's boundaries without filtering first.
///
/// # What this estimates
///
/// The wall-clock seconds of each slice are exact. The **volume, duration and
/// event count are pro-rata estimates**, because a record reports totals for
/// the whole session and never says when within it they accrued. The result is
/// stamped [`Apportionment::ProRata`] whenever anything was divided, and
/// [`Measured`](Apportionment::Measured) when no boundary fell inside — because
/// then nothing was estimated at all.
///
/// Prefer [`split_measured`] where the record's containers carry tariff-time
/// boundaries: those are what the network actually counted.
///
/// ```rust
/// use mediation::{Apportionment, Octets, VolumePair, split_session};
/// # use mediation::{ChargedParty, RecordType, SourceRef, SubscriberRef, UsageRecord};
/// use time::macros::datetime;
///
/// # let record = UsageRecord::new(
/// #     RecordType::PduSession,
/// #     ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
/// #     "2026-07-28T07:00:00+02:00".parse().unwrap(),
/// #     SourceRef::new("pgw-01".parse().unwrap(), 0),
/// # )
/// # .with_closed("2026-07-28T09:00:00+02:00".parse().unwrap())
/// # .with_volume(VolumePair::new(Octets::new(900), Octets::new(300)));
/// // A two-hour session with peak starting an hour in.
/// let split = split_session(&record, &[datetime!(2026-07-28 08:00 +2)]).unwrap();
///
/// assert_eq!(split.len(), 2);
/// assert_eq!(split.basis(), Apportionment::ProRata);
/// // Equal halves, so the volume divides evenly — and sums back to the whole.
/// assert_eq!(split.slices()[0].volume.unwrap().uplink(), Octets::new(450));
/// assert!(split.conserves(&record));
/// ```
pub fn split_session(
    record: &UsageRecord,
    boundaries: &[OffsetDateTime],
) -> Result<UsageSplit, SplitError> {
    let opened = record.opened.to_utc();
    let closed = record.closed.ok_or(SplitError::NoCloseTime)?.to_utc();
    if closed < opened {
        return Err(SplitError::NegativeSpan);
    }

    // Strictly inside: a boundary at either end divides nothing, and admitting
    // it would produce a zero-length slice that carries no usage and no
    // meaning.
    let mut cuts: Vec<OffsetDateTime> = boundaries
        .iter()
        .copied()
        .filter(|at| *at > opened && *at < closed)
        .collect();
    cuts.sort();
    cuts.dedup();

    let mut edges = Vec::with_capacity(cuts.len() + 2);
    edges.push(opened);
    edges.extend(cuts);
    edges.push(closed);

    let mut spans: Vec<u64> = Vec::with_capacity(edges.len() - 1);
    for pair in edges.windows(2) {
        let seconds = (pair[1] - pair[0]).whole_seconds();
        spans.push(u64::try_from(seconds).map_err(|_| SplitError::Overflow)?);
    }

    let uplink = record
        .volume
        .map(|v| apportion(v.uplink().get(), &spans))
        .unwrap_or_default();
    let downlink = record
        .volume
        .map(|v| apportion(v.downlink().get(), &spans))
        .unwrap_or_default();
    let duration = record
        .duration
        .map(|d| apportion(u64::from(d.get()), &spans))
        .unwrap_or_default();
    let events = record
        .events
        .map(|e| apportion(u64::from(e), &spans))
        .unwrap_or_default();

    let mut slices = Vec::with_capacity(spans.len());
    for (index, span) in spans.iter().enumerate() {
        slices.push(TimeSlice {
            start: edges[index],
            end: edges[index + 1],
            wall_clock: Seconds::new(u32::try_from(*span).map_err(|_| SplitError::Overflow)?),
            duration: match duration.get(index) {
                Some(seconds) => Some(Seconds::new(
                    u32::try_from(*seconds).map_err(|_| SplitError::Overflow)?,
                )),
                None => None,
            },
            volume: record
                .volume
                .map(|_| VolumePair::new(Octets::new(uplink[index]), Octets::new(downlink[index]))),
            events: match events.get(index) {
                Some(count) => Some(u32::try_from(*count).map_err(|_| SplitError::Overflow)?),
                None => None,
            },
        });
    }

    // Nothing divided means nothing estimated.
    let basis = if slices.len() == 1 {
        Apportionment::Measured
    } else {
        Apportionment::ProRata
    };
    Ok(UsageSplit { slices, basis })
}

// ── Measured split ───────────────────────────────────────────────────────────

/// Divide a session at the tariff boundaries the network itself recorded.
///
/// A charging element closes a service data container when a tariff period
/// changes, so the containers already carry the volume on each side. Using them
/// needs no assumption about when the octets flowed — which is what makes this
/// a measurement and [`split_session`] an estimate.
///
/// Containers accumulate into the current slice, and a container closed with
/// [`ChangeCondition::TariffTime`] ends it. The first slice starts when the
/// **record** opened, not when its first container closed — a container carries
/// only its end. The result is always [`Apportionment::Measured`].
///
/// Every container must carry a close time: without one there is nothing to
/// place a slice boundary at, and inferring one would put the estimate back in
/// through a side door. A record carrying an **event count** is refused
/// outright — containers report volume and time and never events, so there is
/// nothing to divide the count by.
pub fn split_measured(record: &UsageRecord) -> Result<UsageSplit, SplitError> {
    let containers = &record.containers;
    if containers.is_empty() {
        return Err(SplitError::NoContainers);
    }
    // Containers report volume and time. An event count has nothing to be
    // divided by, and a split that quietly dropped it would fail
    // `UsageSplit::conserves` while calling itself a measurement.
    if record.events.is_some() {
        return Err(SplitError::EventsNotContained);
    }

    let all = containers.as_slice();
    let last = all.len() - 1;

    let mut slices: Vec<TimeSlice> = Vec::new();
    let mut slice_start: CdrTimestamp = record.opened;
    let mut previous_close: CdrTimestamp = record.opened;
    let mut volume = VolumePair::ZERO;
    let mut duration: Option<Seconds> = None;
    let mut first_of_slice = 0usize;

    for (index, container) in all.iter().enumerate() {
        let closed = container.closed.ok_or(SplitError::ContainerNotTimed)?;
        // Containers are in record order, which is chronological. Checked at
        // *every* container rather than only at the one that ends a band: a
        // container that closes before its predecessor is a data fault whether
        // or not the band's own span happens to come out positive, and letting
        // it through would attribute its volume to a stretch of time that ran
        // backwards.
        if closed.to_utc() < previous_close.to_utc() {
            return Err(SplitError::ContainersOutOfOrder);
        }
        previous_close = closed;

        volume = volume
            .checked_add(container.volume)
            .ok_or(SplitError::Overflow)?;
        // Every container in a slice must carry a duration or the slice has
        // none. Folding an absent one in as zero would report a chargeable
        // time shorter than the band actually ran — the same mistake as
        // summing a partial container set and calling it a total.
        duration = if index == first_of_slice {
            container.duration
        } else {
            match (duration, container.duration) {
                (Some(so_far), Some(more)) => {
                    Some(so_far.checked_add(more).ok_or(SplitError::Overflow)?)
                }
                _ => None,
            }
        };

        // A tariff change ends a band; so does running out of containers.
        let ends_slice = container.change_condition == ChangeCondition::TariffTime || index == last;
        if ends_slice {
            let span = (closed.to_utc() - slice_start.to_utc()).whole_seconds();
            if span < 0 {
                return Err(SplitError::ContainersOutOfOrder);
            }
            slices.push(TimeSlice {
                start: slice_start.to_utc(),
                end: closed.to_utc(),
                wall_clock: Seconds::new(u32::try_from(span).map_err(|_| SplitError::Overflow)?),
                duration,
                volume: Some(volume),
                events: None,
            });
            slice_start = closed;
            volume = VolumePair::ZERO;
            duration = None;
            first_of_slice = index + 1;
        }
    }

    Ok(UsageSplit {
        slices,
        basis: Apportionment::Measured,
    })
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::Apportionment;
    use serde::de::{self, Visitor};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::fmt;

    impl Serialize for Apportionment {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_str(self.as_str())
        }
    }

    impl<'de> Deserialize<'de> for Apportionment {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct V;
            impl Visitor<'_> for V {
                type Value = Apportionment;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("MEASURED or PRO_RATA")
                }
                fn visit_str<E: de::Error>(self, v: &str) -> Result<Apportionment, E> {
                    Apportionment::ALL
                        .iter()
                        .find(|a| a.as_str() == v)
                        .copied()
                        .ok_or_else(|| de::Error::custom(format!("unknown apportionment {v:?}")))
                }
            }
            deserializer.deserialize_str(V)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container::{ContainerSet, RatingGroup, ServiceDataContainer};
    use crate::party::{ChargedParty, SubscriberRef};
    use crate::record::{RecordType, SourceRef};
    use time::macros::datetime;

    fn session(opened: &str, closed: &str) -> UsageRecord {
        UsageRecord::new(
            RecordType::PduSession,
            ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
            opened.parse().unwrap(),
            SourceRef::new("pgw-01".parse().unwrap(), 0),
        )
        .with_closed(closed.parse().unwrap())
    }

    // ── Apportionment conserves ──────────────────────────────────────────────

    /// The point of the largest-remainder method: naive division would lose the
    /// truncated units, and a mediation layer that loses octets has broken the
    /// law it exists to uphold.
    #[test]
    fn the_parts_always_sum_to_the_whole() {
        for total in [0u64, 1, 7, 999, 1_000, 1_000_003, u64::MAX / 4] {
            for weights in [
                vec![1u64, 1, 1],
                vec![1, 2, 3, 4],
                vec![7, 2, 1],
                vec![1],
                vec![5, 5],
                vec![1, 0, 1],
            ] {
                let parts = apportion(total, &weights);
                assert_eq!(parts.len(), weights.len());
                assert_eq!(
                    parts.iter().map(|p| u128::from(*p)).sum::<u128>(),
                    u128::from(total),
                    "total {total} over {weights:?}"
                );
            }
        }
    }

    /// The remainder goes to the largest fractional part, ties by position, so
    /// the same input always gives the same answer.
    #[test]
    fn the_remainder_is_distributed_deterministically() {
        assert_eq!(apportion(10, &[1, 1, 1]), vec![4, 3, 3]);
        assert_eq!(apportion(1_000, &[1, 1, 1]), vec![334, 333, 333]);
        assert_eq!(apportion(100, &[7, 2, 1]), vec![70, 20, 10]);

        // Two equal remainders: the earlier slice takes the extra unit.
        assert_eq!(apportion(3, &[1, 1]), vec![2, 1]);
        assert_eq!(apportion(3, &[1, 1]), apportion(3, &[1, 1]));
    }

    /// Weights that sum to zero give no proportion to divide by — but the
    /// total still has to land somewhere, and zeroing it would delete usage.
    /// A zero-length session that reported octets is the real case.
    #[test]
    fn zero_weights_divide_equally_rather_than_deleting_the_total() {
        assert_eq!(apportion(100, &[0, 0]), vec![50, 50]);
        assert_eq!(apportion(1_000, &[0]), vec![1_000]);
        assert_eq!(apportion(10, &[0, 0, 0]), vec![4, 3, 3]);
        // Only an empty slice list has nowhere to put anything.
        assert_eq!(apportion(100, &[]), Vec::<u64>::new());
        assert_eq!(apportion(0, &[1, 2]), vec![0, 0]);
    }

    /// The property that matters, stated over every case: what goes in comes
    /// out, whatever the weights are.
    #[test]
    fn apportionment_never_creates_or_destroys() {
        for total in [0u64, 1, 999, u64::MAX] {
            for weights in [
                vec![0u64],
                vec![0, 0, 0],
                vec![1, 0],
                vec![u64::MAX, 1],
                vec![3, 3, 3, 3, 3],
            ] {
                let parts = apportion(total, &weights);
                assert_eq!(
                    parts.iter().map(|p| u128::from(*p)).sum::<u128>(),
                    u128::from(total),
                    "{total} over {weights:?}"
                );
            }
        }
    }

    /// A weight large enough that `total * weight` leaves the u64 range must
    /// still divide correctly.
    #[test]
    fn large_totals_do_not_overflow() {
        let parts = apportion(u64::MAX, &[u64::MAX, u64::MAX]);
        assert_eq!(
            parts.iter().map(|p| u128::from(*p)).sum::<u128>(),
            u128::from(u64::MAX)
        );
    }

    // ── Splitting a session ──────────────────────────────────────────────────

    /// The case the module exists for: a session straddling a tariff boundary
    /// is two pieces of usage, and the rating layer cannot recover a split that
    /// was never made.
    #[test]
    fn a_session_straddling_a_boundary_becomes_two() {
        let record = session("2026-07-28T07:00:00+02:00", "2026-07-28T09:00:00+02:00")
            .with_volume(VolumePair::new(Octets::new(900), Octets::new(300)))
            .with_duration(Seconds::new(7_200));

        let split = split_session(&record, &[datetime!(2026-07-28 08:00 +2)]).unwrap();

        assert_eq!(split.len(), 2);
        assert!(!split.is_whole());
        assert_eq!(split.basis(), Apportionment::ProRata);
        assert!(!split.basis().is_measured());

        for slice in split.slices() {
            assert_eq!(slice.wall_clock, Seconds::new(3_600));
            assert_eq!(slice.volume.unwrap().uplink(), Octets::new(450));
            assert_eq!(slice.duration, Some(Seconds::new(3_600)));
        }
        assert!(split.conserves(&record));
    }

    /// An uneven split still gives back exactly what it was handed.
    #[test]
    fn an_uneven_split_conserves_to_the_octet() {
        let record = session("2026-07-28T07:00:00+02:00", "2026-07-28T08:00:01+02:00")
            .with_volume(VolumePair::new(Octets::new(1_000_003), Octets::new(7)))
            .with_duration(Seconds::new(3_601))
            .with_events(5);

        let split = split_session(
            &record,
            &[
                datetime!(2026-07-28 07:13:07 +2),
                datetime!(2026-07-28 07:41:59 +2),
            ],
        )
        .unwrap();

        assert_eq!(split.len(), 3);
        assert!(
            split.conserves(&record),
            "apportionment must not create or destroy usage"
        );
        assert_eq!(split.total_volume(), record.volume);
        assert_eq!(split.total_duration(), record.duration);

        let wall: u32 = split.slices().iter().map(|s| s.wall_clock.get()).sum();
        assert_eq!(wall, 3_601, "the wall clock is exact by construction");
    }

    /// No boundary inside means nothing was divided — so nothing was estimated,
    /// and the result must not claim otherwise.
    #[test]
    fn a_session_with_no_boundary_inside_is_still_measured() {
        let record = session("2026-07-28T07:00:00+02:00", "2026-07-28T08:00:00+02:00")
            .with_volume(VolumePair::new(Octets::new(900), Octets::new(300)));

        // Boundaries at the edges and outside divide nothing.
        let split = split_session(
            &record,
            &[
                datetime!(2026-07-28 07:00 +2),
                datetime!(2026-07-28 08:00 +2),
                datetime!(2026-07-28 22:00 +2),
                datetime!(2026-07-27 03:00 +2),
            ],
        )
        .unwrap();

        assert!(split.is_whole());
        assert_eq!(split.basis(), Apportionment::Measured);
        assert!(split.basis().is_measured());
        assert_eq!(split.slices()[0].volume, record.volume);
    }

    /// A caller may hand over a whole day's boundaries in any order, with
    /// repeats, and get the same answer.
    #[test]
    fn boundaries_are_deduplicated_and_ordered() {
        let record = session("2026-07-28T06:00:00+02:00", "2026-07-28T10:00:00+02:00")
            .with_volume(VolumePair::new(Octets::new(400), Octets::new(400)));

        let tidy = split_session(
            &record,
            &[
                datetime!(2026-07-28 08:00 +2),
                datetime!(2026-07-28 09:00 +2),
            ],
        )
        .unwrap();
        let messy = split_session(
            &record,
            &[
                datetime!(2026-07-28 09:00 +2),
                datetime!(2026-07-28 08:00 +2),
                datetime!(2026-07-28 09:00 +2),
                datetime!(2026-07-28 05:00 +2),
            ],
        )
        .unwrap();

        assert_eq!(tidy, messy);
        assert_eq!(tidy.len(), 3);
    }

    /// The same instant written at two offsets is one boundary, not two — the
    /// comparison is on the instant, as everywhere else in this crate.
    #[test]
    fn a_boundary_is_an_instant_not_a_wall_clock_reading() {
        let record = session("2026-07-28T06:00:00+02:00", "2026-07-28T10:00:00+02:00")
            .with_volume(VolumePair::new(Octets::new(400), Octets::new(400)));

        let split = split_session(
            &record,
            &[
                datetime!(2026-07-28 08:00 +2),
                datetime!(2026-07-28 06:00 UTC), // the same instant
            ],
        )
        .unwrap();

        assert_eq!(split.len(), 2, "one boundary, written two ways");
    }

    /// An open session cannot be divided: the last slice would have no end, and
    /// assuming one would invent usage.
    #[test]
    fn an_open_session_cannot_be_split() {
        let open = UsageRecord::new(
            RecordType::PduSession,
            ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
            "2026-07-28T07:00:00+02:00".parse().unwrap(),
            SourceRef::new("pgw-01".parse().unwrap(), 0),
        );
        assert_eq!(
            split_session(&open, &[datetime!(2026-07-28 08:00 +2)]),
            Err(SplitError::NoCloseTime)
        );

        let backwards = session("2026-07-28T09:00:00+02:00", "2026-07-28T07:00:00+02:00");
        assert_eq!(
            split_session(&backwards, &[]),
            Err(SplitError::NegativeSpan)
        );
        assert!(SplitError::NoCloseTime.to_string().contains("close time"));
    }

    /// A record with no quantities splits into slices that claim none — an
    /// absent volume is not zero.
    #[test]
    fn absent_quantities_stay_absent() {
        let record = session("2026-07-28T07:00:00+02:00", "2026-07-28T09:00:00+02:00");
        let split = split_session(&record, &[datetime!(2026-07-28 08:00 +2)]).unwrap();

        assert_eq!(split.len(), 2);
        for slice in split.slices() {
            assert_eq!(slice.volume, None);
            assert_eq!(slice.duration, None);
            assert_eq!(slice.events, None);
            assert_eq!(slice.wall_clock, Seconds::new(3_600));
        }
        assert_eq!(split.total_volume(), None);
        assert!(split.conserves(&record));
    }

    // ── The measured alternative ─────────────────────────────────────────────

    fn container(
        condition: ChangeCondition,
        closed: &str,
        up: u64,
        down: u64,
    ) -> ServiceDataContainer {
        ServiceDataContainer::new(RatingGroup::new(10), condition)
            .with_volume(VolumePair::new(Octets::new(up), Octets::new(down)))
            .with_closed(closed.parse().unwrap())
    }

    /// What the network counted beats what a library assumes: the containers
    /// carry the volume on each side of the tariff change.
    #[test]
    fn containers_give_a_split_that_needed_no_assumption() {
        let containers = ContainerSet::from(vec![
            container(
                ChangeCondition::TariffTime,
                "2026-07-28T08:00:00+02:00",
                1_000,
                50,
            ),
            container(
                ChangeCondition::RecordClosure,
                "2026-07-28T09:00:00+02:00",
                20,
                5,
            ),
        ]);

        let record = session("2026-07-28T07:00:00+02:00", "2026-07-28T09:00:00+02:00")
            .with_containers(containers);
        let split = split_measured(&record).unwrap();

        assert_eq!(split.basis(), Apportionment::Measured);
        assert_eq!(
            split.slices()[0].wall_clock,
            Seconds::new(3_600),
            "the first slice starts when the record opened"
        );
        assert_eq!(split.len(), 2);
        // The burst happened before the boundary, which pro rata would have
        // spread evenly and got wrong.
        assert_eq!(
            split.slices()[0].volume.unwrap().uplink(),
            Octets::new(1_000)
        );
        assert_eq!(split.slices()[1].volume.unwrap().uplink(), Octets::new(20));
    }

    /// Containers accumulate until a tariff change closes the slice, so a QoS
    /// change mid-band does not manufacture a boundary.
    #[test]
    fn only_a_tariff_change_ends_a_band() {
        let containers = ContainerSet::from(vec![
            container(
                ChangeCondition::QosChange,
                "2026-07-28T07:30:00+02:00",
                100,
                10,
            ),
            container(
                ChangeCondition::TariffTime,
                "2026-07-28T08:00:00+02:00",
                200,
                20,
            ),
            container(
                ChangeCondition::RecordClosure,
                "2026-07-28T09:00:00+02:00",
                30,
                3,
            ),
        ]);

        let record = session("2026-07-28T07:00:00+02:00", "2026-07-28T09:00:00+02:00")
            .with_containers(containers);
        let split = split_measured(&record).unwrap();
        assert_eq!(split.len(), 2);
        assert_eq!(split.slices()[0].volume.unwrap().uplink(), Octets::new(300));
        assert_eq!(split.slices()[1].volume.unwrap().uplink(), Octets::new(30));
    }

    /// Containers arrive in record order, which is chronological. One closing
    /// before its band began is a fault, and a zero-second band would be a
    /// plausible number for something that did not happen.
    #[test]
    fn containers_out_of_order_are_refused() {
        let containers = ContainerSet::from(vec![container(
            ChangeCondition::RecordClosure,
            "2026-07-28T06:00:00+02:00",
            10,
            1,
        )]);
        let record = session("2026-07-28T07:00:00+02:00", "2026-07-28T09:00:00+02:00")
            .with_containers(containers);

        assert_eq!(
            split_measured(&record),
            Err(SplitError::ContainersOutOfOrder)
        );
        assert!(
            SplitError::ContainersOutOfOrder
                .to_string()
                .contains("before its band began")
        );
    }

    /// Without a close time there is nothing to place a boundary at, and
    /// inferring one would put the estimate back in through a side door.
    #[test]
    fn an_untimed_container_refuses_rather_than_guesses() {
        let containers = ContainerSet::from(vec![
            ServiceDataContainer::new(RatingGroup::new(10), ChangeCondition::TariffTime)
                .with_volume(VolumePair::new(Octets::new(100), Octets::new(10))),
        ]);
        let record = session("2026-07-28T07:00:00+02:00", "2026-07-28T09:00:00+02:00")
            .with_containers(containers);
        assert_eq!(split_measured(&record), Err(SplitError::ContainerNotTimed));

        let bare = session("2026-07-28T07:00:00+02:00", "2026-07-28T09:00:00+02:00");
        assert_eq!(split_measured(&bare), Err(SplitError::NoContainers));
    }

    /// A container with no duration must not be folded in as zero: the slice
    /// would report a chargeable time shorter than the band actually ran.
    #[test]
    fn one_untimed_container_makes_the_slices_duration_unknown() {
        let timed = |closed: &str, seconds: u32| {
            container(ChangeCondition::QosChange, closed, 10, 1)
                .with_duration(Seconds::new(seconds))
        };

        let both = ContainerSet::from(vec![
            timed("2026-07-28T07:30:00+02:00", 1_800),
            container(
                ChangeCondition::RecordClosure,
                "2026-07-28T08:00:00+02:00",
                10,
                1,
            )
            .with_duration(Seconds::new(1_800)),
        ]);
        let record =
            session("2026-07-28T07:00:00+02:00", "2026-07-28T08:00:00+02:00").with_containers(both);
        assert_eq!(
            split_measured(&record).unwrap().slices()[0].duration,
            Some(Seconds::new(3_600))
        );

        // The second container carries no duration, so the slice has none.
        let partial = ContainerSet::from(vec![
            timed("2026-07-28T07:30:00+02:00", 1_800),
            container(
                ChangeCondition::RecordClosure,
                "2026-07-28T08:00:00+02:00",
                10,
                1,
            ),
        ]);
        let record = session("2026-07-28T07:00:00+02:00", "2026-07-28T08:00:00+02:00")
            .with_containers(partial);
        assert_eq!(split_measured(&record).unwrap().slices()[0].duration, None);

        // And so does the first carrying none.
        let leading = ContainerSet::from(vec![
            container(
                ChangeCondition::QosChange,
                "2026-07-28T07:30:00+02:00",
                10,
                1,
            ),
            timed("2026-07-28T08:00:00+02:00", 1_800),
        ]);
        let record = session("2026-07-28T07:00:00+02:00", "2026-07-28T08:00:00+02:00")
            .with_containers(leading);
        assert_eq!(split_measured(&record).unwrap().slices()[0].duration, None);
    }

    /// Each band starts where the previous one ended, so the slices tile the
    /// session without a gap.
    #[test]
    fn the_bands_tile_the_session() {
        let containers = ContainerSet::from(vec![
            container(
                ChangeCondition::TariffTime,
                "2026-07-28T08:00:00+02:00",
                100,
                10,
            ),
            container(
                ChangeCondition::TariffTime,
                "2026-07-28T09:00:00+02:00",
                200,
                20,
            ),
            container(
                ChangeCondition::RecordClosure,
                "2026-07-28T09:30:00+02:00",
                50,
                5,
            ),
        ]);
        let record = session("2026-07-28T07:00:00+02:00", "2026-07-28T09:30:00+02:00")
            .with_containers(containers);

        let split = split_measured(&record).unwrap();
        assert_eq!(split.len(), 3);
        for pair in split.slices().windows(2) {
            assert_eq!(pair[0].end, pair[1].start, "no gap between bands");
        }
        assert_eq!(split.slices()[0].start, record.opened.to_utc());
        let total: u32 = split.slices().iter().map(|s| s.wall_clock.get()).sum();
        assert_eq!(total, 9_000, "two and a half hours, tiled exactly");
    }

    #[test]
    fn apportionment_codes_round_trip() {
        assert_eq!(Apportionment::ALL.len(), Apportionment::CODES.len());
        for (basis, code) in Apportionment::ALL.iter().zip(Apportionment::CODES) {
            assert_eq!(basis.as_str(), *code);
            assert_eq!(basis.to_string(), *code);
        }
    }
}
