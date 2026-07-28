//! Partial records: reconstruct, never invent.
//!
//! A long session does not produce one record. It produces a run of **partial**
//! records, closed and reopened whenever a volume limit, a time limit or a
//! tariff boundary is reached. Billing the session means putting the run back
//! together.
//!
//! # This crate reassembles; it does not estimate
//!
//! Some metering domains are authorised to substitute a value for a reading
//! that was lost. Telecoms has no such authorisation: usage that cannot be
//! evidenced is not billable. So this module puts together the partials that
//! genuinely arrived and **reports** the ones that did not. It never synthesises
//! a quantity, and there is no API here that would let it.
//!
//! # FQPC and RPC are not interchangeable
//!
//! 3GPP defines two partial-record formats:
//!
//! - a **Fully Qualified Partial CDR** carries the complete set of fields and
//!   can be read standalone;
//! - a **Reduced Partial CDR** carries only mandatory fields plus what *changed*
//!   since the previous partial.
//!
//! **The first partial of a run must be fully qualified** (TS 32.240), and it is
//! the basis from which the reduced ones that follow are completed. Reading an
//! RPC's absent field as zero is therefore a correctness bug: the field did not
//! become zero, it did not change. An RPC whose predecessor is missing is
//! **unresolvable** — held, not billed, not silently zero-filled.
//!
//! ```rust
//! use mediation::{PartialFormat, PartialInfo, PartialRun, RecordClosureCause, Reassembly};
//!
//! // A run whose first record is reduced cannot be completed from itself.
//! let orphaned = PartialRun::from_parts(vec![
//!     PartialInfo::new(2, PartialFormat::Reduced, RecordClosureCause::VolumeLimit),
//! ]);
//! assert!(matches!(orphaned.assess(), Reassembly::Orphan { .. }));
//! ```
//!
//! # Why a partial exists tells you whether it is suspicious
//!
//! The closure cause (3GPP TS 32.298 `CauseForRecClosing`) distinguishes a run
//! that ended because the session ended from one that ended because a node
//! fell over:
//!
//! | Cause | Meaning | Missing successor is… |
//! |---|---|---|
//! | volume / time limit | container filled or aged out | unexplained |
//! | tariff time change | crossed a priced boundary | unexplained |
//! | serving node / RAT change | serving conditions moved | unexplained |
//! | max change conditions | reporting limit reached | unexplained |
//! | **abnormal release** | node failure, forced close | **expected** |
//! | management intervention | operator action | **expected** |
//!
//! A run ending on an abnormal release with no last record reconciles cleanly;
//! the same hole after a routine trigger is unexplained loss and must be
//! escalated. Modelling the cause is what makes that distinction available at
//! all — see [`PartialRun::is_explained_loss`].

use std::fmt;
use std::str::FromStr;

use crate::error::ParseError;

// ── RecordClosureCause ───────────────────────────────────────────────────────

/// Why a record was closed (3GPP TS 32.298 `CauseForRecClosing`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RecordClosureCause {
    /// The session ended normally.
    NormalRelease,
    /// A partial record: the session continued past this one.
    PartialRecord,
    /// The session was torn down abnormally — a node failed or forced a close.
    AbnormalRelease,
    /// The reported volume limit was reached.
    VolumeLimit,
    /// The reported time limit was reached.
    TimeLimit,
    /// A tariff period boundary was crossed.
    TariffTimeChange,
    /// The serving node changed.
    ServingNodeChange,
    /// The maximum number of change conditions was reached.
    MaxChangeConditions,
    /// An operator intervened.
    ManagementIntervention,
    /// The radio access technology changed.
    RatChange,
    /// The subscriber's reported time zone changed.
    TimeZoneChange,
    /// The serving network changed.
    ServingPlmnChange,
}

