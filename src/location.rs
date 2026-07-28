//! Where a record was produced: cells, service areas and tracking areas.
//!
//! Every charging record carries some form of User Location Information, and
//! which form depends on the access generation. They are not interchangeable,
//! and the crate keeps them as distinct types so a 2G Cell Global Identity can
//! never be compared against a 5G one and found equal by accident.
//!
//! # A cell identity cannot be decomposed without the operator's configuration
//!
//! This is the same trap as the two-versus-three-digit MNC, in a new place. The
//! E-UTRAN Cell Identity is 28 bits and the NR Cell Identity is 36, and both are
//! split internally between a base-station identifier and a cell identifier —
//! but **where the split falls is a per-operator deployment choice**, not
//! something the value carries:
//!
//! | Identity | Total | Base station | Notes |
//! |---|---|---|---|
//! | [`Eci`] | 28 bits | 20 (macro) or 28 (home) | a home eNB consumes the whole value |
//! | [`Nci`] | 36 bits | 22 to 32, configured | the remainder is the cell |
//!
//! So [`Eci::enb_id`] takes an [`EnbKind`] and [`Nci::gnb_id`] takes a
//! [`GnbIdLength`]. Neither can be guessed, and a crate that guessed would
//! attribute a cell to the wrong base station silently — which is worse than
//! not answering, because a coverage or fraud report built on it looks fine.
//!
//! ```rust
//! use mediation::{EnbKind, Eci, GnbIdLength, Nci};
//!
//! // The same 28 bits mean two different things depending on the deployment.
//! let eci = Eci::new(0x00A1_B2C3).unwrap();
//! assert_eq!(eci.enb_id(EnbKind::Macro), 0x0000_A1B2);
//! assert_eq!(eci.cell_id(EnbKind::Macro), Some(0xC3));
//! assert_eq!(eci.enb_id(EnbKind::Home), 0x00A1_B2C3);
//! assert_eq!(eci.cell_id(EnbKind::Home), None, "a home eNB has no cell part");
//!
//! // And an NR cell identity needs the operator's configured gNB-ID length.
//! let nci = Nci::new(0x0_1234_5678).unwrap();
//! assert_ne!(
//!     nci.gnb_id(GnbIdLength::new(22).unwrap()),
//!     nci.gnb_id(GnbIdLength::new(24).unwrap()),
//! );
//! ```
//!
//! # A tracking area code carries its width
//!
//! A tracking area code is 16 bits in EPS and 24 bits in 5GS, and an EPS code
//! appears in 5GS **zero-padded** to 24 bits. Storing the number alone would
//! make `0x0001` from a 4G record and `0x000001` from a 5G one look like
//! different values in one direction and equal in the other, depending on which
//! way a comparison was written. [`TrackingAreaCode`] therefore carries its
//! width the way [`Mnc`](crate::Mnc) carries its digit count, and
//! [`widened`](TrackingAreaCode::widened) is the only way across.

use std::fmt;
use std::str::FromStr;

use crate::error::ParseError;
use crate::plmn::{Mcc, PlmnId};

// ── The reserved "no valid area" codes ───────────────────────────────────────

/// `true` when `value` is one of the two codes reserved to mean *no valid
/// area*: all zeros, or all ones less one.
///
/// The same rule at every width — `0000`/`FFFE` at 16 bits, `000000`/`FFFFFE`
/// at 24 — so it is written once. A location area, an EPS tracking area and a
/// 5GS tracking area all use it, and three copies of one rule eventually
/// disagree at one width.
const fn is_reserved_area(value: u32, bits: u8) -> bool {
    value == 0 || value == (1u32 << bits) - 2
}

// ── Location area, cell and service area (2G/3G) ─────────────────────────────

/// A Location Area Code: 16 bits, excluding the two reserved values.
///
/// `0x0000` and `0xFFFE` are reserved to mean *no valid location area* — a
/// handset that has never attached, or one whose stored area was deleted. They
/// reach a charging record when an element copies its internal "unknown"
/// marker into the field, and reading them as ordinary areas puts real usage in
/// a location area that does not exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Lac(u16);

impl Lac {
    /// `true` when the value is reserved to mean *no valid location area*.
    #[must_use]
    pub const fn is_reserved(value: u16) -> bool {
        is_reserved_area(value as u32, 16)
    }

    /// A location area code, or `None` for a reserved value.
    #[must_use]
    pub const fn new(value: u16) -> Option<Self> {
        if Self::is_reserved(value) {
            None
        } else {
            Some(Self(value))
        }
    }

