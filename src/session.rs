//! What a PDU session was established against: a data network and a slice.
//!
//! Every 5G PDU session is associated with **one** DNN and **one** S-NSSAI
//! (3GPP TS 23.501), and both reach the charging record. Together they say
//! which product the usage belongs to, which is why slice-aware tariffs need
//! them and why losing either makes a data record unratable.
//!
//! # An absent slice differentiator is not a zero one
//!
//! An S-NSSAI is a one-octet Slice/Service Type, optionally followed by a
//! three-octet Slice Differentiator (TS 23.003). The SD reserves
//! `0xFFFFFF` for *"no SD value associated with the SST"*, and some protocols
//! simply omit the field instead.
//!
//! Both mean the same thing and neither means `SD = 0`, which is a perfectly
//! valid differentiator an operator may have assigned. [`Snssai::sd`] is
//! therefore an `Option` and the reserved value decodes to `None`, so a slice
//! with differentiator zero cannot be confused with a slice that has none.
//!
//! ```rust
//! use mediation::{SliceServiceType, Snssai};
//!
//! // eMBB with no differentiator — one octet on the wire.
//! let bare = Snssai::from_bytes(&[0x01]).unwrap();
//! assert_eq!(bare.sst(), SliceServiceType::EMBB);
//! assert_eq!(bare.sd(), None);
//!
//! // The reserved differentiator means the same as omitting it.
//! let reserved = Snssai::from_bytes(&[0x01, 0xFF, 0xFF, 0xFF]).unwrap();
//! assert_eq!(reserved, bare);
//!
//! // …and a differentiator of zero is a real slice, distinct from both.
//! let zero = Snssai::from_bytes(&[0x01, 0x00, 0x00, 0x00]).unwrap();
//! assert_ne!(zero, bare);
//! assert_eq!(zero.sd(), Some(0));
//! ```

use std::fmt;
use std::str::FromStr;

use crate::error::{DecodeError, ParseError};
use crate::location::UserLocation;
use crate::plmn::Mcc;

// ── SliceServiceType ─────────────────────────────────────────────────────────

/// The Slice/Service Type of an S-NSSAI: one octet.
///
/// Values 1–4 are standardised (3GPP TS 23.501); the rest are operator
/// specific. Both are accepted, and the distinction is reported rather than
/// enforced, because an operator-specific slice is perfectly valid usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SliceServiceType(u8);

impl SliceServiceType {
    /// Enhanced Mobile Broadband.
    pub const EMBB: Self = Self(1);
    /// Ultra-Reliable Low Latency Communications.
    pub const URLLC: Self = Self(2);
    /// Massive Internet of Things.
    pub const MIOT: Self = Self(3);
    /// Vehicle-to-Everything.
    pub const V2X: Self = Self(4);

    /// A slice type from its raw value.
    ///
    /// Zero is accepted: TS 23.501 does not reserve it, and rejecting a value
    /// the standard permits would drop real usage.
    #[must_use]
    pub const fn new(value: u8) -> Self {
        Self(value)
    }

    /// The raw value.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }

    /// `true` for the standardised types 1–4.
    #[must_use]
    pub const fn is_standardised(self) -> bool {
        self.0 >= 1 && self.0 <= 4
    }

    /// The standardised name, when there is one.
    #[must_use]
    pub const fn name(self) -> Option<&'static str> {
        match self.0 {
            1 => Some("eMBB"),
            2 => Some("URLLC"),
            3 => Some("MIoT"),
            4 => Some("V2X"),
            _ => None,
        }
    }
}

impl fmt::Display for SliceServiceType {
    /// Writes the raw value, not the name: the value is what the wire carries
    /// and what a tariff keys on, while the name is a convenience.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ── Snssai ───────────────────────────────────────────────────────────────────

/// Single Network Slice Selection Assistance Information.
///
/// A slice type, and optionally a differentiator that separates slices sharing
/// one type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Snssai {
    sst: SliceServiceType,
    sd: Option<u32>,
}

