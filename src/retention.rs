//! How long a record may be kept, and what it is anchored to.
//!
//! A mediation layer holds the most sensitive dataset a telco operates: who
//! contacted whom, when, from where. This module does the arithmetic and cites
//! the basis. It ships **no default duration**, and that is the whole design.
//!
//! # Why there is no default
//!
//! The German retention rule has changed three times in three years:
//!
//! | When | What |
//! |---|---|
//! | TKG 2021 | §§ 175/176 required ten weeks of traffic data, four of location data |
//! | 14 August 2023 | the Bundesverwaltungsgericht held it incompatible with Art. 15(1) of Directive 2002/58/EC, and therefore inapplicable |
//! | April 2026 | after a further CJEU ruling, a new draft § 176 TKG requires access providers to store **IP addresses and timestamps for three months**, explicitly excluding content, destinations, location and browsing behaviour |
//!
//! A library that had hard-coded any one of those would have been wrong for
//! most of the period, and wrong in the direction of keeping personal data
//! longer than the law allowed. So [`RetentionPolicy`] is caller-supplied, and
//! what this crate contributes is the arithmetic, the anchor, and the citation.
//!
//! # The anchor is not the event date
//!
//! § 10 TDDDG permits billing traffic data to be kept **up to six months after
//! the invoice is dispatched** — not six months after the call. A record from
//! the start of a billing period is invoiced weeks later, so anchoring on the
//! event date deletes it early and destroys the evidence for a dispute that is
//! still open.
//!
//! [`RetentionBasis`] therefore carries its own anchor, and the intuitive wrong
//! one is not expressible for a billing policy.
//!
//! ```rust
//! use mediation::{RetentionBasis, RetentionPolicy};
//! use time::{Duration, macros::datetime};
//!
//! // § 10 TDDDG: six months from invoice dispatch.
//! let policy = RetentionPolicy::new(
//!     RetentionBasis::BillingTdddg10 {
//!         invoice_dispatched_at: datetime!(2026-08-05 00:00 UTC),
//!     },
//!     Duration::days(183),
//! )
//! .unwrap();
//!
//! // A call from 1 July is kept six months past the August invoice, not past
//! // the call.
//! let event = datetime!(2026-07-01 12:00 UTC);
//! assert!(policy.deadline() > event + Duration::days(183));
//! assert!(!policy.is_expired(datetime!(2027-01-01 00:00 UTC)));
//! assert!(policy.is_expired(datetime!(2027-03-01 00:00 UTC)));
//! ```

use std::fmt;

use time::{Duration, OffsetDateTime};

// ── RetentionBasis ───────────────────────────────────────────────────────────

/// The legal ground a retention period rests on, and the instant it runs from.
///
/// Each variant carries its own anchor, because the anchors genuinely differ
/// and using one basis's anchor with another's duration is how personal data is
/// kept past what any of them allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionBasis {
    /// § 10 TDDDG — Entgeltermittlung und Entgeltabrechnung.
    ///
    /// Billing traffic data may be kept up to six months **after the invoice is
    /// dispatched**. Data not needed for billing must be deleted without delay,
    /// which this crate cannot decide for a caller but which the shortness of
    /// this window is meant to keep in view.
    BillingTdddg10 {
        /// When the invoice covering the record was dispatched.
        invoice_dispatched_at: OffsetDateTime,
    },
    /// A statutory access obligation, running from when the record was created.
    ///
    /// Named without a section number deliberately: the German obligation has
    /// been rewritten three times and its scope now covers IP addresses rather
    /// than traffic data generally. Cite the version you are subject to in your
    /// own code.
    LawfulAccess {
        /// When the record was created.
        recorded_at: OffsetDateTime,
    },
    /// A dispute that suspends deletion until it is resolved.
    ///
    /// Runs from the day the dispute was raised. Deleting evidence while a
    /// customer is contesting the bill it supports is the one retention error
    /// that cannot be undone.
    DisputeHold {
        /// When the dispute was raised.
        raised_at: OffsetDateTime,
    },
    /// An operator's own policy, running from a caller-chosen instant.
    OperatorPolicy {
        /// When the period begins.
        from: OffsetDateTime,
    },
}

impl RetentionBasis {
    /// The instant the retention period runs from.
    #[must_use]
    pub const fn anchor(self) -> OffsetDateTime {
        match self {
            Self::BillingTdddg10 {
                invoice_dispatched_at,
            } => invoice_dispatched_at,
            Self::LawfulAccess { recorded_at } => recorded_at,
            Self::DisputeHold { raised_at } => raised_at,
            Self::OperatorPolicy { from } => from,
        }
    }