    /// The numeric value.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

impl fmt::Display for Lac {
    /// Four uppercase hex digits, the form every element logs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04X}", self.0)
    }
}

/// A Cell Identity: two octets, within a location area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CellId(u16);

impl CellId {
    /// A cell identity. Every 16-bit value is valid.
    #[must_use]
    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    /// The numeric value.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

impl fmt::Display for CellId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04X}", self.0)
    }
}

/// A Service Area Code: two octets, within a location area.
///
/// A service area is a UTRAN grouping of cells, not a cell — which is why it is
/// a separate type from [`CellId`] rather than the same number under two names.
/// A record reporting an SAI locates the subscriber less precisely than one
/// reporting a CGI, and a report that mixed them would overstate its precision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ServiceAreaCode(u16);

impl ServiceAreaCode {
    /// A service area code. Every 16-bit value is valid.
    #[must_use]
    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    /// The numeric value.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

impl fmt::Display for ServiceAreaCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04X}", self.0)
    }
}

/// A Cell Global Identity: network, location area and cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cgi {
    plmn: PlmnId,
    lac: Lac,
    cell: CellId,
}

impl Cgi {
    /// A cell global identity from its parts.
    #[must_use]
    pub const fn new(plmn: PlmnId, lac: Lac, cell: CellId) -> Self {
        Self { plmn, lac, cell }
    }

    /// The network.
    #[must_use]
    pub const fn plmn(self) -> PlmnId {
        self.plmn
    }

    /// The location area.
    #[must_use]
    pub const fn lac(self) -> Lac {
        self.lac
    }

    /// The cell.
    #[must_use]
    pub const fn cell(self) -> CellId {
        self.cell
    }
}

impl fmt::Display for Cgi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}-{}", self.plmn, self.lac, self.cell)
    }
}

/// A Service Area Identity: network, location area and service area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sai {
    plmn: PlmnId,
    lac: Lac,
    sac: ServiceAreaCode,
}

impl Sai {
    /// A service area identity from its parts.
    #[must_use]
    pub const fn new(plmn: PlmnId, lac: Lac, sac: ServiceAreaCode) -> Self {
        Self { plmn, lac, sac }
    }

    /// The network.
    #[must_use]
    pub const fn plmn(self) -> PlmnId {
        self.plmn
    }

    /// The location area.
    #[must_use]
    pub const fn lac(self) -> Lac {
        self.lac
    }

    /// The service area.
    #[must_use]
    pub const fn sac(self) -> ServiceAreaCode {
        self.sac
    }
}

impl fmt::Display for Sai {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}-{}", self.plmn, self.lac, self.sac)
    }
}

// ── E-UTRAN (4G) ─────────────────────────────────────────────────────────────

/// Which kind of eNodeB an [`Eci`] belongs to, and therefore where it splits.
///
/// Not derivable from the value: the same 28 bits are a macro eNB with a cell
/// beneath it, or one home eNB and nothing else. A caller knows which from the
/// deployment; the number does not say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EnbKind {
    /// A macro eNodeB: a 20-bit identifier and an 8-bit cell beneath it.
    Macro,
    /// A home eNodeB: a 28-bit identifier occupying the whole value.
    Home,
}

impl EnbKind {
    /// Every variant, for exhaustive iteration in tests and mappings.
    pub const ALL: &'static [Self] = &[Self::Macro, Self::Home];

    /// The canonical code of each variant, in [`ALL`](Self::ALL) order.
    pub const CODES: &'static [&'static str] = &["MACRO", "HOME"];

    /// How many of the 28 bits the base-station identifier occupies.
    #[must_use]
    pub const fn id_bits(self) -> u8 {
        match self {
            Self::Macro => 20,
            Self::Home => 28,
        }
    }

    /// The canonical string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Macro => "MACRO",
            Self::Home => "HOME",
        }
    }
}

impl fmt::Display for EnbKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for EnbKind {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .find(|k| k.as_str().eq_ignore_ascii_case(s.trim()))
            .copied()
            .ok_or_else(|| ParseError::format("EnbKind", s, "MACRO or HOME"))
    }
}

/// An E-UTRAN Cell Identity: 28 bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Eci(u32);

impl Eci {
    /// How many bits the identity occupies.
    pub const BITS: u32 = 28;

    /// One more than the largest valid value.
    const LIMIT: u32 = 1 << Self::BITS;

