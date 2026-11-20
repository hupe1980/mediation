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
//! use time::macros::date;
//!
//! // Abroad most of the window, but consuming mostly at home.
//! let assessment = FairUseAssessment::from_counts(
//!     /* domestic presence */ 40,
//!     /* roaming presence  */ 80,
//!     /* domestic use */ Octets::new(9_000),
//!     /* roaming use  */ Octets::new(1_000),
//!     /* window days */ 120,
//!     /* window start */ date!(2026 - 03 - 29),
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
use std::collections::BTreeSet;
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
/// Article 4(4) of Implementing Regulation (EU) 2016/2286 observes presence and
/// consumption "cumulatively and for a period of time of at least 4 months".
/// The window is therefore counted in **calendar months**, not days: 120 days
/// is short of four months whenever the window spans 31-day months (finding
/// R2). A shorter window cannot support a conclusion, and this type exists so
/// a caller cannot supply one by accident.
///
/// ```rust
/// use mediation::ObservationWindow;
/// use time::macros::date;
///
/// let window = ObservationWindow::months(4).unwrap();
/// // Four months ending on 28 July begin on 29 March: 122 days, not 120.
/// assert_eq!(window.start(date!(2026 - 07 - 28)), Some(date!(2026 - 03 - 29)));
/// assert!(ObservationWindow::months(3).is_none());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObservationWindow {
    months: u32,
}

impl ObservationWindow {
    /// The regulation's floor, in calendar months.
    pub const MINIMUM_MONTHS: u32 = 4;

    /// The fewest days any four consecutive calendar months hold.
    ///
    /// The evidence floor [`FairUseAssessment::window_sufficient`] applies to
    /// the days a presence series actually covers.
    pub const MINIMUM_DAYS: u32 = 120;

    /// The shortest window the regulation permits: four months.
    #[must_use]
    pub const fn minimum() -> Self {
        Self {
            months: Self::MINIMUM_MONTHS,
        }
    }

    /// A window of `months` calendar months, which must be at least four.
    #[must_use]
    pub const fn months(months: u32) -> Option<Self> {
        if months >= Self::MINIMUM_MONTHS {
            Some(Self { months })
        } else {
            None
        }
    }

    /// The window's length in calendar months.
    #[must_use]
    pub const fn month_count(self) -> u32 {
        self.months
    }

    /// The earliest day the window covers, given the last day it covers.
    ///
    /// Month arithmetic as in BGB §§ 187(2), 188(2) and Regulation (EEC,
    /// Euratom) No 1182/71 Art. 3: a window of `n` months starting on day `d`
    /// ends the day before day `d` of the `n`-th month after, and a month
    /// lacking that day clamps to its last day. Clamping can only lengthen the
    /// window, never shorten it below `n` months.
    ///
    /// `None` when the start falls outside the calendar `time` supports
    /// (finding R3: this never panics).
    #[must_use]
    pub fn start(self, ending: Date) -> Option<Date> {
        let months = i64::from(self.months);
        match ending.next_day() {
            Some(after) => add_calendar_months(after, -months),
            // The calendar's last day ends its month, so the window is whole
            // months back from that month's first day.
            None => add_calendar_months(ending.replace_day(1).ok()?, 1 - months),
        }
    }
}

/// `date` moved by `months` calendar months, clamped to the target month's last
/// day. `None` outside the supported calendar; never panics.
pub(crate) fn add_calendar_months(date: Date, months: i64) -> Option<Date> {
    let index = i64::from(date.year())
        .checked_mul(12)?
        .checked_add(i64::from(u8::from(date.month())) - 1)?
        .checked_add(months)?;
    let year = i32::try_from(index.div_euclid(12)).ok()?;
    let month = time::Month::try_from(u8::try_from(index.rem_euclid(12) + 1).ok()?).ok()?;
    (1..=date.day())
        .rev()
        .find_map(|day| Date::from_calendar_date(year, month, day).ok())
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
    window_start: Date,
}

