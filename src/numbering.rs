//! What a dialled number means: ranges, and who actually terminates it.
//!
//! # A number's range says who *allocated* it, not who terminates it
//!
//! Number portability broke that inference everywhere it was introduced. A
//! German mobile number in the `0151…` range was a Telekom number when it was
//! issued and may be a Vodafone number today, and **interconnect settlement is
//! per terminating operator** — so rating a call on its range pays the wrong
//! partner, in both directions, by amounts that surface only when the partner's
//! invoice disagrees with yours.
//!
//! [`terminating_plmn`] therefore consults portability data first and the
//! allocation range only as a fallback, and returns `None` when neither
//! resolves the number rather than guessing.
//!
//! ```rust
//! use mediation::{NumberRangeSet, PlmnId, terminating_plmn};
//! # use mediation::Msisdn;
//! let mut allocation = NumberRangeSet::new();
//! allocation.insert("49151".parse().unwrap(), "262-01".parse::<PlmnId>().unwrap());
//!
//! let number: Msisdn = "491511234567".parse().unwrap();
//!
//! // Nobody ported it: the allocation range answers.
//! let none_ported = |_: Msisdn| None;
//! assert_eq!(
//!     terminating_plmn(number, &allocation, &none_ported).unwrap().to_string(),
//!     "262-01",
//! );
//!
//! // Once ported, the range is simply wrong.
//! let ported = |_: Msisdn| Some("262-02".parse::<PlmnId>().unwrap());
//! assert_eq!(
//!     terminating_plmn(number, &allocation, &ported).unwrap().to_string(),
//!     "262-02",
//! );
//! ```
//!
//! # Longest prefix wins
//!
//! Number plans nest: `49` is Germany, `49151` is one mobile range inside it,
//! `4915112` may be a block carved out of that. A lookup that returns the first
//! or the shortest match answers with the country when the caller asked about
//! the block — so [`NumberRangeSet`] resolves by **longest prefix**, which is
//! the only reading that respects how ranges are actually assigned.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use crate::digits::DigitString;
use crate::error::ParseError;
use crate::identity::Msisdn;
use crate::plmn::PlmnId;

// ── NumberRange ──────────────────────────────────────────────────────────────

/// A block of E.164 numbers, identified by its leading digits.
///
/// Held in international form without a `+`, exactly as [`Msisdn`] is, so a
/// range and a number are compared digit against digit with no normalisation
/// step where a mistake could hide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NumberRange {
    prefix: DigitString,
}

/// The shape [`NumberRange`] accepts, as rendered in a [`ParseError`].
const RANGE_FORMAT: &str = "1 to 15 decimal digits in international form, such as 49151";

impl NumberRange {
    /// The widest prefix a range may hold — the E.164 maximum.
    pub const MAX_DIGITS: usize = 15;

    /// A range from its leading digits.
    ///
    /// `None` for an empty prefix, which would match every number and is
    /// therefore a range only in the sense that the universe is.
    #[must_use]
    pub fn new(prefix: DigitString) -> Option<Self> {
        if prefix.is_empty() || prefix.len() > Self::MAX_DIGITS {
            None
        } else {
            Some(Self { prefix })
        }
    }

    /// The range's digits.
    #[must_use]
    pub const fn prefix(self) -> DigitString {
        self.prefix
    }

    /// How many digits the prefix holds.
    ///
    /// The specificity of the range: a longer prefix is a narrower block, and
    /// this is what [`NumberRangeSet`] orders matches by.
    #[must_use]
    pub fn len(self) -> usize {
        self.prefix.len()
    }

    /// Always `false` — a range is never constructed empty.
    #[must_use]
    pub fn is_empty(self) -> bool {
        false
    }

    /// `true` when `number` falls inside this range.
    ///
    /// A range at least as long as the number does **not** contain it: `49151`
    /// is not a range that `4915` belongs to, and treating a prefix longer than
    /// the number as a match would put short numbers in blocks carved out of
    /// them.
    #[must_use]
    pub fn contains(self, number: Msisdn) -> bool {
        number.digits().starts_with(self.prefix)
    }
}

impl fmt::Display for NumberRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.prefix.fmt(f)
    }
}

impl FromStr for NumberRange {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseError::format("NumberRange", s, RANGE_FORMAT);
        let t = s.trim();
        let body = t.strip_prefix('+').unwrap_or(t);
        Self::new(DigitString::parse(body, Self::MAX_DIGITS).ok_or_else(err)?).ok_or_else(err)
    }
}

