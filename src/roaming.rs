//! Roaming batches: what a partner sent, and whether it adds up.
//!
//! A GSMA TAP batch is a self-describing file — it states how many events it
//! holds, the window they fall in, and what they total. Those statements are
//! **checkable against the events themselves**, and a batch that disagrees with
//! its own audit block is corrupt however well it parsed.
//!
//! This module models the batch's metadata and its invariants. It is not a
//! codec: decoding TD.57's ASN.1 belongs in a sibling crate, and what arrives
//! here is the decoded header a caller already holds.
//!
//! # Money is an integer with a scale
//!
//! TAP carries monetary values as **integers scaled by `tapDecimalPlaces`** — a
//! charge of `1234` with 3 decimal places is `1.234`. Rendering that through a
//! float reintroduces exactly the error the integer encoding exists to avoid,
//! so [`ScaledAmount`] has no float accessor and never will.
//!
//! The crate still prices nothing. It carries an amount a partner computed,
//! faithfully, so a reconciliation can check it.
//!
//! ```rust
//! use mediation::ScaledAmount;
//!
//! let charge = ScaledAmount::new(1_234, 3);
//! assert_eq!(charge.to_string(), "1.234");
//!
//! // Amounts add only at one scale — 1.234 and 1.23 are different precisions,
//! // and silently rescaling one is how a rounding error enters a settlement.
//! assert!(charge.checked_add(ScaledAmount::new(123, 2)).is_none());
//! assert_eq!(
//!     charge.checked_add(ScaledAmount::new(766, 3)),
//!     Some(ScaledAmount::new(2_000, 3)),
//! );
//! ```
//!
//! # A returned batch and a returned record reconcile differently
//!
//! GSMA TD.32 distinguishes two rejection classes, and conflating them breaks
//! the conservation law in opposite directions:
//!
//! | Class | Rejects | Consequence |
//! |---|---|---|
//! | **Fatal** | the whole file | none of its events ever entered the counts; the sequence number stays open |
//! | **Severe** | individual events | the file counted; the returned events must be subtracted |
//!
//! [`RapReturn::effect_on`] applies each correctly rather than leaving a caller
//! to remember which is which.

use std::fmt;

use crate::cdrtime::CdrTimestamp;
use crate::disposition::BatchReconciliation;
use crate::plmn::TadigCode;
use crate::sequence::{SequenceSpace, SequenceValue};

// ── ScaledAmount ─────────────────────────────────────────────────────────────

/// A monetary amount as an integer and a decimal scale.
///
/// No float accessor exists, and none will: the integer encoding is there
/// precisely because a float cannot represent a settlement amount exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScaledAmount {
    value: i64,
    decimal_places: u8,
}

impl ScaledAmount {
    /// The most decimal places this type carries.
    ///
    /// Nine is beyond any currency and beyond the precision TAP files use; past
    /// it the scaling factor stops fitting comfortably alongside the value.
    pub const MAX_DECIMAL_PLACES: u8 = 9;

    /// An amount at a stated scale.
    ///
    /// A `decimal_places` beyond [`MAX_DECIMAL_PLACES`](Self::MAX_DECIMAL_PLACES)
    /// is **not** clamped. It is stored as given and reported by
    /// [`is_representable`](Self::is_representable), so a caller carrying an
    /// implausible scale learns of it rather than having their amount quietly
    /// rescaled into a different number.
    #[must_use]
    pub const fn new(value: i64, decimal_places: u8) -> Self {
        Self {
            value,
            decimal_places,
        }
    }

    /// Zero at a given scale.
    #[must_use]
    pub const fn zero(decimal_places: u8) -> Self {
        Self::new(0, decimal_places)
    }

    /// The raw integer.
    #[must_use]
    pub const fn value(self) -> i64 {
        self.value
    }

    /// How many of the integer's digits are fractional.
    #[must_use]
    pub const fn decimal_places(self) -> u8 {
        self.decimal_places
    }

