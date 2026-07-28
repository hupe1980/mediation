//! Fair use and permanent roaming (EU 2016/2286).
//!
//! Under Roam Like At Home a provider may apply a fair use policy, and
//! Implementing Regulation (EU) 2016/2286 says how. It is a regulatory
//! classification computed purely from usage history, and every part of it is
//! easy to implement slightly wrong in the direction that costs a customer
//! money.
//!
//! # The safe harbour is an OR, not an AND
//!
//! Either **prevailing domestic consumption** *or* **prevailing domestic
//! presence** over the observation window is proof of non-abusive use. Only
//! when roaming presence **and** roaming consumption both prevail may a
//! provider warn the customer, and only after warning may it surcharge.
//!
//! Reading "presence and consumption are observed cumulatively" as an AND on
//! the safe-harbour side surcharges lawful customers. The two are separate
//! predicates here so the mistake cannot be made silently:
//!
//! ```rust
//! use mediation::{FairUseAssessment, Octets};
//!
//! // Abroad most of the window, but consuming mostly at home.
//! let assessment = FairUseAssessment::from_counts(
//!     /* domestic presence */ 40,
//!     /* roaming presence  */ 80,
//!     /* domestic use */ Octets::new(9_000),
//!     /* roaming use  */ Octets::new(1_000),
//!     /* window days */ 120,
//! );
//!
//! assert!(!assessment.prevailing_domestic_presence());
//! assert!(assessment.prevailing_domestic_consumption());
//! assert!(assessment.prevailing_domestic(), "either alone is the safe harbour");
//! assert!(!assessment.may_warn());
//! ```
//!
//! # Presence is not consumption, and usage records only prove the latter
//!
//! A subscriber can be abroad for six weeks and generate nothing chargeable.
//! Inferring presence from usage alone undercounts roaming presence and biases
//! the assessment toward "prevailing domestic" — the safe direction for the
//! customer, but still wrong, and it silently defeats a sustainability case an
//! operator may legitimately need to make.
//!
//! Presence comes from registration events (5G AMF charging, 3GPP TS 32.256),
//! so [`assess`] takes a presence series **alongside** the usage history rather
//! than deriving one from the other.
//!
//! # Roaming is not the complement of domestic
//!
//! RLAH applies within the EEA. A record from a network outside it is neither
//! domestic nor regulated roaming, and counting it as roaming inflates the
//! roaming side of a test with legal consequences. [`Region`] has three cases
//! and rest-of-world usage is excluded from **both** sides of the ratio.

use std::fmt;

use crate::plmn::PlmnId;
use crate::record::UsageRecord;
use crate::volume::Octets;
use time::Date;

// ── Region ───────────────────────────────────────────────────────────────────

/// Where usage happened, relative to the regulated area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Region {
    /// The subscriber's own **country**.
    ///
    /// Not their own network. A subscriber national-roaming on a second network
    /// at home is domestic: Regulation (EU) 2022/612 regulates roaming *abroad*,
    /// and counting a domestic visit as regulated roaming inflates the side of
    /// the ratio that decides whether a customer may be surcharged.
    /// [`is_abroad`](crate::is_abroad) is the country comparison;
    /// [`UsageRecord::is_roaming`](crate::UsageRecord::is_roaming) is the
    /// network one, and they are not the same question.
    Domestic,
    /// Another network inside the European Economic Area — regulated roaming.
    Eea,
    /// Outside the EEA, where the fair use rules do not reach.
    ///
    /// Excluded from both sides of the ratio. Counting it as roaming inflates a
    /// test that decides whether a customer may be surcharged.
    RestOfWorld,
}

impl Region {
    /// Every variant, for exhaustive iteration in tests and mappings.
    pub const ALL: &'static [Self] = &[Self::Domestic, Self::Eea, Self::RestOfWorld];

    /// `true` when usage here counts toward the assessment at all.
    #[must_use]
    pub const fn is_assessed(self) -> bool {
        !matches!(self, Self::RestOfWorld)
    }