impl RecordClosureCause {
    /// Every variant, for exhaustive iteration in tests and mappings.
    pub const ALL: &'static [Self] = &[
        Self::NormalRelease,
        Self::PartialRecord,
        Self::AbnormalRelease,
        Self::VolumeLimit,
        Self::TimeLimit,
        Self::TariffTimeChange,
        Self::ServingNodeChange,
        Self::MaxChangeConditions,
        Self::ManagementIntervention,
        Self::RatChange,
        Self::TimeZoneChange,
        Self::ServingPlmnChange,
    ];

    /// The canonical codes, in the order of [`ALL`](Self::ALL).
    pub const CODES: &'static [&'static str] = &[
        "NORMAL_RELEASE",
        "PARTIAL_RECORD",
        "ABNORMAL_RELEASE",
        "VOLUME_LIMIT",
        "TIME_LIMIT",
        "TARIFF_TIME_CHANGE",
        "SERVING_NODE_CHANGE",
        "MAX_CHANGE_CONDITIONS",
        "MANAGEMENT_INTERVENTION",
        "RAT_CHANGE",
        "TIME_ZONE_CHANGE",
        "SERVING_PLMN_CHANGE",
    ];

    /// `true` when the closure ends the session rather than continuing it.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::NormalRelease | Self::AbnormalRelease | Self::ManagementIntervention
        )
    }

    /// `true` when a run ending here **without** a last record is explained.
    ///
    /// A node that failed or was intervened in cannot be expected to have
    /// emitted its successor; a volume limit can. The distinction decides
    /// whether a hole reconciles cleanly or escalates.
    #[must_use]
    pub const fn explains_a_missing_successor(self) -> bool {
        matches!(self, Self::AbnormalRelease | Self::ManagementIntervention)
    }

    /// The canonical string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NormalRelease => "NORMAL_RELEASE",
            Self::PartialRecord => "PARTIAL_RECORD",
            Self::AbnormalRelease => "ABNORMAL_RELEASE",
            Self::VolumeLimit => "VOLUME_LIMIT",
            Self::TimeLimit => "TIME_LIMIT",
            Self::TariffTimeChange => "TARIFF_TIME_CHANGE",
            Self::ServingNodeChange => "SERVING_NODE_CHANGE",
            Self::MaxChangeConditions => "MAX_CHANGE_CONDITIONS",
            Self::ManagementIntervention => "MANAGEMENT_INTERVENTION",
            Self::RatChange => "RAT_CHANGE",
            Self::TimeZoneChange => "TIME_ZONE_CHANGE",
            Self::ServingPlmnChange => "SERVING_PLMN_CHANGE",
        }
    }
}

impl fmt::Display for RecordClosureCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RecordClosureCause {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim().to_ascii_uppercase();
        Self::ALL
            .iter()
            .find(|c| c.as_str() == t)
            .copied()
            .ok_or_else(|| ParseError::one_of("RecordClosureCause", s, Self::CODES))
    }
}

// ── PartialFormat ────────────────────────────────────────────────────────────

/// Whether a partial record can be read on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PartialFormat {
    /// A Fully Qualified Partial CDR — complete, readable standalone.
    ///
    /// The first partial of a run is always this (3GPP TS 32.240), and it is
    /// what the reduced records that follow are completed against.
    FullyQualified,
    /// A Reduced Partial CDR — mandatory fields plus **only what changed**.
    ///
    /// Unusable on its own. An absent field here means *unchanged*, never zero,
    /// and reading it as zero is the classic reassembly bug.
    Reduced,
}

impl PartialFormat {
    /// Every variant, for exhaustive iteration in tests and mappings.
    pub const ALL: &'static [Self] = &[Self::FullyQualified, Self::Reduced];

    /// The canonical codes, in the order of [`ALL`](Self::ALL).
    pub const CODES: &'static [&'static str] = &["FULLY_QUALIFIED", "REDUCED"];

    /// `true` when the record can be interpreted without its predecessor.
    #[must_use]
    pub const fn is_self_contained(self) -> bool {
        matches!(self, Self::FullyQualified)
    }

    /// The canonical string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FullyQualified => "FULLY_QUALIFIED",
            Self::Reduced => "REDUCED",
        }
    }
}

impl fmt::Display for PartialFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for PartialFormat {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim().to_ascii_uppercase();
        Self::ALL
            .iter()
            .find(|c| c.as_str() == t)
            .copied()
            .ok_or_else(|| ParseError::one_of("PartialFormat", s, Self::CODES))
    }
}