impl FairUseAssessment {
    /// An assessment from counts a caller computed themselves.
    ///
    /// Present so the predicates can be applied to figures from an existing
    /// system without re-deriving them from records. `window_start` is the
    /// first day of the observation window the counts cover; an alert issued
    /// before it does not rest on this assessment (finding R4).
    #[must_use]
    pub const fn from_counts(
        domestic_presence_days: u32,
        roaming_presence_days: u32,
        domestic_consumption: Octets,
        roaming_consumption: Octets,
        observed_days: u32,
        window_start: Date,
    ) -> Self {
        Self {
            domestic_presence_days,
            roaming_presence_days,
            domestic_consumption,
            roaming_consumption,
            observed_days,
            window_start,
        }
    }

    /// The first day of the observation window.
    #[must_use]
    pub const fn window_start(&self) -> Date {
        self.window_start
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

    /// How many days of the window presence was actually **evidenced** for.
    ///
    /// Not the window's length. [`assess`] counts the distinct days the
    /// presence series covers inside the window, because that is what the
    /// conclusion rests on: a four-month window holding three weeks of
    /// registration data has observed three weeks, however it was configured.
    ///
    /// Filling this with the window's length instead makes
    /// [`window_sufficient`](Self::window_sufficient) a restatement of the
    /// caller's own configuration — always true, since
    /// [`ObservationWindow::months`] already refuses anything shorter — and the
    /// gate that is supposed to stop a provider concluding from thin data
    /// stops nothing.
    #[must_use]
    pub const fn observed_days(&self) -> u32 {
        self.observed_days
    }

    /// `true` when enough of the window was actually observed to conclude
    /// anything.
    ///
    /// Separate from the verdict so a caller cannot act on three weeks of data
    /// by reading only the predicate they were looking for. Insufficient
    /// evidence resolves in the customer's favour: [`may_warn`](Self::may_warn)
    /// is false, which is the direction the regulation puts the burden in.
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

    /// `true` when the provider may apply a surcharge on `as_of`.
    ///
    /// The regulation's sequence is **observe, alert, wait, then surcharge**,
    /// and all four steps are required here.
    ///
    /// Article 5(3) of Implementing Regulation (EU) 2016/2286 requires the
    /// customer to be alerted before any surcharge, and Article 5(4) requires
    /// the alert to give them "a period which cannot be shorter than 2 weeks"
    /// to change their usage pattern. So this takes the alert itself — with the
    /// date it was issued — rather than a bare `bool`: surcharging on the day
    /// of the alert satisfies "was warned" and breaches the regulation, and a
    /// boolean cannot tell the two apart.
    ///
    /// `None` for the alert means none was issued, which is never a surcharge.
    /// An alert issued before the [`window_start`](Self::window_start) does
    /// not rest on this observation and is refused (finding R4).
    ///
    /// ```rust
    /// use mediation::{FairUseAlert, FairUseAssessment, Octets};
    /// use time::macros::date;
    ///
    /// // Roaming prevails on both indicators over a full window.
    /// let assessment = FairUseAssessment::from_counts(
    ///     20, 100, Octets::new(1_000), Octets::new(9_000), 120, date!(2026 - 03 - 01),
    /// );
    /// assert!(assessment.may_warn());
    ///
    /// let alert = FairUseAlert::new(date!(2026 - 07 - 01));
    ///
    /// // Thirteen days on, the customer still has time to change.
    /// assert!(!assessment.may_surcharge(Some(alert), date!(2026 - 07 - 14)));
    /// // Two weeks on, they have had it.
    /// assert!(assessment.may_surcharge(Some(alert), date!(2026 - 07 - 15)));
    /// // No alert is never a surcharge, however long ago the window closed.
    /// assert!(!assessment.may_surcharge(None, date!(2027 - 01 - 01)));
    /// // An alert older than the observation is not an alert about it.
    /// let stale = FairUseAlert::new(date!(2020 - 01 - 01));
    /// assert!(!assessment.may_surcharge(Some(stale), date!(2026 - 07 - 15)));
    /// ```
    #[must_use]
    pub fn may_surcharge(&self, alert: Option<FairUseAlert>, as_of: Date) -> bool {
        self.may_warn()
            && alert.is_some_and(|alert| {
                alert.issued() >= self.window_start && alert.has_elapsed(as_of)
            })
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
///
/// [`observed_days`](FairUseAssessment::observed_days) comes from the
/// **presence series**, not from the window's configured length — see that
/// accessor for why the distinction is what makes
/// [`window_sufficient`](FairUseAssessment::window_sufficient) mean anything.
#[must_use]
pub fn assess(
    usage: &[UsageRecord],
    presence: &[PresenceDay],
    home: PlmnId,
    window: ObservationWindow,
    ending: Date,
    regions: &impl Regions,
) -> FairUseAssessment {
    // A start before the supported calendar covers every representable day.
    let start = window.start(ending).unwrap_or(Date::MIN);

    // Distinct days, not entries: a series that reports a subscriber twice on
    // one day has still observed one day, and counting it twice would tilt a
    // test with legal consequences by however often the feed repeats itself.
    let mut domestic: BTreeSet<Date> = BTreeSet::new();
    let mut roaming: BTreeSet<Date> = BTreeSet::new();
    let mut observed: BTreeSet<Date> = BTreeSet::new();
    for day in presence
        .iter()
        .filter(|d| d.date >= start && d.date <= ending)
    {
        observed.insert(day.date);
        match day.region {
            Region::Domestic => {
                domestic.insert(day.date);
            }
            Region::Eea => {
                roaming.insert(day.date);
            }
            // Outside the regulated area: evidence that the subscriber was
            // observed, but on neither side of the ratio.
            Region::RestOfWorld => {}
        }
    }
    // Article 4(4): "any day when a roaming customer has logged on to the
    // domestic network shall be counted as a day of domestic presence". A day
    // seen both at home and abroad is a domestic day only (finding R1).
    roaming.retain(|day| !domestic.contains(day));
    let (domestic_days, roaming_days) = (domestic.len() as u32, roaming.len() as u32);

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
        observed.len() as u32,
        start,
    )
}

// ── FairUseAlert ─────────────────────────────────────────────────────────────

/// The alert Articles 5(3) and 5(4) require before a fair-use surcharge.
///
/// Two facts, and both are load-bearing: **when** the customer was told, and
/// **how long** they were given. Article 5(4) puts a floor of two weeks under
/// the second, so a provider may be more generous than the regulation and
/// cannot be less.
///
/// Modelled as a value rather than a `bool` because "the customer was warned"
/// is not the question a surcharge turns on. The question is whether the period
/// the warning promised has run out, and only a date answers that.
///
/// ```rust
/// use mediation::FairUseAlert;
/// use time::macros::date;
///
/// let alert = FairUseAlert::new(date!(2026 - 07 - 01));
/// assert_eq!(alert.expires().to_string(), "2026-07-15");
///
/// // A longer period is a provider's to give.
/// assert!(FairUseAlert::with_period(date!(2026 - 07 - 01), 30).is_some());
/// // A shorter one is not the regulation's to allow.
/// assert!(FairUseAlert::with_period(date!(2026 - 07 - 01), 7).is_none());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FairUseAlert {
    issued: Date,
    period_days: u16,
}

impl FairUseAlert {
    /// The floor Article 5(4) puts under the period, in days.
    ///
    /// "A period which cannot be shorter than 2 weeks."
    pub const MINIMUM_PERIOD_DAYS: u16 = 14;