    /// The canonical string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Domestic => "DOMESTIC",
            Self::Eea => "EEA",
            Self::RestOfWorld => "REST_OF_WORLD",
        }
    }
}

impl fmt::Display for Region {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Resolves a serving network to its region relative to a home network.
///
/// Implement this over whatever membership data you maintain. The crate calls
/// it and never guesses: which networks are inside the EEA is a political fact
/// that changes, and a vendored list would be stale on arrival — exactly as
/// with [`MncWidths`](crate::plmn::MncWidths).
///
/// # Compare countries, not networks
///
/// The obvious implementation — `if serving == home { Domestic } else { Eea }` —
/// is wrong, and wrong in the direction that costs a customer money: it files a
/// national roamer under regulated roaming and inflates the ratio that decides
/// whether they may be surcharged. Compare `serving.mcc()` against
/// `home.mcc()`, which is what [`is_abroad`](crate::is_abroad) does for a whole
/// record.
pub trait Regions {
    /// Where `serving` sits relative to `home`.
    fn region_of(&self, serving: PlmnId, home: PlmnId) -> Region;
}

impl<F> Regions for F
where
    F: Fn(PlmnId, PlmnId) -> Region,
{
    fn region_of(&self, serving: PlmnId, home: PlmnId) -> Region {
        self(serving, home)
    }
}

// ── Presence ─────────────────────────────────────────────────────────────────

/// One day of a subscriber's observed presence.
///
/// Derived from registration events, not from usage: a subscriber present and
/// idle is present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PresenceDay {
    /// The local calendar day.
    pub date: Date,
    /// Where the subscriber was registered that day.
    pub region: Region,
}

impl PresenceDay {
    /// A day of presence in a region.
    #[must_use]
    pub const fn new(date: Date, region: Region) -> Self {
        Self { date, region }
    }
}

// ── ObservationWindow ────────────────────────────────────────────────────────

/// How long a fair use policy must observe before it may conclude anything.
///
/// Implementing Regulation (EU) 2016/2286 requires presence and consumption to
/// be observed cumulatively over **at least four months**. A shorter window
/// cannot support a conclusion, and this type exists so a caller cannot supply
/// one by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObservationWindow {
    days: u32,
}

impl ObservationWindow {
    /// The regulation's floor, in days: four months.
    pub const MINIMUM_DAYS: u32 = 120;

    /// The shortest window the regulation permits.
    #[must_use]
    pub const fn minimum() -> Self {
        Self {
            days: Self::MINIMUM_DAYS,
        }
    }

    /// A window of `days`, which must be at least the regulatory floor.
    #[must_use]
    pub const fn new(days: u32) -> Option<Self> {
        if days >= Self::MINIMUM_DAYS {
            Some(Self { days })
        } else {
            None
        }
    }

    /// The window's length in days.
    #[must_use]
    pub const fn days(self) -> u32 {
        self.days
    }

    /// The earliest day the window covers, given the day it ends.
    #[must_use]
    pub fn start(self, ending: Date) -> Date {
        ending - time::Duration::days(i64::from(self.days) - 1)
    }
}

// ── Assessment ───────────────────────────────────────────────────────────────

/// What the observation window showed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FairUseAssessment {
    domestic_presence_days: u32,
    roaming_presence_days: u32,
    domestic_consumption: Octets,
    roaming_consumption: Octets,
    observed_days: u32,
}

impl FairUseAssessment {
    /// An assessment from counts a caller computed themselves.
    ///
    /// Present so the predicates can be applied to figures from an existing
    /// system without re-deriving them from records.
    #[must_use]
    pub const fn from_counts(
        domestic_presence_days: u32,
        roaming_presence_days: u32,
        domestic_consumption: Octets,
        roaming_consumption: Octets,
        observed_days: u32,
    ) -> Self {
        Self {
            domestic_presence_days,
            roaming_presence_days,
            domestic_consumption,
            roaming_consumption,
            observed_days,
        }
    }