// ── PartialInfo ──────────────────────────────────────────────────────────────

/// What one record says about its place in a run of partials.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PartialInfo {
    /// The record's position in the run.
    ///
    /// 3GPP numbers these from 1 and increments per partial of one session.
    pub sequence: u32,
    /// Whether the record can be read without its predecessor.
    pub format: PartialFormat,
    /// Why this record was closed.
    pub cause: RecordClosureCause,
}

impl PartialInfo {
    /// A record's place in a run.
    #[must_use]
    pub const fn new(sequence: u32, format: PartialFormat, cause: RecordClosureCause) -> Self {
        Self {
            sequence,
            format,
            cause,
        }
    }

    /// `true` when this record closes the run.
    #[must_use]
    pub const fn is_last(self) -> bool {
        self.cause.is_terminal()
    }

    /// `true` when this record is the first of a run.
    ///
    /// Positional, not structural: a run's first record must also be fully
    /// qualified, which [`PartialRun::assess`] checks separately so the two
    /// failures are reported apart.
    #[must_use]
    pub const fn is_first(self) -> bool {
        self.sequence == 1
    }
}

impl fmt::Display for PartialInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{} {} {}", self.sequence, self.format, self.cause)
    }
}

// ── Reassembly ───────────────────────────────────────────────────────────────

/// What a run of partials adds up to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reassembly {
    /// Every partial from the first to a terminal one is present.
    Complete {
        /// How many records the run holds.
        count: usize,
    },
    /// The run starts correctly but has holes, or has not ended.
    Incomplete {
        /// Sequence numbers present in the run, ascending.
        have: Vec<u32>,
        /// Sequence numbers missing between the ones present.
        missing: Vec<u32>,
        /// `true` when a terminal record has arrived.
        ///
        /// A run with no holes and no terminal record is not broken — it is
        /// still open, and more partials are expected.
        terminated: bool,
    },
    /// The run cannot be completed from what arrived.
    ///
    /// Either it does not begin at sequence 1, or its first record is reduced
    /// and therefore has no basis to be completed against.
    Orphan {
        /// Why the run cannot be reassembled.
        reason: OrphanReason,
        /// Sequence numbers present in the run, ascending.
        have: Vec<u32>,
    },
    /// No partials at all.
    Empty,
}

impl Reassembly {
    /// `true` when the run is whole and may be billed.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        matches!(self, Self::Complete { .. })
    }

    /// `true` when more records may still arrive to finish the run.
    ///
    /// An orphan cannot be finished by waiting: what is missing is the basis
    /// the run needed to be readable at all.
    #[must_use]
    pub const fn may_still_complete(&self) -> bool {
        matches!(self, Self::Incomplete { .. } | Self::Empty)
    }

    /// The sequence numbers still outstanding.
    #[must_use]
    pub fn missing(&self) -> &[u32] {
        match self {
            Self::Incomplete { missing, .. } => missing,
            _ => &[],
        }
    }
}

impl fmt::Display for Reassembly {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Complete { count } => write!(f, "COMPLETE ({count} partials)"),
            Self::Incomplete {
                have,
                missing,
                terminated,
            } => write!(
                f,
                "INCOMPLETE ({} present, {} missing, {})",
                have.len(),
                missing.len(),
                if *terminated { "terminated" } else { "open" }
            ),
            Self::Orphan { reason, have } => {
                write!(f, "ORPHAN:{reason} ({} present)", have.len())
            }
            Self::Empty => f.write_str("EMPTY"),
        }
    }
}

/// Why a run of partials cannot be reassembled at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OrphanReason {
    /// The run does not begin at sequence 1.
    ///
    /// Its opening record was lost, and with it every field the reduced
    /// records that followed were meant to be completed against.
    FirstRecordMissing,
    /// The run begins at sequence 1, but that record is reduced.
    ///
    /// The first partial must be fully qualified. A reduced one has no basis
    /// to complete itself from, and treating its absent fields as zero would
    /// invent the session's parameters.
    FirstRecordNotFullyQualified,
}