impl Snssai {
    /// The reserved differentiator meaning *no SD associated with this SST*.
    pub const NO_SD: u32 = 0x00FF_FFFF;

    /// A slice identified by type alone.
    #[must_use]
    pub const fn new(sst: SliceServiceType) -> Self {
        Self { sst, sd: None }
    }

    /// A slice identified by type and differentiator.
    ///
    /// `None` when `sd` does not fit 24 bits, or when it is the reserved
    /// [`NO_SD`](Self::NO_SD) value — which must be expressed by
    /// [`new`](Self::new) so that one meaning has one representation.
    #[must_use]
    pub const fn with_sd(sst: SliceServiceType, sd: u32) -> Option<Self> {
        if sd > 0x00FF_FFFF || sd == Self::NO_SD {
            return None;
        }
        Some(Self { sst, sd: Some(sd) })
    }

    /// The slice type.
    #[must_use]
    pub const fn sst(self) -> SliceServiceType {
        self.sst
    }

    /// The differentiator, when the slice has one.
    #[must_use]
    pub const fn sd(self) -> Option<u32> {
        self.sd
    }

    /// Decode from the wire: one octet of SST, optionally three of SD.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] for any length other than 1 or 4. The two lengths are
    /// the only ones TS 23.003 defines, and a three- or five-octet field is a
    /// misaligned read rather than a shorter slice identity.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        match bytes.len() {
            1 => Ok(Self::new(SliceServiceType::new(bytes[0]))),
            4 => {
                let sd =
                    (u32::from(bytes[1]) << 16) | (u32::from(bytes[2]) << 8) | u32::from(bytes[3]);
                let sst = SliceServiceType::new(bytes[0]);
                // The reserved value means "no SD", which is the same slice as
                // one encoded in a single octet.
                Ok(if sd == Self::NO_SD {
                    Self::new(sst)
                } else {
                    Self { sst, sd: Some(sd) }
                })
            }
            _ => Err(DecodeError::new(0, "1 octet (SST) or 4 (SST and SD)")),
        }
    }

    /// Encode to the wire, writing into `out` and returning the octet count.
    ///
    /// Writes one octet for a slice with no differentiator and four for one
    /// with. The reserved value is never written: a slice with no SD is encoded
    /// short, so one meaning has one encoding.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] when `out` is too short.
    pub fn to_bytes(self, out: &mut [u8]) -> Result<usize, DecodeError> {
        match self.sd {
            None => {
                if out.is_empty() {
                    return Err(DecodeError::new(0, "a buffer of at least 1 octet"));
                }
                out[0] = self.sst.get();
                Ok(1)
            }
            Some(sd) => {
                if out.len() < 4 {
                    return Err(DecodeError::new(0, "a buffer of at least 4 octets"));
                }
                out[0] = self.sst.get();
                out[1] = ((sd >> 16) & 0xFF) as u8;
                out[2] = ((sd >> 8) & 0xFF) as u8;
                out[3] = (sd & 0xFF) as u8;
                Ok(4)
            }
        }
    }
}

impl fmt::Display for Snssai {
    /// Writes `sst` or `sst-sd`, the SD as six lower-case hex digits — the
    /// form 3GPP management interfaces use.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.sd {
            None => write!(f, "{}", self.sst),
            Some(sd) => write!(f, "{}-{sd:06x}", self.sst),
        }
    }
}

/// The shape [`Snssai`] accepts, as rendered in a [`ParseError`].
const SNSSAI_FORMAT: &str =
    "a slice type, optionally with a six-hex-digit differentiator, such as 1 or 1-0000a1";

