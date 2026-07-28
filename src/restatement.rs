//! Corrections are new records, never edits.
//!
//! The past changes constantly in mediation: a late partial completes a run
//! already released, a partner returns records they had already booked, a
//! tariff reclassification arrives after month-end. The tempting model is to go
//! back and fix the record.
//!
//! **That model cannot balance.** Once a batch's counts have been reported,
//! editing one of its records silently invalidates the report, and there is no
//! longer any way to reconcile what was released against what is now stored.
//! Worse, it destroys the evidence: the question at audit is *what did you bill,
//! and on what basis*, and a mutated record answers a different question.
//!
//! So [`crate::UsageRecord`] is immutable, [`crate::Disposition`] is terminal,
//! and a correction is an explicit event carrying a **signed delta** — what
//! changes, not what the value becomes. The original batch's reconciliation
//! stays true forever; the correction reconciles in the period it was
//! discovered.
//!
//! ```rust
//! use mediation::{Restatement, RestateReason, UsageDelta};
//! # use mediation::RecordKey;
//! # let key = RecordKey::RecordingEntity {
//! #     entity: "msc-01".parse().unwrap(),
//! #     local_sequence: 1,
//! # };
//! // A late partial adds 300 seconds to a session already released.
//! let correction = Restatement::new(
//!     key,
//!     RestateReason::LatePartial,
//!     UsageDelta::default().with_duration(300),
//!     "2026-08-02T09:00:00+02:00".parse().unwrap(),
//! );
//!
//! // Every restatement can be undone, and the pair nets to nothing.
//! let reversal = correction.clone().reverse();
//! assert_eq!(
//!     correction.delta.checked_add(reversal.delta),
//!     Some(UsageDelta::ZERO),
//! );
//! ```
//!
//! # Deltas are signed, and wider than the quantities they correct
//!
//! A [`crate::volume::VolumePair`] is unsigned `u64` because a volume cannot be
//! negative. A *correction* to one can be, and it can be as large as the
//! original — so [`UsageDelta`] holds `i128`, which represents every difference
//! between two `u64` values without saturating. Narrowing here would silently
//! cap the one number whose whole purpose is to say how wrong the last one was.
//!
//! # Resubmission is not a free-form edit
//!
//! GSMA TD.57 permits a roaming event to be resubmitted in exactly one case:
//! where it was previously returned through the RAP process. That is why
//! [`RestateReason`] is a closed set rather than an operator-supplied string —
//! a correction nobody can name the basis for is the one nobody should accept.

use std::fmt;
use std::str::FromStr;

use crate::cdrtime::CdrTimestamp;
use crate::dedup::RecordKey;
use crate::error::ParseError;

// ── UsageDelta ───────────────────────────────────────────────────────────────

/// A signed change to a record's quantities.
///
/// Every field is a difference, not a replacement: `0` means *unchanged*, which
/// is what lets deltas compose and reverse. A struct of `Option`s would make
/// "unchanged" and "corrected to zero" indistinguishable — the same trap that
/// makes a reduced partial record unreadable on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UsageDelta {
    /// Change in chargeable seconds.
    pub duration: i64,
    /// Change in uplink octets.
    pub uplink: i128,
    /// Change in downlink octets.
    pub downlink: i128,
    /// Change in event count.
    pub events: i64,
}

impl UsageDelta {
    /// No change at all.
    pub const ZERO: Self = Self {
        duration: 0,
        uplink: 0,
        downlink: 0,
        events: 0,
    };

    /// The same delta with a duration change.
    #[must_use]
    pub const fn with_duration(mut self, seconds: i64) -> Self {
        self.duration = seconds;
        self
    }

    /// The same delta with a volume change, uplink and downlink kept apart.
    #[must_use]
    pub const fn with_volume(mut self, uplink: i128, downlink: i128) -> Self {
        self.uplink = uplink;
        self.downlink = downlink;
        self
    }

    /// The same delta with an event-count change.
    #[must_use]
    pub const fn with_events(mut self, events: i64) -> Self {
        self.events = events;
        self
    }