    /// `true` when the scale is one this type can render and compare.
    #[must_use]
    pub const fn is_representable(self) -> bool {
        self.decimal_places <= Self::MAX_DECIMAL_PLACES
    }

    /// Sum, `None` at differing scales or on overflow.
    ///
    /// Refusing to add across scales is deliberate. Rescaling the coarser
    /// operand invents precision it never had; rescaling the finer one discards
    /// precision somebody recorded. Either is a rounding error entering a
    /// settlement silently, so the caller is made to say which they meant.
    #[must_use]
    pub const fn checked_add(self, other: Self) -> Option<Self> {
        if self.decimal_places != other.decimal_places {
            return None;
        }
        match self.value.checked_add(other.value) {
            Some(value) => Some(Self::new(value, self.decimal_places)),
            None => None,
        }
    }

    /// The amount negated, `None` at [`i64::MIN`].
    #[must_use]
    pub const fn checked_negate(self) -> Option<Self> {
        match self.value.checked_neg() {
            Some(value) => Some(Self::new(value, self.decimal_places)),
            None => None,
        }
    }

    /// `true` for an amount of zero, at any scale.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.value == 0
    }
}

impl fmt::Display for ScaledAmount {
    /// Writes the amount with its decimal point, exactly — no rounding, no
    /// float, and the sign in front of the whole number rather than the
    /// integer part.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let places = usize::from(self.decimal_places);
        if places == 0 {
            return write!(f, "{}", self.value);
        }
        let negative = self.value < 0;
        let digits = self.value.unsigned_abs().to_string();
        let padded = if digits.len() <= places {
            format!("{}{digits}", "0".repeat(places - digits.len() + 1))
        } else {
            digits
        };
        let split = padded.len() - places;
        if negative {
            f.write_str("-")?;
        }
        write!(f, "{}.{}", &padded[..split], &padded[split..])
    }
}

// ── TapBatch ─────────────────────────────────────────────────────────────────

/// What a TAP batch says about itself.
///
/// The fields a `batchControlInfo` and `auditControlInfo` carry, without the
/// events. Every one of them is a claim the events can be checked against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapBatch {
    /// The network that produced the batch.
    pub sender: TadigCode,
    /// The network it was sent to.
    pub recipient: TadigCode,
    /// The batch's position in the sequence between this pair.
    pub file_sequence: SequenceValue,
    /// When the file was created.
    pub created: CdrTimestamp,
    /// How many events the audit block claims.
    pub declared_events: u64,
    /// The earliest event timestamp the audit block claims.
    pub earliest_event: Option<CdrTimestamp>,
    /// The latest event timestamp the audit block claims.
    pub latest_event: Option<CdrTimestamp>,
    /// The total charge the audit block claims.
    pub declared_charge: ScaledAmount,
}

impl TapBatch {
    /// The sequence space TAP and RAP file numbers wrap within: `1` … `99 999`.
    pub const SEQUENCE_SPACE: SequenceSpace = SequenceSpace::TAP;

    /// The window TD.57 allows between an event and the file carrying it.
    ///
    /// An event has 30 days from occurrence to reach a TAP file; past that it
    /// can no longer be charged.
    pub const INCLUSION_DAYS: i64 = 30;