impl FromStr for Snssai {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseError::format("Snssai", s, SNSSAI_FORMAT);
        let t = s.trim();
        let (sst, sd) = match t.split_once('-') {
            None => (t, None),
            Some((sst, sd)) => (sst, Some(sd)),
        };
        // Canonical only: decimal SST without sign or leading zero, SD exactly
        // six hex digits (audit S3).
        let canonical_sst = !sst.is_empty()
            && sst.len() <= 3
            && sst.bytes().all(|b| b.is_ascii_digit())
            && (sst == "0" || !sst.starts_with('0'));
        if !canonical_sst {
            return Err(err());
        }
        let sst = SliceServiceType::new(sst.parse().map_err(|_| err())?);
        match sd {
            None => Ok(Self::new(sst)),
            Some(sd) if sd.len() == 6 && sd.bytes().all(|b| b.is_ascii_hexdigit()) => {
                let sd = u32::from_str_radix(sd, 16).map_err(|_| err())?;
                Self::with_sd(sst, sd).ok_or_else(err)
            }
            Some(_) => Err(err()),
        }
    }
}

// ── Dnn ──────────────────────────────────────────────────────────────────────

/// A Data Network Name — the network a PDU session connects to.
///
/// The 5G successor to the APN, and the same shape: dot-separated labels, up to
/// 100 characters, each label at most 63 (3GPP TS 23.003 §9.1). Stored
/// lower-cased, because a DNN is
/// case-insensitive and a value spelled two ways produces two products.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Dnn(String);

/// The shape [`Dnn`] accepts, as rendered in a [`ParseError`].
const DNN_FORMAT: &str =
    "up to 100 characters of dot-separated labels, such as internet or ims.mnc002.mcc262.gprs";

impl Dnn {
    /// The name as stored, lower-cased.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The dot-separated labels, in order.
    #[must_use]
    pub fn labels(&self) -> std::str::Split<'_, char> {
        self.0.split('.')
    }

    /// The network identifier: every label before a trailing operator
    /// identifier `mnc<MNC>.mcc<MCC>.gprs` (3GPP TS 23.003 §9.1), or the whole
    /// name when it has none (audit S2).
    ///
    /// The operator identifier repeats the serving network and carries no
    /// product information; the network identifier may have several labels.
    ///
    /// ```rust
    /// use mediation::Dnn;
    ///
    /// let dnn: Dnn = "corp.example.com.mnc002.mcc262.gprs".parse().unwrap();
    /// assert_eq!(dnn.network_identifier(), "corp.example.com");
    /// ```
    #[must_use]
    pub fn network_identifier(&self) -> &str {
        let name = &self.0[..];
        let operator = |rest: &str| {
            let three = |label: &str, tag: &str| {
                label
                    .strip_prefix(tag)
                    .is_some_and(|d| d.len() == 3 && d.bytes().all(|b| b.is_ascii_digit()))
            };
            let mut labels = rest.split('.');
            matches!(
                (labels.next(), labels.next(), labels.next(), labels.next()),
                (Some(mnc), Some(mcc), Some("gprs"), None) if three(mnc, "mnc") && three(mcc, "mcc")
            )
        };
        name.match_indices('.')
            .map(|(i, _)| i)
            .find(|&i| operator(&name[i + 1..]))
            .map_or(name, |i| &name[..i])
    }
}

impl fmt::Display for Dnn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Dnn {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseError::format("Dnn", s, DNN_FORMAT);
        let t = s.trim();
        let ok = !t.is_empty()
            && t.len() <= 100
            && !t.starts_with('.')
            && !t.ends_with('.')
            && t.split('.')
                .all(|label| !label.is_empty() && label.len() <= 63)
            && t.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.');
        if ok {
            Ok(Self(t.to_ascii_lowercase()))
        } else {
            Err(err())
        }
    }
}

// ── RatType ──────────────────────────────────────────────────────────────────

/// The radio technology a session ran over.
///
/// Present because it is a **priced** distinction — a change of it closes a
/// container (see [`crate::container::ChangeCondition::RatChange`]) precisely
/// so the two sides can be rated differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RatType {
    /// 5G New Radio.
    Nr,
    /// LTE / E-UTRAN.
    Eutran,
    /// 3G / UTRAN.
    Utran,
    /// 2G / GERAN.
    Geran,
    /// Wireless LAN, including untrusted non-3GPP access.
    Wlan,
    /// Fixed-line access.
    Wireline,
    /// Non-terrestrial — satellite access.
    ///
    /// Its own value rather than folded into [`Nr`](Self::Nr): a beam crossing
    /// a border makes jurisdiction a computed property, and a record that
    /// cannot say it came from orbit cannot raise the question.
    NonTerrestrial,
}

