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
//! // § 10 TDDDG: six calendar months from invoice dispatch.
//! let policy = RetentionPolicy::billing_tdddg10(datetime!(2026-08-05 00:00 UTC)).unwrap();
//! assert_eq!(policy.deadline(), Some(datetime!(2027-02-05 00:00 UTC)));
//!
//! // A call from 1 July is kept six months past the August invoice, not past
//! // the call.
//! let event = datetime!(2026-07-01 12:00 UTC);
//! assert!(policy.deadline().unwrap() > event + Duration::days(183));
//! assert!(!policy.is_expired(datetime!(2027-01-01 00:00 UTC)));
//! assert!(policy.is_expired(datetime!(2027-03-01 00:00 UTC)));
//!
//! // Longer than § 10 allows is not a § 10 policy.
//! let dispatched = datetime!(2026-08-31 00:00 UTC);
//! let basis = RetentionBasis::BillingTdddg10 { invoice_dispatched_at: dispatched };
//! assert!(RetentionPolicy::new(basis, Duration::days(181)).is_some()); // 28 Feb
//! assert!(RetentionPolicy::new(basis, Duration::days(182)).is_none()); // 1 Mar
//! ```
//!
//! # A hold has no deadline, and that is not the same as no policy
//!
//! A statutory period has a length. A dispute does not — it is over when it is
//! resolved, and no arithmetic produces that instant. Giving a hold a nominal
//! duration anyway means deletion becomes permitted the moment that nominal
//! period runs out, while the dispute is still open, which is the one retention
//! error that cannot be undone.
//!
//! So [`RetentionPolicy::until_released`] builds a policy with no deadline, and
//! [`RetentionSchedule::deletion`] has **three** answers rather than an
//! `Option`: nothing requires this to be kept, keep it until this instant, or
//! keep it until somebody says stop. The first and the third are opposites, and
//! an `Option<OffsetDateTime>` spells them the same way.
//!
//! ```rust
//! use mediation::{Deletion, RetentionBasis, RetentionPolicy, RetentionSchedule};
//! use time::macros::datetime;
//!
//! let dispute = RetentionPolicy::until_released(RetentionBasis::DisputeHold {
//!     raised_at: datetime!(2026-08-10 00:00 UTC),
//! })
//! .unwrap();
//!
//! let schedule: RetentionSchedule = vec![dispute].into_iter().collect();
//! assert!(schedule.deletion().is_blocked());
//! assert!(!schedule.is_deletable(datetime!(2099-01-01 00:00 UTC)));
//!
//! // Nothing at all is the opposite answer, not the same one.
//! assert_eq!(RetentionSchedule::new().deletion(), Deletion::Immediate);
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

/// A basis and how long it runs — or that it runs until somebody says stop.
///
/// A statutory period has a length. A **hold** does not: a dispute is over when
/// it is resolved, and no date can be computed for that. Modelling the second
/// as the first is how evidence gets deleted while a customer is still
/// contesting the bill it supports, so the two are different constructors and
/// [`deadline`](Self::deadline) has no answer for a hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPolicy {
    basis: RetentionBasis,
    /// `None` for an open hold: kept until released, with no computable date.
    duration: Option<Duration>,
}

impl RetentionPolicy {
    /// § 10 Abs. 2 TDDDG at its longest: "bis zu sechs Monate nach Versendung
    /// der Rechnung".
    ///
    /// Six **calendar** months: the period ends on the same day of the sixth
    /// month after dispatch, or on that month's last day if it lacks that day
    /// (BGB § 188(2), (3)), at the dispatch's time of day. Where the dispatch
    /// offset and UTC disagree on that date, the earlier instant applies, so
    /// the period never outlasts either reading.
    ///
    /// `None` for a sub-second dispatch instant or one whose period leaves the
    /// supported calendar.
    ///
    /// ```rust
    /// use mediation::RetentionPolicy;
    /// use time::macros::datetime;
    ///
    /// let policy = RetentionPolicy::billing_tdddg10(datetime!(2026-08-31 09:00 UTC)).unwrap();
    /// assert_eq!(policy.deadline(), Some(datetime!(2027-02-28 09:00 UTC)));
    /// ```
    #[must_use]
    pub fn billing_tdddg10(invoice_dispatched_at: OffsetDateTime) -> Option<Self> {
        let deadline = tdddg10_limit(invoice_dispatched_at)?;
        Self::new(
            RetentionBasis::BillingTdddg10 {
                invoice_dispatched_at,
            },
            deadline - invoice_dispatched_at,
        )
    }