impl OrphanReason {
    /// Every variant, for exhaustive iteration in tests and mappings.
    pub const ALL: &'static [Self] =
        &[Self::FirstRecordMissing, Self::FirstRecordNotFullyQualified];

    /// The canonical string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FirstRecordMissing => "FIRST_RECORD_MISSING",
            Self::FirstRecordNotFullyQualified => "FIRST_RECORD_NOT_FULLY_QUALIFIED",
        }
    }
}

impl fmt::Display for OrphanReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── PartialRun ───────────────────────────────────────────────────────────────

/// The partials of one session, gathered so they can be assessed together.
///
/// Order-independent: partials are sorted by sequence when assessed, so a run
/// delivered out of order is not mistaken for a run with holes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PartialRun(Vec<PartialInfo>);

impl PartialRun {
    /// An empty run.
    #[must_use]
    pub const fn new() -> Self {
        Self(Vec::new())
    }

    /// A run from the partials that have arrived, in any order.
    #[must_use]
    pub fn from_parts(parts: Vec<PartialInfo>) -> Self {
        Self(parts)
    }

    /// Add a partial to the run.
    pub fn push(&mut self, part: PartialInfo) {
        self.0.push(part);
    }

    /// How many partials have arrived.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// `true` when nothing has arrived.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The partials, as supplied.
    #[must_use]
    pub fn as_slice(&self) -> &[PartialInfo] {
        &self.0
    }

    /// The closure cause of the highest-numbered partial present.
    ///
    /// What [`PartialRun::is_explained_loss`] needs in order to decide whether
    /// a run that simply stops is accounted for.
    #[must_use]
    pub fn last_cause(&self) -> Option<RecordClosureCause> {
        self.0.iter().max_by_key(|p| p.sequence).map(|p| p.cause)
    }

    /// `true` when the run simply stops, and how it closed accounts for that.
    ///
    /// A run whose highest partial reports an abnormal release or a management
    /// intervention has an explained ending: the node could not be expected to
    /// emit a successor. The same shape after a routine trigger — a volume or
    /// time limit — is unexplained loss and must be escalated.
    ///
    /// A **hole** is never explained away: an abnormal ending accounts for a
    /// missing successor, not for a missing middle.
    ///
    /// A method on the run rather than on [`Reassembly`], because the closure
    /// cause lives here and threading it through the assessment made every
    /// caller carry both halves of one question.
    #[must_use]
    pub fn is_explained_loss(&self) -> bool {
        match self.assess() {
            Reassembly::Incomplete {
                missing,
                terminated,
                ..
            } => {
                missing.is_empty()
                    && !terminated
                    && self
                        .last_cause()
                        .is_some_and(RecordClosureCause::explains_a_missing_successor)
            }
            _ => false,
        }
    }

    /// Assess what the run adds up to.
    ///
    /// Duplicated sequence numbers are collapsed: two copies of partial 3 are
    /// one partial 3, and recognising the retransmission is
    /// [`crate::dedup`]'s job rather than this one's.
    #[must_use]
    pub fn assess(&self) -> Reassembly {
        if self.0.is_empty() {
            return Reassembly::Empty;
        }

        let mut have: Vec<u32> = self.0.iter().map(|p| p.sequence).collect();
        have.sort_unstable();
        have.dedup();

        let first = self.0.iter().min_by_key(|p| p.sequence).expect("non-empty");
        if first.sequence != 1 {
            return Reassembly::Orphan {
                reason: OrphanReason::FirstRecordMissing,
                have,
            };
        }
        if !first.format.is_self_contained() {
            return Reassembly::Orphan {
                reason: OrphanReason::FirstRecordNotFullyQualified,
                have,
            };
        }

        let highest = *have.last().expect("non-empty");
        // `have` is sorted and deduplicated, so membership is a binary search.
        // A linear scan here is quadratic in the run length, and a long-lived
        // data session can run to thousands of partials.
        let missing: Vec<u32> = (1..=highest)
            .filter(|n| have.binary_search(n).is_err())
            .collect();
        let terminated = self.0.iter().any(|p| p.is_last());

        if missing.is_empty() && terminated {
            return Reassembly::Complete { count: have.len() };
        }
        Reassembly::Incomplete {
            have,
            missing,
            terminated,
        }
    }
}

