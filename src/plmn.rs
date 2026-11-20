//! Network identity: [`Mcc`], [`Mnc`], [`PlmnId`] and [`TadigCode`].
//!
//! # The MNC is two digits or three, and the digits do not say which
//!
//! An IMSI is `MCC` (3 digits) + `MNC` (**2 or 3**) + `MSIN`, and nothing in
//! the digit string reveals the boundary. North American MCCs use three-digit
//! MNCs; most of the rest of the world uses two. Splitting at a fixed offset
//! therefore misattributes an entire region's records to networks that do not
//! exist — silently, because the result is still a well-formed identifier.
//!
//! The authority is the ITU-T E.212 assignment list, republished operationally
//! through GSMA IR.21. This crate ships the *structure* and the *validation*,
//! and takes the assignment from a caller-supplied [`MncWidths`]. Vendoring a
//! table here would ship a snapshot that is stale the week it is published, and
//! would put the blame for a misattributed record on the wrong party.
//!
//! [`NorthAmericanRule`] covers the one rule that is stable enough to state —
//! MCC 3xx is three digits — and is offered as a starting point, not as an
//! authority.
//!
//! # An MNC is a string, not a number
//!
//! German MNCs under MCC 262 are `01` (Telekom), `02` (Vodafone), `03` and `07`
//! (Telefónica) and `23` (1&1). Stored as an integer, `01` and `1` collapse
//! into one value — merging two operators, or splitting one, depending on which
//! way the coercion falls. [`Mnc`] preserves the declared width, so `262-01`
//! and `262-1` are different values and only one of them is well-formed.
//!
//! ```rust
//! use mediation::{Mnc, PlmnId};
//!
//! let telekom: PlmnId = "262-01".parse().unwrap();
//! assert_eq!(telekom.to_string(), "262-01");
//! assert_eq!(telekom.mnc().width(), 2);
//!
//! // A two-digit MNC and a three-digit one are never equal, even numerically.
//! assert_ne!(Mnc::new(1, 2).unwrap(), Mnc::new(1, 3).unwrap());
//! ```

use std::fmt;
use std::str::FromStr;

use crate::error::{DecodeError, ParseError};

// ── Mcc ──────────────────────────────────────────────────────────────────────

/// An ITU-T E.212 Mobile Country Code: exactly three digits.
///
/// Always recoverable from an IMSI without any external table, which is why it
/// is a separate type from [`PlmnId`] — [`crate::Imsi::mcc`] is infallible while
/// the full network identity is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Mcc(u16);

impl Mcc {
    /// Germany.
    pub const GERMANY: Self = Self(262);

    /// A country code from its numeric value, which must be three digits.
    ///
    /// `000` is rejected: E.212 does not assign it, and a zero default is the
    /// most common way an unparsed field reaches storage looking valid.
    #[must_use]
    pub const fn new(value: u16) -> Option<Self> {
        if value >= 1 && value <= 999 {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The numeric value.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }

    /// `true` when the country code names an actual country.
    ///
    /// ITU-T E.212 sets aside a handful of codes that name no territory:
    ///
    /// | Code | Meaning |
    /// |---|---|
    /// | `001` | test networks (a 3GPP test convention; E.212 reserves 0xx) |
    /// | `901` | international shared code — satellite, maritime, aeronautical (E.212 §F.4) |
    /// | `902` | regional and international organisations' networks (ITU OB 1214) |
    /// | `991` | trials of new international services (E.212 Annex G.1) |
    /// | `999` | internal use within a private network (E.212 Appendix III) |
    ///
    /// A record served by one of these has a well-formed MCC that answers no
    /// question about **where** the usage happened — and where the usage
    /// happened is what retention, lawful interception and roaming regulation
    /// turn on. [`crate::jurisdiction`] reports it as an ambiguity rather than
    /// filing the usage in a country called 901.
    #[must_use]
    pub const fn is_geographic(self) -> bool {
        !matches!(self.0, 1 | 901 | 902 | 991 | 999)
    }

    /// `true` when both codes belong to the same geographic area.
    ///
    /// An MCC is not a country: ITU-T E.212 gives some areas several codes — the
    /// United States 310–316, India 404–406, the United Arab Emirates 424, 430
    /// and 431, the United Kingdom 234–235, Japan 440–441, China 460–461 (List
    /// of MCCs, annex to ITU Operational Bulletin 1117, position on
    /// 1 February 2017). A US subscriber on an MCC 311 network is at home.
    ///
    /// Non-geographic codes are the same area only as themselves. The reverse
    /// case — one code covering several territories, such as 340 (Guadeloupe and
    /// Martinique) or the Crown Dependencies under 234 — needs the operator's
    /// own data and is not resolved here.
    #[must_use]
    pub const fn same_country(self, other: Self) -> bool {
        self.0 == other.0 || (self.is_geographic() && area_of(self.0) == area_of(other.0))
    }

    /// `true` for the North American numbering region, MCC 3xx.
    ///
    /// The one MNC-width rule stable enough to encode. It is still a rule about
    /// a *region*, not a guarantee about a specific assignment — see
    /// [`MncWidths`].
    #[must_use]
    pub const fn is_north_american(self) -> bool {
        self.0 >= 300 && self.0 <= 399
    }
}

/// The lowest MCC of the geographic area `mcc` belongs to (ITU-T E.212).
const fn area_of(mcc: u16) -> u16 {
    match mcc {
        235 => 234,
        311..=316 => 310,
        405 | 406 => 404,
        430 | 431 => 424,
        441 => 440,
        461 => 460,
        other => other,
    }
}

impl fmt::Display for Mcc {
    /// Writes three digits, zero-padded.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:03}", self.0)
    }
}

