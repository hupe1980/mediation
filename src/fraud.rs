//! Fraud signals: evidence, never verdicts.
//!
//! Mediation is where fraud is visible — NRTRDE exists because a roaming
//! extract reaching the home network within four hours is the difference
//! between catching a fraud and paying for it. So the signals belong here.
//!
//! # What this module will not do
//!
//! **It does not score.** A single number implies a calibration this crate
//! cannot have: the weight of an indicator depends on an operator's traffic
//! mix, their partners and their tariffs, none of which a library knows.
//! Detectors return the measurements that fired, and a caller weighs them.
//!
//! **It does not decide.** Suspending a subscriber has legal weight. What is
//! returned is [`Indication`] — the observation and the numbers behind it — so
//! an analyst can see *why* rather than being told *what*.
//!
//! **It ships no lists.** High-risk destination ranges change weekly and are
//! commercial data. They arrive as a [`NumberRangeSet`],
//! exactly as the E.212 table and the EEA membership list do.
//!
//! # What CDR-based detection cannot catch
//!
//! Worth stating plainly, because the industry rarely does. Modern SIM boxes
//! run *human behaviour simulation*: they generate incoming calls, vary call
//! durations, spread contacts and move SIMs between gateways in different
//! cells so the network records human-looking mobility at no cost to the
//! fraudster. Published research finds these evade CDR-based detection with a
//! high success rate.
//!
//! [`bypass_signature`] therefore catches **unsophisticated** SIM boxes and is
//! defeated by simulated ones. It is a triage signal for an analyst's queue,
//! not a control. Anything stronger needs signal-level data — latency at the
//! cellular edge, radio measurements — that never reaches a charging record.
//!
//! ```rust
//! use mediation::{Signal, UniformTransit, impossible_travel};
//! # use mediation::{ChargedParty, RecordType, SourceRef, SubscriberRef, UsageRecord};
//! # use time::Duration;
//! # fn at(opened: &str, serving: &str) -> UsageRecord {
//! #     UsageRecord::new(
//! #         RecordType::OriginatedCall,
//! #         ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
//! #         opened.parse().unwrap(),
//! #         SourceRef::new("msc-01".parse().unwrap(), 0),
//! #     ).with_networks(serving.parse().unwrap(), "262-02".parse().unwrap())
//! # }
//! // Germany, then the United States, twenty minutes apart.
//! let records = vec![
//!     at("2026-07-28T10:00:00+02:00", "262-02"),
//!     at("2026-07-28T10:20:00+02:00", "310-260"),
//! ];
//!
//! let found = impossible_travel(&records, &UniformTransit::new(Duration::hours(4)));
//! assert_eq!(found.len(), 1);
//! assert_eq!(found[0].signal, Signal::ImpossibleTravel);
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use time::Duration;

use crate::cdrtime::{CdrTimestamp, Seconds};
use crate::numbering::NumberRange;
use crate::numbering::NumberRangeSet;
use crate::party::ChargedParty;
use crate::plmn::Mcc;
use crate::record::{RecordType, UsageRecord};

// ── Signal ───────────────────────────────────────────────────────────────────

/// A named fraud indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Signal {
    /// Two records place the subscriber in different countries too soon.
    ImpossibleTravel,
    /// Traffic to a destination range the caller flagged as high risk.
    ///
    /// The shape of International Revenue Share Fraud: traffic is generated to
    /// number ranges whose holder shares the termination revenue with the
    /// fraudster.
    PremiumDestination,
    /// Many short inbound calls, of the kind meant to provoke a callback.
    ///
    /// The Wangiri pattern — one ring and cut, so the victim calls back to a
    /// premium number.
    WangiriPattern,
    /// Traffic shaped like interconnect bypass rather than a person.
    ///
    /// See the module documentation: this catches unsophisticated SIM boxes
    /// and is defeated by ones that simulate human behaviour.
    BypassSignature,
    /// Message traffic concentrated on few destination ranges.
    ///
    /// The shape of artificially inflated traffic in A2P messaging.
    AitSignature,
}

impl Signal {
    /// Every variant, for exhaustive iteration in tests and mappings.
    pub const ALL: &'static [Self] = &[
        Self::ImpossibleTravel,
        Self::PremiumDestination,
        Self::WangiriPattern,
        Self::BypassSignature,
        Self::AitSignature,
    ];

    /// The canonical code of each variant, in [`ALL`](Self::ALL) order.
    pub const CODES: &'static [&'static str] = &[
        "IMPOSSIBLE_TRAVEL",
        "PREMIUM_DESTINATION",
        "WANGIRI_PATTERN",
        "BYPASS_SIGNATURE",
        "AIT_SIGNATURE",
    ];

    /// `true` when the signal is evidence of an anomaly in the data rather than
    /// of a behaviour pattern.
    ///
    /// An impossible journey is arithmetically impossible; a traffic shape is a
    /// judgement. The first justifies acting on one record, the second never
    /// does.
    #[must_use]
    pub const fn is_arithmetic(self) -> bool {
        matches!(self, Self::ImpossibleTravel)
    }

    /// The canonical string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ImpossibleTravel => "IMPOSSIBLE_TRAVEL",
            Self::PremiumDestination => "PREMIUM_DESTINATION",
            Self::WangiriPattern => "WANGIRI_PATTERN",
            Self::BypassSignature => "BYPASS_SIGNATURE",
            Self::AitSignature => "AIT_SIGNATURE",
        }
    }
}

