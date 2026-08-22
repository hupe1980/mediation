//! Data volumes as **integer octets**, and the unit arithmetic around them.
//!
//! # Why not a float
//!
//! A data volume is inherently an integer count of octets, so a floating-point
//! representation is not merely imprecise — it is categorically wrong. Above
//! 2^53 octets (about 9.01 PB) a 64-bit float cannot represent consecutive
//! integers, and a national monthly aggregate crosses that threshold. Counts
//! would then be dropped silently, in the one direction nobody audits.
//!
//! Nothing in this crate converts a volume to a float, not even transiently.
//! A CI lane greps the source for float types to keep it that way.
//!
//! # Why not a total
//!
//! [`VolumePair`] keeps uplink and downlink apart and offers no constructor
//! that takes only a total. Asymmetric tariffs, zero-rated uplink and
//! interconnect settlement all need the two directions separately, and once
//! they are summed the split cannot be recovered. Pre-summing at ingest is the
//! kind of lossy normalisation that looks tidy for a year and then cannot be
//! undone.
//!
//! # Why the unit names are strict
//!
//! Operators bill in both powers of ten and powers of two, sometimes in the
//! same tariff sheet, and "MB" is read as either. This module follows
//! **IEC 80000-13** without exception: `kB` is 1 000 octets and `KiB` is 1 024.
//! Exactly one unit — the octet — reaches storage, and conversion happens only
//! at the boundary, explicitly:
//!
//! ```rust
//! use mediation::{ByteScale, Octets};
//!
//! assert_eq!(Octets::from_scaled(5, ByteScale::Mega).unwrap().get(), 5_000_000);
//! assert_eq!(Octets::from_scaled(5, ByteScale::Mebi).unwrap().get(), 5_242_880);
//! ```
//!
//! A contract that says "5 MB" without saying which is ambiguous in the
//! contract, not in this crate. Resolve it where the ambiguity lives.
//!
//! # Rounding is a rating decision
//!
//! Rounding a volume up to a whole chargeable unit is priced behaviour, so it
//! is offered here as a named primitive and applied by nothing. No aggregation,
//! reassembly or validation path in this crate rounds a quantity. What leaves
//! is the measured value.

use std::fmt;
use std::str::FromStr;

use crate::error::ParseError;

// ── ByteScale ────────────────────────────────────────────────────────────────

/// A multiple of the octet, per IEC 80000-13.
///
/// The decimal prefixes (`kB`, `MB`, …) are powers of 1 000 and the binary ones
/// (`KiB`, `MiB`, …) powers of 1 024. Both are offered because both appear in
/// real tariffs; neither is inferred from the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ByteScale {
    /// A single octet.
    Byte,
    /// 10^3 octets.
    Kilo,
    /// 2^10 octets.
    Kibi,
    /// 10^6 octets.
    Mega,
    /// 2^20 octets.
    Mebi,
    /// 10^9 octets.
    Giga,
    /// 2^30 octets.
    Gibi,
    /// 10^12 octets.
    Tera,
    /// 2^40 octets.
    Tebi,
}

impl ByteScale {
    /// Every variant, coarsest last.
    pub const ALL: &'static [Self] = &[
        Self::Byte,
        Self::Kilo,
        Self::Kibi,
        Self::Mega,
        Self::Mebi,
        Self::Giga,
        Self::Gibi,
        Self::Tera,
        Self::Tebi,
    ];

    /// The canonical symbols, in the order of [`ALL`](Self::ALL).
    pub const CODES: &'static [&'static str] =
        &["B", "kB", "KiB", "MB", "MiB", "GB", "GiB", "TB", "TiB"];

    /// How many octets one unit of this scale is.
    #[must_use]
    pub const fn factor(self) -> u64 {
        match self {
            Self::Byte => 1,
            Self::Kilo => 1_000,
            Self::Kibi => 1_024,
            Self::Mega => 1_000_000,
            Self::Mebi => 1_048_576,
            Self::Giga => 1_000_000_000,
            Self::Gibi => 1_073_741_824,
            Self::Tera => 1_000_000_000_000,
            Self::Tebi => 1_099_511_627_776,
        }
    }

    /// `true` for the binary prefixes (`KiB`, `MiB`, `GiB`, `TiB`).
    #[must_use]
    pub const fn is_binary(self) -> bool {
        matches!(self, Self::Kibi | Self::Mebi | Self::Gibi | Self::Tebi)
    }

    /// The canonical symbol.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Byte => "B",
            Self::Kilo => "kB",
            Self::Kibi => "KiB",
            Self::Mega => "MB",
            Self::Mebi => "MiB",
            Self::Giga => "GB",
            Self::Gibi => "GiB",
            Self::Tera => "TB",
            Self::Tebi => "TiB",
        }
    }
}

