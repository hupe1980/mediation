//! The conservation law: every ingested record reaches exactly one outcome.
//!
//! This is the difference between a mediation layer and a data pipeline. A
//! pipeline moves records; a mediation layer can **prove** it did not lose any.
//!
//! ```text
//! carried_in + ingested == billable + suppressed + duplicate + rejected + held
//! ```
//!
//! [`BatchReconciliation::balances`] is that equation, and a batch that does not
//! satisfy it is not releasable — its completeness is unknown, and billing over
//! unknown completeness invoices an amount nobody can prove is right.
//!
//! # The law spans batches, not just one
//!
//! A [`Disposition::Held`] record comes back. If the batch that re-presents it
//! counts it as newly ingested, the two batches together report it twice; if
//! neither counts it, it vanishes. Both are the conservation law failing at a
//! boundary, which is exactly where nobody looks.
//!
//! So a re-presented record is counted through
//! [`observe_carried`](BatchReconciliation::observe_carried) rather than
//! [`observe`](BatchReconciliation::observe), and [`PeriodLedger`] checks that
//! each batch's `carried_in` equals its predecessor's `held`. Chained that way
//! the law telescopes: across a whole period, everything ingested reached a
//! final outcome or is still explicitly outstanding.
//!
//! ```rust
//! use mediation::{BatchReconciliation, Disposition, SuppressReason};
//!
//! let mut counts = BatchReconciliation::EMPTY;
//! counts.observe(&Disposition::Billable);
//! counts.observe(&Disposition::Billable);
//! counts.observe(&Disposition::Suppressed(SuppressReason::TestIdentity));
//!
//! assert_eq!(counts.ingested(), 3);
//! assert_eq!(counts.released(), 2);
//! assert!(counts.balances());
//! ```
//!
//! # Held is not a sixth kind of failure
//!
//! Four outcomes are final. [`Disposition::Held`] is the one that may return —
//! a partial record whose successor has not arrived, a leg whose counterpart is
//! still in flight, an identity awaiting reference data. It still *counts*, so
//! the batch balances, but it also says the batch is not finished, which is
//! exactly the information a period close needs.
//!
//! # Merging
//!
//! [`BatchReconciliation::checked_add`] exists so a caller can process disjoint
//! key ranges in parallel and recombine. Because the crate reads no clock and
//! performs no I/O, that parallelisation is trivially safe, and the
//! conservation law makes the recombination checkable rather than hopeful.
//!
//! Counts are also the raw material for revenue-assurance reporting, which is
//! why they are a public type rather than a log line.

use std::fmt;

use crate::dedup::{NotKeyable, RecordKey};
use crate::lateness::Lateness;
use crate::party::UnattributableReason;

// ── Reasons ──────────────────────────────────────────────────────────────────

/// Why a record was deliberately not billed.
///
/// Suppression is a decision, not a failure: the record was understood and a
/// policy said not to charge for it. Distinguishing it from rejection is what
/// keeps a revenue-assurance report readable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SuppressReason {
    /// A test SIM, a probe, or an internal test identity.
    TestIdentity,
    /// Zero-rated by policy — sponsored data, an emergency call, a free tier.
    ZeroRated,
    /// Traffic internal to the operator, netted off before billing.
    InternalTraffic,
    /// The record described no usage at all.
    ZeroUsage,
    /// An operator policy not covered by the cases above.
    OperatorPolicy,
}

/// Why a record can never be billed.
///
/// Terminal. A rejected record will not be retried, so anything that *could*
/// succeed later belongs in [`HoldReason`] instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RejectReason {
    /// Past the inclusion deadline — see [`crate::lateness`].
    BeyondDeadline,
    /// Timestamped ahead of the evaluation instant: clock skew or corruption.
    FutureTimestamp,
    /// No charged party could be determined.
    Unattributable(UnattributableReason),
    /// The record carries no natural key, so it cannot be deduplicated.
    ///
    /// Billing an undeduplicatable record risks charging for it twice, which is
    /// the more expensive of the two errors.
    NotKeyable(NotKeyable),
    /// The record failed structural validation.
    Malformed,
}