impl fmt::Display for Signal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── Indication ───────────────────────────────────────────────────────────────

/// One signal firing, with the measurements behind it.
///
/// Carries evidence rather than a score, so an analyst sees why. Records are
/// identified by index into the caller's slice, as everywhere else in this
/// crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Indication {
    /// Which signal fired.
    pub signal: Signal,
    /// The records that produced it, ascending.
    pub records: Vec<usize>,
    /// What was measured, for a human to read.
    pub detail: String,
}

impl Indication {
    /// An indication over the given records.
    #[must_use]
    pub fn new(signal: Signal, records: Vec<usize>, detail: impl Into<String>) -> Self {
        Self {
            signal,
            records,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for Indication {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} over {} record(s): {}",
            self.signal,
            self.records.len(),
            self.detail
        )
    }
}

// ── Impossible travel ────────────────────────────────────────────────────────

/// The shortest plausible journey between two countries.
///
/// A physical claim, not a threshold to tune — and one this crate will not
/// make on a caller's behalf, for the same reason it ships no E.212 table and
/// no EEA membership list. Four hours is a sensible floor for an
/// intercontinental hop and absurd for Germany to Austria, where a train ride
/// is ninety minutes and a flight is fifty. One number for every pair therefore
/// either misses real fraud or accuses real travellers, depending which way it
/// is set.
///
/// Returning `None` means *unknown*, and an unknown pair **never fires**. A
/// fraud signal raised out of ignorance costs an analyst the same time as a
/// real one and teaches them to discount the next.
///
/// Implement it over whatever you have — a matrix of great-circle distances, a
/// timetable, a flat rule for pairs you have not modelled. [`UniformTransit`]
/// is the flat rule, spelled out.
pub trait TransitTimes {
    /// The shortest time in which a subscriber could get from `from` to `to`,
    /// or `None` when the pair is unknown.
    fn minimum_transit(&self, from: Mcc, to: Mcc) -> Option<Duration>;
}

/// One minimum for every pair of countries.
///
/// The simplest possible [`TransitTimes`], and the right starting point: it
/// catches the intercontinental cases that need no geography at all, and says
/// so in the type rather than in a comment beside a constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UniformTransit(Duration);

impl UniformTransit {
    /// A flat minimum for every pair.
    #[must_use]
    pub const fn new(minimum: Duration) -> Self {
        Self(minimum)
    }

    /// The minimum this rule applies.
    #[must_use]
    pub const fn get(self) -> Duration {
        self.0
    }
}

impl TransitTimes for UniformTransit {
    fn minimum_transit(&self, from: Mcc, to: Mcc) -> Option<Duration> {
        // Same country is not a journey; the detector never asks, but a caller
        // reading this trait directly might.
        if from.same_country(to) {
            None
        } else {
            Some(self.0)
        }
    }
}

impl<F> TransitTimes for F
where
    F: Fn(Mcc, Mcc) -> Option<Duration>,
{
    fn minimum_transit(&self, from: Mcc, to: Mcc) -> Option<Duration> {
        self(from, to)
    }
}

/// One record's contribution to a subscriber's movement: where they were, when
/// they got there, and when they were last demonstrably still there.
struct Sighting {
    index: usize,
    country: Mcc,
    arrived: CdrTimestamp,
    departed: CdrTimestamp,
}