    /// Check every claim the batch makes about itself.
    ///
    /// `actual_events` and `actual_charge` come from the events the caller
    /// decoded, so the check is between what the file *says* and what it
    /// *holds* — which is the only comparison worth making.
    ///
    /// Returns every discrepancy found, not the first: a batch with three
    /// problems should be fixed once.
    #[must_use]
    pub fn audit(&self, actual_events: u64, actual_charge: ScaledAmount) -> Vec<BatchDiscrepancy> {
        let mut found = Vec::new();

        if self.sender == self.recipient {
            found.push(BatchDiscrepancy::SenderIsRecipient);
        }
        if self.declared_events != actual_events {
            found.push(BatchDiscrepancy::EventCountMismatch {
                declared: self.declared_events,
                actual: actual_events,
            });
        }
        if self.declared_charge.decimal_places() != actual_charge.decimal_places() {
            found.push(BatchDiscrepancy::ChargeScaleMismatch {
                declared: self.declared_charge.decimal_places(),
                actual: actual_charge.decimal_places(),
            });
        } else if self.declared_charge != actual_charge {
            found.push(BatchDiscrepancy::ChargeMismatch {
                declared: self.declared_charge,
                actual: actual_charge,
            });
        }
        if !self.declared_charge.is_representable() {
            found.push(BatchDiscrepancy::ChargeScaleImplausible {
                decimal_places: self.declared_charge.decimal_places(),
            });
        }

        if let (Some(earliest), Some(latest)) = (self.earliest_event, self.latest_event)
            && earliest.to_utc() > latest.to_utc()
        {
            found.push(BatchDiscrepancy::EventWindowInverted);
        }
        if let Some(earliest) = self.earliest_event {
            let age = self.created.to_utc() - earliest.to_utc();
            if age > time::Duration::days(Self::INCLUSION_DAYS) {
                found.push(BatchDiscrepancy::EventBeyondInclusionWindow { age });
            }
        }
        if self.declared_events == 0
            && (self.earliest_event.is_some() || self.latest_event.is_some())
        {
            found.push(BatchDiscrepancy::EmptyBatchWithAWindow);
        }

        found
    }

    /// `true` when a timestamp falls inside the window the batch declares.
    ///
    /// An event outside it is either misfiled or evidence the audit block is
    /// wrong; either way the batch does not describe what it holds.
    #[must_use]
    pub fn covers(&self, event: CdrTimestamp) -> bool {
        match (self.earliest_event, self.latest_event) {
            (Some(earliest), Some(latest)) => {
                event.to_utc() >= earliest.to_utc() && event.to_utc() <= latest.to_utc()
            }
            _ => false,
        }
    }
}

/// A claim a batch made that its events do not support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchDiscrepancy {
    /// The batch names one network as both sender and recipient.
    SenderIsRecipient,
    /// The audit block's event count is not what arrived.
    EventCountMismatch {
        /// What the audit block claimed.
        declared: u64,
        /// What the events actually number.
        actual: u64,
    },
    /// The audit block's total is not the sum of the events.
    ChargeMismatch {
        /// What the audit block claimed.
        declared: ScaledAmount,
        /// What the events actually sum to.
        actual: ScaledAmount,
    },
    /// The declared and actual totals are at different scales.
    ///
    /// Distinct from a value mismatch: the two numbers are not comparable at
    /// all, and reporting it as a value difference would send someone hunting a
    /// missing charge that is really a decoding fault.
    ChargeScaleMismatch {
        /// The audit block's scale.
        declared: u8,
        /// The events' scale.
        actual: u8,
    },
    /// The declared scale is beyond anything a currency uses.
    ChargeScaleImplausible {
        /// The scale claimed.
        decimal_places: u8,
    },
    /// The earliest event is later than the latest.
    EventWindowInverted,
    /// An event predates the file by more than the inclusion window.
    EventBeyondInclusionWindow {
        /// How old the earliest event was when the file was created.
        age: time::Duration,
    },
    /// The batch declares no events but still states a window.
    EmptyBatchWithAWindow,
}

impl BatchDiscrepancy {
    /// `true` when the batch cannot be accepted at all.
    ///
    /// A count or charge that disagrees means the file does not describe what
    /// it holds, so nothing in it can be trusted. An inclusion-window breach is
    /// about individual events and is reported without condemning the file.
    #[must_use]
    pub const fn rejects_the_file(self) -> bool {
        matches!(
            self,
            Self::SenderIsRecipient
                | Self::EventCountMismatch { .. }
                | Self::ChargeMismatch { .. }
                | Self::ChargeScaleMismatch { .. }
                | Self::EventWindowInverted
        )
    }
}