impl RatType {
    /// Every variant, for exhaustive iteration in tests and mappings.
    pub const ALL: &'static [Self] = &[
        Self::Nr,
        Self::Eutran,
        Self::Utran,
        Self::Geran,
        Self::Wlan,
        Self::Wireline,
        Self::NonTerrestrial,
    ];

    /// The canonical codes, in the order of [`ALL`](Self::ALL).
    pub const CODES: &'static [&'static str] = &[
        "NR",
        "EUTRAN",
        "UTRAN",
        "GERAN",
        "WLAN",
        "WIRELINE",
        "NON_TERRESTRIAL",
    ];

    /// `true` for access whose serving cell does not fix a country.
    ///
    /// Satellite access is the case: a beam crosses borders, so the serving
    /// network alone no longer determines which roaming, tax and lawful
    /// interception regime applies.
    #[must_use]
    pub const fn jurisdiction_is_ambiguous(self) -> bool {
        matches!(self, Self::NonTerrestrial)
    }

    /// The canonical string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Nr => "NR",
            Self::Eutran => "EUTRAN",
            Self::Utran => "UTRAN",
            Self::Geran => "GERAN",
            Self::Wlan => "WLAN",
            Self::Wireline => "WIRELINE",
            Self::NonTerrestrial => "NON_TERRESTRIAL",
        }
    }
}

impl fmt::Display for RatType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RatType {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim().to_ascii_uppercase();
        Self::ALL
            .iter()
            .find(|r| r.as_str() == t)
            .copied()
            .ok_or_else(|| ParseError::one_of("RatType", s, Self::CODES))
    }
}

// ── ChargingCharacteristics ──────────────────────────────────────────────────

/// The sixteen operator-defined charging behaviours a session ran under
/// (3GPP TS 32.251 Annex A, carried as `chargingCharacteristics` in TS 32.298).
///
/// The HSS supplies it with the subscription, the SGSN or MME forwards it at
/// IP-CAN bearer activation, and it reaches every CDR the session produces. It
/// is what decides whether records are generated at all, at what time and
/// volume limits, and which gateway they go to — so a mediation layer that
/// discards it has thrown away the record's own account of why it exists.
///
/// # Sixteen bits, and the crate reads none of them
///
/// TS 32.251 §A.1 is explicit: the parameter is "a string of 16 bits designated
/// as Behaviours (B), **freely defined by Operators**", each pointing at a
/// behaviour "defined on a per operator basis". `0x0800` is prepaid at one
/// operator and a lab profile at the next, and the specification's own tables
/// are labelled informative examples.
///
/// So this type carries the bits, compares them, and renders them. It has no
/// `is_prepaid`, and it will not get one — the same stance the crate takes on
/// E.212 MNC widths, EEA membership and high-risk number ranges. Guessing here
/// would misroute records with a plausible-looking answer, which is worse than
/// no answer.
///
/// ```rust
/// use mediation::ChargingCharacteristics;
///
/// // Four hex digits, the form an operator's provisioning system writes.
/// let profile: ChargingCharacteristics = "0800".parse().unwrap();
/// assert_eq!(profile.to_string(), "0800");
/// assert_eq!(profile.to_bytes(), [0x08, 0x00]);
///
/// // Behaviour 11 is set. What behaviour 11 *means* is the operator's.
/// assert!(profile.behaviour(11));
/// assert!(!profile.behaviour(0));
/// assert_eq!(profile.set_behaviours().collect::<Vec<_>>(), vec![11]);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ChargingCharacteristics(u16);