impl fmt::Display for ByteScale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ByteScale {
    type Err = ParseError;

    /// Parses the IEC 80000-13 symbols.
    ///
    /// # A lower-case `b` is a **bit**
    ///
    /// IEC 80000-13 gives `B` to the byte and `b` to the bit, so `kb` is a
    /// kilobit and `kB` a kilobyte. Accepting `kb` as a kilobyte is a
    /// factor-of-eight error in exactly the direction this module exists to
    /// prevent — a silent unit confusion that produces a plausible number — so
    /// the bit spellings are **rejected** rather than helpfully coerced. A
    /// tariff sheet that says "kb" and means kilobytes is ambiguous in the
    /// tariff sheet; resolve it there.
    ///
    /// `KB` is accepted as `kB`, because that capitalisation is ubiquitous and
    /// is not a bit spelling — the binary kilobyte is `KiB`, which differs by
    /// more than case. The binary prefixes are matched case-insensitively for
    /// the same reason: the `i` disambiguates them on its own.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim();
        match t {
            "B" | "octet" | "octets" | "byte" | "bytes" => return Ok(Self::Byte),
            "kB" | "KB" => return Ok(Self::Kilo),
            "MB" => return Ok(Self::Mega),
            "GB" => return Ok(Self::Giga),
            "TB" => return Ok(Self::Tera),
            _ => {}
        }
        for scale in Self::ALL.iter().filter(|s| s.is_binary()) {
            if t.eq_ignore_ascii_case(scale.as_str()) {
                return Ok(*scale);
            }
        }
        Err(ParseError::one_of("ByteScale", s, Self::CODES))
    }
}

// ── Octets ───────────────────────────────────────────────────────────────────

/// A data volume, counted in octets.
///
/// Deliberately implements no [`Add`](std::ops::Add): every sum is checked,
/// because summing partial records and per-container volumes is precisely where
/// an overflow would arise, and a wrapped total is a plausible-looking wrong
/// number that reaches an invoice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Octets(u64);

impl Octets {
    /// No volume at all.
    pub const ZERO: Self = Self(0);

    /// A volume from a raw octet count.
    #[must_use]
    pub const fn new(octets: u64) -> Self {
        Self(octets)
    }

    /// The raw octet count.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// `true` for a zero volume.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// A volume from a count of larger units, e.g. 5 [`Mega`](ByteScale::Mega).
    ///
    /// `None` on overflow, so a corrupt unit count cannot wrap into a small
    /// plausible volume.
    #[must_use]
    pub const fn from_scaled(count: u64, scale: ByteScale) -> Option<Self> {
        match count.checked_mul(scale.factor()) {
            Some(v) => Some(Self(v)),
            None => None,
        }
    }

    /// Sum, `None` on overflow.
    #[must_use]
    pub const fn checked_add(self, other: Self) -> Option<Self> {
        match self.0.checked_add(other.0) {
            Some(v) => Some(Self(v)),
            None => None,
        }
    }