    /// An alert issued on `issued`, giving the regulation's minimum period.
    #[must_use]
    pub const fn new(issued: Date) -> Self {
        Self {
            issued,
            period_days: Self::MINIMUM_PERIOD_DAYS,
        }
    }

    /// An alert giving a longer period than the minimum.
    ///
    /// `None` below [`MINIMUM_PERIOD_DAYS`](Self::MINIMUM_PERIOD_DAYS), which
    /// the regulation does not permit — a shorter period is not a stricter
    /// policy, it is a non-compliant one.
    #[must_use]
    pub const fn with_period(issued: Date, days: u16) -> Option<Self> {
        if days < Self::MINIMUM_PERIOD_DAYS {
            None
        } else {
            Some(Self {
                issued,
                period_days: days,
            })
        }
    }

    /// When the customer was alerted.
    #[must_use]
    pub const fn issued(self) -> Date {
        self.issued
    }

    /// How many days the customer was given.
    #[must_use]
    pub const fn period_days(self) -> u16 {
        self.period_days
    }

    /// The first date on which a surcharge may be applied.
    ///
    /// `None` only where the arithmetic leaves the calendar `Date` supports.
    #[must_use]
    pub fn checked_expiry(self) -> Option<Date> {
        self.issued
            .checked_add(time::Duration::days(i64::from(self.period_days)))
    }