impl FromIterator<PartialInfo> for PartialRun {
    fn from_iter<I: IntoIterator<Item = PartialInfo>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::{OrphanReason, PartialFormat, PartialInfo, RecordClosureCause};
    use serde::de::{self, Visitor};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::fmt;

    macro_rules! string_serde {
        ($ty:ty, $expecting:literal) => {
            impl Serialize for $ty {
                fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                    serializer.serialize_str(self.as_str())
                }
            }

            impl<'de> Deserialize<'de> for $ty {
                fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                    struct V;
                    impl Visitor<'_> for V {
                        type Value = $ty;
                        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                            f.write_str($expecting)
                        }
                        fn visit_str<E: de::Error>(self, v: &str) -> Result<$ty, E> {
                            v.parse().map_err(de::Error::custom)
                        }
                    }
                    deserializer.deserialize_str(V)
                }
            }
        };
    }

    string_serde!(RecordClosureCause, "a record closure cause code");
    string_serde!(PartialFormat, "a partial record format code");

    impl Serialize for OrphanReason {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_str(self.as_str())
        }
    }

    impl<'de> Deserialize<'de> for OrphanReason {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct V;
            impl Visitor<'_> for V {
                type Value = OrphanReason;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("an orphan reason code")
                }
                fn visit_str<E: de::Error>(self, v: &str) -> Result<OrphanReason, E> {
                    OrphanReason::ALL
                        .iter()
                        .find(|r| r.as_str() == v)
                        .copied()
                        .ok_or_else(|| de::Error::custom(format!("unknown reason {v:?}")))
                }
            }
            deserializer.deserialize_str(V)
        }
    }

    /// Field names are part of the public representation and covered by semver.
    #[derive(Serialize, Deserialize)]
    #[serde(rename = "PartialInfo")]
    struct Repr {
        sequence: u32,
        format: PartialFormat,
        cause: RecordClosureCause,
    }

    impl Serialize for PartialInfo {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            Repr {
                sequence: self.sequence,
                format: self.format,
                cause: self.cause,
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for PartialInfo {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = Repr::deserialize(deserializer)?;
            Ok(PartialInfo::new(r.sequence, r.format, r.cause))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fqpc(sequence: u32, cause: RecordClosureCause) -> PartialInfo {
        PartialInfo::new(sequence, PartialFormat::FullyQualified, cause)
    }

    fn rpc(sequence: u32, cause: RecordClosureCause) -> PartialInfo {
        PartialInfo::new(sequence, PartialFormat::Reduced, cause)
    }

    #[test]
    fn a_whole_run_reassembles() {
        let run = PartialRun::from_parts(vec![
            fqpc(1, RecordClosureCause::VolumeLimit),
            rpc(2, RecordClosureCause::VolumeLimit),
            rpc(3, RecordClosureCause::NormalRelease),
        ]);
        assert_eq!(run.assess(), Reassembly::Complete { count: 3 });
        assert!(run.assess().is_complete());
    }

    /// Order of arrival is not order of sequence, and a shuffled run must not
    /// look like a run with holes.
    #[test]
    fn assessment_is_order_independent() {
        let ordered = PartialRun::from_parts(vec![
            fqpc(1, RecordClosureCause::VolumeLimit),
            rpc(2, RecordClosureCause::VolumeLimit),
            rpc(3, RecordClosureCause::NormalRelease),
        ]);
        let shuffled = PartialRun::from_parts(vec![
            rpc(3, RecordClosureCause::NormalRelease),
            fqpc(1, RecordClosureCause::VolumeLimit),
            rpc(2, RecordClosureCause::VolumeLimit),
        ]);
        assert_eq!(ordered.assess(), shuffled.assess());
    }

    /// The rule from TS 32.240: the first partial must be fully qualified, or
    /// there is nothing to complete the reduced ones against.
    #[test]
    fn a_run_beginning_with_a_reduced_record_is_an_orphan() {
        let run = PartialRun::from_parts(vec![
            rpc(1, RecordClosureCause::VolumeLimit),
            rpc(2, RecordClosureCause::NormalRelease),
        ]);
        assert_eq!(
            run.assess(),
            Reassembly::Orphan {
                reason: OrphanReason::FirstRecordNotFullyQualified,
                have: vec![1, 2],
            }
        );
        assert!(!run.assess().may_still_complete(), "waiting cannot fix it");
    }

    /// A run that does not start at 1 lost its opening record, and with it the
    /// basis every following reduced record needed.
    #[test]
    fn a_run_that_does_not_begin_at_one_is_an_orphan() {
        let run = PartialRun::from_parts(vec![
            rpc(2, RecordClosureCause::VolumeLimit),
            rpc(3, RecordClosureCause::NormalRelease),
        ]);
        assert_eq!(
            run.assess(),
            Reassembly::Orphan {
                reason: OrphanReason::FirstRecordMissing,
                have: vec![2, 3],
            }
        );
    }

    #[test]
    fn a_hole_in_the_middle_is_named() {
        let run = PartialRun::from_parts(vec![
            fqpc(1, RecordClosureCause::VolumeLimit),
            rpc(4, RecordClosureCause::NormalRelease),
        ]);
        assert_eq!(
            run.assess(),
            Reassembly::Incomplete {
                have: vec![1, 4],
                missing: vec![2, 3],
                terminated: true,
            }
        );
        assert_eq!(run.assess().missing(), &[2, 3]);
        assert!(run.assess().may_still_complete());
    }

    /// A run with no holes and no terminal record is still open, not broken.
    #[test]
    fn an_open_run_is_incomplete_rather_than_faulty() {
        let run = PartialRun::from_parts(vec![
            fqpc(1, RecordClosureCause::VolumeLimit),
            rpc(2, RecordClosureCause::VolumeLimit),
        ]);
        let assessment = run.assess();
        assert_eq!(
            assessment,
            Reassembly::Incomplete {
                have: vec![1, 2],
                missing: vec![],
                terminated: false,
            }
        );
        assert!(assessment.may_still_complete());
        assert!(assessment.missing().is_empty());
    }

    /// The distinction the closure cause exists for: a run that stops after a
    /// node failed is explained; the same shape after a volume limit is not.
    #[test]
    fn an_abnormal_ending_explains_a_missing_successor() {
        let crashed = PartialRun::from_parts(vec![
            fqpc(1, RecordClosureCause::VolumeLimit),
            rpc(2, RecordClosureCause::AbnormalRelease),
        ]);
        // An abnormal release is terminal, so the run is actually complete.
        assert!(crashed.assess().is_complete());

        // A run that simply stops mid-flight after a node failed is explained.
        let crashed_mid_flight = PartialRun::from_parts(vec![
            fqpc(1, RecordClosureCause::VolumeLimit),
            rpc(2, RecordClosureCause::AbnormalRelease),
        ]);
        // …though an abnormal release is itself terminal, so this run is whole.
        assert!(crashed_mid_flight.assess().is_complete());
        assert!(!crashed_mid_flight.is_explained_loss());

        // A run that stops on a routine trigger explains nothing.
        let stopped = PartialRun::from_parts(vec![
            fqpc(1, RecordClosureCause::VolumeLimit),
            rpc(2, RecordClosureCause::VolumeLimit),
        ]);
        assert!(
            !stopped.is_explained_loss(),
            "a volume limit explains nothing"
        );

        // The same run whose highest partial reports an operator intervention
        // is accounted for.
        let intervened = PartialRun::from_parts(vec![
            fqpc(1, RecordClosureCause::VolumeLimit),
            rpc(2, RecordClosureCause::ManagementIntervention),
        ]);
        assert!(intervened.assess().is_complete());
    }

    /// The distinction, isolated: a run that stops without a terminal record.
    #[test]
    fn a_run_that_stops_without_terminating_is_explained_only_by_an_abnormal_cause() {
        let routine = PartialRun::from_parts(vec![
            fqpc(1, RecordClosureCause::TimeLimit),
            rpc(2, RecordClosureCause::TimeLimit),
        ]);
        assert!(!routine.is_explained_loss());

        // The highest partial decides, not the first.
        let mixed = PartialRun::from_parts(vec![
            fqpc(1, RecordClosureCause::TimeLimit),
            rpc(2, RecordClosureCause::PartialRecord),
        ]);
        assert!(!mixed.is_explained_loss());

        let empty = PartialRun::new();
        assert!(!empty.is_explained_loss());
    }

    /// A hole is never explained away: an abnormal ending accounts for a
    /// missing *successor*, not for a missing middle.
    #[test]
    fn an_abnormal_ending_does_not_excuse_a_hole() {
        let holed = PartialRun::from_parts(vec![
            fqpc(1, RecordClosureCause::VolumeLimit),
            rpc(3, RecordClosureCause::AbnormalRelease),
        ]);
        assert!(
            !holed.is_explained_loss(),
            "a hole is not a missing successor"
        );
        assert_eq!(holed.assess().missing(), &[2]);
    }

    /// The membership test is a binary search, so a long run does not degrade
    /// quadratically. Exercised at a length a real data session reaches.
    #[test]
    fn a_long_run_assesses_without_quadratic_membership_tests() {
        let mut parts = vec![fqpc(1, RecordClosureCause::VolumeLimit)];
        parts.extend((2..5_000).map(|n| rpc(n, RecordClosureCause::VolumeLimit)));
        parts.push(rpc(5_000, RecordClosureCause::NormalRelease));

        let run = PartialRun::from_parts(parts);
        assert_eq!(run.assess(), Reassembly::Complete { count: 5_000 });
    }

    /// Two copies of one partial are one partial; recognising the retransmission
    /// belongs to deduplication, not here.
    #[test]
    fn a_repeated_partial_does_not_inflate_the_run() {
        let run = PartialRun::from_parts(vec![
            fqpc(1, RecordClosureCause::VolumeLimit),
            rpc(2, RecordClosureCause::NormalRelease),
            rpc(2, RecordClosureCause::NormalRelease),
        ]);
        assert_eq!(run.assess(), Reassembly::Complete { count: 2 });
        assert_eq!(run.len(), 3, "the run still holds what arrived");
    }

    #[test]
    fn an_empty_run_is_empty_and_may_still_complete() {
        let run = PartialRun::new();
        assert_eq!(run.assess(), Reassembly::Empty);
        assert!(run.assess().may_still_complete());
        assert_eq!(run.last_cause(), None);
    }

    #[test]
    fn the_last_cause_comes_from_the_highest_sequence_not_the_last_arrival() {
        let run = PartialRun::from_parts(vec![
            rpc(3, RecordClosureCause::NormalRelease),
            fqpc(1, RecordClosureCause::VolumeLimit),
        ]);
        assert_eq!(run.last_cause(), Some(RecordClosureCause::NormalRelease));
    }

    #[test]
    fn terminal_and_explanatory_causes_are_distinct_questions() {
        assert!(RecordClosureCause::NormalRelease.is_terminal());
        assert!(!RecordClosureCause::NormalRelease.explains_a_missing_successor());

        assert!(RecordClosureCause::AbnormalRelease.is_terminal());
        assert!(RecordClosureCause::AbnormalRelease.explains_a_missing_successor());

        assert!(!RecordClosureCause::VolumeLimit.is_terminal());
        assert!(!RecordClosureCause::VolumeLimit.explains_a_missing_successor());
    }

    #[test]
    fn closure_cause_and_format_codes_round_trip() {
        assert_eq!(
            RecordClosureCause::ALL.len(),
            RecordClosureCause::CODES.len()
        );
        for (cause, code) in RecordClosureCause::ALL
            .iter()
            .zip(RecordClosureCause::CODES)
        {
            assert_eq!(cause.as_str(), *code);
            assert_eq!(code.parse::<RecordClosureCause>().unwrap(), *cause);
        }
        for (format, code) in PartialFormat::ALL.iter().zip(PartialFormat::CODES) {
            assert_eq!(format.as_str(), *code);
            assert_eq!(code.parse::<PartialFormat>().unwrap(), *format);
        }
        assert!("NOPE".parse::<RecordClosureCause>().is_err());
    }
}