    /// A policy from a basis and a duration.
    ///
    /// `None` when any of these holds (findings R5–R7):
    ///
    /// - the duration is not positive: a zero period is not a retention policy
    ///   but an instruction to delete on arrival, which a caller should say
    ///   outright rather than encode as a degenerate window;
    /// - the duration or the anchor is not whole seconds, or the anchor lies
    ///   outside the UTC years 1–9998, which the stored form cannot carry;
    /// - the deadline leaves the supported calendar;
    /// - the basis is § 10 TDDDG and the period ends after
    ///   [`billing_tdddg10`](Self::billing_tdddg10)'s;
    /// - the basis is a dispute hold: a hold ends when the dispute is resolved,
    ///   not on a date, so it is built with
    ///   [`until_released`](Self::until_released).
    #[must_use]
    pub fn new(basis: RetentionBasis, duration: Duration) -> Option<Self> {
        if !duration.is_positive() || duration.subsec_nanoseconds() != 0 || basis.is_a_hold() {
            return None;
        }
        let anchor = whole_second_anchor(basis)?;
        let deadline = anchor.checked_add(duration)?;
        if let RetentionBasis::BillingTdddg10 { .. } = basis
            && deadline > tdddg10_limit(anchor)?
        {
            return None;
        }
        Some(Self {
            basis,
            duration: Some(duration),
        })
    }

    /// A hold that runs until it is released, with no computable deadline.
    ///
    /// `None` for a basis that is not a hold — see
    /// [`RetentionBasis::is_a_hold`] — and for an anchor `new` refuses. A
    /// statutory period without an end is not a retention policy but an
    /// intention to keep personal data forever, and making it expressible
    /// would let one slip in beside three lawful ones.
    ///
    /// A schedule carrying one of these is never deletable. Releasing the hold
    /// is done by rebuilding the schedule without it, because a hold that could
    /// be lifted in place would leave no evidence that it had ever applied.
    ///
    /// ```rust
    /// use mediation::{RetentionBasis, RetentionPolicy, RetentionSchedule};
    /// use time::{Duration, macros::datetime};
    ///
    /// let billing = RetentionPolicy::billing_tdddg10(datetime!(2026-08-05 0:00 UTC)).unwrap();
    /// let dispute = RetentionPolicy::until_released(
    ///     RetentionBasis::DisputeHold { raised_at: datetime!(2026-08-10 0:00 UTC) },
    /// ).unwrap();
    ///
    /// // The billing period runs out; the dispute does not.
    /// let long_after = datetime!(2030-01-01 0:00 UTC);
    /// assert!(billing.is_expired(long_after));
    /// assert!(!dispute.is_expired(long_after));
    ///
    /// let schedule: RetentionSchedule = vec![billing, dispute].into_iter().collect();
    /// assert!(!schedule.is_deletable(long_after), "a hold has no expiry");
    /// ```
    #[must_use]
    pub fn until_released(basis: RetentionBasis) -> Option<Self> {
        if basis.is_a_hold() && whole_second_anchor(basis).is_some() {
            Some(Self {
                basis,
                duration: None,
            })
        } else {
            None
        }
    }

    /// The legal ground and its anchor.
    #[must_use]
    pub const fn basis(self) -> RetentionBasis {
        self.basis
    }

    /// How long the period runs, or `None` for a hold that runs until released.
    #[must_use]
    pub const fn duration(self) -> Option<Duration> {
        self.duration
    }

    /// `true` when this policy has no computable end.
    #[must_use]
    pub const fn is_open(self) -> bool {
        self.duration.is_none()
    }

    /// The instant after which the record must be deleted.
    ///
    /// `None` for an open hold. That is not "no deadline, so delete freely" —
    /// it is the opposite, and [`is_expired`](Self::is_expired) says so.
    /// Checked: construction refuses a deadline outside the calendar, so this
    /// never panics (finding R5).
    #[must_use]
    pub fn deadline(self) -> Option<OffsetDateTime> {
        self.basis.anchor().checked_add(self.duration?)
    }

    /// `true` when the record must already have been deleted at `as_of`.
    ///
    /// Always `false` for an open hold: a dispute is over when it is resolved,
    /// and no instant establishes that.
    ///
    /// `as_of` is supplied, never sampled — replaying an archive must reproduce
    /// the verdicts it originally received, not today's.
    #[must_use]
    pub fn is_expired(self, as_of: OffsetDateTime) -> bool {
        self.deadline().is_some_and(|deadline| as_of > deadline)
    }