/// Why a record is waiting rather than finished.
///
/// The only non-final outcome. Each variant names something that may still
/// arrive, which is what distinguishes it from a rejection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HoldReason {
    /// A partial record whose siblings have not all arrived.
    AwaitingPartial,
    /// A leg whose counterpart has not arrived.
    AwaitingCorrelation,
    /// An identity that a reference-data load could still resolve.
    AwaitingReferenceData,
    /// The record's source has an unfilled sequence gap.
    ///
    /// Completeness is unknown for the whole source, so releasing any of its
    /// records would invoice an unprovable amount.
    SequenceGap,
}

macro_rules! coded_enum {
    ($ty:ty, $all:expr, $($variant:pat => $code:literal),+ $(,)?) => {
        impl $ty {
            /// Every variant, for exhaustive iteration in tests and mappings.
            pub const ALL: &'static [Self] = $all;

            /// The canonical string form.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $($variant => $code,)+
                }
            }
        }

        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl std::str::FromStr for $ty {
            type Err = crate::error::ParseError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let t = s.trim().to_ascii_uppercase();
                Self::ALL
                    .iter()
                    .find(|v| v.as_str() == t)
                    .copied()
                    .ok_or_else(|| {
                        crate::error::ParseError::format(
                            stringify!($ty),
                            s,
                            concat!("one of the ", stringify!($ty), " codes"),
                        )
                    })
            }
        }
    };
}

coded_enum!(
    SuppressReason,
    &[
        Self::TestIdentity,
        Self::ZeroRated,
        Self::InternalTraffic,
        Self::ZeroUsage,
        Self::OperatorPolicy,
    ],
    Self::TestIdentity => "TEST_IDENTITY",
    Self::ZeroRated => "ZERO_RATED",
    Self::InternalTraffic => "INTERNAL_TRAFFIC",
    Self::ZeroUsage => "ZERO_USAGE",
    Self::OperatorPolicy => "OPERATOR_POLICY",
);

coded_enum!(
    HoldReason,
    &[
        Self::AwaitingPartial,
        Self::AwaitingCorrelation,
        Self::AwaitingReferenceData,
        Self::SequenceGap,
    ],
    Self::AwaitingPartial => "AWAITING_PARTIAL",
    Self::AwaitingCorrelation => "AWAITING_CORRELATION",
    Self::AwaitingReferenceData => "AWAITING_REFERENCE_DATA",
    Self::SequenceGap => "SEQUENCE_GAP",
);

impl RejectReason {
    /// The rejection implied by a lateness verdict, if it implies one.
    ///
    /// The bridge between [`crate::lateness`] and this module, so a caller
    /// cannot map [`Lateness::Late`] onto a rejection by mistake — a late
    /// record is still billable.
    #[must_use]
    pub const fn from_lateness(lateness: Lateness) -> Option<Self> {
        match lateness {
            Lateness::BeyondDeadline(_) => Some(Self::BeyondDeadline),
            Lateness::Future(_) => Some(Self::FutureTimestamp),
            Lateness::OnTime | Lateness::Late(_) => None,
        }
    }

    /// A short code naming the rejection.
    #[must_use]
    pub const fn kind(self) -> &'static str {
        match self {
            Self::BeyondDeadline => "BEYOND_DEADLINE",
            Self::FutureTimestamp => "FUTURE_TIMESTAMP",
            Self::Unattributable(_) => "UNATTRIBUTABLE",
            Self::NotKeyable(_) => "NOT_KEYABLE",
            Self::Malformed => "MALFORMED",
        }
    }
}

impl fmt::Display for RejectReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unattributable(reason) => write!(f, "UNATTRIBUTABLE:{reason}"),
            Self::NotKeyable(reason) => write!(f, "NOT_KEYABLE:{reason}"),
            other => f.write_str(other.kind()),
        }
    }
}

// ── Disposition ──────────────────────────────────────────────────────────────

/// What became of one ingested record.
///
/// Exhaustive, and deliberately not `#[non_exhaustive]`: a consumer mapping
/// these outcomes onto their own storage codes should get a compile error when
/// a new one appears, not a silent wildcard that files a rejected record as
/// billable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disposition {
    /// Released downstream to be rated.
    Billable,
    /// Understood, and deliberately not billed.
    Suppressed(SuppressReason),
    /// Already seen; the surviving copy carries the usage.
    Duplicate(RecordKey),
    /// Cannot be billed, and will not be retried.
    Rejected(RejectReason),
    /// Not finished — something that may still arrive is missing.
    Held(HoldReason),
}