// ── NumberRangeSet ───────────────────────────────────────────────────────────

/// Ranges mapped to values, resolved by longest prefix.
///
/// Generic over the value because the same structure answers two different
/// questions: *which operator was this range allocated to* and *is this
/// destination on a high-risk list*. Both are caller-supplied data this crate
/// will not vendor.
///
/// # Lookup is by probe, not by scan
///
/// A national numbering plan is hundreds of thousands of ranges, and every
/// call needs one lookup. A linear scan over the ranges makes that
/// *O(ranges)* per record — fine on the toy set a test builds, and the reason
/// a mediation run that worked in staging takes a week in production.
///
/// The ranges are held in a [`BTreeMap`] keyed on their digits, and a lookup
/// probes the number's own prefixes from longest to shortest. A prefix is at
/// most 15 digits, so a lookup is at most 15 map probes however many ranges
/// are loaded: *O(log ranges)*, independent of the plan's size.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NumberRangeSet<T> {
    /// One entry per distinct prefix. The vector holds every value inserted
    /// against it, in insertion order, so a duplicated range is visible rather
    /// than silently overwritten.
    entries: BTreeMap<DigitString, Vec<T>>,
    values: usize,
}

impl<T> NumberRangeSet<T> {
    /// An empty set.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            values: 0,
        }
    }

    /// Add a range.
    ///
    /// A range added twice keeps **both** values, and the lookup returns the
    /// first inserted. Silently replacing would hide a duplicate in a caller's
    /// reference data, which is a fault worth noticing —
    /// [`duplicates`](Self::duplicates) finds them.
    pub fn insert(&mut self, range: NumberRange, value: T) {
        self.entries.entry(range.prefix).or_default().push(value);
        self.values += 1;
    }

    /// How many values are held, duplicates included.
    #[must_use]
    pub fn len(&self) -> usize {
        self.values
    }

    /// How many distinct ranges are held.
    #[must_use]
    pub fn ranges(&self) -> usize {
        self.entries.len()
    }

    /// `true` when no ranges are held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The most specific range containing `number`, and its value.
    ///
    /// Longest prefix wins. `None` when the number falls in no range at all —
    /// which is an answer, not a failure: a number outside every known block is
    /// exactly what a caller needs to be told about.
    ///
    /// At most [`NumberRange::MAX_DIGITS`] map probes, whatever the plan size.
    #[must_use]
    pub fn longest_match(&self, number: Msisdn) -> Option<(NumberRange, &T)> {
        let digits = number.digits();
        let longest = digits.len().min(NumberRange::MAX_DIGITS);
        for width in (1..=longest).rev() {
            let prefix = digits.prefix(width);
            if let Some(values) = self.entries.get(&prefix)
                && let Some(first) = values.first()
            {
                return Some((NumberRange { prefix }, first));
            }
        }
        None
    }

    /// Every value held against the most specific matching range.
    ///
    /// One entry unless the range was inserted more than once — see
    /// [`duplicates`](Self::duplicates).
    #[must_use]
    pub fn longest_match_all(&self, number: Msisdn) -> Option<(NumberRange, &[T])> {
        let digits = number.digits();
        let longest = digits.len().min(NumberRange::MAX_DIGITS);
        for width in (1..=longest).rev() {
            let prefix = digits.prefix(width);
            if let Some(values) = self.entries.get(&prefix)
                && !values.is_empty()
            {
                return Some((NumberRange { prefix }, values.as_slice()));
            }
        }
        None
    }

    /// Ranges present more than once, ascending.
    ///
    /// A duplicated range in reference data means two answers to one question,
    /// and which one wins is then an accident of load order.
    #[must_use]
    pub fn duplicates(&self) -> Vec<NumberRange> {
        self.entries
            .iter()
            .filter(|(_, values)| values.len() > 1)
            .map(|(prefix, _)| NumberRange { prefix: *prefix })
            .collect()
    }

    /// Every range held, ascending by digits.
    pub fn ranges_iter(&self) -> impl Iterator<Item = NumberRange> + '_ {
        self.entries
            .keys()
            .map(|prefix| NumberRange { prefix: *prefix })
    }
}

impl<T> FromIterator<(NumberRange, T)> for NumberRangeSet<T> {
    fn from_iter<I: IntoIterator<Item = (NumberRange, T)>>(iter: I) -> Self {
        let mut set = Self::new();
        for (range, value) in iter {
            set.insert(range, value);
        }
        set
    }
}

// ── Portability ──────────────────────────────────────────────────────────────