impl fmt::Display for BatchDiscrepancy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SenderIsRecipient => f.write_str("sender and recipient are the same network"),
            Self::EventCountMismatch { declared, actual } => {
                write!(f, "audit block claims {declared} events, {actual} arrived")
            }
            Self::ChargeMismatch { declared, actual } => {
                write!(f, "audit block claims {declared}, events sum to {actual}")
            }
            Self::ChargeScaleMismatch { declared, actual } => write!(
                f,
                "audit block uses {declared} decimal places, events use {actual}"
            ),
            Self::ChargeScaleImplausible { decimal_places } => {
                write!(f, "{decimal_places} decimal places is beyond any currency")
            }
            Self::EventWindowInverted => f.write_str("the earliest event is later than the latest"),
            Self::EventBeyondInclusionWindow { age } => {
                write!(f, "the earliest event is {age} old, past the 30-day window")
            }
            Self::EmptyBatchWithAWindow => {
                f.write_str("no events declared, yet a window is stated")
            }
        }
    }
}

// ── RAP ──────────────────────────────────────────────────────────────────────

/// How much of a batch a partner returned (GSMA TD.32).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RapReturn {
    /// The whole file was rejected.
    ///
    /// None of its events ever entered the recipient's counts, so the sequence
    /// number stays open and a resubmission fills it.
    Fatal,
    /// Individual events were rejected; the file itself was accepted.
    Severe {
        /// How many events came back.
        returned_events: u64,
    },
}

impl RapReturn {
    /// `true` when the whole file must be resent.
    #[must_use]
    pub const fn rejects_the_file(self) -> bool {
        matches!(self, Self::Fatal)
    }

    /// How many events the recipient kept, given what the batch held.
    ///
    /// `None` when a severe return claims more events than the batch contained,
    /// which means the two sides disagree about the file itself.
    #[must_use]
    pub const fn accepted_events(self, batch_events: u64) -> Option<u64> {
        match self {
            Self::Fatal => Some(0),
            Self::Severe { returned_events } => {
                if returned_events > batch_events {
                    None
                } else {
                    Some(batch_events - returned_events)
                }
            }
        }
    }

    /// Apply the return to a reconciliation the batch had produced.
    ///
    /// A **fatal** return means the batch never counted: the result is empty,
    /// and the caller re-presents the file when it is resent. A **severe**
    /// return leaves the batch counted and moves the returned events out of
    /// `billable` and into `rejected`.
    ///
    /// `None` when more events came back than the batch released, which the
    /// caller should treat as a disagreement with the partner rather than as
    /// arithmetic to absorb.
    #[must_use]
    pub fn effect_on(self, batch: BatchReconciliation) -> Option<BatchReconciliation> {
        match self {
            Self::Fatal => Some(BatchReconciliation::EMPTY),
            Self::Severe { returned_events } => {
                if returned_events > batch.released() {
                    return None;
                }
                // Rebuild rather than mutate: the counters are private, and
                // moving records between outcomes is exactly the operation the
                // conservation law exists to police.
                let mut adjusted = BatchReconciliation::EMPTY;
                for _ in 0..(batch.released() - returned_events) {
                    adjusted.observe(&crate::disposition::Disposition::Billable);
                }
                for _ in 0..returned_events {
                    adjusted.observe(&crate::disposition::Disposition::Rejected(
                        crate::disposition::RejectReason::Malformed,
                    ));
                }
                for _ in 0..batch.suppressed() {
                    adjusted.observe(&crate::disposition::Disposition::Suppressed(
                        crate::disposition::SuppressReason::OperatorPolicy,
                    ));
                }
                for _ in 0..batch.rejected() {
                    adjusted.observe(&crate::disposition::Disposition::Rejected(
                        crate::disposition::RejectReason::Malformed,
                    ));
                }
                for _ in 0..batch.held() {
                    adjusted.observe(&crate::disposition::Disposition::Held(
                        crate::disposition::HoldReason::AwaitingCorrelation,
                    ));
                }
                for _ in 0..batch.duplicate() {
                    adjusted.observe(&crate::disposition::Disposition::Suppressed(
                        crate::disposition::SuppressReason::OperatorPolicy,
                    ));
                }
                Some(adjusted)
            }
        }
    }
}