/// Two records placing the subscriber in different countries sooner than any
/// journey allows.
///
/// Compares **countries**, not cells: a country change is a lower bound on the
/// distance travelled that needs no geography, no coordinates and no table.
/// Two records from adjacent cells minutes apart are ordinary; two from
/// Germany and the United States twenty minutes apart are not. Countries are
/// compared with [`Mcc::same_country`], so a move between two MCCs of one
/// country (US 310 and 311) is no journey (finding R14).
///
/// The minimum comes from a caller-supplied [`TransitTimes`], per country pair
/// — see that trait for why one global number is the wrong shape. Pass
/// [`UniformTransit`] for the flat rule.
///
/// Records are compared in **instant order** whatever order they arrive in, so
/// a shuffled input does not manufacture journeys — and **per charged party**,
/// so a whole batch can be passed without every subscriber's country change
/// being read against every other's.
///
/// Only adjacent records in time are compared. That finds every impossible
/// pair only when the transit table answers every pair and obeys the triangle
/// inequality, as [`UniformTransit`] does. With unknown pairs or a table where
/// a detour is faster than the direct route, a non-adjacent impossible pair can
/// go unreported (finding R16).
///
/// # Travel time runs from the first record's **close**
///
/// The subscriber was demonstrably in the first country until that record
/// closed, so that is when the journey could begin. Measuring from the record's
/// *open* instant instead grants a four-hour call four extra hours of travel
/// time — and hides the clearest evidence there is, two sessions that overlap
/// in two countries, which is a cloned or shared SIM rather than an
/// improbably fast journey. Overlapping sessions say so in the detail line.
///
/// ```rust
/// use mediation::{UniformTransit, impossible_travel};
/// use time::Duration;
/// # use mediation::{ChargedParty, RecordType, SourceRef, SubscriberRef, UsageRecord};
/// # fn at(opened: &str, serving: &str) -> UsageRecord {
/// #     UsageRecord::new(
/// #         RecordType::OriginatedCall,
/// #         ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
/// #         opened.parse().unwrap(),
/// #         SourceRef::new("msc-01".parse().unwrap(), 0),
/// #     ).with_networks(serving.parse().unwrap(), "262-02".parse().unwrap())
/// # }
/// let records = vec![
///     at("2026-07-28T10:00:00+02:00", "262-02"), // Germany
///     at("2026-07-28T10:20:00+02:00", "310-260"), // United States
/// ];
///
/// let found = impossible_travel(&records, &UniformTransit::new(Duration::hours(4)));
/// assert_eq!(found.len(), 1);
/// ```
#[must_use]
pub fn impossible_travel(records: &[UsageRecord], transit: &impl TransitTimes) -> Vec<Indication> {
    // Three fields per record: when it began, when the subscriber was last
    // demonstrably still in that country, and which country that was.
    let mut by_party: BTreeMap<&ChargedParty, Vec<Sighting>> = BTreeMap::new();
    for (index, record) in records.iter().enumerate() {
        if let Some(plmn) = record.serving_plmn {
            by_party.entry(&record.party).or_default().push(Sighting {
                index,
                country: plmn.mcc(),
                arrived: record.opened,
                // The subscriber was in this country until the record closed,
                // not until it opened. Measuring departure from the *open*
                // instant grants a four-hour call four extra hours of travel
                // time and lets the clearest evidence there is — two sessions
                // overlapping in two countries — pass as ordinary movement.
                departed: record.closed.unwrap_or(record.opened),
            });
        }
    }

    let mut found = Vec::new();
    for placed in by_party.values_mut() {
        placed.sort_by(|a, b| CdrTimestamp::cmp_instant(&a.arrived, &b.arrived));
        for pair in placed.windows(2) {
            let (first, second) = (&pair[0], &pair[1]);
            let (first_index, first_country) = (first.index, first.country);
            let (second_index, second_country) = (second.index, second.country);
            if first_country.same_country(second_country) {
                continue;
            }
            // An unmodelled pair is not evidence of anything.
            let Some(minimum) = transit.minimum_transit(first_country, second_country) else {
                continue;
            };
            let elapsed = second.arrived.to_utc() - first.departed.to_utc();
            if elapsed >= minimum {
                continue;
            }
            let mut involved = [first_index, second_index];
            involved.sort_unstable();
            found.push(Indication::new(
                Signal::ImpossibleTravel,
                involved.to_vec(),
                if elapsed.is_negative() {
                    // The sessions overlap: the subscriber was in two countries
                    // at once. Worth saying so, because no travel time explains
                    // it and the shape is a cloned or shared SIM rather than an
                    // improbably fast journey.
                    format!(
                        "{first_country} and {second_country} overlap by {}",
                        -elapsed
                    )
                } else {
                    format!("{first_country} to {second_country} in {elapsed}")
                },
            ));
        }
    }
    found
}

// ── Premium destination ──────────────────────────────────────────────────────

/// Traffic to destination ranges the caller flagged as high risk.
///
/// The list is supplied because it is commercial data that changes weekly —
/// International Premium Rate Number ranges are advertised to fraudsters and
/// withdrawn continuously, and a vendored list would be stale on arrival.
///
/// Reads each record's own [`called`](UsageRecord::called) destination. A record
/// without one — or with a short code, which no international range contains —
/// is skipped: an unresolved destination is not evidence of anything.
#[must_use]
pub fn premium_destination<T>(
    records: &[UsageRecord],
    high_risk: &NumberRangeSet<T>,
) -> Vec<Indication> {
    records
        .iter()
        .enumerate()
        .filter_map(|(index, record)| {
            let called = record.called?.as_number()?;
            let (range, _) = high_risk.longest_match(called)?;
            Some(Indication::new(
                Signal::PremiumDestination,
                vec![index],
                format!("destination in flagged range {range}"),
            ))
        })
        .collect()
}

// ── Wangiri ──────────────────────────────────────────────────────────────────