    /// An identity, or `None` when the value does not fit in 28 bits.
    #[must_use]
    pub const fn new(value: u32) -> Option<Self> {
        if value < Self::LIMIT {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The whole 28-bit value.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }

    /// The base-station identifier, given the deployment kind.
    #[must_use]
    pub const fn enb_id(self, kind: EnbKind) -> u32 {
        match kind {
            EnbKind::Macro => self.0 >> 8,
            EnbKind::Home => self.0,
        }
    }

    /// The cell beneath the base station, or `None` for a home eNodeB.
    ///
    /// A home eNodeB identifier fills all 28 bits, so there is no cell part to
    /// return. Returning `0` there would be a plausible-looking value for a
    /// cell that does not exist.
    #[must_use]
    pub const fn cell_id(self, kind: EnbKind) -> Option<u8> {
        match kind {
            EnbKind::Macro => Some((self.0 & 0xFF) as u8),
            EnbKind::Home => None,
        }
    }
}

impl fmt::Display for Eci {
    /// Seven uppercase hex digits — 28 bits, never truncated.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:07X}", self.0)
    }
}

/// An E-UTRAN Cell Global Identity: network plus [`Eci`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ecgi {
    plmn: PlmnId,
    eci: Eci,
}

impl Ecgi {
    /// An E-UTRAN cell global identity from its parts.
    #[must_use]
    pub const fn new(plmn: PlmnId, eci: Eci) -> Self {
        Self { plmn, eci }
    }

    /// The network.
    #[must_use]
    pub const fn plmn(self) -> PlmnId {
        self.plmn
    }

    /// The cell identity.
    #[must_use]
    pub const fn eci(self) -> Eci {
        self.eci
    }
}

impl fmt::Display for Ecgi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.plmn, self.eci)
    }
}

// ── NR (5G) ──────────────────────────────────────────────────────────────────

/// How many of an [`Nci`]'s 36 bits the gNodeB identifier occupies.
///
/// Between 22 and 32, chosen per deployment. The value is not recoverable from
/// a cell identity, so it travels alongside — the same shape as
/// [`MncWidth`](crate::MncWidth).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GnbIdLength(u8);

impl GnbIdLength {
    /// The shortest permitted gNodeB identifier.
    pub const MIN: u8 = 22;
    /// The longest permitted gNodeB identifier.
    pub const MAX: u8 = 32;

    /// A length, or `None` outside 22 to 32 inclusive.
    #[must_use]
    pub const fn new(bits: u8) -> Option<Self> {
        if bits >= Self::MIN && bits <= Self::MAX {
            Some(Self(bits))
        } else {
            None
        }
    }

    /// The number of bits.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }

    /// How many bits are left for the cell beneath the gNodeB.
    #[must_use]
    pub const fn cell_bits(self) -> u8 {
        Nci::BITS as u8 - self.0
    }
}

impl fmt::Display for GnbIdLength {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for GnbIdLength {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseError::format("GnbIdLength", s, "a bit count from 22 to 32");
        Self::new(s.trim().parse::<u8>().map_err(|_| err())?).ok_or_else(err)
    }
}

/// An NR Cell Identity: 36 bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Nci(u64);

impl Nci {
    /// How many bits the identity occupies.
    pub const BITS: u32 = 36;

    /// One more than the largest valid value.
    const LIMIT: u64 = 1 << Self::BITS;

    /// An identity, or `None` when the value does not fit in 36 bits.
    #[must_use]
    pub const fn new(value: u64) -> Option<Self> {
        if value < Self::LIMIT {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The whole 36-bit value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// The gNodeB identifier, given the deployment's configured length.
    #[must_use]
    pub const fn gnb_id(self, length: GnbIdLength) -> u64 {
        self.0 >> length.cell_bits()
    }

    /// The cell beneath the gNodeB, given the deployment's configured length.
    ///
    /// A 32-bit gNodeB identifier leaves four bits, and a 22-bit one leaves
    /// fourteen — so unlike [`Eci::cell_id`] there is always a cell part.
    #[must_use]
    pub const fn cell_id(self, length: GnbIdLength) -> u64 {
        self.0 & ((1u64 << length.cell_bits()) - 1)
    }
}

impl fmt::Display for Nci {
    /// Nine uppercase hex digits — 36 bits, never truncated.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:09X}", self.0)
    }
}

/// An NR Cell Global Identity: network plus [`Nci`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ncgi {
    plmn: PlmnId,
    nci: Nci,
}