impl FromStr for Mcc {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseError::format("Mcc", s, "exactly three decimal digits, such as 262");
        let t = s.trim();
        if t.len() != 3 || !t.bytes().all(|b| b.is_ascii_digit()) {
            return Err(err());
        }
        Self::new(t.parse().map_err(|_| err())?).ok_or_else(err)
    }
}

// ── Mnc ──────────────────────────────────────────────────────────────────────

/// An ITU-T E.212 Mobile Network Code: two or three digits, leading zeros
/// significant.
///
/// The declared width is part of the value. `01` and `001` are different
/// networks, and comparing them numerically merges them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Mnc {
    value: u16,
    width: u8,
}

impl Mnc {
    /// A network code from its numeric value and declared digit width.
    ///
    /// `None` for a width other than 2 or 3, or a value that does not fit the
    /// declared width.
    #[must_use]
    pub const fn new(value: u16, width: u8) -> Option<Self> {
        let fits = match width {
            2 => value <= 99,
            3 => value <= 999,
            _ => false,
        };
        if fits {
            Some(Self { value, width })
        } else {
            None
        }
    }

    /// The numeric value, which is meaningless without [`width`](Self::width).
    #[must_use]
    pub const fn get(self) -> u16 {
        self.value
    }

    /// The declared digit width: 2 or 3.
    #[must_use]
    pub const fn width(self) -> u8 {
        self.width
    }
}

impl fmt::Display for Mnc {
    /// Writes the declared number of digits, zero-padded.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:0width$}", self.value, width = self.width as usize)
    }
}

impl FromStr for Mnc {
    type Err = ParseError;

    /// Parses two or three digits, taking the width from the input.
    ///
    /// The width comes from how many digits were written, so `"01"` and
    /// `"001"` parse to different values rather than to the same one.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseError::format("Mnc", s, "two or three decimal digits, such as 02 or 001");
        let t = s.trim();
        if !matches!(t.len(), 2 | 3) || !t.bytes().all(|b| b.is_ascii_digit()) {
            return Err(err());
        }
        let width = u8::try_from(t.len()).map_err(|_| err())?;
        Self::new(t.parse().map_err(|_| err())?, width).ok_or_else(err)
    }
}

// ── MncWidth resolution ──────────────────────────────────────────────────────

/// How many digits an MCC's network codes occupy.
///
/// The value a caller must supply before an IMSI can be split. See the module
/// documentation for why this crate does not decide it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MncWidth {
    /// Two-digit MNCs — most of the world.
    Two,
    /// Three-digit MNCs — the North American region, and some others.
    Three,
}

impl MncWidth {
    /// The width as a digit count.
    #[must_use]
    pub const fn digits(self) -> u8 {
        match self {
            Self::Two => 2,
            Self::Three => 3,
        }
    }
}