/// Many short inbound calls to one subscriber, of the kind meant to provoke a
/// callback.
///
/// The Wangiri pattern: one ring and cut, so the victim calls back to a premium
/// number. What identifies it is not any single call — a genuine caller who
/// hangs up looks identical — but the **volume of them**, so this fires only
/// when `min_calls` short inbound calls reach **one charged party within
/// `within`** of each other (finding R15). Calls to different subscribers, or
/// spread over months, are not a wave.
///
/// `ring_ceiling` is how short counts as short, supplied by the caller: a
/// network with slow setup legitimately produces more brief calls than one
/// without. `within` is measured from the first call's open instant to the
/// last's, inclusive; a non-positive `within` finds nothing.
///
/// One indication per charged party that fires, naming the busiest window —
/// the first one reached on a tie — in instant order.
///
/// A record with **no** duration is not counted. An unrecorded duration is not
/// a short one, and reading it as zero would make every unmetered record part
/// of the pattern.
#[must_use]
pub fn wangiri_pattern(
    records: &[UsageRecord],
    ring_ceiling: Seconds,
    min_calls: usize,
    within: Duration,
) -> Vec<Indication> {
    if min_calls == 0 || within.is_negative() || within.is_zero() {
        return Vec::new();
    }
    let mut by_party: BTreeMap<&ChargedParty, Vec<(CdrTimestamp, usize)>> = BTreeMap::new();
    for (index, record) in records.iter().enumerate() {
        if record.record_type == RecordType::TerminatedCall
            && record
                .duration
                .is_some_and(|duration| duration <= ring_ceiling)
        {
            by_party
                .entry(&record.party)
                .or_default()
                .push((record.opened, index));
        }
    }

    let mut found = Vec::new();
    for calls in by_party.values_mut() {
        calls.sort_by(|a, b| CdrTimestamp::cmp_instant(&a.0, &b.0).then(a.1.cmp(&b.1)));
        // The widest run of calls whose first and last lie within `within`.
        let (mut best_start, mut best_len, mut first) = (0usize, 0usize, 0usize);
        for last in 0..calls.len() {
            while calls[last].0.to_utc() - calls[first].0.to_utc() > within {
                first += 1;
            }
            if last + 1 - first > best_len {
                (best_start, best_len) = (first, last + 1 - first);
            }
        }
        if best_len < min_calls {
            continue;
        }
        let window = &calls[best_start..best_start + best_len];
        let mut involved: Vec<usize> = window.iter().map(|(_, index)| *index).collect();
        involved.sort_unstable();
        found.push(Indication::new(
            Signal::WangiriPattern,
            involved,
            format!("{best_len} inbound call(s) of at most {ring_ceiling} s within {within}"),
        ));
    }
    found
}

// ── Bypass ───────────────────────────────────────────────────────────────────

/// How a subscriber's traffic is shaped, for bypass triage.
///
/// Every field is a count, so a caller can see the shape rather than a verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TrafficShape {
    /// Calls the subscriber placed.
    pub outbound_calls: u64,
    /// Calls the subscriber received.
    pub inbound_calls: u64,
    /// Distinct serving networks the subscriber appeared on.
    pub distinct_networks: u64,
}

impl TrafficShape {
    /// The shape of a subscriber's records.
    ///
    /// Counts calls only. Data and messaging records say nothing about the
    /// direction of voice traffic, and folding them in would move the ratio
    /// without telling anyone anything.
    #[must_use]
    pub fn of(records: &[UsageRecord]) -> Self {
        let mut shape = Self::default();
        let mut networks = BTreeSet::new();
        for record in records {
            match record.record_type {
                RecordType::OriginatedCall => shape.outbound_calls += 1,
                RecordType::TerminatedCall => shape.inbound_calls += 1,
                _ => continue,
            }
            if let Some(plmn) = record.serving_plmn {
                networks.insert(plmn);
            }
        }
        shape.distinct_networks = networks.len() as u64;
        shape
    }

    /// Calls in either direction.
    ///
    /// Derived rather than stored: a second copy of a sum eventually disagrees
    /// with the parts, and nothing can then say which is right.
    #[must_use]
    pub const fn calls(self) -> u64 {
        self.outbound_calls + self.inbound_calls
    }

    /// The share of calls that were outbound, as a whole percentage.
    ///
    /// `None` when there were no calls: zero of zero is not zero percent.
    /// Computed in integers, multiplying before dividing, so two calls of three
    /// is 66 and not 0.
    #[must_use]
    pub const fn outbound_percent(self) -> Option<u64> {
        match self.calls() {
            0 => None,
            calls => Some(self.outbound_calls * 100 / calls),
        }
    }
}

/// Traffic shaped like interconnect bypass rather than like a person.
///
/// Fires when a subscriber places far more calls than they receive **and**
/// stays on one network — the shape of a SIM sitting in a rack terminating
/// international traffic.
///
/// # This is a triage signal, not a control
///
/// Modern SIM boxes simulate human behaviour: they generate incoming calls,
/// vary durations, and move SIMs between gateways in different cells so the
/// network records human-looking mobility. Published research finds they evade
/// CDR-based detection at a high rate. What this catches is the
/// unsophisticated case, and an operator relying on it alone will conclude they
/// have no bypass problem.
///
/// `min_calls` avoids firing on a handful of records, and
/// `outbound_ratio_percent` is the share of calls that must be outbound.
#[must_use]
pub fn bypass_signature(
    records: &[UsageRecord],
    min_calls: u64,
    outbound_ratio_percent: u64,
) -> Option<Indication> {
    let shape = TrafficShape::of(records);
    let ratio = shape.outbound_percent()?;
    if shape.calls() < min_calls || shape.calls() == 0 {
        return None;
    }
    if ratio < outbound_ratio_percent || shape.distinct_networks > 1 {
        return None;
    }
    // The calls, not every record in the slice: pointing an analyst at a data
    // session that had no bearing on the ratio wastes the one thing an
    // indication is for.
    let involved: Vec<usize> = records
        .iter()
        .enumerate()
        .filter(|(_, record)| {
            matches!(
                record.record_type,
                RecordType::OriginatedCall | RecordType::TerminatedCall
            )
        })
        .map(|(index, _)| index)
        .collect();
    Some(Indication::new(
        Signal::BypassSignature,
        involved,
        format!(
            "{ratio}% of {} call(s) outbound on {} network(s)",
            shape.calls(),
            shape.distinct_networks
        ),
    ))
}

