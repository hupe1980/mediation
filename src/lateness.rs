//! How late a record is, and what that changes.
//!
//! Almost every window in a mediation layer is the same window wearing a
//! different hat: *an event happened at T, it is being considered at `as_of`,
//! and the elapsed time changes what may be done with it.*
//!
//! | Window | Question | Typical bound |
//! |---|---|---|
//! | TAP inclusion | after how long is a roaming event unbillable? | **30 days** (GSMA TD.57) |
//! | NRTRDE | after how long is a fraud extract useless? | **4 hours** (GSMA TD.35) |
//! | Deduplication | how long must a key be remembered? | the partner's resend bound |
//! | Correlation | how long to wait for the other leg? | minutes to hours |
//!
//! # Late and unbillable are different questions
//!
//! [`Lateness::Late`] means the record is still billable but the structures
//! holding its siblings may have moved on — an operational finding.
//! [`Lateness::BeyondDeadline`] means it can never become billable and
//! reconciles as rejected, terminally. Collapsing the two either bills
//! unbillable usage or discards billable usage, and both are found only at
//! audit.
//!
//! A policy may have a lateness threshold with **no** deadline: NRTRDE's four
//! hours is a delivery expectation, not an expiry, so an extract that arrives
//! in five hours is late and still worth processing.
//!
//! # A future event is not an on-time event
//!
//! [`Lateness::Future`] exists because the alternative is silence. A record
//! timestamped ahead of the evaluation instant is a clock-skewed network
//! element or a corrupt field, and folding it into `OnTime` hides the one
//! signal that would have caught it.
//!
//! ```rust
//! use mediation::{Lateness, LatenessPolicy};
//! use time::{Duration, macros::datetime};
//!
//! let policy = LatenessPolicy::tap3();
//! let as_of = datetime!(2026-07-28 00:00:00 UTC);
//!
//! assert_eq!(policy.classify(datetime!(2026-07-27 00:00:00 UTC), as_of), Lateness::OnTime);
//! assert_eq!(
//!     policy.classify(datetime!(2026-06-01 00:00:00 UTC), as_of),
//!     Lateness::BeyondDeadline(Duration::days(57)),
//! );
//! assert!(matches!(
//!     policy.classify(datetime!(2026-07-29 00:00:00 UTC), as_of),
//!     Lateness::Future(_),
//! ));
//! ```
//!
//! # Replay
//!
//! `as_of` is always a parameter. Replaying a 2019 archive in 2026 reproduces
//! the *2019* verdicts; a policy that read the clock would declare every
//! historic record beyond every deadline, which is not a subtle bug but the
//! whole archive rejected.

use std::fmt;

use time::{Duration, OffsetDateTime};

/// How late an event is relative to an evaluation instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lateness {
    /// The event is timestamped after the evaluation instant.
    ///
    /// Carries how far ahead. Always a fault — clock skew or a corrupt field —
    /// never a normal arrival.
    Future(Duration),
    /// Within the lateness threshold.
    OnTime,
    /// Past the lateness threshold, still inside any deadline.
    ///
    /// Carries the age. Billable; the holding structures may have moved on.
    Late(Duration),
    /// Past the deadline. Carries the age. **Never billable.**
    BeyondDeadline(Duration),
}

impl Lateness {
    /// `true` when the record may still be billed.
    ///
    /// A future-dated record is not billable: it describes usage that has not
    /// happened, and pricing it commits to something unverifiable.
    #[must_use]
    pub const fn is_billable(self) -> bool {
        matches!(self, Self::OnTime | Self::Late(_))
    }

    /// `true` when the record needs an operator's attention.
    #[must_use]
    pub const fn is_finding(self) -> bool {
        !matches!(self, Self::OnTime)
    }

    /// How old the event is, when the classification measured it.
    #[must_use]
    pub const fn age(self) -> Option<Duration> {
        match self {
            Self::Late(age) | Self::BeyondDeadline(age) => Some(age),
            Self::OnTime | Self::Future(_) => None,
        }
    }
}

impl fmt::Display for Lateness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Future(by) => write!(f, "FUTURE by {by}"),
            Self::OnTime => f.write_str("ON_TIME"),
            Self::Late(age) => write!(f, "LATE by {age}"),
            Self::BeyondDeadline(age) => write!(f, "BEYOND_DEADLINE by {age}"),
        }
    }
}

/// When an event becomes late, and when it stops being billable.
///
/// Both bounds are promises about the **outside world** rather than tuning
/// knobs. A seven-day deduplication window says "our partners never resend
/// later than seven days"; if that is false, duplicates pass and revenue is
/// billed twice. The constructors are named for the fact they encode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LatenessPolicy {
    late_after: Duration,
    deadline: Option<Duration>,
}