impl Ncgi {
    /// An NR cell global identity from its parts.
    #[must_use]
    pub const fn new(plmn: PlmnId, nci: Nci) -> Self {
        Self { plmn, nci }
    }

    /// The network.
    #[must_use]
    pub const fn plmn(self) -> PlmnId {
        self.plmn
    }

    /// The cell identity.
    #[must_use]
    pub const fn nci(self) -> Nci {
        self.nci
    }
}

impl fmt::Display for Ncgi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.plmn, self.nci)
    }
}

// ── Tracking area ────────────────────────────────────────────────────────────

/// Which system a [`TrackingAreaCode`] was assigned in, and therefore how wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TacWidth {
    /// EPS: 16 bits.
    Eps,
    /// 5GS: 24 bits.
    FiveGs,
}

impl TacWidth {
    /// Every variant, for exhaustive iteration in tests and mappings.
    pub const ALL: &'static [Self] = &[Self::Eps, Self::FiveGs];

    /// The canonical code of each variant, in [`ALL`](Self::ALL) order.
    pub const CODES: &'static [&'static str] = &["EPS", "5GS"];

    /// How many bits the code occupies.
    #[must_use]
    pub const fn bits(self) -> u8 {
        match self {
            Self::Eps => 16,
            Self::FiveGs => 24,
        }
    }

    /// The canonical string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Eps => "EPS",
            Self::FiveGs => "5GS",
        }
    }
}

impl fmt::Display for TacWidth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TacWidth {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .find(|w| w.as_str().eq_ignore_ascii_case(s.trim()))
            .copied()
            .ok_or_else(|| ParseError::format("TacWidth", s, "EPS or 5GS"))
    }
}

/// A Tracking Area Code, carrying the width it was assigned at.
///
/// Not to be confused with [`Tac`](crate::Tac), which is the Type Allocation
/// Code inside an IMEI. The two are unrelated and the industry reuses the
/// abbreviation for both, which is why this type is spelled out.
///
/// `0x0000` and the all-ones-less-one value are reserved to mean *no valid
/// tracking area*, exactly as for [`Lac`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TrackingAreaCode {
    value: u32,
    width: TacWidth,
}

impl TrackingAreaCode {
    /// An EPS tracking area code, or `None` for a reserved value.
    #[must_use]
    pub const fn eps(value: u16) -> Option<Self> {
        if is_reserved_area(value as u32, TacWidth::Eps.bits()) {
            None
        } else {
            Some(Self {
                value: value as u32,
                width: TacWidth::Eps,
            })
        }
    }

    /// A 5GS tracking area code, or `None` for a reserved or oversized value.
    #[must_use]
    pub const fn five_gs(value: u32) -> Option<Self> {
        if value > 0x00FF_FFFF || is_reserved_area(value, TacWidth::FiveGs.bits()) {
            None
        } else {
            Some(Self {
                value,
                width: TacWidth::FiveGs,
            })
        }
    }

    /// The numeric value.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.value
    }

    /// The width the code was assigned at.
    #[must_use]
    pub const fn width(self) -> TacWidth {
        self.width
    }

    /// The same area as a 5GS code, zero-padded if it was an EPS one.
    ///
    /// The only way to compare across systems, and it is deliberately explicit:
    /// an EPS code and the 5GS code with the same number are the same tracking
    /// area, but `==` says they are not, because the values carry different
    /// widths and silently treating them as equal is how a 4G area and a 5G one
    /// merge in a report that meant to keep them apart.
    #[must_use]
    pub const fn widened(self) -> Self {
        Self {
            value: self.value,
            width: TacWidth::FiveGs,
        }
    }

    /// `true` when both name the same area, ignoring the width they were
    /// written at.
    #[must_use]
    pub const fn is_same_area(self, other: Self) -> bool {
        self.value == other.value
    }
}

impl fmt::Display for TrackingAreaCode {
    /// Hex, at the width the code was assigned: four digits or six.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.width {
            TacWidth::Eps => write!(f, "{:04X}", self.value),
            TacWidth::FiveGs => write!(f, "{:06X}", self.value),
        }
    }
}

/// A Tracking Area Identity: network plus [`TrackingAreaCode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tai {
    plmn: PlmnId,
    tac: TrackingAreaCode,
}

impl Tai {
    /// A tracking area identity from its parts.
    #[must_use]
    pub const fn new(plmn: PlmnId, tac: TrackingAreaCode) -> Self {
        Self { plmn, tac }
    }