// ── Artificially inflated traffic ────────────────────────────────────────────

/// Message traffic concentrated on a few destination ranges.
///
/// The shape of artificially inflated traffic in A2P messaging: bots trigger
/// one-time-password floods to attacker-controlled numbers, which cluster into
/// a small number of ranges because that is where the revenue share is.
///
/// Fires when the top range accounts for at least `concentration_percent` of a
/// sender's messages over at least `min_messages`. Both are supplied: a
/// legitimate sender messaging one country looks concentrated too, and only the
/// operator knows which of their senders those are.
///
/// The denominator is **every** message, including those whose destination
/// resolved to no range at all — a sender whose traffic is half unmatched is not
/// concentrated, and dividing only by the matched ones would say it was.
///
/// Counts only [`RecordType::SmsOriginated`] records (finding R17) and reads
/// each message's own [`called`](UsageRecord::called) destination.
#[must_use]
pub fn ait_signature<T>(
    records: &[UsageRecord],
    ranges: &NumberRangeSet<T>,
    min_messages: usize,
    concentration_percent: u64,
) -> Option<Indication> {
    let mut by_range: BTreeMap<NumberRange, Vec<usize>> = BTreeMap::new();
    let mut messages = 0usize;

    for (index, record) in records.iter().enumerate() {
        // Inflated traffic is what a sender originates. A terminated message
        // is somebody else's traffic, and counting it would dilute or inflate
        // the sender's share (finding R17).
        if record.record_type != RecordType::SmsOriginated {
            continue;
        }
        messages += 1;
        if let Some(called) = record.called.and_then(|d| d.as_number())
            && let Some((range, _)) = ranges.longest_match(called)
        {
            by_range.entry(range).or_default().push(index);
        }
    }

    if messages < min_messages || messages == 0 {
        return None;
    }
    // Ties break on the range that sorts last, which is arbitrary but stable:
    // the map is ordered, so the same input always names the same range.
    let (range, involved) = by_range.into_iter().max_by_key(|(_, v)| v.len())?;
    // u128 first: at u64 message counts the multiplication would wrap, and a
    // wrapped share reads as a low one.
    let share = (involved.len() as u128 * 100 / messages as u128) as u64;
    if share < concentration_percent {
        return None;
    }
    let count = involved.len();
    Some(Indication::new(
        Signal::AitSignature,
        involved,
        format!("{count} of {messages} message(s) ({share}%) to range {range}"),
    ))
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::{Indication, Signal, TrafficShape};
    use serde::de::{self, Visitor};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::fmt;

    impl Serialize for Signal {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_str(self.as_str())
        }
    }

    impl<'de> Deserialize<'de> for Signal {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct V;
            impl Visitor<'_> for V {
                type Value = Signal;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("a fraud signal code")
                }
                fn visit_str<E: de::Error>(self, v: &str) -> Result<Signal, E> {
                    Signal::ALL
                        .iter()
                        .find(|s| s.as_str() == v)
                        .copied()
                        .ok_or_else(|| de::Error::custom(format!("unknown signal {v:?}")))
                }
            }
            deserializer.deserialize_str(V)
        }
    }

    /// Field names are part of the public representation and covered by semver.
    #[derive(Serialize, Deserialize)]
    #[serde(rename = "Indication")]
    struct IndicationRepr {
        signal: Signal,
        records: Vec<usize>,
        detail: String,
    }

    impl Serialize for Indication {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            IndicationRepr {
                signal: self.signal,
                records: self.records.clone(),
                detail: self.detail.clone(),
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for Indication {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = IndicationRepr::deserialize(deserializer)?;
            Ok(Indication::new(r.signal, r.records, r.detail))
        }
    }

    #[derive(Serialize, Deserialize)]
    #[serde(rename = "TrafficShape")]
    struct ShapeRepr {
        outbound_calls: u64,
        inbound_calls: u64,
        distinct_networks: u64,
    }

    impl Serialize for TrafficShape {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            ShapeRepr {
                outbound_calls: self.outbound_calls,
                inbound_calls: self.inbound_calls,
                distinct_networks: self.distinct_networks,
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for TrafficShape {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = ShapeRepr::deserialize(deserializer)?;
            Ok(TrafficShape {
                outbound_calls: r.outbound_calls,
                inbound_calls: r.inbound_calls,
                distinct_networks: r.distinct_networks,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Msisdn;
    use crate::numbering::NumberRange;
    use crate::party::SubscriberRef;
    use crate::record::SourceRef;

    /// Gives each record the destination `called` resolves for it.
    fn resolve(
        records: &[UsageRecord],
        called: &impl Fn(usize, &UsageRecord) -> Option<Msisdn>,
    ) -> Vec<UsageRecord> {
        records
            .iter()
            .enumerate()
            .map(|(i, r)| match called(i, r) {
                Some(n) => r.clone().with_called(n.into()),
                None => r.clone(),
            })
            .collect()
    }

    fn call(kind: RecordType, opened: &str, serving: &str) -> UsageRecord {
        UsageRecord::new(
            kind,
            ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
            opened.parse().unwrap(),
            SourceRef::new("msc-01".parse().unwrap(), 0),
        )
        .with_networks(serving.parse().unwrap(), "262-02".parse().unwrap())
    }

    fn subscriber(imsi: &str) -> ChargedParty {
        ChargedParty::Subscriber(SubscriberRef::new(imsi.parse().unwrap()))
    }

    fn high_risk() -> NumberRangeSet<&'static str> {
        let mut set = NumberRangeSet::new();
        set.insert("88216".parse::<NumberRange>().unwrap(), "satellite");
        set.insert("23928".parse::<NumberRange>().unwrap(), "flagged");
        set
    }

    // ── Impossible travel ────────────────────────────────────────────────────

    /// A country change is a lower bound on distance that needs no geography.
    #[test]
    fn two_countries_too_close_together_is_arithmetically_impossible() {
        let records = vec![
            call(
                RecordType::OriginatedCall,
                "2026-07-28T10:00:00+02:00",
                "262-02",
            ),
            call(
                RecordType::OriginatedCall,
                "2026-07-28T10:20:00+02:00",
                "310-260",
            ),
        ];

        let found = impossible_travel(&records, &UniformTransit::new(Duration::hours(4)));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].signal, Signal::ImpossibleTravel);
        assert_eq!(found[0].records, vec![0, 1]);
        assert!(Signal::ImpossibleTravel.is_arithmetic());
    }

    #[test]
    fn a_plausible_journey_does_not_fire() {
        let records = vec![
            call(
                RecordType::OriginatedCall,
                "2026-07-28T02:00:00+02:00",
                "262-02",
            ),
            call(
                RecordType::OriginatedCall,
                "2026-07-28T14:00:00+02:00",
                "310-260",
            ),
        ];
        assert!(impossible_travel(&records, &UniformTransit::new(Duration::hours(4))).is_empty());
    }

    /// Two cells in one country minutes apart are ordinary, whatever the
    /// networks.
    #[test]
    fn staying_in_one_country_never_fires() {
        let records = vec![
            call(
                RecordType::OriginatedCall,
                "2026-07-28T10:00:00+02:00",
                "262-02",
            ),
            call(
                RecordType::OriginatedCall,
                "2026-07-28T10:01:00+02:00",
                "262-01",
            ),
        ];
        assert!(impossible_travel(&records, &UniformTransit::new(Duration::hours(4))).is_empty());
    }

    /// Two subscribers in two countries are two subscribers. Reading a whole
    /// batch as one itinerary would fire on nearly every roaming file there is.
    #[test]
    fn two_subscribers_are_not_one_journey() {
        let mut here = call(
            RecordType::OriginatedCall,
            "2026-07-28T10:00:00+02:00",
            "262-02",
        );
        here.party = subscriber("imsi-262021111111111");
        let mut there = call(
            RecordType::OriginatedCall,
            "2026-07-28T10:20:00+02:00",
            "310-260",
        );
        there.party = subscriber("imsi-262022222222222");

        assert!(
            impossible_travel(
                &[here.clone(), there.clone()],
                &UniformTransit::new(Duration::hours(4))
            )
            .is_empty(),
            "different parties are never compared against each other"
        );

        // The same two records charged to one subscriber do fire.
        there.party = here.party.clone();
        let found = impossible_travel(&[here, there], &UniformTransit::new(Duration::hours(4)));
        assert_eq!(found.len(), 1);
    }

    /// One subscriber's impossible journey must still be found when other
    /// subscribers' records are interleaved with it in time.
    #[test]
    fn interleaved_parties_do_not_hide_a_journey() {
        let mut records = Vec::new();
        for minute in 0..6 {
            let mut other = call(
                RecordType::OriginatedCall,
                &format!("2026-07-28T10:0{minute}:00+02:00"),
                "440-10",
            );
            other.party = subscriber("imsi-262029999999999");
            records.push(other);
        }
        records.insert(
            0,
            call(
                RecordType::OriginatedCall,
                "2026-07-28T10:00:00+02:00",
                "262-02",
            ),
        );
        records.push(call(
            RecordType::OriginatedCall,
            "2026-07-28T10:20:00+02:00",
            "310-260",
        ));

        let found = impossible_travel(&records, &UniformTransit::new(Duration::hours(4)));
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].records, vec![0, 7]);
    }

    /// A shuffled input must not manufacture journeys: comparison is in
    /// instant order whatever order the records arrive in.
    #[test]
    fn detection_is_order_independent() {
        let a = call(
            RecordType::OriginatedCall,
            "2026-07-28T10:00:00+02:00",
            "262-02",
        );
        let b = call(
            RecordType::OriginatedCall,
            "2026-07-28T10:20:00+02:00",
            "310-260",
        );

        let forward = impossible_travel(
            &[a.clone(), b.clone()],
            &UniformTransit::new(Duration::hours(4)),
        );
        let reversed = impossible_travel(&[b, a], &UniformTransit::new(Duration::hours(4)));
        assert_eq!(forward.len(), reversed.len());
        // The indices refer to each caller's own slice, so they differ; the
        // finding does not.
        assert_eq!(forward[0].signal, reversed[0].signal);
        assert_eq!(forward[0].records.len(), 2);
    }

    #[test]
    fn records_without_a_serving_network_are_skipped() {
        let mut unplaced = call(
            RecordType::OriginatedCall,
            "2026-07-28T10:10:00+02:00",
            "310-260",
        );
        unplaced.serving_plmn = None;

        let records = vec![
            call(
                RecordType::OriginatedCall,
                "2026-07-28T10:00:00+02:00",
                "262-02",
            ),
            unplaced,
            call(
                RecordType::OriginatedCall,
                "2026-07-28T18:00:00+02:00",
                "262-02",
            ),
        ];
        assert!(impossible_travel(&records, &UniformTransit::new(Duration::hours(4))).is_empty());
    }

    // ── Premium destination ──────────────────────────────────────────────────

    #[test]
    fn traffic_to_a_flagged_range_is_reported() {
        let records = vec![
            call(
                RecordType::OriginatedCall,
                "2026-07-28T10:00:00+02:00",
                "262-02",
            ),
            call(
                RecordType::OriginatedCall,
                "2026-07-28T11:00:00+02:00",
                "262-02",
            ),
        ];
        let called = |index: usize, _: &UsageRecord| -> Option<Msisdn> {
            if index == 0 {
                "882161234567".parse().ok()
            } else {
                "491701234567".parse().ok()
            }
        };

        let found = premium_destination(&resolve(&records, &called), &high_risk());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].records, vec![0]);
        assert!(found[0].detail.contains("88216"), "{}", found[0].detail);
        assert!(!Signal::PremiumDestination.is_arithmetic());
    }

    // ── Wangiri ──────────────────────────────────────────────────────────────

    /// A single brief call is indistinguishable from a caller who hung up; the
    /// volume is what identifies the pattern.
    #[test]
    fn one_short_call_is_not_a_pattern() {
        let one = vec![
            call(
                RecordType::TerminatedCall,
                "2026-07-28T10:00:00+02:00",
                "262-02",
            )
            .with_duration(Seconds::new(2)),
        ];
        assert!(wangiri_pattern(&one, Seconds::new(5), 10, Duration::hours(1)).is_empty());

        let many: Vec<UsageRecord> = (0..12)
            .map(|i| {
                call(
                    RecordType::TerminatedCall,
                    &format!("2026-07-28T10:{i:02}:00+02:00"),
                    "262-02",
                )
                .with_duration(Seconds::new(2))
            })
            .collect();
        let found = wangiri_pattern(&many, Seconds::new(5), 10, Duration::hours(1));
        assert_eq!(found.len(), 1, "twelve short inbound");
        let found = &found[0];
        assert_eq!(found.signal, Signal::WangiriPattern);
        assert_eq!(found.records.len(), 12);
    }

    /// Outbound calls are not the pattern, however short.
    #[test]
    fn outbound_calls_are_not_wangiri() {
        let outbound: Vec<UsageRecord> = (0..20)
            .map(|i| {
                call(
                    RecordType::OriginatedCall,
                    &format!("2026-07-28T10:{i:02}:00+02:00"),
                    "262-02",
                )
                .with_duration(Seconds::new(1))
            })
            .collect();
        assert!(wangiri_pattern(&outbound, Seconds::new(5), 10, Duration::hours(1)).is_empty());
    }

    // ── Bypass ───────────────────────────────────────────────────────────────

    /// The unsophisticated case: almost all outbound, one network.
    #[test]
    fn a_rack_of_sims_terminating_traffic_shows_a_shape() {
        let records: Vec<UsageRecord> = (0..50)
            .map(|i| {
                call(
                    RecordType::OriginatedCall,
                    &format!("2026-07-28T{:02}:00:00+02:00", i % 24),
                    "262-02",
                )
            })
            .collect();

        let shape = TrafficShape::of(&records);
        assert_eq!(shape.outbound_calls, 50);
        assert_eq!(shape.inbound_calls, 0);
        assert_eq!(shape.distinct_networks, 1);
        assert_eq!(shape.calls(), 50);
        assert_eq!(shape.outbound_percent(), Some(100));

        let found = bypass_signature(&records, 20, 90).unwrap();
        assert_eq!(found.signal, Signal::BypassSignature);
        assert!(found.detail.contains("100%"), "{}", found.detail);
    }

    /// A subscriber who receives calls, or moves between networks, does not
    /// match — which is exactly how a behaviour-simulating SIM box escapes.
    #[test]
    fn simulated_human_behaviour_defeats_the_signature() {
        // Half the calls inbound: the ratio no longer holds.
        let balanced: Vec<UsageRecord> = (0..50)
            .map(|i| {
                call(
                    if i % 2 == 0 {
                        RecordType::OriginatedCall
                    } else {
                        RecordType::TerminatedCall
                    },
                    &format!("2026-07-28T{:02}:00:00+02:00", i % 24),
                    "262-02",
                )
            })
            .collect();
        assert!(bypass_signature(&balanced, 20, 90).is_none());

        // All outbound, but the SIM appears on two networks — the mobility a
        // remote-SIM gateway fabricates at no cost.
        let mobile: Vec<UsageRecord> = (0..50)
            .map(|i| {
                call(
                    RecordType::OriginatedCall,
                    &format!("2026-07-28T{:02}:00:00+02:00", i % 24),
                    if i % 2 == 0 { "262-02" } else { "262-01" },
                )
            })
            .collect();
        assert!(
            bypass_signature(&mobile, 20, 90).is_none(),
            "fabricated mobility defeats the signature, as the module documents"
        );
    }

    #[test]
    fn a_handful_of_calls_never_fires() {
        let few: Vec<UsageRecord> = (0..3)
            .map(|i| {
                call(
                    RecordType::OriginatedCall,
                    &format!("2026-07-28T1{i}:00:00+02:00"),
                    "262-02",
                )
            })
            .collect();
        assert!(bypass_signature(&few, 20, 90).is_none());
        assert!(bypass_signature(&[], 1, 90).is_none());
    }

    /// The ratio multiplies before dividing, so two of three is 66 and not 0.
    #[test]
    fn the_outbound_ratio_is_computed_in_the_right_order() {
        let mixed = vec![
            call(
                RecordType::OriginatedCall,
                "2026-07-28T10:00:00+02:00",
                "262-02",
            ),
            call(
                RecordType::OriginatedCall,
                "2026-07-28T11:00:00+02:00",
                "262-02",
            ),
            call(
                RecordType::TerminatedCall,
                "2026-07-28T12:00:00+02:00",
                "262-02",
            ),
        ];
        let found = bypass_signature(&mixed, 3, 60).unwrap();
        assert!(found.detail.contains("66%"), "{}", found.detail);
        assert!(bypass_signature(&mixed, 3, 70).is_none());
    }

    // ── AIT ──────────────────────────────────────────────────────────────────

    /// Message traffic clustering on one range is the shape of an inflated
    /// one-time-password flood.
    #[test]
    fn messages_concentrated_on_one_range_are_reported() {
        let records: Vec<UsageRecord> = (0..20)
            .map(|i| {
                call(
                    RecordType::SmsOriginated,
                    &format!("2026-07-28T10:{i:02}:00+02:00"),
                    "262-02",
                )
            })
            .collect();
        // Eighteen of twenty go to the flagged satellite range.
        let called = |index: usize, _: &UsageRecord| -> Option<Msisdn> {
            if index < 18 {
                "882161234567".parse().ok()
            } else {
                "491701234567".parse().ok()
            }
        };

        let found = ait_signature(&resolve(&records, &called), &high_risk(), 10, 80).unwrap();
        assert_eq!(found.signal, Signal::AitSignature);
        assert_eq!(found.records.len(), 18);
        assert!(found.detail.contains("90%"), "{}", found.detail);

        // A stricter concentration threshold does not fire.
        assert!(ait_signature(&resolve(&records, &called), &high_risk(), 10, 95).is_none());
    }

    /// A legitimate sender messaging one country looks concentrated too, which
    /// is why the volume floor is supplied.
    #[test]
    fn a_small_number_of_messages_never_fires() {
        let records: Vec<UsageRecord> = (0..3)
            .map(|i| {
                call(
                    RecordType::SmsOriginated,
                    &format!("2026-07-28T10:0{i}:00+02:00"),
                    "262-02",
                )
            })
            .collect();
        let called = |_: usize, _: &UsageRecord| "882161234567".parse::<Msisdn>().ok();
        assert!(ait_signature(&resolve(&records, &called), &high_risk(), 10, 50).is_none());
    }

    #[test]
    fn signal_codes_round_trip() {
        assert_eq!(Signal::ALL.len(), 5);
        assert_eq!(Signal::ALL.len(), Signal::CODES.len());
        for (signal, code) in Signal::ALL.iter().zip(Signal::CODES) {
            assert_eq!(signal.to_string(), signal.as_str());
            assert_eq!(signal.as_str(), *code);
        }
        // Only one signal is arithmetic; the rest are judgements about shape.
        assert_eq!(Signal::ALL.iter().filter(|s| s.is_arithmetic()).count(), 1);
    }
}