impl LatenessPolicy {
    /// GSMA TD.57: a roaming event has 30 days from occurrence to reach a TAP
    /// file, after which it can no longer be charged.
    ///
    /// The threshold and the deadline coincide, so a TAP event is never merely
    /// late — it is on time or it is unbillable.
    #[must_use]
    pub const fn tap3() -> Self {
        Self {
            late_after: Duration::days(30),
            deadline: Some(Duration::days(30)),
        }
    }

    /// GSMA TD.35: a roaming extract should reach the home network within four
    /// hours of the event ending.
    ///
    /// No deadline — a late extract is a partner SLA breach and is still worth
    /// processing, because the fraud it evidences is still live.
    #[must_use]
    pub const fn nrtrde() -> Self {
        Self {
            late_after: Duration::hours(4),
            deadline: None,
        }
    }

    /// A policy whose only bound is a hard deadline.
    ///
    /// `None` for a non-positive duration: a deadline of zero would reject
    /// every record, including the ones arriving in real time.
    #[must_use]
    pub const fn deadline(after: Duration) -> Option<Self> {
        if after.is_positive() {
            Some(Self {
                late_after: after,
                deadline: Some(after),
            })
        } else {
            None
        }
    }

    /// A policy that flags lateness without ever making a record unbillable.
    ///
    /// `None` for a non-positive duration.
    #[must_use]
    pub const fn advisory(late_after: Duration) -> Option<Self> {
        if late_after.is_positive() {
            Some(Self {
                late_after,
                deadline: None,
            })
        } else {
            None
        }
    }

    /// A policy with distinct lateness and deadline bounds.
    ///
    /// `None` unless both are positive and the deadline is at least the
    /// lateness threshold — a deadline that precedes the threshold would make
    /// [`Lateness::Late`] unreachable, so the caller has said something they
    /// did not mean.
    #[must_use]
    pub const fn new(late_after: Duration, deadline: Duration) -> Option<Self> {
        if late_after.is_positive()
            && deadline.is_positive()
            && deadline.whole_seconds() >= late_after.whole_seconds()
        {
            Some(Self {
                late_after,
                deadline: Some(deadline),
            })
        } else {
            None
        }
    }

    /// How long after an event it is considered late.
    #[must_use]
    pub const fn late_after(self) -> Duration {
        self.late_after
    }

    /// How long after an event it stops being billable, when there is a limit.
    #[must_use]
    pub const fn deadline_after(self) -> Option<Duration> {
        self.deadline
    }

    /// The instant `event` becomes late.
    #[must_use]
    pub fn late_at(self, event: OffsetDateTime) -> OffsetDateTime {
        event + self.late_after
    }

    /// The instant `event` stops being billable, when there is one.
    #[must_use]
    pub fn deadline_at(self, event: OffsetDateTime) -> Option<OffsetDateTime> {
        self.deadline.map(|d| event + d)
    }

    /// Classify an event against an evaluation instant.
    ///
    /// `as_of` is supplied, never sampled — see the module documentation.
    #[must_use]
    pub fn classify(self, event: OffsetDateTime, as_of: OffsetDateTime) -> Lateness {
        let age = as_of - event;
        if age.is_negative() {
            return Lateness::Future(-age);
        }
        if let Some(deadline) = self.deadline
            && age > deadline
        {
            return Lateness::BeyondDeadline(age);
        }
        if age > self.late_after {
            return Lateness::Late(age);
        }
        Lateness::OnTime
    }
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::LatenessPolicy;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use time::Duration;

    /// Whole seconds, because a policy bound is a business commitment that is
    /// read by humans as well as machines.
    #[derive(Serialize, Deserialize)]
    #[serde(rename = "LatenessPolicy")]
    struct PolicyRepr {
        late_after_s: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        deadline_after_s: Option<i64>,
    }