impl Disposition {
    /// `true` for the one outcome that reaches a rating engine.
    #[must_use]
    pub const fn is_released(&self) -> bool {
        matches!(self, Self::Billable)
    }

    /// `true` when the record may yet reach a different outcome.
    ///
    /// Only [`Held`](Self::Held). The other four are final, which is what makes
    /// the reconciliation of a closed period meaningful.
    #[must_use]
    pub const fn may_return(&self) -> bool {
        matches!(self, Self::Held(_))
    }

    /// `true` when an operator should look at this record.
    #[must_use]
    pub const fn is_finding(&self) -> bool {
        matches!(self, Self::Rejected(_) | Self::Held(_))
    }

    /// A short code naming the outcome, for grouping and reporting.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Billable => "BILLABLE",
            Self::Suppressed(_) => "SUPPRESSED",
            Self::Duplicate(_) => "DUPLICATE",
            Self::Rejected(_) => "REJECTED",
            Self::Held(_) => "HELD",
        }
    }
}

impl fmt::Display for Disposition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Billable => f.write_str("BILLABLE"),
            Self::Suppressed(reason) => write!(f, "SUPPRESSED:{reason}"),
            Self::Duplicate(key) => write!(f, "DUPLICATE:{key}"),
            Self::Rejected(reason) => write!(f, "REJECTED:{reason}"),
            Self::Held(reason) => write!(f, "HELD:{reason}"),
        }
    }
}

// ── BatchReconciliation ──────────────────────────────────────────────────────

/// The counting law over one batch.
///
/// Every counter is `u64` and every arithmetic operation is checked: a wrapped
/// count would make an unbalanced batch appear balanced, which is the one
/// failure this type exists to make impossible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BatchReconciliation {
    carried_in: u64,
    ingested: u64,
    billable: u64,
    suppressed: u64,
    duplicate: u64,
    rejected: u64,
    held: u64,
}

impl BatchReconciliation {
    /// Nothing counted yet.
    pub const EMPTY: Self = Self {
        carried_in: 0,
        ingested: 0,
        billable: 0,
        suppressed: 0,
        duplicate: 0,
        rejected: 0,
        held: 0,
    };

    /// Count a **newly arrived** record's outcome.
    ///
    /// Increments `ingested` and exactly one of the outcome counters, which is
    /// what makes [`balances`](Self::balances) hold by construction for any
    /// reconciliation built solely through this method and
    /// [`observe_carried`](Self::observe_carried). A batch that fails the check
    /// was therefore assembled some other way — which is precisely what the
    /// check is for.
    pub const fn observe(&mut self, disposition: &Disposition) {
        self.ingested += 1;
        self.count_outcome(disposition);
    }

    /// Count a record **held by an earlier batch** and re-presented here.
    ///
    /// Increments `carried_in` rather than `ingested`, so the record is not
    /// counted as arriving twice. Getting this wrong is the one way the
    /// conservation law fails at a batch boundary, which is the place nobody
    /// audits.
    pub const fn observe_carried(&mut self, disposition: &Disposition) {
        self.carried_in += 1;
        self.count_outcome(disposition);
    }

    const fn count_outcome(&mut self, disposition: &Disposition) {
        match disposition {
            Disposition::Billable => self.billable += 1,
            Disposition::Suppressed(_) => self.suppressed += 1,
            Disposition::Duplicate(_) => self.duplicate += 1,
            Disposition::Rejected(_) => self.rejected += 1,
            Disposition::Held(_) => self.held += 1,
        }
    }

    /// How many records arrived for the first time.
    #[must_use]
    pub const fn ingested(&self) -> u64 {
        self.ingested
    }

    /// How many records an earlier batch held and this one re-presented.
    #[must_use]
    pub const fn carried_in(&self) -> u64 {
        self.carried_in
    }

    /// Everything this batch had to account for: `carried_in + ingested`.
    ///
    /// `None` on overflow, which is itself an imbalance.
    #[must_use]
    pub const fn presented(&self) -> Option<u64> {
        self.carried_in.checked_add(self.ingested)
    }

    /// How many were released downstream.
    #[must_use]
    pub const fn released(&self) -> u64 {
        self.billable
    }

    /// How many were understood and deliberately not billed.
    #[must_use]
    pub const fn suppressed(&self) -> u64 {
        self.suppressed
    }

    /// How many were recognised as retransmissions.
    #[must_use]
    pub const fn duplicate(&self) -> u64 {
        self.duplicate
    }