/// Resolves an [`Mcc`] to the digit width of its network codes.
///
/// Implement this over whatever assignment data you maintain — a database
/// table, a parsed IR.21 dump, a static map. The crate calls it and never
/// guesses on your behalf.
pub trait MncWidths {
    /// The MNC width for `mcc`, or `None` when the assignment is unknown.
    ///
    /// Returning `None` is the correct answer for an unrecognised country code,
    /// and callers surface it as an unresolvable identity rather than falling
    /// back to a default that would attribute the record to a real network that
    /// did not serve it.
    fn width_for(&self, mcc: Mcc) -> Option<MncWidth>;
}

/// The region rule alone: MCC 3xx is three digits, everything else is two.
///
/// Offered as a starting point for development and tests. It is **not** an
/// authority: several countries outside the North American region also assign
/// three-digit MNCs, and this resolver answers `Two` for all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NorthAmericanRule;

impl MncWidths for NorthAmericanRule {
    fn width_for(&self, mcc: Mcc) -> Option<MncWidth> {
        Some(if mcc.is_north_american() {
            MncWidth::Three
        } else {
            MncWidth::Two
        })
    }
}

impl<F> MncWidths for F
where
    F: Fn(Mcc) -> Option<MncWidth>,
{
    fn width_for(&self, mcc: Mcc) -> Option<MncWidth> {
        self(mcc)
    }
}

// ── PlmnId ───────────────────────────────────────────────────────────────────

/// A Public Land Mobile Network identity: [`Mcc`] plus [`Mnc`].
///
/// The canonical string is `MCC-MNC` with the MNC's leading zeros intact —
/// `262-02`, never `262-2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlmnId {
    mcc: Mcc,
    mnc: Mnc,
}

impl PlmnId {
    /// A network identity from its two parts.
    #[must_use]
    pub const fn new(mcc: Mcc, mnc: Mnc) -> Self {
        Self { mcc, mnc }
    }

    /// The country code.
    #[must_use]
    pub const fn mcc(self) -> Mcc {
        self.mcc
    }

    /// The network code, with its declared width.
    #[must_use]
    pub const fn mnc(self) -> Mnc {
        self.mnc
    }

    /// `true` when both identities name the same network.
    ///
    /// Equivalent to `==`; named because the roaming predicates read better with
    /// it and because `serving == home` is a validation rule in its own right.
    #[must_use]
    pub const fn is_same_network(self, other: Self) -> bool {
        self.mcc.get() == other.mcc.get()
            && self.mnc.get() == other.mnc.get()
            && self.mnc.width() == other.mnc.width()
    }

    /// `true` when the two identities are in the same country — see
    /// [`Mcc::same_country`].
    #[must_use]
    pub const fn is_same_country(self, other: Self) -> bool {
        self.mcc.same_country(other.mcc)
    }

    // ── Wire form ────────────────────────────────────────────────────────────

    /// The wire length of a PLMN identity, in octets.
    pub const BCD_OCTETS: usize = 3;