    /// How long remains before deletion, or how far past it is.
    ///
    /// Negative once the deadline has passed, so a report can say *how* overdue
    /// a deletion is rather than merely that it is. `None` for an open hold.
    #[must_use]
    pub fn remaining(self, as_of: OffsetDateTime) -> Option<Duration> {
        Some(self.deadline()? - as_of)
    }
}

/// The basis's anchor, or `None` when it carries a fraction of a second or
/// lies outside the UTC years 1–9998 a stored timestamp carries.
fn whole_second_anchor(basis: RetentionBasis) -> Option<OffsetDateTime> {
    let anchor = basis.anchor();
    let storable =
        crate::cdrtime::CdrTimestamp::from_utc(anchor, crate::cdrtime::QuarterHourOffset::UTC)
            .is_some();
    (anchor.nanosecond() == 0 && storable).then_some(anchor)
}

/// The last instant § 10 Abs. 2 TDDDG permits for an invoice dispatched at
/// `dispatched`: six calendar months on, read in the dispatch offset and in
/// UTC, whichever is earlier.
fn tdddg10_limit(dispatched: OffsetDateTime) -> Option<OffsetDateTime> {
    let six_months = |at: OffsetDateTime| {
        crate::fairuse::add_calendar_months(at.date(), 6).map(|date| at.replace_date(date))
    };
    let local = six_months(dispatched)?;
    let utc = six_months(dispatched.checked_to_offset(time::UtcOffset::UTC)?)?;
    Some(local.min(utc))
}

impl fmt::Display for RetentionPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.duration {
            Some(duration) => write!(f, "{} for {duration}", self.basis),
            None => write!(f, "{} until released", self.basis),
        }
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

    /// What the schedule permits, and when.
    ///
    /// The one call a deletion path should make. Three answers, because there
    /// are three situations and collapsing any two of them deletes something it
    /// should not — see [`Deletion`].
    #[must_use]
    pub fn deletion(&self) -> Deletion {
        // A hold outranks every dated policy, however far off that date is:
        // "keep until somebody says stop" has no instant to compare against.
        if let Some(open) = self.0.iter().find(|p| p.is_open()) {
            return Deletion::Blocked(open.basis());
        }
        match self.0.iter().filter_map(|p| p.deadline()).max() {
            Some(deadline) => Deletion::After(deadline),
            None => Deletion::Immediate,
        }
    }

    /// The latest deadline any dated policy imposes.
    ///
    /// `None` when no policy applies **and** when a hold blocks deletion, which
    /// are opposite situations — ask [`deletion`](Self::deletion) instead
    /// wherever the difference matters.
    #[must_use]
    pub fn governing_deadline(&self) -> Option<OffsetDateTime> {
        self.0.iter().filter_map(|p| p.deadline()).max()
    }

    /// The policy that governs: an open hold if there is one, otherwise the
    /// latest deadline.
    #[must_use]
    pub fn governing_policy(&self) -> Option<RetentionPolicy> {
        self.0
            .iter()
            .find(|p| p.is_open())
            .or_else(|| {
                self.0
                    .iter()
                    .filter(|p| !p.is_open())
                    .max_by_key(|p| p.deadline())
            })
            .copied()
    }

    /// `true` when every policy has expired and the record must be deleted.
    ///
    /// An empty schedule is deletable immediately: no basis to keep the data is
    /// not the same as no deadline. An open hold is **never** deletable, at any
    /// instant — deleting evidence while a customer is contesting the bill it
    /// supports is the one retention error that cannot be undone.
    #[must_use]
    pub fn is_deletable(&self, as_of: OffsetDateTime) -> bool {
        match self.deletion() {
            Deletion::Immediate => true,
            Deletion::After(deadline) => as_of > deadline,
            Deletion::Blocked(_) => false,
        }
    }

    /// `true` when any policy suspends deletion rather than scheduling it.
    #[must_use]
    pub fn is_held(&self) -> bool {
        self.0.iter().any(|p| p.basis().is_a_hold())
    }
}

/// What a [`RetentionSchedule`] permits.
///
/// Three answers rather than an `Option<OffsetDateTime>`, because "there is no
/// deadline" is ambiguous in exactly the direction that costs somebody their
/// data: it can mean *nothing requires this to be kept*, or *this must be kept
/// and no date can be put on it*. Those are opposites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deletion {
    /// No basis requires the record to be kept.
    ///
    /// Under GDPR Art. 5(1)(e) that is an instruction to delete, not a licence
    /// to keep indefinitely.
    Immediate,
    /// The record must be kept until this instant, and deleted after it.
    After(OffsetDateTime),
    /// A hold suspends deletion, and no date can be computed for its end.
    ///
    /// Carries the basis so a report can say which hold, raised when.
    Blocked(RetentionBasis),
}