    /// A short code naming the basis, for a deletion report.
    #[must_use]
    pub const fn kind(self) -> &'static str {
        match self {
            Self::BillingTdddg10 { .. } => "BILLING_TDDDG_10",
            Self::LawfulAccess { .. } => "LAWFUL_ACCESS",
            Self::DisputeHold { .. } => "DISPUTE_HOLD",
            Self::OperatorPolicy { .. } => "OPERATOR_POLICY",
        }
    }

    /// `true` when the basis suspends deletion rather than scheduling it.
    ///
    /// A dispute hold outlives its nominal period if the dispute is still open,
    /// which a caller must decide — the crate only distinguishes the kind.
    #[must_use]
    pub const fn is_a_hold(self) -> bool {
        matches!(self, Self::DisputeHold { .. })
    }
}

impl fmt::Display for RetentionBasis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} from {}", self.kind(), self.anchor())
    }
}

// ── RetentionPolicy ──────────────────────────────────────────────────────────

/// A basis and how long it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPolicy {
    basis: RetentionBasis,
    duration: Duration,
}

impl RetentionPolicy {
    /// A policy from a basis and a duration.
    ///
    /// `None` for a non-positive duration: a zero period is not a retention
    /// policy but an instruction to delete on arrival, which a caller should
    /// say outright rather than encode as a degenerate window.
    #[must_use]
    pub const fn new(basis: RetentionBasis, duration: Duration) -> Option<Self> {
        if duration.is_positive() {
            Some(Self { basis, duration })
        } else {
            None
        }
    }

    /// The legal ground and its anchor.
    #[must_use]
    pub const fn basis(self) -> RetentionBasis {
        self.basis
    }

    /// How long the period runs.
    #[must_use]
    pub const fn duration(self) -> Duration {
        self.duration
    }

    /// The instant after which the record must be deleted.
    #[must_use]
    pub fn deadline(self) -> OffsetDateTime {
        self.basis.anchor() + self.duration
    }

    /// `true` when the record must already have been deleted at `as_of`.
    ///
    /// `as_of` is supplied, never sampled — replaying an archive must reproduce
    /// the verdicts it originally received, not today's.
    #[must_use]
    pub fn is_expired(self, as_of: OffsetDateTime) -> bool {
        as_of > self.deadline()
    }

    /// How long remains before deletion, or how far past it is.
    ///
    /// Negative once the deadline has passed, so a report can say *how* overdue
    /// a deletion is rather than merely that it is.
    #[must_use]
    pub fn remaining(self, as_of: OffsetDateTime) -> Duration {
        self.deadline() - as_of
    }
}

impl fmt::Display for RetentionPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} for {}", self.basis, self.duration)
    }
}

// ── Schedule ─────────────────────────────────────────────────────────────────

/// Every policy that applies to one record.
///
/// Records commonly sit under more than one: a billing period and an open
/// dispute, or a statutory obligation and an operator policy. The **longest**
/// governs, because deleting while any basis still requires the data is the
/// error that cannot be undone — and the shortest is what a
/// minimisation review should be asked about.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetentionSchedule(Vec<RetentionPolicy>);

impl RetentionSchedule {
    /// No policies at all.
    #[must_use]
    pub const fn new() -> Self {
        Self(Vec::new())
    }

    /// Add a policy.
    pub fn push(&mut self, policy: RetentionPolicy) {
        self.0.push(policy);
    }

    /// Every policy, in the order supplied.
    #[must_use]
    pub fn policies(&self) -> &[RetentionPolicy] {
        &self.0
    }

    /// `true` when no policy applies.
    ///
    /// A record under no policy has **no basis to be kept**, which under
    /// GDPR Art. 5(1)(e) is not a licence to keep it indefinitely but the
    /// opposite. [`is_deletable`](Self::is_deletable) treats it accordingly.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// How many policies apply.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// The latest deadline any policy imposes.
    ///
    /// `None` when no policy applies.
    #[must_use]
    pub fn governing_deadline(&self) -> Option<OffsetDateTime> {
        self.0.iter().map(|p| p.deadline()).max()
    }

    /// The policy whose deadline governs.
    #[must_use]
    pub fn governing_policy(&self) -> Option<RetentionPolicy> {
        self.0.iter().copied().max_by_key(|p| p.deadline())
    }