    /// The network.
    #[must_use]
    pub const fn plmn(self) -> PlmnId {
        self.plmn
    }

    /// The tracking area.
    #[must_use]
    pub const fn tac(self) -> TrackingAreaCode {
        self.tac
    }
}

impl fmt::Display for Tai {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.plmn, self.tac)
    }
}

// ── User location ────────────────────────────────────────────────────────────

/// Where a record says the subscriber was.
///
/// A sum type rather than a struct of options, because a record reports **one**
/// of these — and a struct with five optional fields invites a reader to check
/// them in an order that silently prefers the least precise.
///
/// Deliberately not `#[non_exhaustive]`: a consumer mapping these onto their own
/// location model wants a compile error when a new access generation appears,
/// not a wildcard arm that files 6G usage under "unknown".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UserLocation {
    /// A 2G cell.
    Cgi(Cgi),
    /// A 3G service area.
    Sai(Sai),
    /// A 4G cell.
    Ecgi(Ecgi),
    /// A 5G cell.
    Ncgi(Ncgi),
    /// A tracking area only, with no cell.
    Tai(Tai),
}

impl UserLocation {
    /// The network the location belongs to.
    #[must_use]
    pub const fn plmn(self) -> PlmnId {
        match self {
            Self::Cgi(v) => v.plmn(),
            Self::Sai(v) => v.plmn(),
            Self::Ecgi(v) => v.plmn(),
            Self::Ncgi(v) => v.plmn(),
            Self::Tai(v) => v.plmn(),
        }
    }

    /// The country the location is in.
    ///
    /// The one question every form answers, and the only one this crate uses:
    /// a country is what regulation, settlement and roaming turn on, and it is
    /// recoverable without any cell database at all.
    #[must_use]
    pub const fn mcc(self) -> Mcc {
        self.plmn().mcc()
    }

    /// `true` when the location names an individual cell.
    ///
    /// A tracking area is an area, not a cell — an assessment that treated it
    /// as one would claim a precision the record does not carry.
    #[must_use]
    pub const fn is_cell(self) -> bool {
        !matches!(self, Self::Tai(_))
    }

    /// The canonical string form of the variant.
    #[must_use]
    pub const fn kind(self) -> &'static str {
        match self {
            Self::Cgi(_) => "CGI",
            Self::Sai(_) => "SAI",
            Self::Ecgi(_) => "ECGI",
            Self::Ncgi(_) => "NCGI",
            Self::Tai(_) => "TAI",
        }
    }
}