    /// How many can never be billed.
    #[must_use]
    pub const fn rejected(&self) -> u64 {
        self.rejected
    }

    /// How many are waiting for something that may still arrive.
    #[must_use]
    pub const fn held(&self) -> u64 {
        self.held
    }

    /// Whether every ingested record reached exactly one outcome.
    #[must_use]
    pub const fn balances(&self) -> bool {
        // Written as nested matches rather than a combinator chain so the whole
        // check stays `const`. An overflowing sum returns `false`: it cannot
        // equal an ingested count that did not itself overflow, so an
        // unbalanced batch never reads as balanced by luck.
        macro_rules! accumulate {
            ($acc:expr, $field:ident) => {
                match $acc.checked_add(self.$field) {
                    Some(value) => value,
                    None => return false,
                }
            };
        }
        let total = self.billable;
        let total = accumulate!(total, suppressed);
        let total = accumulate!(total, duplicate);
        let total = accumulate!(total, rejected);
        let total = accumulate!(total, held);
        match self.presented() {
            Some(presented) => total == presented,
            None => false,
        }
    }

    /// `true` when the batch balances and nothing is still waiting.
    ///
    /// The precondition for closing a period over this batch: a held record may
    /// still become billable, so a period closed while records are held will
    /// need reopening.
    #[must_use]
    pub const fn is_final(&self) -> bool {
        self.balances() && self.held == 0
    }

    /// Combine two reconciliations, `None` on overflow.
    ///
    /// For recombining work done over disjoint key ranges.
    #[must_use]
    pub const fn checked_add(self, other: Self) -> Option<Self> {
        macro_rules! add {
            ($field:ident) => {
                match self.$field.checked_add(other.$field) {
                    Some(v) => v,
                    None => return None,
                }
            };
        }
        Some(Self {
            carried_in: add!(carried_in),
            ingested: add!(ingested),
            billable: add!(billable),
            suppressed: add!(suppressed),
            duplicate: add!(duplicate),
            rejected: add!(rejected),
            held: add!(held),
        })
    }
}

impl fmt::Display for BatchReconciliation {
    /// One line, in the order of the conservation law, so an imbalance is
    /// visible by inspection.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "carried_in={} + ingested={} = billable={} + suppressed={} + duplicate={} + rejected={} + held={} [{}]",
            self.carried_in,
            self.ingested,
            self.billable,
            self.suppressed,
            self.duplicate,
            self.rejected,
            self.held,
            if self.balances() {
                "balanced"
            } else {
                "UNBALANCED"
            }
        )
    }
}

impl FromIterator<Disposition> for BatchReconciliation {
    fn from_iter<I: IntoIterator<Item = Disposition>>(iter: I) -> Self {
        let mut counts = Self::EMPTY;
        for disposition in iter {
            counts.observe(&disposition);
        }
        counts
    }
}

impl<'a> FromIterator<&'a Disposition> for BatchReconciliation {
    fn from_iter<I: IntoIterator<Item = &'a Disposition>>(iter: I) -> Self {
        let mut counts = Self::EMPTY;
        for disposition in iter {
            counts.observe(disposition);
        }
        counts
    }
}

// ── PeriodLedger ─────────────────────────────────────────────────────────────

/// Why a chain of batches does not add up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainBreak {
    /// A batch does not satisfy the conservation law on its own.
    Unbalanced {
        /// Which batch, counting from zero.
        batch: usize,
    },
    /// The period's running totals overflowed when this batch was added.
    ///
    /// Distinct from [`Unbalanced`](Self::Unbalanced): every batch may be
    /// internally consistent and the *period* still exceed what a `u64` can
    /// count. Reporting it as an imbalance would send an operator looking for a
    /// lost record that does not exist.
    TotalsOverflow {
        /// Which batch, counting from zero.
        batch: usize,
    },
    /// A batch's `carried_in` does not match what was outstanding before it.
    ///
    /// The failure mode this whole type exists to catch: held records counted
    /// twice across the boundary, or dropped at it.
    CarryMismatch {
        /// Which batch, counting from zero.
        batch: usize,
        /// What the previous batch left outstanding.
        expected: u64,
        /// What this batch claimed to carry in.
        found: u64,
    },
}