    /// `true` when every policy has expired and the record must be deleted.
    ///
    /// An empty schedule is deletable immediately: no basis to keep the data is
    /// not the same as no deadline.
    #[must_use]
    pub fn is_deletable(&self, as_of: OffsetDateTime) -> bool {
        match self.governing_deadline() {
            None => true,
            Some(deadline) => as_of > deadline,
        }
    }

    /// `true` when any policy suspends deletion rather than scheduling it.
    #[must_use]
    pub fn is_held(&self) -> bool {
        self.0.iter().any(|p| p.basis().is_a_hold())
    }
}

impl FromIterator<RetentionPolicy> for RetentionSchedule {
    fn from_iter<I: IntoIterator<Item = RetentionPolicy>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl fmt::Display for RetentionSchedule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.governing_policy() {
            None => f.write_str("no retention basis"),
            Some(policy) => write!(f, "{} policy(ies), governed by {}", self.0.len(), policy),
        }
    }
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::{RetentionBasis, RetentionPolicy};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use time::{Duration, OffsetDateTime};

    /// An internally tagged representation, as elsewhere in the crate. The
    /// anchor is named per variant so a stored policy says what it runs from.
    #[derive(Serialize, Deserialize)]
    #[serde(tag = "kind", rename = "RetentionBasis")]
    enum BasisRepr {
        #[serde(rename = "BILLING_TDDDG_10")]
        BillingTdddg10 {
            invoice_dispatched_at: crate::cdrtime::CdrTimestamp,
        },
        #[serde(rename = "LAWFUL_ACCESS")]
        LawfulAccess {
            recorded_at: crate::cdrtime::CdrTimestamp,
        },
        #[serde(rename = "DISPUTE_HOLD")]
        DisputeHold {
            raised_at: crate::cdrtime::CdrTimestamp,
        },
        #[serde(rename = "OPERATOR_POLICY")]
        OperatorPolicy { from: crate::cdrtime::CdrTimestamp },
    }

    fn to_stamp(at: OffsetDateTime) -> crate::cdrtime::CdrTimestamp {
        crate::cdrtime::CdrTimestamp::from_utc(at, crate::cdrtime::QuarterHourOffset::UTC)
    }

    impl Serialize for RetentionBasis {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let repr = match self {
                Self::BillingTdddg10 {
                    invoice_dispatched_at,
                } => BasisRepr::BillingTdddg10 {
                    invoice_dispatched_at: to_stamp(*invoice_dispatched_at),
                },
                Self::LawfulAccess { recorded_at } => BasisRepr::LawfulAccess {
                    recorded_at: to_stamp(*recorded_at),
                },
                Self::DisputeHold { raised_at } => BasisRepr::DisputeHold {
                    raised_at: to_stamp(*raised_at),
                },
                Self::OperatorPolicy { from } => BasisRepr::OperatorPolicy {
                    from: to_stamp(*from),
                },
            };
            repr.serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for RetentionBasis {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            Ok(match BasisRepr::deserialize(deserializer)? {
                BasisRepr::BillingTdddg10 {
                    invoice_dispatched_at,
                } => Self::BillingTdddg10 {
                    invoice_dispatched_at: invoice_dispatched_at.to_utc(),
                },
                BasisRepr::LawfulAccess { recorded_at } => Self::LawfulAccess {
                    recorded_at: recorded_at.to_utc(),
                },
                BasisRepr::DisputeHold { raised_at } => Self::DisputeHold {
                    raised_at: raised_at.to_utc(),
                },
                BasisRepr::OperatorPolicy { from } => Self::OperatorPolicy {
                    from: from.to_utc(),
                },
            })
        }
    }

    /// The duration is whole seconds: a retention period is a commitment read
    /// by humans as well as machines.
    #[derive(Serialize, Deserialize)]
    #[serde(rename = "RetentionPolicy")]
    struct PolicyRepr {
        basis: RetentionBasis,
        duration_s: i64,
    }

    impl Serialize for RetentionPolicy {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            PolicyRepr {
                basis: self.basis(),
                duration_s: self.duration().whole_seconds(),
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for RetentionPolicy {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = PolicyRepr::deserialize(deserializer)?;
            RetentionPolicy::new(r.basis, Duration::seconds(r.duration_s))
                .ok_or_else(|| serde::de::Error::custom("a retention period must be positive"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    const INVOICED: OffsetDateTime = datetime!(2026-08-05 00:00 UTC);
    const EVENT: OffsetDateTime = datetime!(2026-07-01 12:00 UTC);

    fn billing() -> RetentionPolicy {
        RetentionPolicy::new(
            RetentionBasis::BillingTdddg10 {
                invoice_dispatched_at: INVOICED,
            },
            Duration::days(183),
        )
        .unwrap()
    }

    /// The error this API shape prevents: § 10 TDDDG runs from the invoice, not
    /// from the call, and anchoring on the event deletes evidence early.
    #[test]
    fn the_billing_period_runs_from_the_invoice_not_the_event() {
        let policy = billing();
        assert_eq!(policy.basis().anchor(), INVOICED);
        assert_eq!(policy.deadline(), INVOICED + Duration::days(183));

        // The record is kept well past six months from the call itself.
        assert!(policy.deadline() > EVENT + Duration::days(183));
        assert!(!policy.is_expired(datetime!(2027-01-01 00:00 UTC)));
        assert!(policy.is_expired(datetime!(2027-03-01 00:00 UTC)));
    }

    /// `as_of` is a parameter, so replaying an archive reproduces the verdicts
    /// it originally received.
    #[test]
    fn expiry_is_judged_against_a_supplied_instant() {
        let policy = billing();
        let just_before = policy.deadline();
        let just_after = policy.deadline() + Duration::seconds(1);

        assert!(!policy.is_expired(just_before), "the deadline is inclusive");
        assert!(policy.is_expired(just_after));
        assert_eq!(policy.remaining(just_before), Duration::ZERO);
        assert!(policy.remaining(just_after).is_negative());
    }

    /// A zero period is an instruction to delete on arrival, which a caller
    /// should say outright.
    #[test]
    fn a_non_positive_period_is_not_a_policy() {
        let basis = RetentionBasis::OperatorPolicy { from: EVENT };
        assert_eq!(RetentionPolicy::new(basis, Duration::ZERO), None);
        assert_eq!(RetentionPolicy::new(basis, Duration::days(-1)), None);
        assert!(RetentionPolicy::new(basis, Duration::seconds(1)).is_some());
    }

    /// Records commonly sit under several policies, and the longest governs:
    /// deleting while any basis still requires the data cannot be undone.
    #[test]
    fn the_longest_policy_governs_a_schedule() {
        let short = RetentionPolicy::new(
            RetentionBasis::OperatorPolicy { from: EVENT },
            Duration::days(30),
        )
        .unwrap();
        let dispute = RetentionPolicy::new(
            RetentionBasis::DisputeHold {
                raised_at: datetime!(2026-09-01 00:00 UTC),
            },
            Duration::days(365),
        )
        .unwrap();

        let schedule: RetentionSchedule = vec![short, billing(), dispute].into_iter().collect();

        assert_eq!(schedule.len(), 3);
        assert_eq!(schedule.governing_policy(), Some(dispute));
        assert_eq!(schedule.governing_deadline(), Some(dispute.deadline()));
        assert!(schedule.is_held());

        // The short policy expiring does not make the record deletable.
        let after_short = EVENT + Duration::days(60);
        assert!(short.is_expired(after_short));
        assert!(!schedule.is_deletable(after_short));

        assert!(schedule.is_deletable(dispute.deadline() + Duration::seconds(1)));
    }

    /// No basis to keep the data is not a licence to keep it indefinitely.
    #[test]
    fn a_record_under_no_policy_is_deletable_immediately() {
        let schedule = RetentionSchedule::new();
        assert!(schedule.is_empty());
        assert_eq!(schedule.governing_deadline(), None);
        assert!(schedule.is_deletable(EVENT));
        assert!(!schedule.is_held());
        assert_eq!(schedule.to_string(), "no retention basis");
    }

    #[test]
    fn each_basis_carries_its_own_anchor() {
        let cases = [
            (
                RetentionBasis::BillingTdddg10 {
                    invoice_dispatched_at: INVOICED,
                },
                INVOICED,
                "BILLING_TDDDG_10",
            ),
            (
                RetentionBasis::LawfulAccess { recorded_at: EVENT },
                EVENT,
                "LAWFUL_ACCESS",
            ),
            (
                RetentionBasis::DisputeHold { raised_at: EVENT },
                EVENT,
                "DISPUTE_HOLD",
            ),
            (
                RetentionBasis::OperatorPolicy { from: EVENT },
                EVENT,
                "OPERATOR_POLICY",
            ),
        ];
        for (basis, anchor, kind) in cases {
            assert_eq!(basis.anchor(), anchor);
            assert_eq!(basis.kind(), kind);
        }

        assert!(RetentionBasis::DisputeHold { raised_at: EVENT }.is_a_hold());
        assert!(!RetentionBasis::LawfulAccess { recorded_at: EVENT }.is_a_hold());
    }
}