    /// Days observed at home.
    #[must_use]
    pub const fn domestic_presence_days(&self) -> u32 {
        self.domestic_presence_days
    }

    /// Days observed on another EEA network.
    #[must_use]
    pub const fn roaming_presence_days(&self) -> u32 {
        self.roaming_presence_days
    }

    /// Volume consumed at home.
    #[must_use]
    pub const fn domestic_consumption(&self) -> Octets {
        self.domestic_consumption
    }

    /// Volume consumed on another EEA network.
    #[must_use]
    pub const fn roaming_consumption(&self) -> Octets {
        self.roaming_consumption
    }

    /// How many days the window actually spanned.
    #[must_use]
    pub const fn observed_days(&self) -> u32 {
        self.observed_days
    }

    /// `true` when the window is long enough to conclude anything.
    ///
    /// Separate from the verdict so a caller cannot act on three months of data
    /// by reading only the predicate they were looking for.
    #[must_use]
    pub const fn window_sufficient(&self) -> bool {
        self.observed_days >= ObservationWindow::MINIMUM_DAYS
    }

    /// `true` when more of the observed days were spent at home than abroad.
    ///
    /// Equal days are **not** prevailing roaming: the regulation makes
    /// prevailing domestic use the safe harbour, and a tie does not establish
    /// the abuse the provider would need to show.
    #[must_use]
    pub const fn prevailing_domestic_presence(&self) -> bool {
        self.domestic_presence_days >= self.roaming_presence_days
    }

    /// `true` when more was consumed at home than abroad.
    ///
    /// Ties resolve in the customer's favour, as with presence.
    #[must_use]
    pub const fn prevailing_domestic_consumption(&self) -> bool {
        self.domestic_consumption.get() >= self.roaming_consumption.get()
    }

    /// The safe harbour: **either** indicator alone establishes non-abusive use.
    #[must_use]
    pub const fn prevailing_domestic(&self) -> bool {
        self.prevailing_domestic_presence() || self.prevailing_domestic_consumption()
    }

    /// `true` when the provider may warn the customer.
    ///
    /// Requires a sufficient window **and** that neither safe harbour applies —
    /// that is, roaming presence and roaming consumption both prevail.
    #[must_use]
    pub const fn may_warn(&self) -> bool {
        self.window_sufficient() && !self.prevailing_domestic()
    }

    /// `true` when the provider may apply a surcharge.
    ///
    /// The regulation's sequence is observe, warn, then surcharge — so this
    /// takes whether a warning was actually issued rather than inferring it.
    /// Surcharging without having warned is the common error, and it is not
    /// expressible here.
    #[must_use]
    pub const fn may_surcharge(&self, customer_was_warned: bool) -> bool {
        self.may_warn() && customer_was_warned
    }

    /// Why the assessment reached its verdict, for a notice a human reads.
    #[must_use]
    pub const fn safe_harbour(&self) -> Option<SafeHarbour> {
        if !self.window_sufficient() {
            return Some(SafeHarbour::WindowTooShort);
        }
        match (
            self.prevailing_domestic_presence(),
            self.prevailing_domestic_consumption(),
        ) {
            (true, true) => Some(SafeHarbour::BothIndicators),
            (true, false) => Some(SafeHarbour::Presence),
            (false, true) => Some(SafeHarbour::Consumption),
            (false, false) => None,
        }
    }
}

impl fmt::Display for FairUseAssessment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} day(s): presence {}/{} home/roaming, use {}/{} — ",
            self.observed_days,
            self.domestic_presence_days,
            self.roaming_presence_days,
            self.domestic_consumption,
            self.roaming_consumption,
        )?;
        match self.safe_harbour() {
            Some(reason) => write!(f, "non-abusive ({reason})"),
            None => f.write_str("roaming prevails on both indicators"),
        }
    }
}