    /// The first date on which a surcharge may be applied.
    ///
    /// # Panics
    ///
    /// Only for an alert issued within a fortnight of the maximum date `time`
    /// supports, which is the year 9999 by default. Use
    /// [`checked_expiry`](Self::checked_expiry) where that is reachable.
    #[must_use]
    pub fn expires(self) -> Date {
        self.checked_expiry()
            .expect("an alert date a fortnight inside the supported calendar")
    }

    /// `true` when the period the alert promised has run out by `as_of`.
    ///
    /// The comparison is on the **expiry date itself**: a period that started on
    /// the first and runs two weeks is over on the fifteenth, not the
    /// fourteenth.
    #[must_use]
    pub fn has_elapsed(self, as_of: Date) -> bool {
        self.checked_expiry().is_some_and(|expiry| as_of >= expiry)
    }
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
    use time::Date;

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
        /// The window's first day, `YYYY-MM-DD`.
        window_start: DateRepr,
    }

    /// A calendar date as `YYYY-MM-DD`.
    struct DateRepr(Date);

    const DATE_FORMAT: &[time::format_description::FormatItem<'static>] =
        time::macros::format_description!("[year]-[month]-[day]");

    impl Serialize for DateRepr {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let text = self
                .0
                .format(DATE_FORMAT)
                .map_err(serde::ser::Error::custom)?;
            serializer.serialize_str(&text)
        }
    }