    /// `true` when nothing changes.
    ///
    /// A restatement carrying no change is a statement that the record was
    /// right, which is occasionally what an operator means and always worth
    /// being able to say.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.duration == 0 && self.uplink == 0 && self.downlink == 0 && self.events == 0
    }

    /// The delta that undoes this one.
    ///
    /// `None` when a field is at its minimum and cannot be negated — a
    /// theoretical case, kept fallible rather than saturating because a
    /// saturated reversal would not actually reverse anything.
    #[must_use]
    pub const fn checked_negate(self) -> Option<Self> {
        match (
            self.duration.checked_neg(),
            self.uplink.checked_neg(),
            self.downlink.checked_neg(),
            self.events.checked_neg(),
        ) {
            (Some(duration), Some(uplink), Some(downlink), Some(events)) => Some(Self {
                duration,
                uplink,
                downlink,
                events,
            }),
            _ => None,
        }
    }

    /// Two deltas applied one after the other, `None` on overflow.
    ///
    /// Composition is what lets a chain of corrections be collapsed into one
    /// before it reaches a ledger, and it is checked because a wrapped
    /// correction is a plausible-looking wrong number.
    #[must_use]
    pub const fn checked_add(self, other: Self) -> Option<Self> {
        match (
            self.duration.checked_add(other.duration),
            self.uplink.checked_add(other.uplink),
            self.downlink.checked_add(other.downlink),
            self.events.checked_add(other.events),
        ) {
            (Some(duration), Some(uplink), Some(downlink), Some(events)) => Some(Self {
                duration,
                uplink,
                downlink,
                events,
            }),
            _ => None,
        }
    }

    /// `true` when the correction increases what will be billed.
    ///
    /// A rough direction rather than a price: this crate does not know a
    /// tariff, and a delta that adds volume while removing duration has no
    /// single sign. `None` says exactly that.
    #[must_use]
    pub const fn increases_usage(self) -> Option<bool> {
        let up = self.duration > 0 || self.uplink > 0 || self.downlink > 0 || self.events > 0;
        let down = self.duration < 0 || self.uplink < 0 || self.downlink < 0 || self.events < 0;
        match (up, down) {
            (true, false) => Some(true),
            (false, true) => Some(false),
            _ => None,
        }
    }
}

impl fmt::Display for UsageDelta {
    /// Writes every component signed, so the direction is never inferred.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:+} s, {:+}/{:+} octets, {:+} event(s)",
            self.duration, self.uplink, self.downlink, self.events
        )
    }
}

// ── RestateReason ────────────────────────────────────────────────────────────

/// The basis on which a record is being corrected.
///
/// A closed set rather than free text: a correction nobody can name the basis
/// for is the one nobody should accept, and GSMA TD.57 permits a roaming event
/// to be resubmitted in exactly one case — where it was returned through RAP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RestateReason {
    /// A partial record arrived after its run was released.
    LatePartial,
    /// A partner returned records already booked, through the RAP process.
    ReturnedByPartner,
    /// The usage was reclassified — a tariff period or service correction.
    Reclassification,
    /// A counterpart arrived and revealed the original was double counted.
    DoubleCountCorrection,
    /// The original record was found to be wrong on inspection.
    OperatorCorrection,
}

impl RestateReason {
    /// Every variant, for exhaustive iteration in tests and mappings.
    pub const ALL: &'static [Self] = &[
        Self::LatePartial,
        Self::ReturnedByPartner,
        Self::Reclassification,
        Self::DoubleCountCorrection,
        Self::OperatorCorrection,
    ];

    /// The canonical codes, in the order of [`ALL`](Self::ALL).
    pub const CODES: &'static [&'static str] = &[
        "LATE_PARTIAL",
        "RETURNED_BY_PARTNER",
        "RECLASSIFICATION",
        "DOUBLE_COUNT_CORRECTION",
        "OPERATOR_CORRECTION",
    ];

    /// `true` when the correction originates outside this operator.
    ///
    /// A partner-driven correction has an external audit trail — a RAP file
    /// with its own sequence number — and settling one needs that reference,
    /// which an internally-found error does not have.
    #[must_use]
    pub const fn is_partner_driven(self) -> bool {
        matches!(self, Self::ReturnedByPartner)
    }

    /// The canonical string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LatePartial => "LATE_PARTIAL",
            Self::ReturnedByPartner => "RETURNED_BY_PARTNER",
            Self::Reclassification => "RECLASSIFICATION",
            Self::DoubleCountCorrection => "DOUBLE_COUNT_CORRECTION",
            Self::OperatorCorrection => "OPERATOR_CORRECTION",
        }
    }
}

impl fmt::Display for RestateReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RestateReason {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim().to_ascii_uppercase();
        Self::ALL
            .iter()
            .find(|r| r.as_str() == t)
            .copied()
            .ok_or_else(|| ParseError::one_of("RestateReason", s, Self::CODES))
    }
}