/// Which safe harbour a subscriber fell under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SafeHarbour {
    /// The window is too short to conclude anything at all.
    WindowTooShort,
    /// Prevailing domestic presence.
    Presence,
    /// Prevailing domestic consumption.
    Consumption,
    /// Both indicators prevail domestically.
    BothIndicators,
}

impl SafeHarbour {
    /// The canonical string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WindowTooShort => "WINDOW_TOO_SHORT",
            Self::Presence => "PREVAILING_DOMESTIC_PRESENCE",
            Self::Consumption => "PREVAILING_DOMESTIC_CONSUMPTION",
            Self::BothIndicators => "BOTH_INDICATORS_DOMESTIC",
        }
    }
}

impl fmt::Display for SafeHarbour {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Assess a subscriber's usage and presence over an observation window.
///
/// `presence` comes from registration events and `usage` from charging records
/// — see the module documentation for why one cannot be derived from the other.
/// Both are filtered to the window ending on `ending`.
///
/// Records whose serving or home network is unknown are skipped: an
/// unattributable record cannot be placed in a region, and guessing would put
/// it on one side of a test with legal consequences.
#[must_use]
pub fn assess(
    usage: &[UsageRecord],
    presence: &[PresenceDay],
    home: PlmnId,
    window: ObservationWindow,
    ending: Date,
    regions: &impl Regions,
) -> FairUseAssessment {
    let start = window.start(ending);

    let (mut domestic_days, mut roaming_days) = (0u32, 0u32);
    for day in presence
        .iter()
        .filter(|d| d.date >= start && d.date <= ending)
    {
        match day.region {
            Region::Domestic => domestic_days = domestic_days.saturating_add(1),
            Region::Eea => roaming_days = roaming_days.saturating_add(1),
            Region::RestOfWorld => {}
        }
    }

    let (mut domestic_use, mut roaming_use) = (Octets::ZERO, Octets::ZERO);
    for record in usage {
        let date = record.local_date();
        if date < start || date > ending {
            continue;
        }
        let (Some(serving), Some(record_home)) = (record.serving_plmn, record.home_plmn) else {
            continue;
        };
        let Some(volume) = record.billable_volume() else {
            continue;
        };
        let Some(total) = volume.checked_total() else {
            continue;
        };
        // The record's own home network is used, not the argument: a record
        // filed against a different subscription is not this one's usage.
        if !record_home.is_same_network(home) {
            continue;
        }
        // Saturating, not checked-then-skipped: dropping a record on overflow
        // would silently remove it from a comparison that decides whether a
        // customer may be surcharged. Reaching the ceiling takes 18 exabytes in
        // one window, and a saturated side is visibly wrong rather than quietly
        // short.
        match regions.region_of(serving, home) {
            Region::Domestic => domestic_use = domestic_use.saturating_add(total),
            Region::Eea => roaming_use = roaming_use.saturating_add(total),
            Region::RestOfWorld => {}
        }
    }

    FairUseAssessment::from_counts(
        domestic_days,
        roaming_days,
        domestic_use,
        roaming_use,
        window.days(),
    )
}

// ── VolumeAllowance ──────────────────────────────────────────────────────────

/// A fair use volume limit, and the alert the regulation requires.
///
/// Regulation (EU) 2022/612 requires the provider to inform the customer of the
/// limit in advance **and** to alert them on reaching 80 % of it. The threshold
/// is part of the type rather than a caller's constant, because it is the law
/// rather than a policy choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VolumeAllowance {
    limit: Octets,
}

impl VolumeAllowance {
    /// The fraction of the limit at which the customer must be alerted.
    pub const ALERT_PERCENT: u64 = 80;

    /// An allowance of `limit`. `None` for a zero limit, which is not an
    /// allowance but a refusal to serve.
    #[must_use]
    pub const fn new(limit: Octets) -> Option<Self> {
        if limit.get() == 0 {
            None
        } else {
            Some(Self { limit })
        }
    }

    /// The limit itself.
    #[must_use]
    pub const fn limit(self) -> Octets {
        self.limit
    }