impl fmt::Display for RapReturn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fatal => f.write_str("FATAL: the whole file"),
            Self::Severe { returned_events } => {
                write!(f, "SEVERE: {returned_events} event(s)")
            }
        }
    }
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::ScaledAmount;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// The integer and its scale, never a decimal string and never a float —
    /// both would need re-parsing to be added.
    #[derive(Serialize, Deserialize)]
    #[serde(rename = "ScaledAmount")]
    struct Repr {
        value: i64,
        decimal_places: u8,
    }

    impl Serialize for ScaledAmount {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            Repr {
                value: self.value(),
                decimal_places: self.decimal_places(),
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for ScaledAmount {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = Repr::deserialize(deserializer)?;
            Ok(ScaledAmount::new(r.value, r.decimal_places))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disposition::{Disposition, HoldReason};

    fn seq(value: u32) -> SequenceValue {
        SequenceValue::new(TapBatch::SEQUENCE_SPACE, value).expect("in space")
    }

    fn batch(declared_events: u64, charge: ScaledAmount) -> TapBatch {
        TapBatch {
            sender: "DEUD1".parse().unwrap(),
            recipient: "GBRVF".parse().unwrap(),
            file_sequence: seq(42),
            created: "2026-07-28T10:00:00+02:00".parse().unwrap(),
            declared_events,
            earliest_event: Some("2026-07-20T10:00:00+02:00".parse().unwrap()),
            latest_event: Some("2026-07-27T10:00:00+02:00".parse().unwrap()),
            declared_charge: charge,
        }
    }

    // ── ScaledAmount ─────────────────────────────────────────────────────────

    #[test]
    fn an_amount_renders_exactly_without_a_float() {
        assert_eq!(ScaledAmount::new(1_234, 3).to_string(), "1.234");
        assert_eq!(ScaledAmount::new(1_234, 0).to_string(), "1234");
        assert_eq!(ScaledAmount::new(5, 3).to_string(), "0.005");
        assert_eq!(ScaledAmount::new(0, 2).to_string(), "0.00");
        assert_eq!(ScaledAmount::new(-1_234, 3).to_string(), "-1.234");
        assert_eq!(ScaledAmount::new(-5, 3).to_string(), "-0.005");
    }

    /// Adding across scales would either invent precision or discard it, so the
    /// caller is made to say which they meant.
    #[test]
    fn amounts_add_only_at_one_scale() {
        let three = ScaledAmount::new(1_234, 3);
        assert_eq!(three.checked_add(ScaledAmount::new(123, 2)), None);
        assert_eq!(
            three.checked_add(ScaledAmount::new(766, 3)),
            Some(ScaledAmount::new(2_000, 3))
        );
        assert_eq!(
            ScaledAmount::new(i64::MAX, 3).checked_add(ScaledAmount::new(1, 3)),
            None
        );
    }

    #[test]
    fn an_amount_negates_and_an_implausible_scale_is_reported() {
        let amount = ScaledAmount::new(1_234, 3);
        assert_eq!(
            amount.checked_negate().unwrap(),
            ScaledAmount::new(-1_234, 3)
        );
        assert_eq!(ScaledAmount::new(i64::MIN, 3).checked_negate(), None);

        assert!(amount.is_representable());
        assert!(!ScaledAmount::new(1, 12).is_representable());
        assert!(ScaledAmount::zero(2).is_zero());
    }

    // ── Batch audit ──────────────────────────────────────────────────────────

    #[test]
    fn a_batch_that_describes_itself_correctly_reports_nothing() {
        let charge = ScaledAmount::new(1_234, 3);
        assert!(batch(10, charge).audit(10, charge).is_empty());
    }

    /// The check worth having: the file's own claims against what it holds.
    #[test]
    fn a_count_or_charge_that_disagrees_rejects_the_file() {
        let charge = ScaledAmount::new(1_234, 3);

        let miscounted = batch(10, charge).audit(9, charge);
        assert_eq!(
            miscounted,
            vec![BatchDiscrepancy::EventCountMismatch {
                declared: 10,
                actual: 9
            }]
        );
        assert!(miscounted[0].rejects_the_file());

        let mischarged = batch(10, charge).audit(10, ScaledAmount::new(1_000, 3));
        assert!(matches!(
            mischarged.as_slice(),
            [BatchDiscrepancy::ChargeMismatch { .. }]
        ));
        assert!(mischarged[0].rejects_the_file());
    }

    /// A scale difference is a decoding fault, not a missing charge — reporting
    /// it as a value difference sends someone hunting the wrong thing.
    #[test]
    fn a_scale_difference_is_distinguished_from_a_value_difference() {
        let declared = ScaledAmount::new(1_234, 3);
        let actual = ScaledAmount::new(1_234, 2);

        let found = batch(10, declared).audit(10, actual);
        assert_eq!(
            found,
            vec![BatchDiscrepancy::ChargeScaleMismatch {
                declared: 3,
                actual: 2,
            }]
        );
        assert!(
            !found
                .iter()
                .any(|d| matches!(d, BatchDiscrepancy::ChargeMismatch { .. })),
            "a scale mismatch must not also read as a value mismatch"
        );
    }

    /// Every discrepancy is reported: a batch with three problems should be
    /// fixed once.
    #[test]
    fn every_discrepancy_is_reported_not_only_the_first() {
        let mut broken = batch(10, ScaledAmount::new(1, 12));
        broken.recipient = broken.sender;
        broken.earliest_event = Some("2026-01-01T10:00:00+02:00".parse().unwrap());

        let found = broken.audit(9, ScaledAmount::new(1, 12));
        assert!(found.len() >= 4, "{found:?}");
        assert!(found.contains(&BatchDiscrepancy::SenderIsRecipient));
        assert!(
            found
                .iter()
                .any(|d| matches!(d, BatchDiscrepancy::EventCountMismatch { .. }))
        );
        assert!(
            found
                .iter()
                .any(|d| matches!(d, BatchDiscrepancy::ChargeScaleImplausible { .. }))
        );
        assert!(
            found
                .iter()
                .any(|d| matches!(d, BatchDiscrepancy::EventBeyondInclusionWindow { .. }))
        );
    }

    /// TD.57's 30-day rule, checked against the file's own creation time.
    #[test]
    fn the_thirty_day_inclusion_window_is_measured_from_file_creation() {
        let charge = ScaledAmount::zero(3);

        let mut inside = batch(1, charge);
        inside.earliest_event = Some("2026-06-29T10:00:00+02:00".parse().unwrap());
        assert!(
            !inside
                .audit(1, charge)
                .iter()
                .any(|d| matches!(d, BatchDiscrepancy::EventBeyondInclusionWindow { .. })),
            "29 days is inside the window"
        );

        let mut outside = batch(1, charge);
        outside.earliest_event = Some("2026-06-27T10:00:00+02:00".parse().unwrap());
        assert!(
            outside
                .audit(1, charge)
                .iter()
                .any(|d| matches!(d, BatchDiscrepancy::EventBeyondInclusionWindow { .. }))
        );
    }

    /// An inclusion breach is about individual events, so it does not condemn
    /// the file the way a count mismatch does.
    #[test]
    fn an_inclusion_breach_does_not_reject_the_whole_file() {
        assert!(
            !BatchDiscrepancy::EventBeyondInclusionWindow {
                age: time::Duration::days(40)
            }
            .rejects_the_file()
        );
        assert!(BatchDiscrepancy::EventWindowInverted.rejects_the_file());
    }

    #[test]
    fn an_inverted_or_impossible_window_is_caught() {
        let charge = ScaledAmount::zero(3);

        let mut inverted = batch(1, charge);
        inverted.earliest_event = Some("2026-07-27T10:00:00+02:00".parse().unwrap());
        inverted.latest_event = Some("2026-07-20T10:00:00+02:00".parse().unwrap());
        assert!(
            inverted
                .audit(1, charge)
                .contains(&BatchDiscrepancy::EventWindowInverted)
        );

        let mut empty = batch(0, charge);
        empty.earliest_event = Some("2026-07-27T10:00:00+02:00".parse().unwrap());
        assert!(
            empty
                .audit(0, charge)
                .contains(&BatchDiscrepancy::EmptyBatchWithAWindow)
        );
    }

    #[test]
    fn the_declared_window_is_checkable_against_an_event() {
        let charge = ScaledAmount::zero(3);
        let batch = batch(1, charge);

        assert!(batch.covers("2026-07-25T10:00:00+02:00".parse().unwrap()));
        assert!(!batch.covers("2026-07-19T10:00:00+02:00".parse().unwrap()));
        assert!(!batch.covers("2026-07-28T09:00:00+02:00".parse().unwrap()));
    }

    #[test]
    fn a_tap_sequence_wraps_within_the_published_range() {
        assert_eq!(TapBatch::SEQUENCE_SPACE.first(), 1);
        assert_eq!(TapBatch::SEQUENCE_SPACE.last(), 99_999);
        assert!(SequenceValue::new(TapBatch::SEQUENCE_SPACE, 0).is_none());
    }

    // ── RAP ──────────────────────────────────────────────────────────────────

    /// The distinction that breaks the conservation law when it is missed: a
    /// fatal return means the batch never counted at all.
    #[test]
    fn a_fatal_return_empties_the_batch_and_a_severe_one_adjusts_it() {
        let mut original = BatchReconciliation::EMPTY;
        for _ in 0..10 {
            original.observe(&Disposition::Billable);
        }

        let fatal = RapReturn::Fatal.effect_on(original).unwrap();
        assert_eq!(fatal, BatchReconciliation::EMPTY);
        assert_eq!(fatal.ingested(), 0, "none of it ever counted");
        assert_eq!(RapReturn::Fatal.accepted_events(10), Some(0));

        let severe = RapReturn::Severe { returned_events: 3 }
            .effect_on(original)
            .unwrap();
        assert_eq!(severe.ingested(), 10, "the file still counted");
        assert_eq!(severe.released(), 7);
        assert_eq!(severe.rejected(), 3);
        assert!(severe.balances());
        assert_eq!(
            RapReturn::Severe { returned_events: 3 }.accepted_events(10),
            Some(7)
        );
    }

    /// Returning more than was released means the two sides disagree about the
    /// file, which is not arithmetic to absorb.
    #[test]
    fn returning_more_than_was_released_is_a_disagreement() {
        let mut original = BatchReconciliation::EMPTY;
        original.observe(&Disposition::Billable);

        assert_eq!(
            RapReturn::Severe { returned_events: 5 }.effect_on(original),
            None
        );
        assert_eq!(
            RapReturn::Severe { returned_events: 5 }.accepted_events(1),
            None
        );
    }

    /// Outcomes other than billable survive a severe return, and the law still
    /// holds afterwards.
    #[test]
    fn a_severe_return_preserves_the_batchs_other_outcomes() {
        let mut original = BatchReconciliation::EMPTY;
        for _ in 0..5 {
            original.observe(&Disposition::Billable);
        }
        original.observe(&Disposition::Held(HoldReason::AwaitingPartial));
        original.observe(&Disposition::Rejected(
            crate::disposition::RejectReason::Malformed,
        ));

        let adjusted = RapReturn::Severe { returned_events: 2 }
            .effect_on(original)
            .unwrap();
        assert_eq!(adjusted.ingested(), original.ingested());
        assert_eq!(adjusted.released(), 3);
        assert_eq!(adjusted.held(), 1);
        assert!(adjusted.balances());
    }

    #[test]
    fn returns_render_legibly() {
        assert_eq!(RapReturn::Fatal.to_string(), "FATAL: the whole file");
        assert_eq!(
            RapReturn::Severe { returned_events: 3 }.to_string(),
            "SEVERE: 3 event(s)"
        );
        assert!(RapReturn::Fatal.rejects_the_file());
        assert!(!RapReturn::Severe { returned_events: 3 }.rejects_the_file());
    }
}