/// Resolves a number to the network it has been ported to.
///
/// Implement this over your portability data — a national database, a routing
/// prefix the originating switch prepended, a partner feed. The crate calls it
/// and ships no database, exactly as with [`MncWidths`](crate::plmn::MncWidths).
///
/// Returning `None` means *not ported*, not *unknown*. A lookup that cannot
/// distinguish the two should say so by failing before it gets here, because
/// treating "unknown" as "not ported" silently falls back to the allocation
/// range — which is the mistake this whole module exists to prevent.
pub trait Porting {
    /// The network `number` has been ported to, or `None` if it has not.
    fn ported_to(&self, number: Msisdn) -> Option<PlmnId>;
}

impl<F> Porting for F
where
    F: Fn(Msisdn) -> Option<PlmnId>,
{
    fn ported_to(&self, number: Msisdn) -> Option<PlmnId> {
        self(number)
    }
}

/// The network that actually terminates a number.
///
/// Portability first, allocation second. `None` when the number is in no known
/// range and has not been ported — which a caller must treat as unresolvable
/// rather than defaulting, since settling against the wrong partner is a
/// two-sided error that only surfaces at reconciliation.
#[must_use]
pub fn terminating_plmn(
    number: Msisdn,
    allocation: &NumberRangeSet<PlmnId>,
    porting: &impl Porting,
) -> Option<PlmnId> {
    porting.ported_to(number).or_else(|| {
        allocation
            .longest_match(number)
            .map(|(_, network)| *network)
    })
}