    /// Decode the three-octet PLMN identity of 3GPP TS 24.008 §10.5.1.13.
    ///
    /// The layout is the one every location information element embeds:
    ///
    /// ```text
    /// octet 1: MCC digit 2 | MCC digit 1
    /// octet 2: MNC digit 3 | MCC digit 3
    /// octet 3: MNC digit 2 | MNC digit 1
    /// ```
    ///
    /// with the nibbles swapped as usual and `0xF` in the MNC-digit-3 position
    /// when the network code is two digits. **That filler is the only thing on
    /// the wire that says how wide the MNC is** — which is why this decoder,
    /// unlike [`Imsi::plmn`](crate::Imsi::plmn), needs no
    /// [`MncWidths`] resolver: the encoding carries what the digit string does
    /// not.
    ///
    /// ```rust
    /// use mediation::PlmnId;
    ///
    /// // 262-02: MCC 262, two-digit MNC, so the third MNC nibble is the filler.
    /// let telekom = PlmnId::from_bcd(&[0x62, 0xF2, 0x20]).unwrap();
    /// assert_eq!(telekom.to_string(), "262-02");
    /// assert_eq!(telekom.to_bcd(), [0x62, 0xF2, 0x20]);
    ///
    /// // 310-260: a three-digit MNC fills the same nibble with a digit.
    /// let tmobile = PlmnId::from_bcd(&[0x13, 0x00, 0x62]).unwrap();
    /// assert_eq!(tmobile.to_string(), "310-260");
    /// ```
    ///
    /// # Errors
    ///
    /// [`DecodeError`] for the wrong length, a nibble that is neither a decimal
    /// digit nor the filler in the one position that permits it, or an MCC of
    /// `000`.
    pub fn from_bcd(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() != Self::BCD_OCTETS {
            return Err(DecodeError::new(0, "exactly 3 octets"));
        }
        let nibble = |octet: u8, high: bool| if high { octet >> 4 } else { octet & 0x0F };

        let mcc_digits = [
            nibble(bytes[0], false),
            nibble(bytes[0], true),
            nibble(bytes[1], false),
        ];
        let mnc3 = nibble(bytes[1], true);
        let mnc_low = [nibble(bytes[2], false), nibble(bytes[2], true)];

        for (index, digit) in mcc_digits.iter().enumerate() {
            if *digit > 9 {
                return Err(DecodeError::new(index / 2, "a decimal MCC digit"));
            }
        }
        for digit in mnc_low {
            if digit > 9 {
                return Err(DecodeError::new(2, "a decimal MNC digit"));
            }
        }

        let mcc_value = u16::from(mcc_digits[0]) * 100
            + u16::from(mcc_digits[1]) * 10
            + u16::from(mcc_digits[2]);
        let mcc = Mcc::new(mcc_value).ok_or_else(|| DecodeError::new(0, "an assigned MCC"))?;

        // The filler in the MNC-digit-3 position is what makes the code two
        // digits wide. Any other non-decimal value there is corrupt.
        let mnc = if mnc3 == 0x0F {
            Mnc::new(u16::from(mnc_low[0]) * 10 + u16::from(mnc_low[1]), 2)
        } else if mnc3 <= 9 {
            Mnc::new(
                u16::from(mnc_low[0]) * 100 + u16::from(mnc_low[1]) * 10 + u16::from(mnc3),
                3,
            )
        } else {
            return Err(DecodeError::new(1, "a decimal MNC digit or the 0xF filler"));
        };
        let mnc = mnc.ok_or_else(|| DecodeError::new(2, "an MNC that fits its width"))?;
        Ok(Self::new(mcc, mnc))
    }

    /// Encode to the three-octet PLMN identity of 3GPP TS 24.008 §10.5.1.13.
    ///
    /// Round-trips with [`from_bcd`](Self::from_bcd) for every value: the
    /// declared MNC width decides whether the third MNC nibble is a digit or
    /// the filler, which is exactly the information a bare digit string loses.
    #[must_use]
    pub const fn to_bcd(self) -> [u8; Self::BCD_OCTETS] {
        let mcc = self.mcc.get();
        let (mcc1, mcc2, mcc3) = ((mcc / 100) as u8, ((mcc / 10) % 10) as u8, (mcc % 10) as u8);
        let mnc = self.mnc.get();
        let (mnc1, mnc2, mnc3) = if self.mnc.width() == 3 {
            ((mnc / 100) as u8, ((mnc / 10) % 10) as u8, (mnc % 10) as u8)
        } else {
            ((mnc / 10) as u8, (mnc % 10) as u8, 0x0F)
        };
        [(mcc2 << 4) | mcc1, (mnc3 << 4) | mcc3, (mnc2 << 4) | mnc1]
    }
}

impl fmt::Display for PlmnId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.mcc, self.mnc)
    }
}

/// The shape [`PlmnId`] accepts, as rendered in a [`ParseError`].
const PLMN_FORMAT: &str = "MCC-MNC with the MNC's leading zeros, such as 262-02 or 310-260";

impl FromStr for PlmnId {
    type Err = ParseError;

    /// Parses `MCC-MNC`.
    ///
    /// The separator is required: without it, `26202` cannot be split without
    /// the very table this type exists to make explicit.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseError::format("PlmnId", s, PLMN_FORMAT);
        let (mcc, mnc) = s.trim().split_once('-').ok_or_else(err)?;
        Ok(Self::new(
            mcc.parse().map_err(|_| err())?,
            mnc.parse().map_err(|_| err())?,
        ))
    }
}

// ── TadigCode ────────────────────────────────────────────────────────────────

/// A GSMA TADIG code: five characters identifying a roaming partner.
///
/// The first three name the country and the last two the operator — `DEUD1`,
/// `GBRVF`, `USACG`. TADIG codes and [`PlmnId`]s identify the same operators
/// through different registries, and neither is derivable from the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TadigCode([u8; 5]);

