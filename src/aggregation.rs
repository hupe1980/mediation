//! What the batch adds up to.
//!
//! Every other layer of this crate decides whether a record *may* be billed.
//! This one says what the bill is **of** — and it is the last place a wrong
//! number can still be caught, because after it the quantities become money.
//!
//! # A total never travels without its evidence
//!
//! [`aggregate_released`] returns a [`ReleasedTotals`]: the sum **and** the
//! [`crate::BatchReconciliation`] that produced it. A total
//! on its own cannot be audited — nobody can tell whether 4 TB is every record
//! or the two-thirds that happened to parse — so the two are one value and the
//! conservation law travels with the number it justifies.
//!
//! ```rust
//! use mediation::{Disposition, aggregate_released};
//! # use mediation::{ChargedParty, Octets, RecordType, SourceRef, SubscriberRef, UsageRecord,
//! #                 VolumePair};
//! # fn record(up: u64) -> UsageRecord {
//! #     UsageRecord::new(
//! #         RecordType::PduSession,
//! #         ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
//! #         "2026-07-28T10:00:00+02:00".parse().unwrap(),
//! #         SourceRef::new("pgw-01".parse().unwrap(), 0),
//! #     ).with_volume(VolumePair::new(Octets::new(up), Octets::ZERO))
//! # }
//! let records = vec![record(100), record(200), record(400)];
//! let dispositions = vec![
//!     Disposition::Billable,
//!     Disposition::Billable,
//!     Disposition::Rejected(mediation::RejectReason::Malformed),
//! ];
//!
//! let released = aggregate_released(&records, &dispositions).unwrap();
//!
//! assert_eq!(released.total().volume().uplink(), Octets::new(300));
//! assert_eq!(released.total().records(), 2);
//! assert_eq!(released.reconciliation().ingested(), 3, "all three are accounted for");
//! assert!(released.reconciliation().balances());
//! ```
//!
//! # Aggregation refuses to guess
//!
//! A record carrying both a total and a container breakdown makes two
//! statements of one fact. When they disagree, summing either is **picking a
//! side** — and only the operator knows which node is misbehaving. So
//! aggregation stops and names the record, rather than producing a number that
//! is quietly wrong by the size of the discrepancy.
//!
//! Overflow is treated the same way: a wrapped total is a small plausible
//! number, and small plausible wrong numbers are the ones that reach an invoice.

use std::collections::BTreeMap;
use std::fmt;

use crate::cdrtime::Seconds;
use crate::container::RatingGroup;
use crate::disposition::{BatchReconciliation, Disposition};
use crate::record::UsageRecord;
use crate::volume::VolumePair;

// ── UsageTotal ───────────────────────────────────────────────────────────────

/// Summed usage, with the count of records that produced it.
///
/// The count is not decoration: a total without it cannot be checked against a
/// disposition report, and a total that cannot be checked is a number somebody
/// has to trust.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UsageTotal {
    volume: VolumePair,
    duration: Seconds,
    events: u64,
    records: u64,
    unquantified: u64,
}

impl UsageTotal {
    /// Nothing summed yet.
    pub const ZERO: Self = Self {
        volume: VolumePair::ZERO,
        duration: Seconds::ZERO,
        events: 0,
        records: 0,
        unquantified: 0,
    };

    /// Transferred volume, uplink and downlink kept apart.
    #[must_use]
    pub const fn volume(&self) -> VolumePair {
        self.volume
    }

    /// Chargeable time.
    #[must_use]
    pub const fn duration(&self) -> Seconds {
        self.duration
    }

    /// Discrete events — messages, API invocations.
    #[must_use]
    pub const fn events(&self) -> u64 {
        self.events
    }

    /// How many records were summed.
    #[must_use]
    pub const fn records(&self) -> u64 {
        self.records
    }

    /// How many of those records carried **no quantity at all**.
    ///
    /// A record with no volume, no duration and no event count contributes
    /// nothing, which is arithmetically correct and operationally suspicious: a
    /// field a decoder dropped contributes nothing too, and the total is then
    /// short by exactly that amount with nothing to show for it.
    ///
    /// Counting them makes the silence visible in the total itself. A batch
    /// where this is non-zero should be read alongside
    /// [`Rule::ZeroUsage`](crate::validation::Rule::ZeroUsage) and
    /// [`Rule::QuantityDoesNotMatchService`](crate::validation::Rule::QuantityDoesNotMatchService),
    /// which say *which* records they were.
    #[must_use]
    pub const fn unquantified(&self) -> u64 {
        self.unquantified
    }