    impl<'de> Deserialize<'de> for DateRepr {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct V;
            impl Visitor<'_> for V {
                type Value = DateRepr;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("a date such as \"2026-03-29\"")
                }
                fn visit_str<E: de::Error>(self, v: &str) -> Result<DateRepr, E> {
                    Date::parse(v, DATE_FORMAT)
                        .map(DateRepr)
                        .map_err(de::Error::custom)
                }
            }
            deserializer.deserialize_str(V)
        }
    }

    impl Serialize for FairUseAssessment {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            Repr {
                domestic_presence_days: self.domestic_presence_days(),
                roaming_presence_days: self.roaming_presence_days(),
                domestic_consumption: self.domestic_consumption(),
                roaming_consumption: self.roaming_consumption(),
                observed_days: self.observed_days(),
                window_start: DateRepr(self.window_start()),
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
                r.window_start.0,
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

    const START: Date = date!(2026 - 01 - 01);

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
        let by_consumption = FairUseAssessment::from_counts(
            40,
            80,
            Octets::new(9_000),
            Octets::new(1_000),
            120,
            START,
        );
        assert!(!by_consumption.prevailing_domestic_presence());
        assert!(by_consumption.prevailing_domestic_consumption());
        assert!(by_consumption.prevailing_domestic());
        assert!(!by_consumption.may_warn());
        assert_eq!(
            by_consumption.safe_harbour(),
            Some(SafeHarbour::Consumption)
        );

        // Home most days, but consuming mostly abroad.
        let by_presence = FairUseAssessment::from_counts(
            80,
            40,
            Octets::new(1_000),
            Octets::new(9_000),
            120,
            START,
        );
        assert!(by_presence.prevailing_domestic());
        assert!(!by_presence.may_warn());
        assert_eq!(by_presence.safe_harbour(), Some(SafeHarbour::Presence));
    }

    /// Only when both indicators point abroad may the provider act.
    #[test]
    fn a_warning_needs_both_indicators_to_prevail_abroad() {
        let permanent = FairUseAssessment::from_counts(
            20,
            100,
            Octets::new(500),
            Octets::new(9_500),
            120,
            START,
        );
        assert!(!permanent.prevailing_domestic());
        assert!(permanent.may_warn());
        assert_eq!(permanent.safe_harbour(), None);
    }

    /// The regulation's sequence is observe, alert, wait, then surcharge — and
    /// none of the four steps is skippable.
    #[test]
    fn a_surcharge_requires_an_alert_that_has_run_its_course() {
        let permanent = FairUseAssessment::from_counts(
            20,
            100,
            Octets::new(500),
            Octets::new(9_500),
            120,
            START,
        );
        assert!(permanent.may_warn());

        let day = date!(2026 - 07 - 01);
        assert!(
            !permanent.may_surcharge(None, day),
            "no alert, no surcharge"
        );

        // Article 5(4): at least two weeks to change the usage pattern. The day
        // the alert goes out is not the day a surcharge may start.
        let alert = FairUseAlert::new(day);
        assert!(!permanent.may_surcharge(Some(alert), day));
        assert!(!permanent.may_surcharge(Some(alert), date!(2026 - 07 - 14)));
        assert!(permanent.may_surcharge(Some(alert), date!(2026 - 07 - 15)));

        // A provider may give longer, and is then held to it.
        let generous = FairUseAlert::with_period(day, 30).expect("above the floor");
        assert!(!permanent.may_surcharge(Some(generous), date!(2026 - 07 - 15)));
        assert!(permanent.may_surcharge(Some(generous), date!(2026 - 07 - 31)));

        // And a customer inside the safe harbour cannot be surcharged however
        // long ago somebody alerted them.
        let lawful = FairUseAssessment::from_counts(
            80,
            40,
            Octets::new(9_000),
            Octets::new(1_000),
            120,
            START,
        );
        assert!(!lawful.may_surcharge(Some(alert), date!(2027 - 01 - 01)));
    }

    /// The floor is the regulation's, not a policy knob.
    #[test]
    fn an_alert_shorter_than_a_fortnight_is_not_constructible() {
        let day = date!(2026 - 07 - 01);
        assert!(FairUseAlert::with_period(day, 13).is_none());
        assert_eq!(
            FairUseAlert::with_period(day, 14),
            Some(FairUseAlert::new(day))
        );
        assert_eq!(FairUseAlert::new(day).expires(), date!(2026 - 07 - 15));
    }

    /// Ties resolve in the customer's favour: an equal split does not establish
    /// the abuse the provider would have to show.
    #[test]
    fn an_equal_split_is_not_prevailing_roaming() {
        let tied =
            FairUseAssessment::from_counts(60, 60, Octets::new(500), Octets::new(500), 120, START);
        assert!(tied.prevailing_domestic_presence());
        assert!(tied.prevailing_domestic_consumption());
        assert!(!tied.may_warn());
        assert_eq!(tied.safe_harbour(), Some(SafeHarbour::BothIndicators));
    }

    /// A window shorter than four months cannot support any conclusion.
    #[test]
    fn a_short_window_blocks_every_conclusion() {
        let short =
            FairUseAssessment::from_counts(10, 80, Octets::new(100), Octets::new(9_900), 90, START);
        assert!(!short.window_sufficient());
        assert!(!short.may_warn());
        assert!(!short.may_surcharge(
            Some(FairUseAlert::new(date!(2026 - 01 - 01))),
            date!(2026 - 07 - 28)
        ));
        assert_eq!(short.safe_harbour(), Some(SafeHarbour::WindowTooShort));

        assert_eq!(ObservationWindow::months(3), None);
        assert_eq!(ObservationWindow::minimum().month_count(), 4);
        assert!(ObservationWindow::months(6).is_some());
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
        let mut day = window.start(ending).unwrap();
        for index in 0..120 {
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