/// The shape [`ChargingCharacteristics`] accepts, as rendered in a [`ParseError`].
const CHARGING_CHARACTERISTICS_FORMAT: &str = "exactly four hexadecimal digits, such as 0800";

impl ChargingCharacteristics {
    /// The wire length, in octets.
    pub const OCTETS: usize = 2;

    /// No behaviour selected.
    ///
    /// Distinct from *absent*: a record that carried `0000` said the operator
    /// selected nothing, and one that carried no field said nothing at all.
    /// [`UsageRecord`](crate::UsageRecord) keeps the difference in an `Option`.
    pub const NONE: Self = Self(0);

    /// A profile from its sixteen bits.
    ///
    /// Bit `n` of this value is behaviour `n`, counting from the least
    /// significant — which makes [`to_bytes`](Self::to_bytes) the big-endian
    /// pair and the [`Display`](std::fmt::Display) form the same four hex
    /// digits an operator's provisioning system shows.
    #[must_use]
    pub const fn from_bits(bits: u16) -> Self {
        Self(bits)
    }

    /// The sixteen bits.
    #[must_use]
    pub const fn bits(self) -> u16 {
        self.0
    }

    /// Decode the two-octet field, most significant octet first.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] for any length but two.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        match bytes {
            [high, low] => Ok(Self(u16::from_be_bytes([*high, *low]))),
            _ => Err(DecodeError::new(0, "exactly 2 octets")),
        }
    }

    /// Encode to the two-octet field.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; Self::OCTETS] {
        self.0.to_be_bytes()
    }

    /// Whether behaviour `index` is selected. `false` for an index past 15.
    #[must_use]
    pub const fn behaviour(self, index: u8) -> bool {
        index < 16 && self.0 & (1u16 << index) != 0
    }

    /// The indices of every behaviour selected, ascending.
    ///
    /// The form a routing table is keyed by, and the form a report reads: an
    /// operator's profile document lists behaviour numbers, not a hex value.
    pub fn set_behaviours(self) -> impl Iterator<Item = u8> {
        (0u8..16).filter(move |index| self.behaviour(*index))
    }

    /// How many behaviours are selected.
    #[must_use]
    pub const fn count(self) -> u32 {
        self.0.count_ones()
    }

    /// `true` when no behaviour is selected.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for ChargingCharacteristics {
    /// Four upper-case hex digits, always — the form the field is written in,
    /// and one spelling per value so two records cannot disagree in a report.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04X}", self.0)
    }
}

impl FromStr for ChargingCharacteristics {
    type Err = ParseError;

    /// Parses four hex digits, either case.
    ///
    /// Exactly four: `800` is not `0800` in a field whose bit positions are the
    /// whole meaning, and accepting a short form would silently shift every
    /// behaviour index by four.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim();
        if t.len() != 4 || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ParseError::format(
                "ChargingCharacteristics",
                s,
                CHARGING_CHARACTERISTICS_FORMAT,
            ));
        }
        u16::from_str_radix(t, 16).map(Self).map_err(|_| {
            ParseError::format(
                "ChargingCharacteristics",
                s,
                CHARGING_CHARACTERISTICS_FORMAT,
            )
        })
    }
}

// ── SessionContext ───────────────────────────────────────────────────────────

/// What a data session was established against.
///
/// Every field optional, because real records are: a circuit-switched call has
/// no DNN, and a pre-5G context has no slice.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionContext {
    /// The data network the session connected to.
    pub dnn: Option<Dnn>,
    /// The network slice the session ran on.
    pub snssai: Option<Snssai>,
    /// The radio technology the session ran over.
    pub rat: Option<RatType>,
    /// Where the subscriber was, in whichever form the access reported.
    pub location: Option<UserLocation>,
    /// The operator-defined charging behaviours the session ran under.
    ///
    /// `None` when the record carried no field, which is not the same as
    /// [`ChargingCharacteristics::NONE`] — see that constant.
    pub charging_characteristics: Option<ChargingCharacteristics>,
}