    /// `true` when every summed record contributed something.
    #[must_use]
    pub const fn is_fully_quantified(&self) -> bool {
        self.unquantified == 0
    }

    /// `true` when nothing has been summed.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.records == 0
    }

    /// Add one record's quantities, `None` on overflow.
    ///
    /// Uses the record's own totals where it has them and the container sum
    /// otherwise — see [`UsageRecord::billable_volume`]. An absent quantity
    /// contributes nothing **and** increments
    /// [`unquantified`](Self::unquantified), so a dropped field is visible in
    /// the total rather than merely absent from it.
    #[must_use]
    pub fn checked_add_record(self, record: &UsageRecord) -> Option<Self> {
        Some(Self {
            volume: self
                .volume
                .checked_add(record.billable_volume().unwrap_or(VolumePair::ZERO))?,
            duration: self
                .duration
                .checked_add(record.duration.unwrap_or(Seconds::ZERO))?,
            events: self
                .events
                .checked_add(u64::from(record.events.unwrap_or(0)))?,
            records: self.records.checked_add(1)?,
            unquantified: self
                .unquantified
                .checked_add(u64::from(record.is_zero_usage()))?,
        })
    }

    /// Combine two totals, `None` on overflow.
    ///
    /// For recombining work done over disjoint key ranges, exactly as
    /// [`BatchReconciliation::checked_add`](crate::BatchReconciliation::checked_add).
    #[must_use]
    pub fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            volume: self.volume.checked_add(other.volume)?,
            duration: self.duration.checked_add(other.duration)?,
            events: self.events.checked_add(other.events)?,
            records: self.records.checked_add(other.records)?,
            unquantified: self.unquantified.checked_add(other.unquantified)?,
        })
    }
}

impl fmt::Display for UsageTotal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} record(s): {} octets, {} s, {} event(s)",
            self.records, self.volume, self.duration, self.events
        )?;
        if self.unquantified > 0 {
            write!(f, " — {} carried no quantity", self.unquantified)?;
        }
        Ok(())
    }
}

// ── Errors ───────────────────────────────────────────────────────────────────

/// Why a batch could not be summed.
///
/// Both variants name the record, because a total that cannot be produced is
/// useless without knowing which record to go and look at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggregationError {
    /// A record's own total and its container breakdown disagree.
    ///
    /// Summing either would pick a side. See
    /// [`Rule::ContainerSumMismatch`](crate::validation::Rule::ContainerSumMismatch).
    ContainerMismatch {
        /// Index into the caller's slice.
        record_index: usize,
    },
    /// The running total exceeded what its counters can hold.
    Overflow {
        /// Index into the caller's slice at which the total overflowed.
        record_index: usize,
    },
    /// The records and dispositions supplied are not the same length.
    ///
    /// They must correspond one to one, or the total cannot be tied to the
    /// reconciliation that justifies it — which is the whole point of pairing
    /// them.
    LengthMismatch {
        /// How many records were supplied.
        records: usize,
        /// How many dispositions were supplied.
        dispositions: usize,
    },
}

impl fmt::Display for AggregationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ContainerMismatch { record_index } => write!(
                f,
                "record {record_index} disagrees with its own container breakdown"
            ),
            Self::Overflow { record_index } => {
                write!(f, "the total overflowed at record {record_index}")
            }
            Self::LengthMismatch {
                records,
                dispositions,
            } => write!(
                f,
                "{records} record(s) against {dispositions} disposition(s)"
            ),
        }
    }
}

impl std::error::Error for AggregationError {}

// ── ReleasedTotals ───────────────────────────────────────────────────────────

/// What was released, and the evidence that it was all of it.
///
/// The two travel together because neither is checkable alone: a total says
/// nothing about completeness, and a reconciliation says nothing about size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReleasedTotals {
    total: UsageTotal,
    reconciliation: BatchReconciliation,
}

impl ReleasedTotals {
    /// The summed usage of the records that were released.
    #[must_use]
    pub const fn total(&self) -> UsageTotal {
        self.total
    }

    /// Where every ingested record went.
    #[must_use]
    pub const fn reconciliation(&self) -> BatchReconciliation {
        self.reconciliation
    }

    /// `true` when the batch balances **and** the summed record count matches
    /// what the reconciliation says was released.
    ///
    /// The second half is the check the pairing exists for: a total built from
    /// a different set of records than the one dispositioned would balance and
    /// still be wrong.
    #[must_use]
    pub const fn is_consistent(&self) -> bool {
        self.reconciliation.balances() && self.total.records == self.reconciliation.released()
    }
}