    /// Sum, saturating at [`u64::MAX`].
    ///
    /// For the few places a running total must keep going rather than stop —
    /// a regulatory ratio, a diagnostic counter. Saturating is the lesser evil
    /// there: `u64::MAX` octets is 18 exabytes and visibly wrong, while
    /// discarding the operand silently loses a record from a comparison that
    /// decides something.
    ///
    /// Anything that reaches an invoice uses [`checked_add`](Self::checked_add)
    /// instead, and stops.
    #[must_use]
    pub const fn saturating_add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }

    /// Difference, `None` when it would go below zero.
    ///
    /// A negative volume is not a small volume; it is a broken counter, and the
    /// caller should say so rather than receive a saturated zero.
    #[must_use]
    pub const fn checked_sub(self, other: Self) -> Option<Self> {
        match self.0.checked_sub(other.0) {
            Some(v) => Some(Self(v)),
            None => None,
        }
    }

    /// The number of whole chargeable units this volume occupies, rounding up.
    ///
    /// A rating primitive. Nothing in this crate calls it — see the module
    /// documentation.
    ///
    /// ```rust
    /// use mediation::{ByteScale, Octets};
    ///
    /// // One octet over a kilobyte is two chargeable kilobytes.
    /// assert_eq!(Octets::new(1_001).units_ceil(ByteScale::Kilo), 2);
    /// assert_eq!(Octets::new(1_000).units_ceil(ByteScale::Kilo), 1);
    /// assert_eq!(Octets::ZERO.units_ceil(ByteScale::Kilo), 0);
    /// ```
    #[must_use]
    pub const fn units_ceil(self, scale: ByteScale) -> u64 {
        let factor = scale.factor();
        self.0.div_ceil(factor)
    }

    /// The number of whole units this volume contains, discarding a remainder.
    #[must_use]
    pub const fn units_floor(self, scale: ByteScale) -> u64 {
        self.0 / scale.factor()
    }

    /// This volume rounded up to a whole multiple of `scale`.
    ///
    /// A rating primitive; `None` on overflow.
    #[must_use]
    pub const fn round_up_to(self, scale: ByteScale) -> Option<Self> {
        match self.units_ceil(scale).checked_mul(scale.factor()) {
            Some(v) => Some(Self(v)),
            None => None,
        }
    }
}

impl fmt::Display for Octets {
    /// Writes the plain octet count, with no unit suffix.
    ///
    /// A suffix would imply that some other unit is also canonical. Exactly one
    /// is.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Octets {
    type Err = ParseError;

    /// Parses a plain octet count.
    ///
    /// Deliberately does **not** accept a unit suffix: `"5MB"` would have to
    /// guess between 5 000 000 and 5 242 880. Parse the number and the
    /// [`ByteScale`] separately, then call [`from_scaled`](Self::from_scaled).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.trim().parse::<u64>().map(Self).map_err(|_| {
            ParseError::format(
                "Octets",
                s,
                "a non-negative whole number of octets, with no unit suffix",
            )
        })
    }
}

impl From<Octets> for u64 {
    fn from(o: Octets) -> Self {
        o.0
    }
}

impl From<u64> for Octets {
    fn from(o: u64) -> Self {
        Self(o)
    }
}

// ── VolumePair ───────────────────────────────────────────────────────────────

/// Uplink and downlink volumes, kept apart.
///
/// There is no constructor taking a total. See the module documentation for
/// why: the split is not recoverable once lost, and several downstream
/// decisions need it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct VolumePair {
    uplink: Octets,
    downlink: Octets,
}

impl VolumePair {
    /// Both directions zero.
    pub const ZERO: Self = Self {
        uplink: Octets::ZERO,
        downlink: Octets::ZERO,
    };

    /// A pair from its two directions.
    #[must_use]
    pub const fn new(uplink: Octets, downlink: Octets) -> Self {
        Self { uplink, downlink }
    }

    /// Volume sent by the subscriber.
    #[must_use]
    pub const fn uplink(self) -> Octets {
        self.uplink
    }

    /// Volume received by the subscriber.
    #[must_use]
    pub const fn downlink(self) -> Octets {
        self.downlink
    }