    impl Serialize for LatenessPolicy {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            PolicyRepr {
                late_after_s: self.late_after().whole_seconds(),
                deadline_after_s: self.deadline_after().map(Duration::whole_seconds),
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for LatenessPolicy {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = PolicyRepr::deserialize(deserializer)?;
            let late_after = Duration::seconds(r.late_after_s);
            let policy = match r.deadline_after_s {
                Some(d) => LatenessPolicy::new(late_after, Duration::seconds(d)),
                None => LatenessPolicy::advisory(late_after),
            };
            policy.ok_or_else(|| serde::de::Error::custom("inconsistent lateness policy bounds"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    const NOW: OffsetDateTime = datetime!(2026-07-28 00:00:00 UTC);

    #[test]
    fn the_tap_deadline_matches_the_published_thirty_days() {
        let policy = LatenessPolicy::tap3();
        assert_eq!(policy.deadline_after(), Some(Duration::days(30)));

        // A day inside the window is on time …
        assert_eq!(
            policy.classify(NOW - Duration::days(29), NOW),
            Lateness::OnTime
        );
        // … and a day outside it can never be charged.
        assert_eq!(
            policy.classify(NOW - Duration::days(31), NOW),
            Lateness::BeyondDeadline(Duration::days(31))
        );
    }

    /// Exactly at the bound is inside it: a 30-day-old TAP event is still
    /// includable, and an off-by-one here rejects a day of real revenue.
    #[test]
    fn the_boundary_is_inclusive() {
        let policy = LatenessPolicy::tap3();
        assert_eq!(
            policy.classify(NOW - Duration::days(30), NOW),
            Lateness::OnTime
        );
        assert_eq!(
            policy.classify(NOW - Duration::days(30) - Duration::seconds(1), NOW),
            Lateness::BeyondDeadline(Duration::days(30) + Duration::seconds(1))
        );
    }

    /// Late and unbillable are different verdicts with different consequences.
    #[test]
    fn late_is_not_the_same_as_unbillable() {
        let policy = LatenessPolicy::new(Duration::days(1), Duration::days(30)).unwrap();

        let late = policy.classify(NOW - Duration::days(5), NOW);
        assert_eq!(late, Lateness::Late(Duration::days(5)));
        assert!(late.is_billable());
        assert!(late.is_finding());

        let expired = policy.classify(NOW - Duration::days(40), NOW);
        assert!(!expired.is_billable());
        assert!(expired.is_finding());
    }

    /// NRTRDE's four hours is a delivery expectation, not an expiry.
    #[test]
    fn a_policy_may_flag_lateness_without_ever_expiring() {
        let policy = LatenessPolicy::nrtrde();
        assert_eq!(policy.deadline_after(), None);

        let very_late = policy.classify(NOW - Duration::days(365), NOW);
        assert!(matches!(very_late, Lateness::Late(_)));
        assert!(
            very_late.is_billable(),
            "no deadline means never unbillable"
        );
    }

    /// A future record is a clock-skew fault, and folding it into `OnTime`
    /// hides the one signal that would catch it.
    #[test]
    fn a_future_event_is_neither_on_time_nor_billable() {
        let policy = LatenessPolicy::tap3();
        let ahead = policy.classify(NOW + Duration::hours(3), NOW);
        assert_eq!(ahead, Lateness::Future(Duration::hours(3)));
        assert!(!ahead.is_billable());
        assert!(ahead.is_finding());
        assert_eq!(ahead.age(), None);
    }

    /// The reason `as_of` is a parameter: an archive replayed years later must
    /// reproduce the verdicts it originally received.
    #[test]
    fn replaying_an_archive_reproduces_the_original_verdicts() {
        let policy = LatenessPolicy::tap3();
        let event = datetime!(2019-03-01 12:00:00 UTC);
        let then = datetime!(2019-03-02 12:00:00 UTC);

        // The verdict at the time …
        assert_eq!(policy.classify(event, then), Lateness::OnTime);
        // … is still the verdict when the archive is reprocessed in 2026,
        // because the evaluation instant is replayed with it.
        assert_eq!(policy.classify(event, then), Lateness::OnTime);
        // Judging it against today would reject the entire archive.
        assert!(!policy.classify(event, NOW).is_billable());
    }

    #[test]
    fn deadline_and_lateness_instants_are_derivable() {
        let policy = LatenessPolicy::tap3();
        let event = datetime!(2026-07-01 00:00:00 UTC);
        assert_eq!(policy.late_at(event), datetime!(2026-07-31 00:00:00 UTC));
        assert_eq!(
            policy.deadline_at(event),
            Some(datetime!(2026-07-31 00:00:00 UTC))
        );
        assert_eq!(LatenessPolicy::nrtrde().deadline_at(event), None);
    }

    #[test]
    fn incoherent_bounds_are_rejected() {
        assert!(LatenessPolicy::deadline(Duration::ZERO).is_none());
        assert!(LatenessPolicy::deadline(Duration::seconds(-1)).is_none());
        assert!(LatenessPolicy::advisory(Duration::ZERO).is_none());
        // A deadline before the lateness threshold makes `Late` unreachable.
        assert!(LatenessPolicy::new(Duration::days(10), Duration::days(5)).is_none());
        assert!(LatenessPolicy::new(Duration::days(5), Duration::days(5)).is_some());
    }
}