// ── Restatement ──────────────────────────────────────────────────────────────

/// A correction to a record that has already been released.
///
/// Carries the key of what it corrects rather than a copy of it: the original
/// is immutable and still the evidence, and a restatement is a second event
/// that refers to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restatement {
    /// The record this corrects.
    ///
    /// A [`RecordKey`] because it is **stable across retransmission** — which
    /// is exactly the property a correction needs in order to find its target
    /// months later.
    pub supersedes: RecordKey,
    /// The basis for the correction.
    pub reason: RestateReason,
    /// What changes, signed.
    pub delta: UsageDelta,
    /// When the correction was issued.
    pub issued_at: CdrTimestamp,
}

impl Restatement {
    /// A correction to a released record.
    #[must_use]
    pub const fn new(
        supersedes: RecordKey,
        reason: RestateReason,
        delta: UsageDelta,
        issued_at: CdrTimestamp,
    ) -> Self {
        Self {
            supersedes,
            reason,
            delta,
            issued_at,
        }
    }

    /// The restatement that undoes this one.
    ///
    /// Issued at the same instant, because a reversal corrects the same fact
    /// rather than describing a later one. A caller reversing at a different
    /// time should say so by constructing the reversal directly.
    ///
    /// # Panics
    ///
    /// Never for any delta this crate constructs: negating requires each
    /// component to be away from its type's minimum, and no realistic
    /// correction is `i128::MIN` octets. It panics rather than returning a
    /// saturated delta, because a reversal that does not reverse is worse than
    /// no reversal at all.
    #[must_use]
    pub fn reverse(self) -> Self {
        Self {
            delta: self
                .delta
                .checked_negate()
                .expect("a delta at its type's minimum cannot be reversed"),
            ..self
        }
    }

    /// `true` when the correction changes nothing.
    #[must_use]
    pub const fn is_noop(&self) -> bool {
        self.delta.is_zero()
    }
}

impl fmt::Display for Restatement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} supersedes {} [{}] at {}",
            self.reason, self.supersedes, self.delta, self.issued_at
        )
    }
}

// ── RestatementLog ───────────────────────────────────────────────────────────

/// Every correction issued against one set of records.
///
/// Append-only by construction: there is no method that removes or edits an
/// entry, because a withdrawn correction is itself a correction and must leave
/// a trace.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RestatementLog(Vec<Restatement>);

impl RestatementLog {
    /// An empty log.
    #[must_use]
    pub const fn new() -> Self {
        Self(Vec::new())
    }

    /// Append a correction.
    pub fn push(&mut self, restatement: Restatement) {
        self.0.push(restatement);
    }

    /// Every correction, in the order it was issued.
    #[must_use]
    pub fn entries(&self) -> &[Restatement] {
        &self.0
    }

    /// How many corrections have been issued.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// `true` when nothing has been corrected.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The net change to one record across every correction against it.
    ///
    /// `None` on overflow. A record corrected and then un-corrected nets to
    /// [`UsageDelta::ZERO`], which is the property that makes a log auditable
    /// rather than merely long.
    #[must_use]
    pub fn net_for(&self, key: &RecordKey) -> Option<UsageDelta> {
        self.0
            .iter()
            .filter(|r| &r.supersedes == key)
            .try_fold(UsageDelta::ZERO, |acc, r| acc.checked_add(r.delta))
    }

    /// The net change across every record, `None` on overflow.
    #[must_use]
    pub fn net(&self) -> Option<UsageDelta> {
        self.0
            .iter()
            .try_fold(UsageDelta::ZERO, |acc, r| acc.checked_add(r.delta))
    }

    /// The records corrected, each once, in first-correction order.
    #[must_use]
    pub fn corrected_records(&self) -> Vec<&RecordKey> {
        let mut seen = Vec::new();
        for restatement in &self.0 {
            if !seen.contains(&&restatement.supersedes) {
                seen.push(&restatement.supersedes);
            }
        }
        seen
    }
}