impl fmt::Display for ChainBreak {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unbalanced { batch } => write!(f, "batch {batch} does not balance"),
            Self::TotalsOverflow { batch } => {
                write!(f, "period totals overflowed at batch {batch}")
            }
            Self::CarryMismatch {
                batch,
                expected,
                found,
            } => write!(
                f,
                "batch {batch} carried in {found}, but {expected} were outstanding"
            ),
        }
    }
}

impl std::error::Error for ChainBreak {}

/// A sequence of batches, checked so the conservation law telescopes.
///
/// Each batch must balance on its own **and** carry in exactly what its
/// predecessor left held. Chained that way, one equation covers a whole billing
/// period: everything ever ingested has reached a final outcome, or is still
/// explicitly outstanding and named as such.
///
/// ```rust
/// use mediation::{BatchReconciliation, Disposition, HoldReason, PeriodLedger};
///
/// let mut first = BatchReconciliation::EMPTY;
/// first.observe(&Disposition::Billable);
/// first.observe(&Disposition::Held(HoldReason::AwaitingPartial));
///
/// // The second batch re-presents the held record; the successor arrived.
/// let mut second = BatchReconciliation::EMPTY;
/// second.observe_carried(&Disposition::Billable);
/// second.observe(&Disposition::Billable);
///
/// let mut ledger = PeriodLedger::new();
/// ledger.append(first).unwrap();
/// ledger.append(second).unwrap();
///
/// assert_eq!(ledger.outstanding(), 0);
/// assert_eq!(ledger.totals().released(), 3);
/// assert!(ledger.is_closable());
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeriodLedger {
    totals: BatchReconciliation,
    batches: usize,
    outstanding: u64,
    breaks: Vec<ChainBreak>,
}

impl PeriodLedger {
    /// An empty period.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            totals: BatchReconciliation::EMPTY,
            batches: 0,
            outstanding: 0,
            breaks: Vec::new(),
        }
    }

    /// Add the next batch of the period.
    ///
    /// # Errors
    ///
    /// [`ChainBreak`] when the batch does not balance, or when its `carried_in`
    /// disagrees with what was outstanding. The break is **also** recorded, so
    /// a caller that chooses to continue past one still sees it in
    /// [`breaks`](Self::breaks) — a period with a hole in the middle must not
    /// look clean because the caller kept going.
    pub fn append(&mut self, batch: BatchReconciliation) -> Result<(), ChainBreak> {
        let index = self.batches;
        self.batches += 1;

        let mut first_break = None;
        if !batch.balances() {
            let broken = ChainBreak::Unbalanced { batch: index };
            self.breaks.push(broken);
            first_break = Some(broken);
        }
        if batch.carried_in() != self.outstanding {
            let broken = ChainBreak::CarryMismatch {
                batch: index,
                expected: self.outstanding,
                found: batch.carried_in(),
            };
            self.breaks.push(broken);
            first_break = first_break.or(Some(broken));
        }

        self.outstanding = batch.held();
        match self.totals.checked_add(batch) {
            Some(totals) => self.totals = totals,
            None => {
                // The running totals are left untouched: a wrapped total would
                // be a plausible-looking wrong number, and the break says the
                // period can no longer be summed rather than that a record was
                // lost.
                let broken = ChainBreak::TotalsOverflow { batch: index };
                self.breaks.push(broken);
                first_break = first_break.or(Some(broken));
            }
        }

        match first_break {
            Some(broken) => Err(broken),
            None => Ok(()),
        }
    }

    /// How many records the most recent batch left held.
    #[must_use]
    pub const fn outstanding(&self) -> u64 {
        self.outstanding
    }

    /// How many batches the period holds.
    #[must_use]
    pub const fn batches(&self) -> usize {
        self.batches
    }

    /// Every break found, in the order they were found.
    #[must_use]
    pub fn breaks(&self) -> &[ChainBreak] {
        &self.breaks
    }

    /// The summed counts across the period.
    ///
    /// `carried_in` here is the sum of every batch's carry, which is a measure
    /// of how much work crossed a boundary rather than of anything owed.
    #[must_use]
    pub const fn totals(&self) -> BatchReconciliation {
        self.totals
    }

    /// `true` when the period may be closed.
    ///
    /// Every batch balanced, every carry matched, and nothing is still held.
    /// A period closed while records are outstanding will need reopening.
    #[must_use]
    pub fn is_closable(&self) -> bool {
        self.breaks.is_empty() && self.outstanding == 0
    }
}