impl SessionContext {
    /// No context at all.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            dnn: None,
            snssai: None,
            rat: None,
            location: None,
            charging_characteristics: None,
        }
    }

    /// `true` when nothing about the session's context was recorded.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.dnn.is_none()
            && self.snssai.is_none()
            && self.rat.is_none()
            && self.location.is_none()
            && self.charging_characteristics.is_none()
    }

    /// The same context with a data network.
    #[must_use]
    pub fn with_dnn(mut self, dnn: Dnn) -> Self {
        self.dnn = Some(dnn);
        self
    }

    /// The same context with a slice.
    #[must_use]
    pub const fn with_snssai(mut self, snssai: Snssai) -> Self {
        self.snssai = Some(snssai);
        self
    }

    /// The same context with a charging-characteristics profile.
    #[must_use]
    pub const fn with_charging_characteristics(mut self, cc: ChargingCharacteristics) -> Self {
        self.charging_characteristics = Some(cc);
        self
    }

    /// The same context with a radio technology.
    #[must_use]
    pub const fn with_rat(mut self, rat: RatType) -> Self {
        self.rat = Some(rat);
        self
    }

    /// The same context with the location the access reported.
    #[must_use]
    pub const fn with_location(mut self, location: UserLocation) -> Self {
        self.location = Some(location);
        self
    }

    /// The country the session was served in, if a location was recorded.
    ///
    /// The only question this crate asks of a location: a country is what
    /// regulation, settlement and roaming turn on, and it needs no cell
    /// database. Anything finer is a caller's own lookup.
    #[must_use]
    pub const fn serving_country(&self) -> Option<Mcc> {
        match self.location {
            Some(location) => Some(location.mcc()),
            None => None,
        }
    }

    /// `true` when the access makes the session's jurisdiction ambiguous.
    ///
    /// `false` when the access type is unknown: an unrecorded technology is not
    /// evidence of anything, and treating it as ambiguous would flag every
    /// legacy record.
    #[must_use]
    pub fn jurisdiction_is_ambiguous(&self) -> bool {
        self.rat.is_some_and(RatType::jurisdiction_is_ambiguous)
    }
}