impl FromIterator<Restatement> for RestatementLog {
    fn from_iter<I: IntoIterator<Item = Restatement>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl fmt::Display for RestatementLog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.net() {
            Some(net) => write!(
                f,
                "{} correction(s) over {} record(s), net {net}",
                self.0.len(),
                self.corrected_records().len()
            ),
            None => write!(f, "{} correction(s), net overflows", self.0.len()),
        }
    }
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::{RestateReason, Restatement, RestatementLog, UsageDelta};
    use serde::de::{self, Visitor};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::fmt;

    impl Serialize for RestateReason {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_str(self.as_str())
        }
    }

    impl<'de> Deserialize<'de> for RestateReason {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct V;
            impl Visitor<'_> for V {
                type Value = RestateReason;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("a restatement reason code")
                }
                fn visit_str<E: de::Error>(self, v: &str) -> Result<RestateReason, E> {
                    v.parse().map_err(de::Error::custom)
                }
            }
            deserializer.deserialize_str(V)
        }
    }

    /// Field names are part of the public representation and covered by semver.
    ///
    /// The octet deltas are strings: JSON numbers are read as doubles by many
    /// parsers, and a correction wide enough to matter is exactly the value
    /// that would lose precision.
    #[derive(Serialize, Deserialize)]
    #[serde(rename = "UsageDelta")]
    struct DeltaRepr {
        #[serde(default)]
        duration_s: i64,
        #[serde(default)]
        uplink_octets: String,
        #[serde(default)]
        downlink_octets: String,
        #[serde(default)]
        events: i64,
    }

    impl Serialize for UsageDelta {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            DeltaRepr {
                duration_s: self.duration,
                uplink_octets: self.uplink.to_string(),
                downlink_octets: self.downlink.to_string(),
                events: self.events,
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for UsageDelta {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = DeltaRepr::deserialize(deserializer)?;
            let parse = |raw: &str| -> Result<i128, D::Error> {
                if raw.is_empty() {
                    return Ok(0);
                }
                raw.parse::<i128>().map_err(de::Error::custom)
            };
            Ok(UsageDelta {
                duration: r.duration_s,
                uplink: parse(&r.uplink_octets)?,
                downlink: parse(&r.downlink_octets)?,
                events: r.events,
            })
        }
    }

    #[derive(Serialize, Deserialize)]
    #[serde(rename = "Restatement")]
    struct Repr {
        supersedes: crate::dedup::RecordKey,
        reason: RestateReason,
        delta: UsageDelta,
        issued_at: crate::cdrtime::CdrTimestamp,
    }

    impl Serialize for Restatement {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            Repr {
                supersedes: self.supersedes.clone(),
                reason: self.reason,
                delta: self.delta,
                issued_at: self.issued_at,
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for Restatement {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = Repr::deserialize(deserializer)?;
            Ok(Restatement::new(
                r.supersedes,
                r.reason,
                r.delta,
                r.issued_at,
            ))
        }
    }

    impl Serialize for RestatementLog {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            self.entries().serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for RestatementLog {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            Vec::<Restatement>::deserialize(deserializer).map(RestatementLog::from_iter)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(serial: u32) -> RecordKey {
        RecordKey::RecordingEntity {
            entity: "msc-01".parse().unwrap(),
            local_sequence: serial,
        }
    }

    fn issued() -> CdrTimestamp {
        "2026-08-02T09:00:00+02:00".parse().unwrap()
    }

    fn correction(serial: u32, delta: UsageDelta) -> Restatement {
        Restatement::new(key(serial), RestateReason::LatePartial, delta, issued())
    }

    /// The property that makes a correction log auditable: every entry can be
    /// undone, and the pair nets to nothing.
    #[test]
    fn a_correction_and_its_reversal_net_to_zero() {
        let delta = UsageDelta::default()
            .with_duration(300)
            .with_volume(1_000, -4_000)
            .with_events(2);
        let forward = correction(1, delta);
        let back = forward.clone().reverse();

        assert_eq!(back.delta, delta.checked_negate().unwrap());
        assert_eq!(
            forward.delta.checked_add(back.delta),
            Some(UsageDelta::ZERO)
        );

        let log: RestatementLog = vec![forward, back].into_iter().collect();
        assert_eq!(log.net_for(&key(1)), Some(UsageDelta::ZERO));
        assert_eq!(log.len(), 2, "both entries survive; nothing is removed");
    }

    /// Zero means unchanged, which is what lets deltas compose.
    #[test]
    fn zero_is_unchanged_rather_than_corrected_to_zero() {
        let volume_only = UsageDelta::default().with_volume(500, 0);
        assert_eq!(volume_only.duration, 0);
        assert!(!volume_only.is_zero());
        assert!(UsageDelta::ZERO.is_zero());

        let composed = volume_only
            .checked_add(UsageDelta::default().with_duration(60))
            .unwrap();
        assert_eq!(composed.uplink, 500);
        assert_eq!(composed.duration, 60);
    }

    /// A delta must be able to express a correction as large as the value it
    /// corrects, in either direction.
    #[test]
    fn a_delta_spans_the_whole_range_of_the_value_it_corrects() {
        let wipe_out = UsageDelta::default().with_volume(-i128::from(u64::MAX), 0);
        assert_eq!(wipe_out.uplink, -i128::from(u64::MAX));

        // And the reverse of that reverses it exactly.
        assert_eq!(
            wipe_out
                .checked_negate()
                .unwrap()
                .checked_add(wipe_out)
                .unwrap(),
            UsageDelta::ZERO
        );
    }

    #[test]
    fn composition_is_checked() {
        let huge = UsageDelta::default().with_volume(i128::MAX, 0);
        let one = UsageDelta::default().with_volume(1, 0);
        assert_eq!(huge.checked_add(one), None);
        assert_eq!(
            UsageDelta::default()
                .with_duration(i64::MIN)
                .checked_negate(),
            None
        );
    }

    /// The direction of a correction is only meaningful when every component
    /// agrees; a mixed delta has no single sign and says so.
    #[test]
    fn a_mixed_correction_has_no_direction() {
        assert_eq!(
            UsageDelta::default().with_duration(60).increases_usage(),
            Some(true)
        );
        assert_eq!(
            UsageDelta::default().with_duration(-60).increases_usage(),
            Some(false)
        );
        assert_eq!(
            UsageDelta::default()
                .with_duration(60)
                .with_volume(-100, 0)
                .increases_usage(),
            None,
            "more time and less volume has no single sign without a tariff"
        );
        assert_eq!(UsageDelta::ZERO.increases_usage(), None);
    }

    /// Corrections against several records net independently.
    #[test]
    fn corrections_net_per_record_and_in_total() {
        let log: RestatementLog = vec![
            correction(1, UsageDelta::default().with_duration(100)),
            correction(2, UsageDelta::default().with_duration(-40)),
            correction(1, UsageDelta::default().with_duration(5)),
        ]
        .into_iter()
        .collect();

        assert_eq!(
            log.net_for(&key(1)),
            Some(UsageDelta::default().with_duration(105))
        );
        assert_eq!(
            log.net_for(&key(2)),
            Some(UsageDelta::default().with_duration(-40))
        );
        assert_eq!(log.net(), Some(UsageDelta::default().with_duration(65)));
        assert_eq!(log.corrected_records().len(), 2);

        // A record nobody corrected nets to nothing rather than erroring.
        assert_eq!(log.net_for(&key(99)), Some(UsageDelta::ZERO));
    }

    /// A restatement carrying no change is a statement that the record was
    /// right, which is occasionally what an operator means.
    #[test]
    fn a_no_op_correction_is_representable_and_named() {
        let confirmation = correction(1, UsageDelta::ZERO);
        assert!(confirmation.is_noop());
        assert_eq!(
            confirmation.delta.to_string(),
            "+0 s, +0/+0 octets, +0 event(s)"
        );
    }

    /// A partner-driven correction has an external audit trail; an internal one
    /// does not, and settling them differs.
    #[test]
    fn partner_driven_corrections_are_distinguishable() {
        assert!(RestateReason::ReturnedByPartner.is_partner_driven());
        for reason in RestateReason::ALL {
            let expected = *reason == RestateReason::ReturnedByPartner;
            assert_eq!(reason.is_partner_driven(), expected, "{reason}");
        }
    }

    /// A restatement refers to its target by a key that survives
    /// retransmission, which is what lets it find the record months later.
    #[test]
    fn a_restatement_names_its_target_by_a_stable_key() {
        let entry = correction(7, UsageDelta::default().with_duration(1));
        assert_eq!(entry.supersedes, key(7));
        assert!(
            entry.to_string().contains("RECORDING_ENTITY:msc-01/7"),
            "{entry}"
        );
    }

    #[test]
    fn reason_codes_round_trip() {
        assert_eq!(RestateReason::ALL.len(), RestateReason::CODES.len());
        for (reason, code) in RestateReason::ALL.iter().zip(RestateReason::CODES) {
            assert_eq!(reason.as_str(), *code);
            assert_eq!(code.parse::<RestateReason>().unwrap(), *reason);
        }
        assert!("NOPE".parse::<RestateReason>().is_err());
    }

    #[test]
    fn an_empty_log_nets_to_nothing() {
        let log = RestatementLog::new();
        assert!(log.is_empty());
        assert_eq!(log.net(), Some(UsageDelta::ZERO));
        assert!(log.corrected_records().is_empty());
    }
}