impl fmt::Display for ReleasedTotals {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} [{}]",
            self.total,
            if self.is_consistent() {
                "consistent"
            } else {
                "INCONSISTENT"
            }
        )
    }
}

// ── The engine ───────────────────────────────────────────────────────────────

/// Sum every record supplied.
///
/// Every record, without regard to whether it should be billed — pass only what
/// you mean to sum, or use [`aggregate_released`], which does the filtering and
/// keeps the evidence.
///
/// # Errors
///
/// [`AggregationError`] when a record disagrees with its own container
/// breakdown, or the total overflows.
pub fn aggregate(records: &[UsageRecord]) -> Result<UsageTotal, AggregationError> {
    let mut total = UsageTotal::ZERO;
    for (record_index, record) in records.iter().enumerate() {
        check_containers(record, record_index)?;
        total = total
            .checked_add_record(record)
            .ok_or(AggregationError::Overflow { record_index })?;
    }
    Ok(total)
}

/// Sum only the records a disposition released, keeping the reconciliation.
///
/// `records` and `dispositions` must correspond one to one.
///
/// # Errors
///
/// [`AggregationError`] when the two slices differ in length, when a released
/// record disagrees with its container breakdown, or when the total overflows.
pub fn aggregate_released(
    records: &[UsageRecord],
    dispositions: &[Disposition],
) -> Result<ReleasedTotals, AggregationError> {
    if records.len() != dispositions.len() {
        return Err(AggregationError::LengthMismatch {
            records: records.len(),
            dispositions: dispositions.len(),
        });
    }

    let mut total = UsageTotal::ZERO;
    let mut reconciliation = BatchReconciliation::EMPTY;
    for (record_index, (record, disposition)) in records.iter().zip(dispositions).enumerate() {
        reconciliation.observe(disposition);
        if !disposition.is_released() {
            continue;
        }
        check_containers(record, record_index)?;
        total = total
            .checked_add_record(record)
            .ok_or(AggregationError::Overflow { record_index })?;
    }

    Ok(ReleasedTotals {
        total,
        reconciliation,
    })
}

/// Sum by a caller-supplied key.
///
/// A key function rather than an enum of grouping strategies: the useful
/// groupings are per subscriber, per tenant, per local day, per service and
/// combinations of those, and a closed set would be wrong within a month.
///
/// ```rust
/// use mediation::aggregate_by;
/// # use mediation::{ChargedParty, Octets, RecordType, SourceRef, SubscriberRef, UsageRecord,
/// #                 VolumePair};
/// # fn record(day: &str, up: u64) -> UsageRecord {
/// #     UsageRecord::new(
/// #         RecordType::PduSession,
/// #         ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
/// #         day.parse().unwrap(),
/// #         SourceRef::new("pgw-01".parse().unwrap(), 0),
/// #     ).with_volume(VolumePair::new(Octets::new(up), Octets::ZERO))
/// # }
/// let records = vec![
///     record("2026-07-28T10:00:00+02:00", 100),
///     record("2026-07-28T22:00:00+02:00", 200),
///     record("2026-07-29T10:00:00+02:00", 400),
/// ];
///
/// // Grouped by the *local* day, which is the day a subscriber recognises.
/// let by_day = aggregate_by(&records, UsageRecord::local_date).unwrap();
/// assert_eq!(by_day.len(), 2);
/// ```
///
/// # Errors
///
/// As [`aggregate`].
pub fn aggregate_by<K, F>(
    records: &[UsageRecord],
    key: F,
) -> Result<BTreeMap<K, UsageTotal>, AggregationError>
where
    K: Ord,
    F: Fn(&UsageRecord) -> K,
{
    let mut buckets: BTreeMap<K, UsageTotal> = BTreeMap::new();
    for (record_index, record) in records.iter().enumerate() {
        check_containers(record, record_index)?;
        let entry = buckets.entry(key(record)).or_insert(UsageTotal::ZERO);
        *entry = entry
            .checked_add_record(record)
            .ok_or(AggregationError::Overflow { record_index })?;
    }
    Ok(buckets)
}