impl fmt::Display for SessionContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut wrote = false;
        for part in [
            self.dnn.as_ref().map(ToString::to_string),
            self.snssai.map(|s| format!("slice {s}")),
            self.rat.map(|r| r.to_string()),
            self.location.map(|l| l.to_string()),
            self.charging_characteristics
                .map(|c| format!("charging characteristics {c}")),
        ]
        .into_iter()
        .flatten()
        {
            if wrote {
                f.write_str(" / ")?;
            }
            f.write_str(&part)?;
            wrote = true;
        }
        if !wrote {
            f.write_str("no context")?;
        }
        Ok(())
    }
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::{
        ChargingCharacteristics, Dnn, RatType, SessionContext, SliceServiceType, Snssai,
        UserLocation,
    };
    use serde::de::{self, Visitor};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::fmt;

    macro_rules! string_serde {
        ($ty:ty, $expecting:literal) => {
            impl Serialize for $ty {
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

    string_serde!(Dnn, "a data network name such as \"internet\"");
    string_serde!(RatType, "a radio access type code such as \"NR\"");
    string_serde!(Snssai, "a slice identity such as \"1-0000a1\"");
    string_serde!(
        ChargingCharacteristics,
        "four hexadecimal digits such as \"0800\""
    );

    impl Serialize for SliceServiceType {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_u8(self.get())
        }
    }

    impl<'de> Deserialize<'de> for SliceServiceType {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            u8::deserialize(deserializer).map(SliceServiceType::new)
        }
    }

    /// Field names are part of the public representation and covered by semver.
    #[derive(Serialize, Deserialize)]
    #[serde(rename = "SessionContext")]
    struct Repr {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dnn: Option<Dnn>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        snssai: Option<Snssai>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rat: Option<RatType>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        location: Option<UserLocation>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        charging_characteristics: Option<ChargingCharacteristics>,
    }

    impl Serialize for SessionContext {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            Repr {
                dnn: self.dnn.clone(),
                snssai: self.snssai,
                rat: self.rat,
                location: self.location,
                charging_characteristics: self.charging_characteristics,
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for SessionContext {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = Repr::deserialize(deserializer)?;
            Ok(SessionContext {
                dnn: r.dnn,
                snssai: r.snssai,
                rat: r.rat,
                location: r.location,
                charging_characteristics: r.charging_characteristics,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug the `Option` exists to prevent: a differentiator of zero is a
    /// real slice, and the reserved value means there is none.
    #[test]
    fn an_absent_differentiator_is_not_a_zero_one() {
        let bare = Snssai::from_bytes(&[0x01]).unwrap();
        let reserved = Snssai::from_bytes(&[0x01, 0xFF, 0xFF, 0xFF]).unwrap();
        let zero = Snssai::from_bytes(&[0x01, 0x00, 0x00, 0x00]).unwrap();

        assert_eq!(bare, reserved, "the reserved value means no SD");
        assert_eq!(bare.sd(), None);
        assert_ne!(zero, bare);
        assert_eq!(zero.sd(), Some(0));
    }

    /// One meaning, one encoding: a slice with no SD is written short, never
    /// with the reserved value.
    #[test]
    fn a_slice_with_no_differentiator_encodes_short() {
        let bare = Snssai::new(SliceServiceType::EMBB);
        let mut buffer = [0u8; 4];
        assert_eq!(bare.to_bytes(&mut buffer).unwrap(), 1);
        assert_eq!(buffer[0], 0x01);

        // …and the constructor refuses to build the reserved value directly.
        assert_eq!(Snssai::with_sd(SliceServiceType::EMBB, Snssai::NO_SD), None);
    }

    #[test]
    fn slice_identities_round_trip_over_the_wire() {
        let cases = [
            (Snssai::new(SliceServiceType::EMBB), 1usize),
            (
                Snssai::with_sd(SliceServiceType::URLLC, 0x00_00A1).unwrap(),
                4,
            ),
            (Snssai::with_sd(SliceServiceType::new(255), 0).unwrap(), 4),
            (
                Snssai::with_sd(SliceServiceType::MIOT, 0x00FF_FFFE).unwrap(),
                4,
            ),
        ];
        for (slice, expected_len) in cases {
            let mut buffer = [0u8; 4];
            let written = slice.to_bytes(&mut buffer).unwrap();
            assert_eq!(written, expected_len, "{slice}");
            assert_eq!(Snssai::from_bytes(&buffer[..written]).unwrap(), slice);
        }
    }

    #[test]
    fn slice_identities_round_trip_as_strings() {
        for text in ["1", "2-0000a1", "255-000000", "3-fffffe"] {
            let slice: Snssai = text.parse().unwrap();
            assert_eq!(slice.to_string(), text);
        }
        for bad in ["", "1-", "1-abc", "1-0000A1G", "256", "1-ffffff"] {
            assert!(bad.parse::<Snssai>().is_err(), "{bad:?} must not parse");
        }
    }

    /// A length the standard does not define is a misaligned read, not a
    /// shorter slice identity.
    #[test]
    fn only_the_two_defined_lengths_decode() {
        assert!(Snssai::from_bytes(&[]).is_err());
        assert!(Snssai::from_bytes(&[0x01, 0x00]).is_err());
        assert!(Snssai::from_bytes(&[0x01, 0x00, 0x00]).is_err());
        assert!(Snssai::from_bytes(&[0x01, 0x00, 0x00, 0x00, 0x00]).is_err());
    }

    #[test]
    fn standardised_slice_types_are_named() {
        assert_eq!(SliceServiceType::EMBB.name(), Some("eMBB"));
        assert_eq!(SliceServiceType::V2X.name(), Some("V2X"));
        assert_eq!(SliceServiceType::new(200).name(), None);
        assert!(!SliceServiceType::new(200).is_standardised());
        assert!(!SliceServiceType::new(0).is_standardised());
    }

    /// A DNN is case-insensitive, so it is stored one way — two spellings would
    /// be two products.
    #[test]
    fn a_data_network_name_has_one_spelling() {
        let lower: Dnn = "internet".parse().unwrap();
        for spelling in ["INTERNET", "Internet", " internet "] {
            assert_eq!(spelling.parse::<Dnn>().unwrap(), lower);
        }
        assert_eq!(lower.to_string(), "internet");
    }

    #[test]
    fn a_data_network_name_splits_into_labels() {
        let dnn: Dnn = "ims.mnc002.mcc262.gprs".parse().unwrap();
        assert_eq!(dnn.network_identifier(), "ims");
        assert_eq!(dnn.labels().count(), 4);

        let bare: Dnn = "internet".parse().unwrap();
        assert_eq!(bare.network_identifier(), "internet");
    }

    #[test]
    fn malformed_data_network_names_are_rejected() {
        for bad in [
            "",
            ".internet",
            "internet.",
            "a..b",
            "has space",
            "under_score",
        ] {
            assert!(bad.parse::<Dnn>().is_err(), "{bad:?} must not parse");
        }
        assert!("x".repeat(101).parse::<Dnn>().is_err());
        assert!("x".repeat(64).parse::<Dnn>().is_err());
        let longest = format!("{}.{}", "x".repeat(63), "y".repeat(36));
        assert!(longest.parse::<Dnn>().is_ok());
    }

    /// Satellite access is its own value because it makes jurisdiction a
    /// computed property rather than a lookup.
    #[test]
    fn only_non_terrestrial_access_makes_jurisdiction_ambiguous() {
        for rat in RatType::ALL {
            let expected = *rat == RatType::NonTerrestrial;
            assert_eq!(rat.jurisdiction_is_ambiguous(), expected, "{rat}");
        }

        let orbit = SessionContext::none().with_rat(RatType::NonTerrestrial);
        assert!(orbit.jurisdiction_is_ambiguous());

        // An unrecorded technology is not evidence of anything.
        assert!(!SessionContext::none().jurisdiction_is_ambiguous());
    }

    #[test]
    fn a_session_context_renders_what_it_has() {
        assert_eq!(SessionContext::none().to_string(), "no context");

        let full = SessionContext::none()
            .with_dnn("internet".parse().unwrap())
            .with_snssai(Snssai::with_sd(SliceServiceType::EMBB, 0xA1).unwrap())
            .with_rat(RatType::Nr);
        assert_eq!(full.to_string(), "internet / slice 1-0000a1 / NR");
        assert!(!full.is_empty());
    }

    /// A location answers exactly one question here — the country — and a
    /// context with no location answers it with `None` rather than a guess.
    #[test]
    fn a_location_yields_the_serving_country_and_nothing_finer() {
        use crate::location::{Ncgi, Nci, UserLocation};

        let cell = UserLocation::Ncgi(Ncgi::new(
            "262-02".parse().unwrap(),
            Nci::new(0x0_1234_5678).unwrap(),
        ));
        let context = SessionContext::none()
            .with_rat(RatType::Nr)
            .with_location(cell);

        assert_eq!(context.serving_country(), Some("262".parse().unwrap()));
        assert!(context.to_string().contains("ncgi:262-02-"));

        assert_eq!(SessionContext::none().serving_country(), None);
        assert!(!SessionContext::none().with_location(cell).is_empty());
    }

    #[test]
    fn rat_codes_round_trip() {
        assert_eq!(RatType::ALL.len(), RatType::CODES.len());
        for (rat, code) in RatType::ALL.iter().zip(RatType::CODES) {
            assert_eq!(rat.as_str(), *code);
            assert_eq!(code.parse::<RatType>().unwrap(), *rat);
        }
        assert!("SATELLITE".parse::<RatType>().is_err());
    }
}