    /// Both directions summed, `None` on overflow.
    ///
    /// Checked, and a method rather than a field, so that a total is always
    /// derived at the point of use and never stored in place of the pair.
    #[must_use]
    pub const fn checked_total(self) -> Option<Octets> {
        self.uplink.checked_add(self.downlink)
    }

    /// `true` when both directions are zero.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.uplink.is_zero() && self.downlink.is_zero()
    }

    /// Element-wise sum, `None` on overflow in either direction.
    ///
    /// Used to fold the containers of a record, or the partials of a session,
    /// without ever collapsing the two directions.
    #[must_use]
    pub const fn checked_add(self, other: Self) -> Option<Self> {
        match (
            self.uplink.checked_add(other.uplink),
            self.downlink.checked_add(other.downlink),
        ) {
            (Some(up), Some(down)) => Some(Self::new(up, down)),
            _ => None,
        }
    }
}

impl fmt::Display for VolumePair {
    /// Writes `uplink/downlink` in octets — one string, both numbers, no total.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.uplink, self.downlink)
    }
}

/// The shape [`VolumePair`] accepts, as rendered in a [`ParseError`].
const VOLUME_PAIR_FORMAT: &str = "two octet counts separated by a slash, such as 1024/8192";

impl FromStr for VolumePair {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseError::format("VolumePair", s, VOLUME_PAIR_FORMAT);
        let (up, down) = s.trim().split_once('/').ok_or_else(err)?;
        Ok(Self::new(
            up.parse().map_err(|_| err())?,
            down.parse().map_err(|_| err())?,
        ))
    }
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::{ByteScale, Octets, VolumePair};
    use serde::de::{self, Visitor};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::fmt;

    impl Serialize for Octets {
        /// A JSON number — a volume is a quantity, not a label.
        ///
        /// JSON numbers are read as doubles by many parsers, which is the very
        /// failure this type exists to avoid; a consumer handling volumes above
        /// 2^53 octets should decode into an integer type explicitly.
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_u64(self.get())
        }
    }

    impl<'de> Deserialize<'de> for Octets {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            u64::deserialize(deserializer).map(Octets::new)
        }
    }

    impl Serialize for ByteScale {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_str(self.as_str())
        }
    }

    impl<'de> Deserialize<'de> for ByteScale {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct V;
            impl Visitor<'_> for V {
                type Value = ByteScale;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("an IEC 80000-13 byte unit such as \"MB\" or \"MiB\"")
                }
                fn visit_str<E: de::Error>(self, v: &str) -> Result<ByteScale, E> {
                    v.parse().map_err(de::Error::custom)
                }
            }
            deserializer.deserialize_str(V)
        }
    }

    /// A struct with two named octet fields, so neither direction can be read
    /// as the other and neither is optional.
    #[derive(Serialize, Deserialize)]
    #[serde(rename = "VolumePair")]
    struct VolumePairRepr {
        uplink_octets: Octets,
        downlink_octets: Octets,
    }

    impl Serialize for VolumePair {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            VolumePairRepr {
                uplink_octets: self.uplink(),
                downlink_octets: self.downlink(),
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for VolumePair {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = VolumePairRepr::deserialize(deserializer)?;
            Ok(VolumePair::new(r.uplink_octets, r.downlink_octets))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_and_binary_prefixes_are_different_numbers() {
        assert_eq!(
            Octets::from_scaled(5, ByteScale::Mega).unwrap().get(),
            5_000_000
        );
        assert_eq!(
            Octets::from_scaled(5, ByteScale::Mebi).unwrap().get(),
            5_242_880
        );
        assert!(ByteScale::Mebi.is_binary() && !ByteScale::Mega.is_binary());
    }

    /// The volume this type exists for: past 2^53 octets a float loses counts,
    /// while the integer stays exact.
    #[test]
    fn volumes_beyond_the_float_horizon_stay_exact() {
        const TWO_POW_53: u64 = 9_007_199_254_740_992;
        let big = Octets::new(TWO_POW_53);
        let one_more = big.checked_add(Octets::new(1)).unwrap();
        assert_eq!(one_more.get(), TWO_POW_53 + 1);
        assert_ne!(one_more, big);
    }

    #[test]
    fn arithmetic_is_checked_in_both_directions() {
        assert_eq!(Octets::new(u64::MAX).checked_add(Octets::new(1)), None);
        assert_eq!(Octets::new(1).checked_sub(Octets::new(2)), None);
        assert_eq!(
            Octets::new(3).checked_sub(Octets::new(2)),
            Some(Octets::new(1))
        );
        assert_eq!(Octets::from_scaled(u64::MAX, ByteScale::Tera), None);
    }

    /// The two sums differ deliberately: one stops, one keeps going visibly.
    #[test]
    fn saturating_and_checked_sums_fail_differently() {
        let huge = Octets::new(u64::MAX);
        assert_eq!(huge.checked_add(Octets::new(1)), None);
        assert_eq!(huge.saturating_add(Octets::new(1)), huge);
        assert_eq!(
            Octets::new(2).saturating_add(Octets::new(3)),
            Octets::new(5)
        );
    }

    #[test]
    fn chargeable_units_round_up_and_floor_down() {
        let v = Octets::new(1_001);
        assert_eq!(v.units_ceil(ByteScale::Kilo), 2);
        assert_eq!(v.units_floor(ByteScale::Kilo), 1);
        assert_eq!(v.round_up_to(ByteScale::Kilo), Some(Octets::new(2_000)));

        assert_eq!(Octets::ZERO.units_ceil(ByteScale::Kilo), 0);
        assert_eq!(Octets::new(1_000).units_ceil(ByteScale::Kilo), 1);
    }

    /// The pair keeps two numbers; a total is derived, never stored.
    #[test]
    fn uplink_and_downlink_stay_apart() {
        let v = VolumePair::new(Octets::new(100), Octets::new(900));
        assert_eq!(v.uplink().get(), 100);
        assert_eq!(v.downlink().get(), 900);
        assert_eq!(v.checked_total(), Some(Octets::new(1_000)));
        assert_eq!(v.to_string(), "100/900");
        assert_eq!("100/900".parse::<VolumePair>().unwrap(), v);
    }

    #[test]
    fn pair_addition_folds_each_direction_independently() {
        let a = VolumePair::new(Octets::new(1), Octets::new(2));
        let b = VolumePair::new(Octets::new(10), Octets::new(20));
        assert_eq!(
            a.checked_add(b),
            Some(VolumePair::new(Octets::new(11), Octets::new(22)))
        );

        let huge = VolumePair::new(Octets::new(u64::MAX), Octets::ZERO);
        assert_eq!(huge.checked_add(a), None);
    }

    /// A unit suffix on a bare number would have to guess a scale, so it is
    /// rejected rather than assumed.
    #[test]
    fn octets_reject_a_unit_suffix() {
        assert_eq!("5000".parse::<Octets>().unwrap(), Octets::new(5_000));
        for s in ["5MB", "5 MB", "5.0", "-1", ""] {
            assert!(s.parse::<Octets>().is_err(), "{s:?} must not parse");
        }
    }

    #[test]
    fn byte_scale_symbols_round_trip() {
        for scale in ByteScale::ALL {
            assert_eq!(scale.as_str().parse::<ByteScale>().unwrap(), *scale);
        }
        // The capitalised decimal kilobyte is unambiguous under IEC and accepted.
        assert_eq!("KB".parse::<ByteScale>().unwrap(), ByteScale::Kilo);
        assert_eq!("KiB".parse::<ByteScale>().unwrap(), ByteScale::Kibi);
        assert!("MBps".parse::<ByteScale>().is_err());
    }

    #[test]
    fn every_scale_code_matches_its_variant() {
        assert_eq!(ByteScale::ALL.len(), ByteScale::CODES.len());
        for (scale, code) in ByteScale::ALL.iter().zip(ByteScale::CODES) {
            assert_eq!(scale.as_str(), *code);
        }
    }
}