/// Sum per rating group, from the container breakdown.
///
/// A record with no containers contributes to the `None` bucket: its usage is
/// real but unclassified, and folding it into an arbitrary group would price it
/// under a tariff nothing said it belonged to.
///
/// Duration and event counts are **not** apportioned across a record's
/// containers — a record does not say how its seconds divide between rating
/// groups, and splitting them evenly would invent the division. Only volume,
/// which the containers do carry, is attributed.
///
/// # Errors
///
/// [`AggregationError::Overflow`] when a group's volume exceeds what it can
/// hold. Container mismatches are **not** checked here: this function reads the
/// containers alone and never the record total, so the two cannot disagree.
pub fn aggregate_rating_groups(
    records: &[UsageRecord],
) -> Result<BTreeMap<Option<RatingGroup>, VolumePair>, AggregationError> {
    let mut buckets: BTreeMap<Option<RatingGroup>, VolumePair> = BTreeMap::new();
    for (record_index, record) in records.iter().enumerate() {
        if record.containers.is_empty() {
            let entry = buckets.entry(None).or_insert(VolumePair::ZERO);
            *entry = entry
                .checked_add(record.volume.unwrap_or(VolumePair::ZERO))
                .ok_or(AggregationError::Overflow { record_index })?;
            continue;
        }
        for container in &record.containers {
            let entry = buckets
                .entry(Some(container.rating_group))
                .or_insert(VolumePair::ZERO);
            *entry = entry
                .checked_add(container.volume)
                .ok_or(AggregationError::Overflow { record_index })?;
        }
    }
    Ok(buckets)
}