/// `true` when a number terminates somewhere other than its allocation range.
///
/// The population worth reporting on: a settlement built on ranges is wrong by
/// exactly these numbers, and knowing how many there are sizes the exposure.
#[must_use]
pub fn is_ported(
    number: Msisdn,
    allocation: &NumberRangeSet<PlmnId>,
    porting: &impl Porting,
) -> bool {
    match (porting.ported_to(number), allocation.longest_match(number)) {
        (Some(ported), Some((_, allocated))) => !ported.is_same_network(*allocated),
        (Some(_), None) => true,
        (None, _) => false,
    }
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::NumberRange;
    use serde::de::{self, Visitor};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::fmt;

    impl Serialize for NumberRange {
        /// The digits, as a string, so leading zeros survive.
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.collect_str(self)
        }
    }

    impl<'de> Deserialize<'de> for NumberRange {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct V;
            impl Visitor<'_> for V {
                type Value = NumberRange;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("a number range such as \"49151\"")
                }
                fn visit_str<E: de::Error>(self, v: &str) -> Result<NumberRange, E> {
                    v.parse().map_err(de::Error::custom)
                }
            }
            deserializer.deserialize_str(V)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn number(digits: &str) -> Msisdn {
        digits.parse().expect("a valid number")
    }

    fn range(digits: &str) -> NumberRange {
        digits.parse().expect("a valid range")
    }

    fn allocation() -> NumberRangeSet<PlmnId> {
        let mut set = NumberRangeSet::new();
        set.insert(range("49"), "262-01".parse().unwrap());
        set.insert(range("49151"), "262-01".parse().unwrap());
        set.insert(range("4915112"), "262-23".parse().unwrap());
        set.insert(range("49170"), "262-02".parse().unwrap());
        set
    }

    /// Ranges nest, and answering with the country when the caller asked about
    /// a block is the whole reason for longest-prefix matching.
    #[test]
    fn the_most_specific_range_wins() {
        let set = allocation();

        // A number in the wider range, where no block was carved out.
        let (matched, network) = set.longest_match(number("491519234567")).unwrap();
        assert_eq!(matched.to_string(), "49151");
        assert_eq!(network.to_string(), "262-01");

        // The carved-out block, not the range it sits inside.
        let (matched, network) = set.longest_match(number("491511234567")).unwrap();
        assert_eq!(matched.to_string(), "4915112");
        assert_eq!(network.to_string(), "262-23");

        // A number matching only the country falls back to it.
        let (matched, _) = set.longest_match(number("4930123456")).unwrap();
        assert_eq!(matched.to_string(), "49");
    }

    /// A number in no known range is unresolvable, which is an answer.
    #[test]
    fn a_number_outside_every_range_matches_nothing() {
        let set = allocation();
        assert!(set.longest_match(number("12125550100")).is_none());
        assert!(NumberRangeSet::<PlmnId>::new().is_empty());
    }

    /// A prefix longer than the number is not a range the number belongs to.
    #[test]
    fn a_range_longer_than_the_number_does_not_contain_it() {
        assert!(!range("49151").contains(number("4915")));
        assert!(range("49151").contains(number("491511")));
        assert!(
            range("49151").contains(number("49151")),
            "equal length matches"
        );
    }

    /// An empty prefix would match everything, which is not a range.
    #[test]
    fn an_empty_range_is_rejected() {
        assert!("".parse::<NumberRange>().is_err());
        assert!(NumberRange::new(DigitString::EMPTY).is_none());
        assert!(
            "+49151".parse::<NumberRange>().is_ok(),
            "a leading + is dropped"
        );
        assert!(
            "1234567890123456".parse::<NumberRange>().is_err(),
            "16 digits"
        );
    }

    /// A duplicated range means two answers to one question, and which wins
    /// becomes an accident of load order.
    #[test]
    fn duplicated_ranges_are_reported_rather_than_silently_replaced() {
        let mut set = NumberRangeSet::new();
        set.insert(range("49151"), "262-01".parse::<PlmnId>().unwrap());
        set.insert(range("49151"), "262-02".parse::<PlmnId>().unwrap());
        set.insert(range("49170"), "262-02".parse::<PlmnId>().unwrap());

        assert_eq!(set.len(), 3, "both entries are kept");
        assert_eq!(set.duplicates(), vec![range("49151")]);
        assert!(allocation().duplicates().is_empty());
    }

    // ── Portability ──────────────────────────────────────────────────────────

    /// The bug this module exists for: a settlement built on ranges pays the
    /// wrong partner for every ported number.
    #[test]
    fn a_ported_number_terminates_somewhere_its_range_does_not_say() {
        let set = allocation();
        let victim = number("491519234567");

        let not_ported = |_: Msisdn| None;
        assert_eq!(
            terminating_plmn(victim, &set, &not_ported)
                .unwrap()
                .to_string(),
            "262-01",
            "the allocation range answers when nothing was ported"
        );
        assert!(!is_ported(victim, &set, &not_ported));

        let ported = |_: Msisdn| Some("262-02".parse::<PlmnId>().unwrap());
        assert_eq!(
            terminating_plmn(victim, &set, &ported).unwrap().to_string(),
            "262-02",
            "portability overrides the range"
        );
        assert!(is_ported(victim, &set, &ported));
    }

    /// Porting a number back to the network that was allocated it is not a
    /// porting for settlement purposes: the answer is the same either way.
    #[test]
    fn porting_back_to_the_allocated_network_is_not_a_difference() {
        let set = allocation();
        let victim = number("491519234567");
        let ported_home = |_: Msisdn| Some("262-01".parse::<PlmnId>().unwrap());

        assert_eq!(
            terminating_plmn(victim, &set, &ported_home)
                .unwrap()
                .to_string(),
            "262-01"
        );
        assert!(!is_ported(victim, &set, &ported_home));
    }

    /// A number in no range that has nonetheless been ported still resolves —
    /// and counts as ported, because the range says nothing at all.
    #[test]
    fn a_ported_number_outside_every_range_still_resolves() {
        let set = allocation();
        let foreign = number("12125550100");
        let ported = |_: Msisdn| Some("262-02".parse::<PlmnId>().unwrap());

        assert_eq!(
            terminating_plmn(foreign, &set, &ported)
                .unwrap()
                .to_string(),
            "262-02"
        );
        assert!(is_ported(foreign, &set, &ported));
    }

    /// Neither source resolving is unresolvable, not a default.
    #[test]
    fn an_unknown_number_resolves_to_nothing() {
        let set = allocation();
        let not_ported = |_: Msisdn| None;
        assert_eq!(
            terminating_plmn(number("12125550100"), &set, &not_ported),
            None
        );
    }

    /// The same structure answers a different question, which is why it is
    /// generic.
    #[test]
    fn a_range_set_also_carries_risk_tags() {
        let mut risky: NumberRangeSet<&'static str> = NumberRangeSet::new();
        risky.insert(range("88216"), "satellite");
        risky.insert(range("1900"), "premium");

        let (_, tag) = risky.longest_match(number("882161234567")).unwrap();
        assert_eq!(*tag, "satellite");
        assert!(risky.longest_match(number("491701234567")).is_none());
    }

    #[test]
    fn ranges_round_trip_through_their_string_form() {
        for text in ["49", "49151", "4915112", "1"] {
            assert_eq!(text.parse::<NumberRange>().unwrap().to_string(), text);
        }
    }
}