impl fmt::Display for PeriodLedger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} batch(es), {} outstanding, {} break(s) [{}]",
            self.batches,
            self.outstanding,
            self.breaks.len(),
            if self.is_closable() {
                "closable"
            } else {
                "OPEN"
            }
        )
    }
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::{BatchReconciliation, HoldReason, SuppressReason};
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

    string_serde!(SuppressReason, "a suppression reason code");
    string_serde!(HoldReason, "a hold reason code");

    /// Field names are part of the public representation and covered by semver.
    #[derive(Serialize, Deserialize)]
    #[serde(rename = "BatchReconciliation")]
    struct Repr {
        #[serde(default)]
        carried_in: u64,
        ingested: u64,
        billable: u64,
        suppressed: u64,
        duplicate: u64,
        rejected: u64,
        held: u64,
    }

    impl Serialize for BatchReconciliation {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            Repr {
                carried_in: self.carried_in(),
                ingested: self.ingested(),
                billable: self.released(),
                suppressed: self.suppressed(),
                duplicate: self.duplicate(),
                rejected: self.rejected(),
                held: self.held(),
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for BatchReconciliation {
        /// Deserialises **without** requiring the law to hold.
        ///
        /// A stored reconciliation that does not balance is exactly the thing a
        /// caller needs to read back and investigate; refusing to decode it
        /// would hide the evidence.
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = Repr::deserialize(deserializer)?;
            Ok(BatchReconciliation {
                carried_in: r.carried_in,
                ingested: r.ingested,
                billable: r.billable,
                suppressed: r.suppressed,
                duplicate: r.duplicate,
                rejected: r.rejected,
                held: r.held,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lateness::Lateness;
    use time::Duration;

    fn key() -> RecordKey {
        RecordKey::RecordingEntity {
            entity: "msc-01".parse().unwrap(),
            local_sequence: 1,
        }
    }

    #[test]
    fn every_outcome_counts_exactly_once() {
        let dispositions = [
            Disposition::Billable,
            Disposition::Billable,
            Disposition::Suppressed(SuppressReason::TestIdentity),
            Disposition::Duplicate(key()),
            Disposition::Rejected(RejectReason::BeyondDeadline),
            Disposition::Held(HoldReason::AwaitingPartial),
        ];
        let counts: BatchReconciliation = dispositions.iter().collect();

        assert_eq!(counts.ingested(), 6);
        assert_eq!(counts.released(), 2);
        assert_eq!(counts.suppressed(), 1);
        assert_eq!(counts.duplicate(), 1);
        assert_eq!(counts.rejected(), 1);
        assert_eq!(counts.held(), 1);
        assert!(counts.balances());
    }

    /// A batch with something still in flight balances, and is still not final:
    /// a period closed over it would need reopening.
    #[test]
    fn holding_a_record_balances_but_does_not_finish_the_batch() {
        let counts: BatchReconciliation = vec![
            Disposition::Billable,
            Disposition::Held(HoldReason::AwaitingCorrelation),
        ]
        .into_iter()
        .collect();

        assert!(counts.balances());
        assert!(!counts.is_final());

        let settled: BatchReconciliation = vec![Disposition::Billable].into_iter().collect();
        assert!(settled.is_final());
    }

    /// The law cannot be broken through the public counting path, which is the
    /// point — an unbalanced value must have been assembled some other way.
    #[test]
    fn counting_through_observe_always_balances() {
        let mut counts = BatchReconciliation::EMPTY;
        for reason in SuppressReason::ALL {
            counts.observe(&Disposition::Suppressed(*reason));
        }
        for reason in HoldReason::ALL {
            counts.observe(&Disposition::Held(*reason));
        }
        counts.observe(&Disposition::Billable);
        counts.observe(&Disposition::Duplicate(key()));
        counts.observe(&Disposition::Rejected(RejectReason::Malformed));
        assert!(counts.balances());
        assert_eq!(
            counts.ingested(),
            (SuppressReason::ALL.len() + HoldReason::ALL.len() + 3) as u64
        );
    }

    #[test]
    fn a_deserialised_imbalance_is_readable_and_reported() {
        let broken = BatchReconciliation {
            carried_in: 0,
            ingested: 10,
            billable: 3,
            suppressed: 0,
            duplicate: 0,
            rejected: 0,
            held: 0,
        };
        assert!(!broken.balances());
        assert!(!broken.is_final());
        assert!(broken.to_string().contains("UNBALANCED"), "{broken}");
    }

    /// The boundary the ledger exists to guard: a held record re-presented as
    /// newly ingested is counted twice across two batches that each balance.
    #[test]
    fn a_carried_record_is_not_counted_as_a_new_arrival() {
        let mut first = BatchReconciliation::EMPTY;
        first.observe(&Disposition::Billable);
        first.observe(&Disposition::Held(HoldReason::AwaitingPartial));
        assert_eq!(first.held(), 1);

        // Correct: the re-presented record is carried, not ingested.
        let mut correct = BatchReconciliation::EMPTY;
        correct.observe_carried(&Disposition::Billable);
        assert_eq!(correct.carried_in(), 1);
        assert_eq!(correct.ingested(), 0);
        assert!(correct.balances());

        let mut ledger = PeriodLedger::new();
        ledger.append(first).unwrap();
        ledger.append(correct).unwrap();

        assert!(ledger.is_closable());
        assert_eq!(
            ledger.totals().ingested(),
            2,
            "two records arrived, in the first batch"
        );
        assert_eq!(
            ledger.totals().carried_in(),
            1,
            "one of them crossed a boundary rather than arriving twice"
        );
        assert_eq!(ledger.totals().released(), 2);
    }

    /// The same record counted as a new arrival balances *within* its batch and
    /// still breaks the period.
    #[test]
    fn counting_a_carried_record_as_new_breaks_the_chain() {
        let mut first = BatchReconciliation::EMPTY;
        first.observe(&Disposition::Held(HoldReason::AwaitingPartial));

        let mut wrong = BatchReconciliation::EMPTY;
        wrong.observe(&Disposition::Billable);
        assert!(wrong.balances(), "the batch is internally consistent");

        let mut ledger = PeriodLedger::new();
        ledger.append(first).unwrap();
        assert_eq!(
            ledger.append(wrong),
            Err(ChainBreak::CarryMismatch {
                batch: 1,
                expected: 1,
                found: 0,
            })
        );
        assert!(!ledger.is_closable());
    }

    /// Dropping a held record is the mirror failure and is caught the same way.
    #[test]
    fn dropping_a_held_record_breaks_the_chain() {
        let mut first = BatchReconciliation::EMPTY;
        for _ in 0..3 {
            first.observe(&Disposition::Held(HoldReason::AwaitingCorrelation));
        }

        let mut second = BatchReconciliation::EMPTY;
        second.observe_carried(&Disposition::Billable);
        second.observe_carried(&Disposition::Billable);

        let mut ledger = PeriodLedger::new();
        ledger.append(first).unwrap();
        assert!(matches!(
            ledger.append(second),
            Err(ChainBreak::CarryMismatch { found: 2, .. })
        ));
    }

    /// A period stays open while anything is held, however clean the batches.
    #[test]
    fn a_period_with_outstanding_records_is_not_closable() {
        let mut batch = BatchReconciliation::EMPTY;
        batch.observe(&Disposition::Billable);
        batch.observe(&Disposition::Held(HoldReason::SequenceGap));

        let mut ledger = PeriodLedger::new();
        ledger.append(batch).unwrap();
        assert_eq!(ledger.outstanding(), 1);
        assert!(!ledger.is_closable());
        assert!(ledger.breaks().is_empty(), "outstanding is not a break");
        assert!(ledger.to_string().contains("OPEN"), "{ledger}");
    }

    /// A break in the middle is remembered even if the caller keeps going.
    #[test]
    fn a_break_survives_a_caller_that_continues() {
        let mut first = BatchReconciliation::EMPTY;
        first.observe(&Disposition::Held(HoldReason::AwaitingPartial));

        let mut wrong = BatchReconciliation::EMPTY;
        wrong.observe(&Disposition::Billable);

        let mut clean = BatchReconciliation::EMPTY;
        clean.observe(&Disposition::Billable);

        let mut ledger = PeriodLedger::new();
        ledger.append(first).unwrap();
        let _ = ledger.append(wrong);
        ledger.append(clean).unwrap();

        assert_eq!(ledger.breaks().len(), 1);
        assert!(!ledger.is_closable(), "a past break is never forgiven");
    }

    /// A period can exceed what a `u64` counts while every batch in it is
    /// internally consistent — that is an overflow, not a lost record.
    #[test]
    fn a_totals_overflow_is_not_reported_as_an_imbalance() {
        let mut huge = BatchReconciliation::EMPTY;
        for _ in 0..2 {
            huge.observe(&Disposition::Billable);
        }
        // Force the running totals past what they can hold.
        let mut period = PeriodLedger::new();
        period.totals = BatchReconciliation {
            carried_in: 0,
            ingested: u64::MAX,
            billable: u64::MAX,
            suppressed: 0,
            duplicate: 0,
            rejected: 0,
            held: 0,
        };

        assert_eq!(
            period.append(huge),
            Err(ChainBreak::TotalsOverflow { batch: 0 })
        );
        assert!(!period.is_closable());
        assert!(period.to_string().contains("break"), "{period}");
    }

    #[test]
    fn an_empty_period_is_closable_and_says_nothing_else() {
        let ledger = PeriodLedger::new();
        assert!(ledger.is_closable());
        assert_eq!(ledger.batches(), 0);
        assert_eq!(ledger.outstanding(), 0);
    }

    #[test]
    fn merging_recombines_parallel_work() {
        let left: BatchReconciliation = vec![Disposition::Billable].into_iter().collect();
        let right: BatchReconciliation = vec![
            Disposition::Billable,
            Disposition::Rejected(RejectReason::Malformed),
        ]
        .into_iter()
        .collect();

        let merged = left.checked_add(right).unwrap();
        assert_eq!(merged.ingested(), 3);
        assert_eq!(merged.released(), 2);
        assert!(merged.balances());
    }

    /// A wrapped count would make an unbalanced batch look balanced, so every
    /// addition is checked.
    #[test]
    fn merging_is_checked() {
        let huge = BatchReconciliation {
            carried_in: 0,
            ingested: u64::MAX,
            billable: u64::MAX,
            suppressed: 0,
            duplicate: 0,
            rejected: 0,
            held: 0,
        };
        let one: BatchReconciliation = vec![Disposition::Billable].into_iter().collect();
        assert_eq!(huge.checked_add(one), None);
    }

    /// The bridge from lateness to rejection: a late record is still billable,
    /// and only the two terminal verdicts map across.
    #[test]
    fn only_terminal_lateness_verdicts_become_rejections() {
        assert_eq!(RejectReason::from_lateness(Lateness::OnTime), None);
        assert_eq!(
            RejectReason::from_lateness(Lateness::Late(Duration::days(1))),
            None
        );
        assert_eq!(
            RejectReason::from_lateness(Lateness::BeyondDeadline(Duration::days(40))),
            Some(RejectReason::BeyondDeadline)
        );
        assert_eq!(
            RejectReason::from_lateness(Lateness::Future(Duration::hours(1))),
            Some(RejectReason::FutureTimestamp)
        );
    }

    #[test]
    fn dispositions_report_their_own_finality() {
        assert!(Disposition::Billable.is_released());
        assert!(!Disposition::Billable.may_return());
        assert!(!Disposition::Billable.is_finding());

        let held = Disposition::Held(HoldReason::SequenceGap);
        assert!(!held.is_released());
        assert!(held.may_return());
        assert!(held.is_finding());

        let rejected = Disposition::Rejected(RejectReason::Malformed);
        assert!(!rejected.may_return(), "a rejection is terminal");
        assert!(rejected.is_finding());
    }

    #[test]
    fn reason_codes_round_trip() {
        for reason in SuppressReason::ALL {
            assert_eq!(reason.as_str().parse::<SuppressReason>().unwrap(), *reason);
        }
        for reason in HoldReason::ALL {
            assert_eq!(reason.as_str().parse::<HoldReason>().unwrap(), *reason);
        }
        assert!("NOPE".parse::<SuppressReason>().is_err());
    }

    #[test]
    fn dispositions_render_legibly() {
        assert_eq!(
            Disposition::Suppressed(SuppressReason::ZeroRated).to_string(),
            "SUPPRESSED:ZERO_RATED"
        );
        assert_eq!(
            Disposition::Rejected(RejectReason::NotKeyable(NotKeyable::NoCorrelatorPresent))
                .to_string(),
            "REJECTED:NOT_KEYABLE:NO_CORRELATOR_PRESENT"
        );
        assert_eq!(
            Disposition::Held(HoldReason::SequenceGap).to_string(),
            "HELD:SEQUENCE_GAP"
        );
    }
}