    /// The consumption at which the alert falls due.
    ///
    /// Multiplies before dividing, in 128 bits, so the threshold is exact for
    /// every limit. Dividing first — `limit / 100 * 80` — throws away the
    /// remainder and puts the alert at 80 octets for a 199-octet allowance,
    /// which alerts a customer at 40 % of their allowance and tells them
    /// something untrue.
    ///
    /// Rounded **down**, so the alert is never sent later than 80 %: the
    /// regulation sets a ceiling on when the customer learns, not a target.
    #[must_use]
    pub const fn alert_at(self) -> Octets {
        let scaled = (self.limit.get() as u128 * Self::ALERT_PERCENT as u128) / 100;
        Octets::new(scaled as u64)
    }

    /// Where consumption sits against the allowance.
    #[must_use]
    pub const fn status(self, consumed: Octets) -> AllowanceStatus {
        if consumed.get() >= self.limit.get() {
            AllowanceStatus::Exceeded
        } else if consumed.get() >= self.alert_at().get() {
            AllowanceStatus::AlertDue
        } else {
            AllowanceStatus::Within
        }
    }
}

/// Where a subscriber's consumption sits against their allowance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AllowanceStatus {
    /// Below the alert threshold.
    Within,
    /// At or past 80 % of the limit; the customer must be alerted.
    AlertDue,
    /// At or past the limit; a surcharge may apply.
    Exceeded,
}

impl AllowanceStatus {
    /// `true` when the regulation requires the customer to be told.
    #[must_use]
    pub const fn requires_alert(self) -> bool {
        matches!(self, Self::AlertDue | Self::Exceeded)
    }

    /// The canonical string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Within => "WITHIN",
            Self::AlertDue => "ALERT_DUE",
            Self::Exceeded => "EXCEEDED",
        }
    }
}