impl Deletion {
    /// `true` when a hold suspends deletion.
    #[must_use]
    pub const fn is_blocked(self) -> bool {
        matches!(self, Self::Blocked(_))
    }

    /// The deadline, where there is one.
    #[must_use]
    pub const fn deadline(self) -> Option<OffsetDateTime> {
        match self {
            Self::After(at) => Some(at),
            _ => None,
        }
    }
}

impl fmt::Display for Deletion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Immediate => f.write_str("delete now: no basis to keep it"),
            Self::After(at) => write!(f, "delete after {at}"),
            Self::Blocked(basis) => write!(f, "held by {basis}"),
        }
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

    fn to_stamp<E: serde::ser::Error>(
        at: OffsetDateTime,
    ) -> Result<crate::cdrtime::CdrTimestamp, E> {
        crate::cdrtime::CdrTimestamp::from_utc(at, crate::cdrtime::QuarterHourOffset::UTC)
            .ok_or_else(|| E::custom("retention anchor outside UTC years 1-9998"))
    }

    impl Serialize for RetentionBasis {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let repr = match self {
                Self::BillingTdddg10 {
                    invoice_dispatched_at,
                } => BasisRepr::BillingTdddg10 {
                    invoice_dispatched_at: to_stamp(*invoice_dispatched_at)?,
                },
                Self::LawfulAccess { recorded_at } => BasisRepr::LawfulAccess {
                    recorded_at: to_stamp(*recorded_at)?,
                },
                Self::DisputeHold { raised_at } => BasisRepr::DisputeHold {
                    raised_at: to_stamp(*raised_at)?,
                },
                Self::OperatorPolicy { from } => BasisRepr::OperatorPolicy {
                    from: to_stamp(*from)?,
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
        /// Absent for a hold that runs until released, which has no length.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_s: Option<i64>,
    }

    impl Serialize for RetentionPolicy {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            PolicyRepr {
                basis: self.basis(),
                duration_s: self.duration().map(Duration::whole_seconds),
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for RetentionPolicy {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = PolicyRepr::deserialize(deserializer)?;
            match r.duration_s {
                Some(seconds) => RetentionPolicy::new(r.basis, Duration::seconds(seconds))
                    .ok_or_else(|| {
                        serde::de::Error::custom(
                            "a retention period must be positive whole seconds, inside the \
                             calendar, within § 10 TDDDG for billing, and absent for a hold",
                        )
                    }),
                None => RetentionPolicy::until_released(r.basis).ok_or_else(|| {
                    serde::de::Error::custom("only a hold may run without a duration")
                }),
            }
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
        assert_eq!(policy.deadline(), Some(INVOICED + Duration::days(183)));

        // The record is kept well past six months from the call itself.
        assert!(policy.deadline().unwrap() > EVENT + Duration::days(183));
        assert!(!policy.is_expired(datetime!(2027-01-01 00:00 UTC)));
        assert!(policy.is_expired(datetime!(2027-03-01 00:00 UTC)));
    }

    /// `as_of` is a parameter, so replaying an archive reproduces the verdicts
    /// it originally received.
    #[test]
    fn expiry_is_judged_against_a_supplied_instant() {
        let policy = billing();
        let just_before = policy.deadline().unwrap();
        let just_after = policy.deadline().unwrap() + Duration::seconds(1);

        assert!(!policy.is_expired(just_before), "the deadline is inclusive");
        assert!(policy.is_expired(just_after));
        assert_eq!(policy.remaining(just_before), Some(Duration::ZERO));
        assert!(policy.remaining(just_after).unwrap().is_negative());
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
            RetentionBasis::LawfulAccess {
                recorded_at: datetime!(2026-09-01 00:00 UTC),
            },
            Duration::days(365),
        )
        .unwrap();

        let schedule: RetentionSchedule = vec![short, billing(), dispute].into_iter().collect();

        assert_eq!(schedule.len(), 3);
        assert_eq!(schedule.governing_policy(), Some(dispute));
        assert_eq!(schedule.governing_deadline(), dispute.deadline());
        assert!(!schedule.is_held());

        // The short policy expiring does not make the record deletable.
        let after_short = EVENT + Duration::days(60);
        assert!(short.is_expired(after_short));
        assert!(!schedule.is_deletable(after_short));

        assert!(schedule.is_deletable(dispute.deadline().unwrap() + Duration::seconds(1)));
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