/// A record must not disagree with itself before it is summed.
fn check_containers(record: &UsageRecord, record_index: usize) -> Result<(), AggregationError> {
    if record.containers_agree() == Some(false) {
        return Err(AggregationError::ContainerMismatch { record_index });
    }
    Ok(())
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::{ReleasedTotals, UsageTotal};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// Field names are part of the public representation and covered by semver.
    #[derive(Serialize, Deserialize)]
    #[serde(rename = "UsageTotal")]
    struct TotalRepr {
        volume: crate::volume::VolumePair,
        duration_s: crate::cdrtime::Seconds,
        events: u64,
        records: u64,
        #[serde(default)]
        unquantified: u64,
    }

    impl Serialize for UsageTotal {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            TotalRepr {
                volume: self.volume(),
                duration_s: self.duration(),
                events: self.events(),
                records: self.records(),
                unquantified: self.unquantified(),
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for UsageTotal {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = TotalRepr::deserialize(deserializer)?;
            Ok(UsageTotal {
                volume: r.volume,
                duration: r.duration_s,
                events: r.events,
                records: r.records,
                unquantified: r.unquantified,
            })
        }
    }

    #[derive(Serialize, Deserialize)]
    #[serde(rename = "ReleasedTotals")]
    struct ReleasedRepr {
        total: UsageTotal,
        reconciliation: crate::disposition::BatchReconciliation,
    }

    impl Serialize for ReleasedTotals {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            ReleasedRepr {
                total: self.total(),
                reconciliation: self.reconciliation(),
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for ReleasedTotals {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = ReleasedRepr::deserialize(deserializer)?;
            Ok(ReleasedTotals {
                total: r.total,
                reconciliation: r.reconciliation,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container::{ChangeCondition, ContainerSet, ServiceDataContainer};
    use crate::disposition::{RejectReason, SuppressReason};
    use crate::party::{ChargedParty, SubscriberRef};
    use crate::record::{RecordType, SourceRef};
    use crate::volume::Octets;

    fn party() -> ChargedParty {
        ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap()))
    }

    fn record(opened: &str, up: u64, down: u64) -> UsageRecord {
        UsageRecord::new(
            RecordType::PduSession,
            party(),
            opened.parse().unwrap(),
            SourceRef::new("pgw-01".parse().unwrap(), 0),
        )
        .with_volume(VolumePair::new(Octets::new(up), Octets::new(down)))
    }

    fn container(group: u32, up: u64) -> ServiceDataContainer {
        ServiceDataContainer::new(RatingGroup::new(group), ChangeCondition::RecordClosure)
            .with_volume(VolumePair::new(Octets::new(up), Octets::ZERO))
    }

    #[test]
    fn quantities_sum_with_the_directions_kept_apart() {
        let records = vec![
            record("2026-07-28T10:00:00+02:00", 100, 900),
            record("2026-07-28T11:00:00+02:00", 50, 450),
        ];
        let total = aggregate(&records).unwrap();

        assert_eq!(total.volume().uplink(), Octets::new(150));
        assert_eq!(total.volume().downlink(), Octets::new(1_350));
        assert_eq!(total.records(), 2);
        assert!(!total.is_empty());
    }

    /// The signature move: a total never travels without the evidence that it
    /// is all of it.
    #[test]
    fn a_total_carries_the_reconciliation_that_justifies_it() {
        let records = vec![
            record("2026-07-28T10:00:00+02:00", 100, 0),
            record("2026-07-28T11:00:00+02:00", 200, 0),
            record("2026-07-28T12:00:00+02:00", 400, 0),
        ];
        let dispositions = vec![
            Disposition::Billable,
            Disposition::Billable,
            Disposition::Rejected(RejectReason::Malformed),
        ];

        let released = aggregate_released(&records, &dispositions).unwrap();
        assert_eq!(released.total().volume().uplink(), Octets::new(300));
        assert_eq!(released.total().records(), 2);
        assert_eq!(released.reconciliation().ingested(), 3);
        assert_eq!(released.reconciliation().released(), 2);
        assert!(released.is_consistent());
    }

    /// Only released records are summed; everything else still counts.
    #[test]
    fn suppressed_and_duplicate_records_are_counted_but_not_summed() {
        let records = vec![
            record("2026-07-28T10:00:00+02:00", 100, 0),
            record("2026-07-28T11:00:00+02:00", 999, 0),
            record("2026-07-28T12:00:00+02:00", 999, 0),
        ];
        let dispositions = vec![
            Disposition::Billable,
            Disposition::Suppressed(SuppressReason::TestIdentity),
            Disposition::Duplicate(crate::dedup::RecordKey::RecordingEntity {
                entity: "msc-01".parse().unwrap(),
                local_sequence: 1,
            }),
        ];

        let released = aggregate_released(&records, &dispositions).unwrap();
        assert_eq!(released.total().volume().uplink(), Octets::new(100));
        assert_eq!(released.reconciliation().ingested(), 3);
        assert!(released.is_consistent());
    }

    /// Pairing records with dispositions is what ties the total to the law; a
    /// mismatch means they cannot correspond.
    #[test]
    fn records_and_dispositions_must_correspond() {
        let records = vec![record("2026-07-28T10:00:00+02:00", 1, 0)];
        assert_eq!(
            aggregate_released(&records, &[]),
            Err(AggregationError::LengthMismatch {
                records: 1,
                dispositions: 0,
            })
        );
    }

    /// A record that disagrees with itself is named, not averaged.
    #[test]
    fn a_container_mismatch_stops_the_sum_and_names_the_record() {
        let mut broken = record("2026-07-28T10:00:00+02:00", 999, 0);
        broken.containers = ContainerSet::from(vec![container(10, 100)]);

        let records = vec![record("2026-07-28T09:00:00+02:00", 1, 0), broken];
        assert_eq!(
            aggregate(&records),
            Err(AggregationError::ContainerMismatch { record_index: 1 })
        );
    }

    /// A wrapped total is a small plausible number, which is the kind that
    /// reaches an invoice.
    #[test]
    fn an_overflowing_total_is_reported_and_located() {
        let records = vec![
            record("2026-07-28T10:00:00+02:00", u64::MAX, 0),
            record("2026-07-28T11:00:00+02:00", 1, 0),
        ];
        assert_eq!(
            aggregate(&records),
            Err(AggregationError::Overflow { record_index: 1 })
        );
    }

    /// Grouping is by the *local* day, which is the day a subscriber
    /// recognises on an itemised bill.
    #[test]
    fn grouping_by_local_day_follows_the_subscribers_calendar() {
        let records = vec![
            // Just after local midnight in Nepal: the 28th locally, the 27th UTC.
            record("2026-07-28T00:30:00+05:45", 100, 0),
            record("2026-07-28T23:00:00+05:45", 200, 0),
            record("2026-07-29T01:00:00+05:45", 400, 0),
        ];
        let by_day = aggregate_by(&records, UsageRecord::local_date).unwrap();

        assert_eq!(by_day.len(), 2);
        let first = by_day.values().next().unwrap();
        assert_eq!(first.volume().uplink(), Octets::new(300));
        assert_eq!(first.records(), 2);
    }

    #[test]
    fn grouping_by_service_domain_partitions_the_batch() {
        let mut sms = record("2026-07-28T10:00:00+02:00", 0, 0);
        sms.record_type = RecordType::SmsOriginated;
        sms.volume = None;
        sms.events = Some(3);

        let records = vec![record("2026-07-28T10:00:00+02:00", 100, 0), sms];
        let by_service = aggregate_by(&records, UsageRecord::service).unwrap();

        assert_eq!(by_service.len(), 2);
        assert_eq!(by_service[&crate::record::ServiceDomain::Sms].events(), 3);
        assert_eq!(
            by_service[&crate::record::ServiceDomain::Data]
                .volume()
                .uplink(),
            Octets::new(100)
        );
    }

    /// Records with no containers land in `None` rather than an arbitrary
    /// group: the usage is real but unclassified.
    #[test]
    fn unclassified_usage_gets_its_own_bucket() {
        let mut classified = record("2026-07-28T10:00:00+02:00", 300, 0);
        classified.containers = ContainerSet::from(vec![container(10, 100), container(20, 200)]);
        let unclassified = record("2026-07-28T11:00:00+02:00", 50, 0);

        let groups = aggregate_rating_groups(&[classified, unclassified]).unwrap();
        assert_eq!(groups.len(), 3);
        assert_eq!(
            groups[&Some(RatingGroup::new(10))].uplink(),
            Octets::new(100)
        );
        assert_eq!(
            groups[&Some(RatingGroup::new(20))].uplink(),
            Octets::new(200)
        );
        assert_eq!(groups[&None].uplink(), Octets::new(50));
    }

    /// Rating-group aggregation reads the containers alone, so a record whose
    /// total disagrees with them cannot poison it.
    #[test]
    fn rating_group_sums_never_consult_the_record_total() {
        let mut disagreeing = record("2026-07-28T10:00:00+02:00", 9_999, 0);
        disagreeing.containers = ContainerSet::from(vec![container(10, 100)]);

        // `aggregate` refuses this record …
        assert!(aggregate(std::slice::from_ref(&disagreeing)).is_err());
        // … while the per-group sum reads only what the containers say.
        let groups = aggregate_rating_groups(std::slice::from_ref(&disagreeing)).unwrap();
        assert_eq!(
            groups[&Some(RatingGroup::new(10))].uplink(),
            Octets::new(100)
        );
    }

    /// A record that contributed nothing is counted, so a dropped field is
    /// visible in the total rather than merely absent from it.
    #[test]
    fn records_that_contributed_nothing_are_counted() {
        let mut empty = record("2026-07-28T11:00:00+02:00", 0, 0);
        empty.volume = None;

        let total = aggregate(&[record("2026-07-28T10:00:00+02:00", 100, 0), empty]).unwrap();

        assert_eq!(total.records(), 2);
        assert_eq!(total.unquantified(), 1);
        assert!(!total.is_fully_quantified());
        assert!(total.to_string().contains("carried no quantity"), "{total}");

        // A batch where every record contributed says so.
        let clean = aggregate(&[record("2026-07-28T10:00:00+02:00", 100, 0)]).unwrap();
        assert!(clean.is_fully_quantified());
        assert!(!clean.to_string().contains("carried no quantity"));
    }

    #[test]
    fn totals_merge_for_recombining_parallel_work() {
        let left = aggregate(&[record("2026-07-28T10:00:00+02:00", 100, 0)]).unwrap();
        let right = aggregate(&[
            record("2026-07-28T11:00:00+02:00", 200, 0),
            record("2026-07-28T12:00:00+02:00", 400, 0),
        ])
        .unwrap();

        let merged = left.checked_add(right).unwrap();
        assert_eq!(merged.volume().uplink(), Octets::new(700));
        assert_eq!(merged.records(), 3);

        // …and merging is checked, like every other sum here.
        let huge = UsageTotal::ZERO
            .checked_add_record(&record("2026-07-28T10:00:00+02:00", u64::MAX, 0))
            .unwrap();
        assert_eq!(huge.checked_add(huge), None);
    }

    #[test]
    fn an_empty_batch_totals_to_nothing() {
        let total = aggregate(&[]).unwrap();
        assert_eq!(total, UsageTotal::ZERO);
        assert!(total.is_empty());
        assert!(
            aggregate_by(&[], UsageRecord::local_date)
                .unwrap()
                .is_empty()
        );

        let released = aggregate_released(&[], &[]).unwrap();
        assert!(released.is_consistent());
    }

    /// A record with only containers is summed from them, so its usage is not
    /// lost for want of a record-level total.
    #[test]
    fn a_record_with_only_containers_still_contributes() {
        let mut container_only = record("2026-07-28T10:00:00+02:00", 0, 0);
        container_only.volume = None;
        container_only.containers = ContainerSet::from(vec![container(10, 250)]);

        let total = aggregate(&[container_only]).unwrap();
        assert_eq!(total.volume().uplink(), Octets::new(250));
    }
}