impl fmt::Display for AllowanceStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::{AllowanceStatus, FairUseAssessment, Region, SafeHarbour};
    use serde::de::{self, Visitor};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::fmt;

    macro_rules! code_serde {
        ($ty:ty, $all:expr, $expecting:literal) => {
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
                            $all.iter()
                                .find(|c| c.as_str() == v)
                                .copied()
                                .ok_or_else(|| de::Error::custom(format!("unknown code {v:?}")))
                        }
                    }
                    deserializer.deserialize_str(V)
                }
            }
        };
    }

    code_serde!(Region, Region::ALL, "a region code such as \"EEA\"");
    code_serde!(
        AllowanceStatus,
        [
            AllowanceStatus::Within,
            AllowanceStatus::AlertDue,
            AllowanceStatus::Exceeded
        ],
        "an allowance status code"
    );
    code_serde!(
        SafeHarbour,
        [
            SafeHarbour::WindowTooShort,
            SafeHarbour::Presence,
            SafeHarbour::Consumption,
            SafeHarbour::BothIndicators
        ],
        "a safe harbour code"
    );

    /// Field names are part of the public representation and covered by semver.
    #[derive(Serialize, Deserialize)]
    #[serde(rename = "FairUseAssessment")]
    struct Repr {
        domestic_presence_days: u32,
        roaming_presence_days: u32,
        domestic_consumption: crate::volume::Octets,
        roaming_consumption: crate::volume::Octets,
        observed_days: u32,
    }

    impl Serialize for FairUseAssessment {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            Repr {
                domestic_presence_days: self.domestic_presence_days(),
                roaming_presence_days: self.roaming_presence_days(),
                domestic_consumption: self.domestic_consumption(),
                roaming_consumption: self.roaming_consumption(),
                observed_days: self.observed_days(),
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for FairUseAssessment {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = Repr::deserialize(deserializer)?;
            Ok(FairUseAssessment::from_counts(
                r.domestic_presence_days,
                r.roaming_presence_days,
                r.domestic_consumption,
                r.roaming_consumption,
                r.observed_days,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::party::{ChargedParty, SubscriberRef};
    use crate::record::{RecordType, SourceRef};
    use crate::volume::VolumePair;
    use time::macros::date;

    fn home() -> PlmnId {
        "262-02".parse().unwrap()
    }

    /// A resolver that knows one EEA partner and treats everything else as
    /// outside — supplied, never guessed.
    fn regions() -> impl Regions {
        |serving: PlmnId, home: PlmnId| {
            // Country, not network: a national roamer is at home.
            if serving.is_same_country(home) {
                Region::Domestic
            } else if serving.mcc().get() == 234 {
                Region::Eea
            } else {
                Region::RestOfWorld
            }
        }
    }

    fn record(day: &str, serving: &str, octets: u64) -> UsageRecord {
        UsageRecord::new(
            RecordType::PduSession,
            ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
            format!("{day}T10:00:00+02:00").parse().unwrap(),
            SourceRef::new("pgw-01".parse().unwrap(), 0),
        )
        .with_networks(serving.parse().unwrap(), home())
        .with_volume(VolumePair::new(Octets::new(octets), Octets::ZERO))
    }

    // ── The safe harbour ─────────────────────────────────────────────────────

    /// The error this API shape exists to prevent: reading the safe harbour as
    /// an AND surcharges lawful customers.
    #[test]
    fn either_indicator_alone_is_the_safe_harbour() {
        // Abroad most days, but consuming mostly at home.
        let by_consumption =
            FairUseAssessment::from_counts(40, 80, Octets::new(9_000), Octets::new(1_000), 120);
        assert!(!by_consumption.prevailing_domestic_presence());
        assert!(by_consumption.prevailing_domestic_consumption());
        assert!(by_consumption.prevailing_domestic());
        assert!(!by_consumption.may_warn());
        assert_eq!(
            by_consumption.safe_harbour(),
            Some(SafeHarbour::Consumption)
        );

        // Home most days, but consuming mostly abroad.
        let by_presence =
            FairUseAssessment::from_counts(80, 40, Octets::new(1_000), Octets::new(9_000), 120);
        assert!(by_presence.prevailing_domestic());
        assert!(!by_presence.may_warn());
        assert_eq!(by_presence.safe_harbour(), Some(SafeHarbour::Presence));
    }

    /// Only when both indicators point abroad may the provider act.
    #[test]
    fn a_warning_needs_both_indicators_to_prevail_abroad() {
        let permanent =
            FairUseAssessment::from_counts(20, 100, Octets::new(500), Octets::new(9_500), 120);
        assert!(!permanent.prevailing_domestic());
        assert!(permanent.may_warn());
        assert_eq!(permanent.safe_harbour(), None);
    }

    /// The regulation's sequence is observe, warn, then surcharge — and
    /// surcharging without warning is not expressible.
    #[test]
    fn a_surcharge_requires_a_warning_to_have_been_issued() {
        let permanent =
            FairUseAssessment::from_counts(20, 100, Octets::new(500), Octets::new(9_500), 120);
        assert!(permanent.may_warn());
        assert!(!permanent.may_surcharge(false), "no warning, no surcharge");
        assert!(permanent.may_surcharge(true));

        // And a customer inside the safe harbour cannot be surcharged even if
        // somebody sent them a warning.
        let lawful =
            FairUseAssessment::from_counts(80, 40, Octets::new(9_000), Octets::new(1_000), 120);
        assert!(!lawful.may_surcharge(true));
    }

    /// Ties resolve in the customer's favour: an equal split does not establish
    /// the abuse the provider would have to show.
    #[test]
    fn an_equal_split_is_not_prevailing_roaming() {
        let tied = FairUseAssessment::from_counts(60, 60, Octets::new(500), Octets::new(500), 120);
        assert!(tied.prevailing_domestic_presence());
        assert!(tied.prevailing_domestic_consumption());
        assert!(!tied.may_warn());
        assert_eq!(tied.safe_harbour(), Some(SafeHarbour::BothIndicators));
    }

    /// A window shorter than four months cannot support any conclusion.
    #[test]
    fn a_short_window_blocks_every_conclusion() {
        let short =
            FairUseAssessment::from_counts(10, 80, Octets::new(100), Octets::new(9_900), 90);
        assert!(!short.window_sufficient());
        assert!(!short.may_warn());
        assert!(!short.may_surcharge(true));
        assert_eq!(short.safe_harbour(), Some(SafeHarbour::WindowTooShort));

        assert_eq!(ObservationWindow::new(90), None);
        assert_eq!(ObservationWindow::minimum().days(), 120);
        assert!(ObservationWindow::new(180).is_some());
    }

    // ── Building an assessment from records ──────────────────────────────────

    /// Presence comes from registration, not from usage: a subscriber abroad
    /// and idle is still abroad.
    #[test]
    fn presence_is_counted_separately_from_consumption() {
        let window = ObservationWindow::minimum();
        let ending = date!(2026 - 07 - 28);

        // Registered abroad for most of the window, but consuming at home.
        let mut presence = Vec::new();
        let mut day = window.start(ending);
        for index in 0..window.days() {
            presence.push(PresenceDay::new(
                day,
                if index < 30 {
                    Region::Domestic
                } else {
                    Region::Eea
                },
            ));
            day += time::Duration::days(1);
        }

        let usage = vec![
            record("2026-07-20", "262-02", 9_000),
            record("2026-07-21", "234-15", 1_000),
        ];

        let assessment = assess(&usage, &presence, home(), window, ending, &regions());
        assert_eq!(assessment.domestic_presence_days(), 30);
        assert_eq!(assessment.roaming_presence_days(), 90);
        assert_eq!(assessment.domestic_consumption(), Octets::new(9_000));
        assert_eq!(assessment.roaming_consumption(), Octets::new(1_000));

        // Presence prevails abroad, consumption at home — the safe harbour
        // still applies.
        assert!(!assessment.prevailing_domestic_presence());
        assert!(assessment.prevailing_domestic());
    }

    /// Rest-of-world usage is excluded from both sides, not counted as roaming.
    #[test]
    fn usage_outside_the_regulated_area_counts_on_neither_side() {
        let window = ObservationWindow::minimum();
        let ending = date!(2026 - 07 - 28);
        let usage = vec![
            record("2026-07-20", "262-02", 1_000),
            record("2026-07-21", "234-15", 500),
            // A network outside the EEA entirely.
            record("2026-07-22", "310-260", 9_000),
        ];

        let assessment = assess(&usage, &[], home(), window, ending, &regions());
        assert_eq!(assessment.domestic_consumption(), Octets::new(1_000));
        assert_eq!(assessment.roaming_consumption(), Octets::new(500));
        assert!(
            assessment.prevailing_domestic(),
            "9 GB outside the EEA must not tip a test it is not part of"
        );
    }

    /// A record is never dropped from the comparison: an implausible volume
    /// saturates its side visibly rather than vanishing from it.
    #[test]
    fn an_overflowing_side_saturates_rather_than_losing_records() {
        let window = ObservationWindow::minimum();
        let ending = date!(2026 - 07 - 28);
        let usage = vec![
            record("2026-07-20", "234-15", u64::MAX),
            record("2026-07-21", "234-15", u64::MAX),
            record("2026-07-22", "262-02", 1),
        ];

        let assessment = assess(&usage, &[], home(), window, ending, &regions());
        assert_eq!(assessment.roaming_consumption(), Octets::new(u64::MAX));
        assert_eq!(assessment.domestic_consumption(), Octets::new(1));
        assert!(!assessment.prevailing_domestic_consumption());
    }

    #[test]
    fn records_outside_the_window_are_ignored() {
        let window = ObservationWindow::minimum();
        let ending = date!(2026 - 07 - 28);
        let usage = vec![
            record("2026-07-20", "234-15", 1_000),
            // Before the window opened.
            record("2026-01-01", "234-15", 9_000),
        ];

        let assessment = assess(&usage, &[], home(), window, ending, &regions());
        assert_eq!(assessment.roaming_consumption(), Octets::new(1_000));
    }

    /// A record that cannot be placed in a region is skipped rather than
    /// guessed onto one side of a test with legal consequences.
    #[test]
    fn unplaceable_records_are_skipped() {
        let window = ObservationWindow::minimum();
        let ending = date!(2026 - 07 - 28);

        let mut no_networks = record("2026-07-20", "234-15", 5_000);
        no_networks.serving_plmn = None;

        let mut other_subscription = record("2026-07-21", "234-15", 5_000);
        other_subscription.home_plmn = Some("262-01".parse().unwrap());

        let usage = vec![
            no_networks,
            other_subscription,
            record("2026-07-22", "234-15", 100),
        ];
        let assessment = assess(&usage, &[], home(), window, ending, &regions());
        assert_eq!(assessment.roaming_consumption(), Octets::new(100));
    }

    // ── Volume allowance ─────────────────────────────────────────────────────

    /// Regulation (EU) 2022/612 requires an alert at 80 % of the limit.
    #[test]
    fn the_alert_falls_due_at_eighty_percent() {
        let allowance = VolumeAllowance::new(Octets::new(10_000)).unwrap();
        assert_eq!(allowance.alert_at(), Octets::new(8_000));

        assert_eq!(
            allowance.status(Octets::new(7_999)),
            AllowanceStatus::Within
        );
        assert_eq!(
            allowance.status(Octets::new(8_000)),
            AllowanceStatus::AlertDue
        );
        assert_eq!(
            allowance.status(Octets::new(10_000)),
            AllowanceStatus::Exceeded
        );
        assert!(allowance.status(Octets::new(9_000)).requires_alert());
        assert!(!allowance.status(Octets::ZERO).requires_alert());
    }

    /// The threshold is exact for every limit, not only round ones. Dividing
    /// before multiplying would alert a 199-octet allowance at 80 — 40 % — and
    /// tell the customer something untrue.
    #[test]
    fn the_alert_threshold_is_exact_for_limits_that_are_not_round() {
        assert_eq!(
            VolumeAllowance::new(Octets::new(199)).unwrap().alert_at(),
            Octets::new(159),
            "199 * 0.8 is 159.2, rounded down"
        );
        assert_eq!(
            VolumeAllowance::new(Octets::new(999)).unwrap().alert_at(),
            Octets::new(799),
            "999 * 0.8 is 799.2, rounded down"
        );
        assert_eq!(
            VolumeAllowance::new(Octets::new(5)).unwrap().alert_at(),
            Octets::new(4)
        );

        // …and the multiply cannot overflow, even at the widest limit.
        let widest = VolumeAllowance::new(Octets::new(u64::MAX)).unwrap();
        assert!(widest.alert_at().get() < u64::MAX);
        assert_eq!(widest.status(widest.alert_at()), AllowanceStatus::AlertDue);
    }

    #[test]
    fn a_zero_allowance_is_not_an_allowance() {
        assert_eq!(VolumeAllowance::new(Octets::ZERO), None);
        assert!(VolumeAllowance::new(Octets::new(1)).is_some());
    }

    #[test]
    fn regions_are_supplied_and_only_two_are_assessed() {
        assert!(Region::Domestic.is_assessed());
        assert!(Region::Eea.is_assessed());
        assert!(!Region::RestOfWorld.is_assessed());

        let resolver = regions();
        assert_eq!(resolver.region_of(home(), home()), Region::Domestic);
        assert_eq!(
            resolver.region_of("262-01".parse().unwrap(), home()),
            Region::Domestic,
            "a national roamer is at home; calling this EEA would inflate the \
             ratio that decides whether a customer may be surcharged"
        );
        assert_eq!(
            resolver.region_of("234-15".parse().unwrap(), home()),
            Region::Eea
        );
        assert_eq!(
            resolver.region_of("310-260".parse().unwrap(), home()),
            Region::RestOfWorld
        );
    }
}