impl fmt::Display for UserLocation {
    /// The kind and the value, so two forms never render alike.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cgi(v) => write!(f, "cgi:{v}"),
            Self::Sai(v) => write!(f, "sai:{v}"),
            Self::Ecgi(v) => write!(f, "ecgi:{v}"),
            Self::Ncgi(v) => write!(f, "ncgi:{v}"),
            Self::Tai(v) => write!(f, "tai:{v}"),
        }
    }
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::{
        CellId, Cgi, Ecgi, Eci, EnbKind, GnbIdLength, Lac, Ncgi, Nci, Sai, ServiceAreaCode,
        TacWidth, Tai, TrackingAreaCode, UserLocation,
    };
    use crate::plmn::PlmnId;
    use serde::de::{self, Visitor};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::fmt;

    /// Serialises as its canonical string, deserialises through `FromStr`.
    macro_rules! string_enum {
        ($t:ty, $expecting:literal) => {
            impl Serialize for $t {
                fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                    s.serialize_str(self.as_str())
                }
            }

            impl<'de> Deserialize<'de> for $t {
                fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                    struct V;
                    impl Visitor<'_> for V {
                        type Value = $t;
                        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                            f.write_str($expecting)
                        }
                        fn visit_str<E: de::Error>(self, v: &str) -> Result<$t, E> {
                            v.parse().map_err(de::Error::custom)
                        }
                    }
                    d.deserialize_str(V)
                }
            }
        };
    }

    string_enum!(EnbKind, "MACRO or HOME");
    string_enum!(TacWidth, "EPS or 5GS");

    /// Serialises as a number, rejecting out-of-range values on the way back in
    /// rather than truncating them.
    macro_rules! checked_number {
        ($t:ty, $raw:ty, $ctor:expr, $expecting:literal) => {
            impl Serialize for $t {
                fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                    self.get().serialize(s)
                }
            }

            impl<'de> Deserialize<'de> for $t {
                fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                    let raw = <$raw>::deserialize(d)?;
                    #[allow(clippy::redundant_closure_call)]
                    ($ctor)(raw)
                        .ok_or_else(|| de::Error::custom(format!("{raw} is not {}", $expecting)))
                }
            }
        };
    }

    checked_number!(
        Lac,
        u16,
        Lac::new,
        "a location area code (0000 and FFFE are reserved)"
    );
    checked_number!(Eci, u32, Eci::new, "a 28-bit E-UTRAN cell identity");
    checked_number!(Nci, u64, Nci::new, "a 36-bit NR cell identity");
    checked_number!(
        GnbIdLength,
        u8,
        GnbIdLength::new,
        "a gNB-ID length of 22 to 32 bits"
    );

    /// Serialises as a number; every value of the underlying width is valid.
    macro_rules! plain_number {
        ($t:ty, $raw:ty, $ctor:expr) => {
            impl Serialize for $t {
                fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                    self.get().serialize(s)
                }
            }

            impl<'de> Deserialize<'de> for $t {
                fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                    #[allow(clippy::redundant_closure_call)]
                    Ok(($ctor)(<$raw>::deserialize(d)?))
                }
            }
        };
    }

    plain_number!(CellId, u16, CellId::new);
    plain_number!(ServiceAreaCode, u16, ServiceAreaCode::new);

    #[derive(Serialize, Deserialize)]
    #[serde(rename = "TrackingAreaCode")]
    struct TacRepr {
        value: u32,
        width: TacWidth,
    }

    impl Serialize for TrackingAreaCode {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            TacRepr {
                value: self.get(),
                width: self.width(),
            }
            .serialize(s)
        }
    }

    impl<'de> Deserialize<'de> for TrackingAreaCode {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            let r = TacRepr::deserialize(d)?;
            let built = match r.width {
                TacWidth::Eps => u16::try_from(r.value).ok().and_then(TrackingAreaCode::eps),
                TacWidth::FiveGs => TrackingAreaCode::five_gs(r.value),
            };
            built.ok_or_else(|| {
                de::Error::custom(format!(
                    "{} is not a valid {} tracking area code",
                    r.value, r.width
                ))
            })
        }
    }

    /// Every composite names its parts, so a stored location stays readable.
    macro_rules! composite {
        ($t:ty, $name:literal, { $($field:ident : $ty:ty),+ $(,)? }) => {
            #[derive(Serialize, Deserialize)]
            #[serde(rename = $name)]
            struct Repr {
                $($field: $ty),+
            }

            impl Serialize for $t {
                fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                    Repr { $($field: self.$field()),+ }.serialize(s)
                }
            }

            impl<'de> Deserialize<'de> for $t {
                fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                    let r = Repr::deserialize(d)?;
                    Ok(<$t>::new($(r.$field),+))
                }
            }
        };
    }

    mod cgi {
        use super::*;
        composite!(Cgi, "Cgi", { plmn: PlmnId, lac: Lac, cell: CellId });
    }
    mod sai {
        use super::*;
        composite!(Sai, "Sai", { plmn: PlmnId, lac: Lac, sac: ServiceAreaCode });
    }
    mod ecgi {
        use super::*;
        composite!(Ecgi, "Ecgi", { plmn: PlmnId, eci: Eci });
    }
    mod ncgi {
        use super::*;
        composite!(Ncgi, "Ncgi", { plmn: PlmnId, nci: Nci });
    }
    mod tai {
        use super::*;
        composite!(Tai, "Tai", { plmn: PlmnId, tac: TrackingAreaCode });
    }

    /// Internally tagged on `kind`, so adding an access generation does not
    /// renumber the others.
    #[derive(Serialize, Deserialize)]
    #[serde(tag = "kind", rename = "UserLocation")]
    enum LocationRepr {
        #[serde(rename = "CGI")]
        Cgi(Cgi),
        #[serde(rename = "SAI")]
        Sai(Sai),
        #[serde(rename = "ECGI")]
        Ecgi(Ecgi),
        #[serde(rename = "NCGI")]
        Ncgi(Ncgi),
        #[serde(rename = "TAI")]
        Tai(Tai),
    }

    impl Serialize for UserLocation {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            match *self {
                UserLocation::Cgi(v) => LocationRepr::Cgi(v),
                UserLocation::Sai(v) => LocationRepr::Sai(v),
                UserLocation::Ecgi(v) => LocationRepr::Ecgi(v),
                UserLocation::Ncgi(v) => LocationRepr::Ncgi(v),
                UserLocation::Tai(v) => LocationRepr::Tai(v),
            }
            .serialize(s)
        }
    }

    impl<'de> Deserialize<'de> for UserLocation {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            Ok(match LocationRepr::deserialize(d)? {
                LocationRepr::Cgi(v) => UserLocation::Cgi(v),
                LocationRepr::Sai(v) => UserLocation::Sai(v),
                LocationRepr::Ecgi(v) => UserLocation::Ecgi(v),
                LocationRepr::Ncgi(v) => UserLocation::Ncgi(v),
                LocationRepr::Tai(v) => UserLocation::Tai(v),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plmn() -> PlmnId {
        "262-02".parse().expect("a valid network")
    }

    // ── The reserved location areas ──────────────────────────────────────────

    /// A reserved code means *no valid area*, and reading it as an ordinary one
    /// files real usage in a location area that does not exist.
    #[test]
    fn the_two_reserved_location_areas_are_rejected() {
        assert!(Lac::new(0x0000).is_none());
        assert!(Lac::new(0xFFFE).is_none());
        assert!(Lac::new(0x0001).is_some());
        assert!(Lac::new(0xFFFF).is_some(), "only the two are reserved");
        assert!(Lac::is_reserved(0xFFFE));
        assert!(!Lac::is_reserved(0xFFFF));

        // The same rule holds at both widths, which is why it is written once.
        assert_eq!(
            (0u32..=0xFFFF)
                .filter(|v| super::is_reserved_area(*v, 16))
                .count(),
            2
        );
        assert!(super::is_reserved_area(0x00FF_FFFE, 24));
        assert!(!super::is_reserved_area(0x0000_FFFE, 24));
        assert_eq!(Lac::new(0x00AB).unwrap().to_string(), "00AB");
    }

    #[test]
    fn the_same_two_values_are_reserved_for_tracking_areas() {
        assert!(TrackingAreaCode::eps(0x0000).is_none());
        assert!(TrackingAreaCode::eps(0xFFFE).is_none());
        assert!(TrackingAreaCode::five_gs(0x00_0000).is_none());
        assert!(TrackingAreaCode::five_gs(0xFF_FFFE).is_none());
        assert!(TrackingAreaCode::five_gs(0x01_0000).is_some());
        assert!(
            TrackingAreaCode::five_gs(0x0100_0000).is_none(),
            "more than 24 bits"
        );
    }

    // ── The split that cannot be guessed ─────────────────────────────────────

    /// The same 28 bits are two different base stations depending on the
    /// deployment, and only a caller knows which.
    #[test]
    fn a_cell_identity_splits_differently_for_a_home_enodeb() {
        let eci = Eci::new(0x00A1_B2C3).unwrap();

        assert_eq!(eci.enb_id(EnbKind::Macro), 0x0000_A1B2);
        assert_eq!(eci.cell_id(EnbKind::Macro), Some(0xC3));

        assert_eq!(eci.enb_id(EnbKind::Home), 0x00A1_B2C3);
        assert_eq!(
            eci.cell_id(EnbKind::Home),
            None,
            "a 28-bit home identifier leaves no cell part"
        );
        assert_eq!(EnbKind::Macro.id_bits() + 8, EnbKind::Home.id_bits());
    }

    /// An NR cell identity means nothing without the operator's configured
    /// gNodeB-identifier length.
    #[test]
    fn an_nr_cell_identity_needs_the_configured_length() {
        let nci = Nci::new(0x0_1234_5678).unwrap();

        let short = GnbIdLength::new(22).unwrap();
        let long = GnbIdLength::new(32).unwrap();

        assert_eq!(short.cell_bits(), 14);
        assert_eq!(long.cell_bits(), 4);
        assert_ne!(nci.gnb_id(short), nci.gnb_id(long));

        // Whatever the split, the two halves reconstruct the whole.
        for bits in GnbIdLength::MIN..=GnbIdLength::MAX {
            let length = GnbIdLength::new(bits).unwrap();
            let rebuilt = (nci.gnb_id(length) << length.cell_bits()) | nci.cell_id(length);
            assert_eq!(rebuilt, nci.get(), "at {bits} bits");
        }
    }

    #[test]
    fn identities_are_bounded_by_their_bit_widths() {
        assert!(Eci::new((1 << 28) - 1).is_some());
        assert!(Eci::new(1 << 28).is_none());
        assert!(Nci::new((1 << 36) - 1).is_some());
        assert!(Nci::new(1 << 36).is_none());

        assert!(GnbIdLength::new(21).is_none());
        assert!(GnbIdLength::new(33).is_none());
        assert_eq!(GnbIdLength::new(22).unwrap().get(), 22);
    }

    // ── Width is part of a tracking area code ────────────────────────────────

    /// An EPS code appears in 5GS zero-padded, so the same area written at two
    /// widths must not compare equal by accident — nor be impossible to
    /// compare at all.
    #[test]
    fn a_tracking_area_carries_the_width_it_was_written_at() {
        let eps = TrackingAreaCode::eps(0x1234).unwrap();
        let five_gs = TrackingAreaCode::five_gs(0x00_1234).unwrap();

        assert_ne!(eps, five_gs, "different widths are different values");
        assert_eq!(eps.widened(), five_gs, "and this is the way across");
        assert!(eps.is_same_area(five_gs));

        assert_eq!(eps.to_string(), "1234");
        assert_eq!(five_gs.to_string(), "001234");
        assert_eq!(eps.width(), TacWidth::Eps);
        assert_eq!(TacWidth::Eps.bits() + 8, TacWidth::FiveGs.bits());
    }

    // ── User location ────────────────────────────────────────────────────────

    /// Every form answers the country question, which is the only one this
    /// crate asks of a location.
    #[test]
    fn every_location_form_yields_a_country() {
        let lac = Lac::new(0x1234).unwrap();
        let forms = [
            UserLocation::Cgi(Cgi::new(plmn(), lac, CellId::new(0xABCD))),
            UserLocation::Sai(Sai::new(plmn(), lac, ServiceAreaCode::new(0xABCD))),
            UserLocation::Ecgi(Ecgi::new(plmn(), Eci::new(0x0012_3456).unwrap())),
            UserLocation::Ncgi(Ncgi::new(plmn(), Nci::new(0x0_1234_5678).unwrap())),
            UserLocation::Tai(Tai::new(
                plmn(),
                TrackingAreaCode::five_gs(0x00_1234).unwrap(),
            )),
        ];

        for form in forms {
            assert_eq!(form.mcc(), plmn().mcc());
            assert_eq!(form.plmn(), plmn());
        }

        // A tracking area is an area, not a cell.
        assert!(forms[0].is_cell());
        assert!(!forms[4].is_cell());
    }

    /// Two forms of the same numbers must never render alike, or a report
    /// keyed on the string silently merges a 2G cell with a 3G service area.
    #[test]
    fn the_forms_are_distinguishable_as_strings() {
        let lac = Lac::new(0x1234).unwrap();
        let cgi = UserLocation::Cgi(Cgi::new(plmn(), lac, CellId::new(0xABCD)));
        let sai = UserLocation::Sai(Sai::new(plmn(), lac, ServiceAreaCode::new(0xABCD)));

        assert_ne!(cgi.to_string(), sai.to_string());
        assert_eq!(cgi.to_string(), "cgi:262-02-1234-ABCD");
        assert_eq!(sai.to_string(), "sai:262-02-1234-ABCD");
        assert_eq!(cgi.kind(), "CGI");
    }

    /// A cell identity renders at its full width, so a leading zero is never
    /// dropped and two cells never collide as strings.
    #[test]
    fn identities_render_at_their_full_width() {
        assert_eq!(Eci::new(0x1).unwrap().to_string(), "0000001");
        assert_eq!(Nci::new(0x1).unwrap().to_string(), "000000001");
        assert_eq!(CellId::new(0x1).to_string(), "0001");
        assert_eq!(ServiceAreaCode::new(0x1).to_string(), "0001");
    }

    #[test]
    fn vocabulary_codes_round_trip() {
        for (kind, code) in EnbKind::ALL.iter().zip(EnbKind::CODES) {
            assert_eq!(kind.as_str(), *code);
            assert_eq!(code.parse::<EnbKind>().unwrap(), *kind);
        }
        for (width, code) in TacWidth::ALL.iter().zip(TacWidth::CODES) {
            assert_eq!(width.as_str(), *code);
            assert_eq!(code.parse::<TacWidth>().unwrap(), *width);
        }
        assert_eq!("24".parse::<GnbIdLength>().unwrap().get(), 24);
        assert!("21".parse::<GnbIdLength>().is_err());
        assert!("LTE".parse::<TacWidth>().is_err());
    }
}