impl TadigCode {
    /// A code from five ASCII alphanumeric characters.
    ///
    /// Lower case is accepted and normalised to upper, so one operator has one
    /// value however a partner's file spelled it.
    #[must_use]
    pub fn new(code: &str) -> Option<Self> {
        let t = code.trim();
        if t.len() != 5 || !t.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return None;
        }
        let mut buf = [0u8; 5];
        for (slot, byte) in buf.iter_mut().zip(t.bytes()) {
            *slot = byte.to_ascii_uppercase();
        }
        Some(Self(buf))
    }

    /// The whole code.
    ///
    /// # Panics
    ///
    /// Never: [`new`](Self::new) is the only constructor and accepts nothing
    /// but ASCII alphanumerics, so the buffer is always valid UTF-8. It panics
    /// rather than substituting a placeholder, because a placeholder would
    /// become a roaming partner that does not exist.
    #[must_use]
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).expect("a TADIG code is ASCII by construction")
    }

    /// The three-character country part.
    #[must_use]
    pub fn country(&self) -> &str {
        &self.as_str()[..3]
    }

    /// The two-character operator part.
    #[must_use]
    pub fn operator(&self) -> &str {
        &self.as_str()[3..]
    }
}

impl fmt::Display for TadigCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TadigCode {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s).ok_or_else(|| {
            ParseError::format(
                "TadigCode",
                s,
                "five alphanumeric characters, such as DEUD1",
            )
        })
    }
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::{Mcc, Mnc, PlmnId, TadigCode};
    use serde::de::{self, Visitor};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::fmt;

    macro_rules! string_serde {
        ($ty:ty, $expecting:literal) => {
            impl Serialize for $ty {
                /// The canonical string, so leading zeros survive the round trip
                /// that a numeric representation would strip.
                fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                    serializer.collect_str(self)
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

    string_serde!(Mcc, "a three-digit country code such as \"262\"");
    string_serde!(Mnc, "a two- or three-digit network code such as \"02\"");
    string_serde!(PlmnId, "a network identity such as \"262-02\"");
    string_serde!(TadigCode, "a five-character TADIG code such as \"DEUD1\"");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn several_codes_can_be_one_country() {
        let m = |v| Mcc::new(v).unwrap();
        assert!(m(310).same_country(m(311)));
        assert!(m(316).same_country(m(310)));
        assert!(m(404).same_country(m(406)));
        assert!(m(234).same_country(m(235)));
        assert!(m(424).same_country(m(431)));
        assert!(!m(262).same_country(m(232)));
        assert!(!m(310).same_country(m(302)), "Canada is not the US");
        assert!(m(901).same_country(m(901)));
        assert!(!m(901).same_country(m(999)));
    }

    /// The bug this module exists to prevent: an MNC held as a number loses its
    /// leading zero and merges two networks.
    #[test]
    fn leading_zeros_are_part_of_the_value() {
        let telekom: PlmnId = "262-01".parse().unwrap();
        assert_eq!(telekom.to_string(), "262-01");
        assert_eq!(telekom.mnc().get(), 1);
        assert_eq!(telekom.mnc().width(), 2);

        // Same number, different width, different network.
        let two = Mnc::new(1, 2).unwrap();
        let three = Mnc::new(1, 3).unwrap();
        assert_ne!(two, three);
        assert_eq!(two.to_string(), "01");
        assert_eq!(three.to_string(), "001");
    }

    /// Round-trip every constructible identity through the three-octet form.
    /// The filler nibble is the only thing that says how wide the MNC is, so
    /// the width must survive the trip in both directions.
    #[test]
    fn the_plmn_wire_form_round_trips_and_carries_the_mnc_width() {
        for (mcc, mnc_value, width, octets) in [
            (262u16, 2u16, 2u8, [0x62, 0xF2, 0x20]),
            (262, 1, 2, [0x62, 0xF2, 0x10]),
            (310, 260, 3, [0x13, 0x00, 0x62]),
            (1, 1, 2, [0x00, 0xF1, 0x10]),
            (999, 999, 3, [0x99, 0x99, 0x99]),
        ] {
            let plmn = PlmnId::new(Mcc::new(mcc).unwrap(), Mnc::new(mnc_value, width).unwrap());
            assert_eq!(plmn.to_bcd(), octets, "{plmn}");
            assert_eq!(PlmnId::from_bcd(&octets).unwrap(), plmn);
        }

        // Same digits, different widths, different octets — and never equal.
        let two = PlmnId::new(Mcc::new(262).unwrap(), Mnc::new(1, 2).unwrap());
        let three = PlmnId::new(Mcc::new(262).unwrap(), Mnc::new(1, 3).unwrap());
        assert_ne!(two.to_bcd(), three.to_bcd());
    }

    #[test]
    fn the_plmn_wire_form_rejects_what_it_cannot_read() {
        // Wrong length.
        assert!(PlmnId::from_bcd(&[0x62, 0xF2]).is_err());
        // A non-decimal MCC nibble.
        assert!(PlmnId::from_bcd(&[0x6A, 0xF2, 0x20]).is_err());
        // A filler where an MNC digit belongs.
        assert!(PlmnId::from_bcd(&[0x62, 0xF2, 0xF0]).is_err());
        // MCC 000 is not assigned, and is the shape an unset field takes.
        assert!(PlmnId::from_bcd(&[0x00, 0xF0, 0x00]).is_err());
        // 0xE in the MNC-digit-3 position is neither a digit nor the filler.
        assert!(PlmnId::from_bcd(&[0x62, 0xE2, 0x20]).is_err());
    }

    #[test]
    fn german_networks_render_canonically() {
        for (s, mnc) in [("262-01", 1), ("262-02", 2), ("262-03", 3), ("262-23", 23)] {
            let p: PlmnId = s.parse().unwrap();
            assert_eq!(p.mcc(), Mcc::GERMANY);
            assert_eq!(p.mnc().get(), mnc);
            assert_eq!(p.to_string(), s);
        }
    }

    #[test]
    fn a_three_digit_mnc_round_trips() {
        let p: PlmnId = "310-260".parse().unwrap();
        assert_eq!(p.mnc().width(), 3);
        assert_eq!(p.to_string(), "310-260");
        assert!(p.mcc().is_north_american());
    }

    #[test]
    fn malformed_network_identities_are_rejected() {
        for s in [
            "262-2",    // MNC width 1
            "262-0234", // MNC width 4
            "26-02",    // MCC width 2
            "2620-2",   // MCC width 4
            "26202",    // no separator — unsplittable without a table
            "262-ab",   // not digits
            "",
        ] {
            assert!(s.parse::<PlmnId>().is_err(), "{s:?} must not parse");
        }
    }

    #[test]
    fn mcc_zero_is_not_a_country() {
        assert_eq!(Mcc::new(0), None);
        assert!("000".parse::<Mcc>().is_err());
        assert!(Mcc::new(1).is_some());
    }

    /// The width rule is a caller's decision; the built-in resolver states only
    /// the region rule and says so.
    #[test]
    fn width_resolution_is_supplied_not_guessed() {
        let rule = NorthAmericanRule;
        assert_eq!(
            rule.width_for(Mcc::new(310).unwrap()),
            Some(MncWidth::Three)
        );
        assert_eq!(rule.width_for(Mcc::GERMANY), Some(MncWidth::Two));

        // A closure is a resolver too, so a caller's table needs no newtype.
        let table = |mcc: Mcc| match mcc.get() {
            262 => Some(MncWidth::Two),
            _ => None,
        };
        assert_eq!(table.width_for(Mcc::GERMANY), Some(MncWidth::Two));
        assert_eq!(table.width_for(Mcc::new(310).unwrap()), None);
    }

    #[test]
    fn same_network_compares_all_three_components() {
        let a: PlmnId = "262-02".parse().unwrap();
        let b: PlmnId = "262-002".parse().unwrap();
        assert!(!a.is_same_network(b));
        assert!(a.is_same_country(b));
        assert!(a.is_same_network(a));
    }

    #[test]
    fn tadig_codes_normalise_case_and_split() {
        let t: TadigCode = "deud1".parse().unwrap();
        assert_eq!(t.to_string(), "DEUD1");
        assert_eq!(t.country(), "DEU");
        assert_eq!(t.operator(), "D1");
        assert_eq!("DEUD1".parse::<TadigCode>().unwrap(), t);

        for s in ["DEUD", "DEUD11", "DEU-1", ""] {
            assert!(s.parse::<TadigCode>().is_err(), "{s:?} must not parse");
        }
    }
}
