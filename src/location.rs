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

use crate::error::{DecodeError, ParseError};
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

/// A Routing Area Code: one octet, within a location area.
///
/// The GPRS grouping of cells. An SGSN charging record reports a **routing
/// area**, not a cell, so a record set that mixes 2G/3G packet usage with
/// circuit usage is mixing two precisions — which is why this is a type of its
/// own rather than a number beside a [`Lac`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RoutingAreaCode(u8);

impl RoutingAreaCode {
    /// A routing area code. Every 8-bit value is valid.
    #[must_use]
    pub const fn new(value: u8) -> Self {
        Self(value)
    }

    /// The numeric value.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl fmt::Display for RoutingAreaCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02X}", self.0)
    }
}

/// A Location Area Identity: network plus [`Lac`].
///
/// The coarsest location a record carries, and the only one a circuit-switched
/// element is obliged to report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Lai {
    plmn: PlmnId,
    lac: Lac,
}

impl Lai {
    /// A location area identity from its parts.
    #[must_use]
    pub const fn new(plmn: PlmnId, lac: Lac) -> Self {
        Self { plmn, lac }
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
}

impl fmt::Display for Lai {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.plmn, self.lac)
    }
}

/// A Routing Area Identity: network, location area and routing area.
///
/// What an SGSN reports. Kept apart from [`Cgi`] because it locates the
/// subscriber less precisely, and a report that mixed them would overstate what
/// the records actually said.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Rai {
    plmn: PlmnId,
    lac: Lac,
    rac: RoutingAreaCode,
}

impl Rai {
    /// A routing area identity from its parts.
    #[must_use]
    pub const fn new(plmn: PlmnId, lac: Lac, rac: RoutingAreaCode) -> Self {
        Self { plmn, lac, rac }
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

    /// The routing area.
    #[must_use]
    pub const fn rac(self) -> RoutingAreaCode {
        self.rac
    }

    /// The location area this routing area sits in.
    #[must_use]
    pub const fn lai(self) -> Lai {
        Lai::new(self.plmn, self.lac)
    }
}

impl fmt::Display for Rai {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}-{}", self.plmn, self.lac, self.rac)
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

// ── The string forms ─────────────────────────────────────────────────────────

/// Parse exactly `width` upper-case hex digits.
///
/// Strict on the digit count, because the count is what distinguishes the
/// forms: `262-02-002A` is a tracking area and `262-02-00002A` is the same
/// number at 5GS width, and accepting either spelling for both would make the
/// two indistinguishable in stored data.
fn hex_exact(field: &str, width: usize) -> Option<u32> {
    if field.len() != width || !field.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(field, 16).ok()
}

/// Split `MCC-MNC-<rest>` into the network and the remaining fields.
fn split_plmn(s: &str, expected_fields: usize) -> Option<(PlmnId, Vec<&str>)> {
    let fields: Vec<&str> = s.trim().split('-').collect();
    if fields.len() != expected_fields {
        return None;
    }
    let plmn = format!("{}-{}", fields[0], fields[1]).parse().ok()?;
    Some((plmn, fields[2..].to_vec()))
}

/// `FromStr` mirroring a composite's `Display`, so one value has one spelling
/// in **both** directions. A type that renders but does not parse cannot be
/// round-tripped through a log line, a CSV column or a JSON document — and the
/// serde representations here are built on exactly this pair.
macro_rules! parsed_composite {
    ($t:ty, $name:literal, $fields:expr, $shape:literal, $build:expr) => {
        impl FromStr for $t {
            type Err = ParseError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let err = || ParseError::format($name, s, $shape);
                let (plmn, rest) = split_plmn(s, $fields).ok_or_else(err)?;
                #[allow(clippy::redundant_closure_call)]
                ($build)(plmn, rest.as_slice()).ok_or_else(err)
            }
        }
    };
}

parsed_composite!(
    Lai,
    "Lai",
    3,
    "a location area identity such as 262-02-1A2B",
    |plmn, rest: &[&str]| {
        let lac = Lac::new(u16::try_from(hex_exact(rest[0], 4)?).ok()?)?;
        Some(Lai::new(plmn, lac))
    }
);

parsed_composite!(
    Rai,
    "Rai",
    4,
    "a routing area identity such as 262-02-1A2B-05",
    |plmn, rest: &[&str]| {
        let lac = Lac::new(u16::try_from(hex_exact(rest[0], 4)?).ok()?)?;
        let rac = RoutingAreaCode::new(u8::try_from(hex_exact(rest[1], 2)?).ok()?);
        Some(Rai::new(plmn, lac, rac))
    }
);

parsed_composite!(
    Cgi,
    "Cgi",
    4,
    "a cell global identity such as 262-02-1A2B-00FF",
    |plmn, rest: &[&str]| {
        let lac = Lac::new(u16::try_from(hex_exact(rest[0], 4)?).ok()?)?;
        let cell = CellId::new(u16::try_from(hex_exact(rest[1], 4)?).ok()?);
        Some(Cgi::new(plmn, lac, cell))
    }
);

parsed_composite!(
    Sai,
    "Sai",
    4,
    "a service area identity such as 262-02-1A2B-0007",
    |plmn, rest: &[&str]| {
        let lac = Lac::new(u16::try_from(hex_exact(rest[0], 4)?).ok()?)?;
        let sac = ServiceAreaCode::new(u16::try_from(hex_exact(rest[1], 4)?).ok()?);
        Some(Sai::new(plmn, lac, sac))
    }
);

parsed_composite!(
    Ecgi,
    "Ecgi",
    3,
    "an E-UTRAN cell global identity such as 262-02-0A1B2C3",
    |plmn, rest: &[&str]| Some(Ecgi::new(plmn, Eci::new(hex_exact(rest[0], 7)?)?))
);

parsed_composite!(
    Ncgi,
    "Ncgi",
    3,
    "an NR cell global identity such as 262-02-012345678",
    |plmn, rest: &[&str]| {
        let field = rest[0];
        if field.len() != 9 || !field.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        Some(Ncgi::new(
            plmn,
            Nci::new(u64::from_str_radix(field, 16).ok()?)?,
        ))
    }
);

parsed_composite!(
    Tai,
    "Tai",
    3,
    "a tracking area identity such as 262-02-002A (EPS) or 262-02-00002A (5GS)",
    |plmn, rest: &[&str]| {
        // The digit count *is* the width. An EPS code written at 5GS width is a
        // different value here, which is the whole point of `TacWidth`.
        let field = rest[0];
        let tac = match field.len() {
            4 => TrackingAreaCode::eps(u16::try_from(hex_exact(field, 4)?).ok()?),
            6 => TrackingAreaCode::five_gs(hex_exact(field, 6)?),
            _ => None,
        }?;
        Some(Tai::new(plmn, tac))
    }
);

/// The shape [`UserLocation`] accepts, as rendered in a [`ParseError`].
const USER_LOCATION_FORMAT: &str =
    "a tagged location such as ecgi:262-02-0A1B2C3, cgi:262-02-1A2B-00FF or tai:262-02-002A";

impl FromStr for UserLocation {
    type Err = ParseError;

    /// Parses the tagged form [`Display`](fmt::Display) writes.
    ///
    /// The tag is required. `262-02-1A2B` is a location area and a tracking
    /// area written at EPS width, and nothing in the digits says which — the
    /// same reason the enum exists rather than a struct of options.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseError::format("UserLocation", s, USER_LOCATION_FORMAT);
        let (tag, body) = s.trim().split_once(':').ok_or_else(err)?;
        match tag.to_ascii_lowercase().as_str() {
            "cgi" => body.parse().map(Self::Cgi),
            "sai" => body.parse().map(Self::Sai),
            "ecgi" => body.parse().map(Self::Ecgi),
            "ncgi" => body.parse().map(Self::Ncgi),
            "tai" => body.parse().map(Self::Tai),
            "rai" => body.parse().map(Self::Rai),
            "lai" => body.parse().map(Self::Lai),
            _ => return Err(err()),
        }
        .map_err(|_| err())
    }
}

// ── The wire form: User Location Information ─────────────────────────────────

/// Read a big-endian `u16` at `at`.
const fn u16_at(bytes: &[u8], at: usize) -> u16 {
    ((bytes[at] as u16) << 8) | bytes[at + 1] as u16
}

/// Decode the three-octet PLMN identity and a location area code, the common
/// head of every 2G and 3G identity in a ULI.
fn plmn_and_lac(bytes: &[u8]) -> Result<(PlmnId, Lac), DecodeError> {
    let plmn = PlmnId::from_bcd(&bytes[..3])?;
    // 0x0000 and 0xFFFE are reserved to mean "no valid location area". They
    // reach a record when an element copies its own "unknown" marker into the
    // field, and reading one as an area puts real usage somewhere that does not
    // exist — so the decoder refuses rather than inventing a place.
    let lac = Lac::new(u16_at(bytes, 3))
        .ok_or_else(|| DecodeError::new(3, "a location area code that is not reserved"))?;
    Ok((plmn, lac))
}

impl Lai {
    /// The wire length of a Location Area Identity, in octets.
    pub const OCTETS: usize = 5;

    /// Decode the five-octet LAI: PLMN identity, then a 16-bit location area.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] for the wrong length, a malformed PLMN identity, or a
    /// reserved location area code.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() != Self::OCTETS {
            return Err(DecodeError::new(0, "exactly 5 octets"));
        }
        let (plmn, lac) = plmn_and_lac(bytes)?;
        Ok(Self::new(plmn, lac))
    }

    /// Encode to the five-octet LAI.
    #[must_use]
    pub fn to_bytes(self) -> [u8; Self::OCTETS] {
        let plmn = self.plmn.to_bcd();
        let lac = self.lac.get().to_be_bytes();
        [plmn[0], plmn[1], plmn[2], lac[0], lac[1]]
    }
}

impl Cgi {
    /// The wire length of a Cell Global Identity, in octets.
    pub const OCTETS: usize = 7;

    /// Decode the seven-octet CGI of 3GPP TS 29.274 §8.21.1.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] for the wrong length, a malformed PLMN identity, or a
    /// reserved location area code.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() != Self::OCTETS {
            return Err(DecodeError::new(0, "exactly 7 octets"));
        }
        let (plmn, lac) = plmn_and_lac(bytes)?;
        Ok(Self::new(plmn, lac, CellId::new(u16_at(bytes, 5))))
    }

    /// Encode to the seven-octet CGI.
    #[must_use]
    pub fn to_bytes(self) -> [u8; Self::OCTETS] {
        let plmn = self.plmn().to_bcd();
        let lac = self.lac().get().to_be_bytes();
        let cell = self.cell().get().to_be_bytes();
        [plmn[0], plmn[1], plmn[2], lac[0], lac[1], cell[0], cell[1]]
    }
}

impl Sai {
    /// The wire length of a Service Area Identity, in octets.
    pub const OCTETS: usize = 7;

    /// Decode the seven-octet SAI of 3GPP TS 29.274 §8.21.2.
    ///
    /// # Errors
    ///
    /// As [`Cgi::from_bytes`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() != Self::OCTETS {
            return Err(DecodeError::new(0, "exactly 7 octets"));
        }
        let (plmn, lac) = plmn_and_lac(bytes)?;
        Ok(Self::new(plmn, lac, ServiceAreaCode::new(u16_at(bytes, 5))))
    }

    /// Encode to the seven-octet SAI.
    #[must_use]
    pub fn to_bytes(self) -> [u8; Self::OCTETS] {
        let plmn = self.plmn().to_bcd();
        let lac = self.lac().get().to_be_bytes();
        let sac = self.sac().get().to_be_bytes();
        [plmn[0], plmn[1], plmn[2], lac[0], lac[1], sac[0], sac[1]]
    }
}

impl Rai {
    /// The wire length of a Routing Area Identity inside a ULI, in octets.
    ///
    /// Seven, not six. TS 29.274 pads the one-octet routing area code with a
    /// spare octet, which every other specification carrying an RAI does not —
    /// a six-octet reader walks off the end of the field and reads the next
    /// identity's PLMN as a location area.
    pub const OCTETS: usize = 7;

    /// The value TS 29.274 fixes the spare octet at.
    pub const SPARE: u8 = 0xFF;

    /// Decode the seven-octet RAI of 3GPP TS 29.274 §8.21.3.
    ///
    /// # Errors
    ///
    /// As [`Cgi::from_bytes`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() != Self::OCTETS {
            return Err(DecodeError::new(0, "exactly 7 octets"));
        }
        let (plmn, lac) = plmn_and_lac(bytes)?;
        Ok(Self::new(plmn, lac, RoutingAreaCode::new(bytes[5])))
    }

    /// Encode to the seven-octet RAI, spare octet included.
    #[must_use]
    pub fn to_bytes(self) -> [u8; Self::OCTETS] {
        let plmn = self.plmn.to_bcd();
        let lac = self.lac.get().to_be_bytes();
        [
            plmn[0],
            plmn[1],
            plmn[2],
            lac[0],
            lac[1],
            self.rac.get(),
            Self::SPARE,
        ]
    }
}

impl Tai {
    /// The wire length of an EPS Tracking Area Identity, in octets.
    pub const EPS_OCTETS: usize = 5;

    /// The wire length of a 5GS Tracking Area Identity, in octets.
    pub const FIVE_GS_OCTETS: usize = 6;

    /// Decode a TAI, at either width.
    ///
    /// Five octets is the EPS form (16-bit code, TS 29.274 §8.21.4) and six the
    /// 5GS one (24-bit code, TS 24.501). **The width is taken from the field's
    /// length and kept**, because an EPS code appears in 5GS zero-padded and
    /// the two must not silently compare equal — see [`TrackingAreaCode`].
    ///
    /// # Errors
    ///
    /// [`DecodeError`] for any other length, a malformed PLMN identity, or a
    /// reserved tracking area code.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        let plmn_len = PlmnId::BCD_OCTETS;
        let tac = match bytes.len() {
            Self::EPS_OCTETS => TrackingAreaCode::eps(u16_at(bytes, plmn_len)),
            Self::FIVE_GS_OCTETS => TrackingAreaCode::five_gs(
                (u32::from(bytes[plmn_len]) << 16)
                    | (u32::from(bytes[plmn_len + 1]) << 8)
                    | u32::from(bytes[plmn_len + 2]),
            ),
            _ => return Err(DecodeError::new(0, "5 octets (EPS) or 6 (5GS)")),
        };
        let tac = tac.ok_or_else(|| {
            DecodeError::new(plmn_len, "a tracking area code that is not reserved")
        })?;
        Ok(Self::new(PlmnId::from_bcd(&bytes[..plmn_len])?, tac))
    }

    /// Encode to the wire, writing into `out` and returning the octet count.
    ///
    /// Five octets for an EPS code and six for a 5GS one — the width the code
    /// was assigned at, never widened on the way out.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] when `out` is too short.
    pub fn to_bytes(self, out: &mut [u8]) -> Result<usize, DecodeError> {
        let needed = match self.tac().width() {
            TacWidth::Eps => Self::EPS_OCTETS,
            TacWidth::FiveGs => Self::FIVE_GS_OCTETS,
        };
        if out.len() < needed {
            return Err(DecodeError::new(0, "a buffer of 5 octets (EPS) or 6 (5GS)"));
        }
        out[..3].copy_from_slice(&self.plmn().to_bcd());
        let value = self.tac().get();
        match self.tac().width() {
            TacWidth::Eps => out[3..5].copy_from_slice(&(value as u16).to_be_bytes()),
            TacWidth::FiveGs => {
                out[3] = (value >> 16) as u8;
                out[4] = (value >> 8) as u8;
                out[5] = value as u8;
            }
        }
        Ok(needed)
    }
}

impl Ecgi {
    /// The wire length of an E-UTRAN Cell Global Identity, in octets.
    pub const OCTETS: usize = 7;

    /// Decode the seven-octet ECGI of 3GPP TS 29.274 §8.21.5.
    ///
    /// The cell identity is 28 bits, so the top nibble of the fourth octet is
    /// spare. It is **not** required to be zero — elements do set it — but it is
    /// masked off rather than folded into the identity, which would produce a
    /// cell that does not exist.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] for the wrong length or a malformed PLMN identity.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() != Self::OCTETS {
            return Err(DecodeError::new(0, "exactly 7 octets"));
        }
        let raw = u32::from_be_bytes([bytes[3], bytes[4], bytes[5], bytes[6]]);
        let eci = Eci::new(raw & 0x0FFF_FFFF)
            .ok_or_else(|| DecodeError::new(3, "a 28-bit E-UTRAN cell identity"))?;
        Ok(Self::new(PlmnId::from_bcd(&bytes[..3])?, eci))
    }

    /// Encode to the seven-octet ECGI, with the spare nibble zeroed.
    #[must_use]
    pub fn to_bytes(self) -> [u8; Self::OCTETS] {
        let plmn = self.plmn().to_bcd();
        let eci = self.eci().get().to_be_bytes();
        [plmn[0], plmn[1], plmn[2], eci[0], eci[1], eci[2], eci[3]]
    }
}

impl Ncgi {
    /// The wire length of an NR Cell Global Identity, in octets.
    ///
    /// Three of PLMN identity and five holding a 36-bit cell identity, as
    /// NGAP (3GPP TS 38.413) carries it: the identity occupies the **top** 36
    /// bits of the five octets, so the low nibble of the last one is spare.
    pub const OCTETS: usize = 8;

    /// Decode the eight-octet NCGI.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] for the wrong length or a malformed PLMN identity.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() != Self::OCTETS {
            return Err(DecodeError::new(0, "exactly 8 octets"));
        }
        let raw = u64::from(u32::from_be_bytes([bytes[3], bytes[4], bytes[5], bytes[6]])) << 8
            | u64::from(bytes[7]);
        // A BIT STRING (SIZE(36)) is left-aligned in the octets it occupies.
        let nci =
            Nci::new(raw >> 4).ok_or_else(|| DecodeError::new(3, "a 36-bit NR cell identity"))?;
        Ok(Self::new(PlmnId::from_bcd(&bytes[..3])?, nci))
    }

    /// Encode to the eight-octet NCGI, with the spare nibble zeroed.
    #[must_use]
    pub fn to_bytes(self) -> [u8; Self::OCTETS] {
        let plmn = self.plmn().to_bcd();
        let shifted = self.nci().get() << 4;
        [
            plmn[0],
            plmn[1],
            plmn[2],
            (shifted >> 32) as u8,
            (shifted >> 24) as u8,
            (shifted >> 16) as u8,
            (shifted >> 8) as u8,
            shifted as u8,
        ]
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
    /// A GPRS routing area only, with no cell.
    Rai(Rai),
    /// A location area only — the coarsest a record reports.
    Lai(Lai),
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
            Self::Rai(v) => v.plmn(),
            Self::Lai(v) => v.plmn(),
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
        matches!(
            self,
            Self::Cgi(_) | Self::Sai(_) | Self::Ecgi(_) | Self::Ncgi(_)
        )
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
            Self::Rai(_) => "RAI",
            Self::Lai(_) => "LAI",
        }
    }
}

// ── The ULI information element ──────────────────────────────────────────────

/// A macro eNodeB identifier, as a ULI carries it: network plus 20 bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MacroEnbId {
    plmn: PlmnId,
    id: u32,
}

impl MacroEnbId {
    /// The wire length, in octets.
    pub const OCTETS: usize = 6;

    /// How many bits the identifier occupies.
    pub const BITS: u32 = 20;

    /// An identifier, or `None` when the value does not fit 20 bits.
    #[must_use]
    pub const fn new(plmn: PlmnId, id: u32) -> Option<Self> {
        if id < (1 << Self::BITS) {
            Some(Self { plmn, id })
        } else {
            None
        }
    }

    /// The network.
    #[must_use]
    pub const fn plmn(self) -> PlmnId {
        self.plmn
    }

    /// The identifier.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.id
    }

    /// Decode the six-octet field of 3GPP TS 29.274 §8.21.7.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] for the wrong length or a malformed PLMN identity.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() != Self::OCTETS {
            return Err(DecodeError::new(0, "exactly 6 octets"));
        }
        let id = u32::from_be_bytes([0, bytes[3], bytes[4], bytes[5]]) & 0x000F_FFFF;
        Self::new(PlmnId::from_bcd(&bytes[..3])?, id)
            .ok_or_else(|| DecodeError::new(3, "a 20-bit macro eNodeB identifier"))
    }

    /// Encode to the six-octet field.
    #[must_use]
    pub fn to_bytes(self) -> [u8; Self::OCTETS] {
        let plmn = self.plmn.to_bcd();
        [
            plmn[0],
            plmn[1],
            plmn[2],
            (self.id >> 16) as u8,
            (self.id >> 8) as u8,
            self.id as u8,
        ]
    }
}

impl fmt::Display for MacroEnbId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{:05X}", self.plmn, self.id)
    }
}

/// Which of the two extended macro eNodeB shapes a value takes.
///
/// The `SMeNB` bit selects between them, and the identifier is a different
/// width in each — reading one as the other shifts every base station in the
/// deployment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExtendedEnbKind {
    /// `SMeNB` clear: a 21-bit long macro eNodeB identifier.
    Long,
    /// `SMeNB` set: an 18-bit short macro eNodeB identifier.
    Short,
}

impl ExtendedEnbKind {
    /// How many bits the identifier occupies.
    #[must_use]
    pub const fn bits(self) -> u32 {
        match self {
            Self::Long => 21,
            Self::Short => 18,
        }
    }

    /// The canonical string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Long => "LONG",
            Self::Short => "SHORT",
        }
    }
}

impl fmt::Display for ExtendedEnbKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl ExtendedEnbKind {
    /// Every variant, for exhaustive iteration in tests and mappings.
    pub const ALL: &'static [Self] = &[Self::Long, Self::Short];

    /// The canonical code of each variant, in [`ALL`](Self::ALL) order.
    pub const CODES: &'static [&'static str] = &["LONG", "SHORT"];
}

impl FromStr for ExtendedEnbKind {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .find(|k| k.as_str().eq_ignore_ascii_case(s.trim()))
            .copied()
            .ok_or_else(|| ParseError::one_of("ExtendedEnbKind", s, Self::CODES))
    }
}

/// An extended macro eNodeB identifier (3GPP TS 29.274 §8.21.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExtendedMacroEnbId {
    plmn: PlmnId,
    kind: ExtendedEnbKind,
    id: u32,
}

impl ExtendedMacroEnbId {
    /// The wire length, in octets.
    pub const OCTETS: usize = 6;

    /// An identifier, or `None` when the value does not fit its shape.
    #[must_use]
    pub const fn new(plmn: PlmnId, kind: ExtendedEnbKind, id: u32) -> Option<Self> {
        if id < (1 << kind.bits()) {
            Some(Self { plmn, kind, id })
        } else {
            None
        }
    }

    /// The network.
    #[must_use]
    pub const fn plmn(self) -> PlmnId {
        self.plmn
    }

    /// Which shape the identifier takes.
    #[must_use]
    pub const fn kind(self) -> ExtendedEnbKind {
        self.kind
    }

    /// The identifier.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.id
    }

    /// Decode the six-octet field.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] for the wrong length or a malformed PLMN identity.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() != Self::OCTETS {
            return Err(DecodeError::new(0, "exactly 6 octets"));
        }
        let raw = u32::from_be_bytes([0, bytes[3], bytes[4], bytes[5]]);
        let kind = if bytes[3] & 0x80 != 0 {
            ExtendedEnbKind::Short
        } else {
            ExtendedEnbKind::Long
        };
        let id = raw & ((1 << kind.bits()) - 1);
        Self::new(PlmnId::from_bcd(&bytes[..3])?, kind, id)
            .ok_or_else(|| DecodeError::new(3, "an extended macro eNodeB identifier"))
    }

    /// Encode to the six-octet field.
    #[must_use]
    pub fn to_bytes(self) -> [u8; Self::OCTETS] {
        let plmn = self.plmn.to_bcd();
        let flag = match self.kind {
            ExtendedEnbKind::Long => 0,
            ExtendedEnbKind::Short => 0x0080_0000,
        };
        let packed = flag | self.id;
        [
            plmn[0],
            plmn[1],
            plmn[2],
            (packed >> 16) as u8,
            (packed >> 8) as u8,
            packed as u8,
        ]
    }
}

impl fmt::Display for ExtendedMacroEnbId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}-{:06X}", self.plmn, self.kind, self.id)
    }
}

/// The User Location Information element of 3GPP TS 29.274 §8.21.
///
/// # A ULI is a *set*, not a location
///
/// The element is a flag octet followed by whichever identities the flags
/// select, and a network commonly reports **more than one** — an ECGI and the
/// TAI it sits in, or a CGI and its RAI. Modelling it as a single value forces
/// a decoder to pick one, and the natural pick is whichever field it happened
/// to check first, which varies between decoders and therefore between the two
/// sides of a reconciliation.
///
/// So this type carries all of them, and [`most_precise`](Self::most_precise)
/// is the named, documented way to reduce the set to one
/// [`UserLocation`] when a caller genuinely needs one.
///
/// # The field order is fixed and the flags are not
///
/// The identities appear in a fixed order — CGI, SAI, RAI, TAI, ECGI, LAI,
/// macro eNodeB, extended macro eNodeB — regardless of which flags are set, and
/// each is a different width. A decoder that reads them in flag order, or that
/// gets one width wrong, does not fail: it reads the *next* identity's PLMN
/// octets as this one's location area and produces a plausible cell in the
/// wrong country.
///
/// ```rust
/// use mediation::{Uli, UserLocation};
///
/// // Flags 0x18: ECGI and TAI, which is what an LTE record usually carries.
/// let field = [
///     0x18,
///     0x62, 0xF2, 0x20, 0x00, 0x2A, // TAI: 262-02, tracking area 002A
///     0x62, 0xF2, 0x20, 0x00, 0xA1, 0xB2, 0xC3, // ECGI: 262-02, cell 00A1B2C3
/// ];
///
/// let uli = Uli::decode(&field).unwrap();
/// assert!(uli.tai.is_some() && uli.ecgi.is_some());
/// assert_eq!(uli.mcc().unwrap().unwrap().to_string(), "262");
///
/// // A cell is more precise than the tracking area containing it.
/// assert!(matches!(uli.most_precise(), Some(UserLocation::Ecgi(_))));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Uli {
    /// A 2G cell, when the element carried one.
    pub cgi: Option<Cgi>,
    /// A 3G service area.
    pub sai: Option<Sai>,
    /// A GPRS routing area.
    pub rai: Option<Rai>,
    /// A tracking area.
    pub tai: Option<Tai>,
    /// A 4G cell.
    pub ecgi: Option<Ecgi>,
    /// A location area.
    pub lai: Option<Lai>,
    /// A macro eNodeB.
    pub macro_enb: Option<MacroEnbId>,
    /// An extended macro eNodeB.
    pub extended_macro_enb: Option<ExtendedMacroEnbId>,
}

impl Uli {
    /// Bit 1 of the flag octet.
    const FLAG_CGI: u8 = 0x01;
    /// Bit 2.
    const FLAG_SAI: u8 = 0x02;
    /// Bit 3.
    const FLAG_RAI: u8 = 0x04;
    /// Bit 4.
    const FLAG_TAI: u8 = 0x08;
    /// Bit 5.
    const FLAG_ECGI: u8 = 0x10;
    /// Bit 6.
    const FLAG_LAI: u8 = 0x20;
    /// Bit 7.
    const FLAG_MACRO_ENB: u8 = 0x40;
    /// Bit 8.
    const FLAG_EXT_MACRO_ENB: u8 = 0x80;

    /// An empty set.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            cgi: None,
            sai: None,
            rai: None,
            tai: None,
            ecgi: None,
            lai: None,
            macro_enb: None,
            extended_macro_enb: None,
        }
    }

    /// Decode the element.
    ///
    /// A ULI **inside a ULI** is not a thing, so the element is a flat
    /// sequence and the decode is a single pass. Trailing octets beyond the
    /// selected fields are ignored: TS 29.274 declares the element extensible,
    /// and a later release adding a field must not make today's records
    /// undecodable.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] for an empty element, for one that ends before a field
    /// the flags promised, or for a field this crate rejects — a reserved
    /// location area code, an unassigned MCC.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let (flags, rest) = bytes
            .split_first()
            .ok_or_else(|| DecodeError::new(0, "a flag octet"))?;
        let mut at = 1usize;
        let mut uli = Self::new();

        // `at` walks the whole element, so every field's error offset and every
        // slice come from one cursor rather than from a shrinking tail.
        macro_rules! take {
            ($flag:expr, $len:expr, $what:literal) => {{
                if flags & $flag == 0 {
                    None
                } else {
                    let from = at - 1;
                    if rest.len() < from + $len {
                        return Err(DecodeError::new(at, $what));
                    }
                    at += $len;
                    Some(&rest[from..from + $len])
                }
            }};
        }

        // TS 29.274 §8.21: "If more than one identity of different type is
        // present, then they shall be sorted in the following order: CGI, SAI,
        // RAI, TAI, ECGI, LAI" — which is the flag order, with the two
        // base-station identifiers appended in the order their flags were
        // added. Both the reader and the writer below follow it.
        if let Some(field) = take!(Self::FLAG_CGI, Cgi::OCTETS, "a 7-octet CGI") {
            uli.cgi = Some(Cgi::from_bytes(field).map_err(|e| shift(e, at - Cgi::OCTETS))?);
        }
        if let Some(field) = take!(Self::FLAG_SAI, Sai::OCTETS, "a 7-octet SAI") {
            uli.sai = Some(Sai::from_bytes(field).map_err(|e| shift(e, at - Sai::OCTETS))?);
        }
        if let Some(field) = take!(Self::FLAG_RAI, Rai::OCTETS, "a 7-octet RAI") {
            uli.rai = Some(Rai::from_bytes(field).map_err(|e| shift(e, at - Rai::OCTETS))?);
        }
        if let Some(field) = take!(Self::FLAG_TAI, Tai::EPS_OCTETS, "a 5-octet TAI") {
            uli.tai = Some(Tai::from_bytes(field).map_err(|e| shift(e, at - Tai::EPS_OCTETS))?);
        }
        if let Some(field) = take!(Self::FLAG_ECGI, Ecgi::OCTETS, "a 7-octet ECGI") {
            uli.ecgi = Some(Ecgi::from_bytes(field).map_err(|e| shift(e, at - Ecgi::OCTETS))?);
        }
        if let Some(field) = take!(Self::FLAG_LAI, Lai::OCTETS, "a 5-octet LAI") {
            uli.lai = Some(Lai::from_bytes(field).map_err(|e| shift(e, at - Lai::OCTETS))?);
        }
        if let Some(field) = take!(
            Self::FLAG_MACRO_ENB,
            MacroEnbId::OCTETS,
            "a 6-octet macro eNodeB identifier"
        ) {
            uli.macro_enb =
                Some(MacroEnbId::from_bytes(field).map_err(|e| shift(e, at - MacroEnbId::OCTETS))?);
        }
        if let Some(field) = take!(
            Self::FLAG_EXT_MACRO_ENB,
            ExtendedMacroEnbId::OCTETS,
            "a 6-octet extended macro eNodeB identifier"
        ) {
            uli.extended_macro_enb = Some(
                ExtendedMacroEnbId::from_bytes(field)
                    .map_err(|e| shift(e, at - ExtendedMacroEnbId::OCTETS))?,
            );
        }
        Ok(uli)
    }

    /// How many octets [`encode`](Self::encode) will write.
    ///
    /// The TAI is counted at [`Tai::EPS_OCTETS`], which is the only width this
    /// element carries — see [`encode`](Self::encode), which refuses a 5GS one
    /// rather than writing an octet the decoder would misread.
    #[must_use]
    pub fn encoded_len(&self) -> usize {
        1 + self.cgi.map_or(0, |_| Cgi::OCTETS)
            + self.sai.map_or(0, |_| Sai::OCTETS)
            + self.rai.map_or(0, |_| Rai::OCTETS)
            + self.tai.map_or(0, |_| Tai::EPS_OCTETS)
            + self.ecgi.map_or(0, |_| Ecgi::OCTETS)
            + self.lai.map_or(0, |_| Lai::OCTETS)
            + self.macro_enb.map_or(0, |_| MacroEnbId::OCTETS)
            + self
                .extended_macro_enb
                .map_or(0, |_| ExtendedMacroEnbId::OCTETS)
    }

    /// Encode the element, writing into `out` and returning the octet count.
    ///
    /// # A 5GS tracking area is refused
    ///
    /// The ULI IE's TAI field is five octets — a 16-bit EPS code (TS 29.274
    /// §8.21.4). A 24-bit 5GS code does not fit it, and writing six octets
    /// anyway would shift every field after the TAI by one: this crate's own
    /// [`decode`](Self::decode) would read the next identity's octets as part
    /// of the tracking area, and any other decoder would do something else
    /// again. A 5GS tracking area belongs in a 5GS location reporting element,
    /// so it is rejected here rather than corrupted.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] when `out` is shorter than
    /// [`encoded_len`](Self::encoded_len), or when the TAI is a 5GS one.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, DecodeError> {
        if let Some(tai) = self.tai
            && tai.tac().width() != TacWidth::Eps
        {
            return Err(DecodeError::new(
                1,
                "an EPS tracking area; the ULI element has no 5GS TAI field",
            ));
        }
        let needed = self.encoded_len();
        if out.len() < needed {
            return Err(DecodeError::new(0, "a buffer of encoded_len() octets"));
        }
        let mut flags = 0u8;
        for (present, bit) in [
            (self.cgi.is_some(), Self::FLAG_CGI),
            (self.sai.is_some(), Self::FLAG_SAI),
            (self.rai.is_some(), Self::FLAG_RAI),
            (self.tai.is_some(), Self::FLAG_TAI),
            (self.ecgi.is_some(), Self::FLAG_ECGI),
            (self.lai.is_some(), Self::FLAG_LAI),
            (self.macro_enb.is_some(), Self::FLAG_MACRO_ENB),
            (self.extended_macro_enb.is_some(), Self::FLAG_EXT_MACRO_ENB),
        ] {
            if present {
                flags |= bit;
            }
        }
        out[0] = flags;

        let mut at = 1usize;
        macro_rules! put {
            ($value:expr) => {
                if let Some(value) = $value {
                    let encoded = value.to_bytes();
                    out[at..at + encoded.len()].copy_from_slice(&encoded);
                    at += encoded.len();
                }
            };
        }
        put!(self.cgi);
        put!(self.sai);
        put!(self.rai);
        if let Some(tai) = self.tai {
            at += tai.to_bytes(&mut out[at..])?;
        }
        put!(self.ecgi);
        put!(self.lai);
        put!(self.macro_enb);
        put!(self.extended_macro_enb);
        Ok(at)
    }

    /// `true` when the element selected no identity at all.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.cgi.is_none()
            && self.sai.is_none()
            && self.rai.is_none()
            && self.tai.is_none()
            && self.ecgi.is_none()
            && self.lai.is_none()
            && self.macro_enb.is_none()
            && self.extended_macro_enb.is_none()
    }

    /// The most precise identity the element carried, as a [`UserLocation`].
    ///
    /// Cell before area, and newer access before older where both are present:
    /// ECGI, CGI, SAI, then the areas — TAI, RAI, LAI. The base-station
    /// identifiers are not locations and are never returned.
    ///
    /// `None` for an element that selected nothing, which is a decoding or
    /// provenance gap rather than a place.
    #[must_use]
    pub fn most_precise(&self) -> Option<UserLocation> {
        if let Some(v) = self.ecgi {
            return Some(UserLocation::Ecgi(v));
        }
        if let Some(v) = self.cgi {
            return Some(UserLocation::Cgi(v));
        }
        if let Some(v) = self.sai {
            return Some(UserLocation::Sai(v));
        }
        if let Some(v) = self.tai {
            return Some(UserLocation::Tai(v));
        }
        if let Some(v) = self.rai {
            return Some(UserLocation::Rai(v));
        }
        self.lai.map(UserLocation::Lai)
    }

    /// The country every identity in the element agrees on, eNodeB identities
    /// included (audit L1), as the first identity's MCC.
    ///
    /// Two MCCs of one country agree ([`Mcc::same_country`]). `Ok(None)` when
    /// the element carried no identity at all, and
    /// `Err(UliCountryConflict)` when two of them name different countries —
    /// which is a fault, not a tie to break, exactly as in
    /// [`crate::jurisdiction`]. The three answers are kept apart because they
    /// call for three different things: nothing to read, a feed to fix, and a
    /// country.
    ///
    /// ```rust
    /// use mediation::{Cgi, Lai, Uli};
    /// # use mediation::{CellId, Lac};
    /// # let lac = Lac::new(0x0BC4).unwrap();
    /// let mut uli = Uli::new();
    /// assert_eq!(uli.mcc(), Ok(None), "nothing to read is not a conflict");
    ///
    /// uli.cgi = Some(Cgi::new("262-02".parse().unwrap(), lac, CellId::new(1)));
    /// assert_eq!(uli.mcc().unwrap().map(|m| m.to_string()), Some("262".into()));
    ///
    /// // A second identity from another country is a fault in the feed.
    /// uli.lai = Some(Lai::new("214-07".parse().unwrap(), lac));
    /// assert!(uli.mcc().is_err());
    /// ```
    ///
    /// # Errors
    ///
    /// [`UliCountryConflict`] naming the two countries that disagreed.
    pub fn mcc(&self) -> Result<Option<Mcc>, UliCountryConflict> {
        let mut found: Option<Mcc> = None;
        for plmn in [
            self.cgi.map(Cgi::plmn),
            self.sai.map(Sai::plmn),
            self.rai.map(Rai::plmn),
            self.tai.map(Tai::plmn),
            self.ecgi.map(Ecgi::plmn),
            self.lai.map(Lai::plmn),
            self.macro_enb.map(MacroEnbId::plmn),
            self.extended_macro_enb.map(ExtendedMacroEnbId::plmn),
        ]
        .into_iter()
        .flatten()
        {
            match found {
                None => found = Some(plmn.mcc()),
                Some(seen) if !seen.same_country(plmn.mcc()) => {
                    return Err(UliCountryConflict {
                        first: seen,
                        second: plmn.mcc(),
                    });
                }
                Some(_) => {}
            }
        }
        Ok(found)
    }
}

/// Two identities in one [`Uli`] named different countries.
///
/// A record cannot have been produced in two places, so this is a decoding
/// fault or a mislabelled element rather than an ambiguity to resolve. Both
/// codes are carried so a caller can see which feed is wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UliCountryConflict {
    /// The country the first identity named, in element order.
    pub first: Mcc,
    /// The country that disagreed with it.
    pub second: Mcc,
}

impl fmt::Display for UliCountryConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the element names both {} and {}",
            self.first, self.second
        )
    }
}

impl std::error::Error for UliCountryConflict {}

/// Move a field-local decode error onto the enclosing element's offsets.
fn shift(error: DecodeError, by: usize) -> DecodeError {
    DecodeError::new(error.at() + by, error.expected())
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
            Self::Rai(v) => write!(f, "rai:{v}"),
            Self::Lai(v) => write!(f, "lai:{v}"),
        }
    }
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::{
        CellId, Cgi, Ecgi, Eci, EnbKind, ExtendedEnbKind, ExtendedMacroEnbId, GnbIdLength, Lac,
        Lai, MacroEnbId, Ncgi, Nci, Rai, RoutingAreaCode, Sai, ServiceAreaCode, TacWidth, Tai,
        TrackingAreaCode, Uli, UserLocation,
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
    plain_number!(RoutingAreaCode, u8, RoutingAreaCode::new);

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
    mod lai {
        use super::*;
        composite!(Lai, "Lai", { plmn: PlmnId, lac: Lac });
    }
    mod rai {
        use super::*;
        composite!(Rai, "Rai", { plmn: PlmnId, lac: Lac, rac: RoutingAreaCode });
    }

    /// The base-station identifiers name their parts, so a stored value stays
    /// readable — and the extended one names its shape, because the identifier
    /// is a different width in each and the two must never read alike.
    mod macro_enb {
        use super::*;

        #[derive(Serialize, Deserialize)]
        #[serde(rename = "MacroEnbId")]
        struct Repr {
            plmn: PlmnId,
            id: u32,
        }

        impl Serialize for MacroEnbId {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                Repr {
                    plmn: self.plmn(),
                    id: self.get(),
                }
                .serialize(s)
            }
        }

        impl<'de> Deserialize<'de> for MacroEnbId {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let r = Repr::deserialize(d)?;
                MacroEnbId::new(r.plmn, r.id).ok_or_else(|| {
                    de::Error::custom(format!("{} is not a 20-bit macro eNodeB id", r.id))
                })
            }
        }
    }

    mod ext_macro_enb {
        use super::*;

        #[derive(Serialize, Deserialize)]
        #[serde(rename = "ExtendedMacroEnbId")]
        struct Repr {
            plmn: PlmnId,
            kind: ExtendedEnbKind,
            id: u32,
        }

        impl Serialize for ExtendedMacroEnbId {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                Repr {
                    plmn: self.plmn(),
                    kind: self.kind(),
                    id: self.get(),
                }
                .serialize(s)
            }
        }

        impl<'de> Deserialize<'de> for ExtendedMacroEnbId {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let r = Repr::deserialize(d)?;
                ExtendedMacroEnbId::new(r.plmn, r.kind, r.id).ok_or_else(|| {
                    de::Error::custom(format!("{} does not fit a {} identifier", r.id, r.kind))
                })
            }
        }
    }

    /// The whole element, one optional field per identity — the same shape the
    /// flag octet has, so what was carried survives the round trip.
    mod uli {
        use super::*;

        #[derive(Serialize, Deserialize, Default)]
        #[serde(rename = "Uli", default)]
        struct Repr {
            cgi: Option<Cgi>,
            sai: Option<Sai>,
            rai: Option<Rai>,
            tai: Option<Tai>,
            ecgi: Option<Ecgi>,
            lai: Option<Lai>,
            macro_enb: Option<MacroEnbId>,
            extended_macro_enb: Option<ExtendedMacroEnbId>,
        }

        impl Serialize for Uli {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                Repr {
                    cgi: self.cgi,
                    sai: self.sai,
                    rai: self.rai,
                    tai: self.tai,
                    ecgi: self.ecgi,
                    lai: self.lai,
                    macro_enb: self.macro_enb,
                    extended_macro_enb: self.extended_macro_enb,
                }
                .serialize(s)
            }
        }

        impl<'de> Deserialize<'de> for Uli {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let r = Repr::deserialize(d)?;
                Ok(Uli {
                    cgi: r.cgi,
                    sai: r.sai,
                    rai: r.rai,
                    tai: r.tai,
                    ecgi: r.ecgi,
                    lai: r.lai,
                    macro_enb: r.macro_enb,
                    extended_macro_enb: r.extended_macro_enb,
                })
            }
        }
    }

    string_enum!(ExtendedEnbKind, "LONG or SHORT");

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
        #[serde(rename = "RAI")]
        Rai(Rai),
        #[serde(rename = "LAI")]
        Lai(Lai),
    }

    impl Serialize for UserLocation {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            match *self {
                UserLocation::Cgi(v) => LocationRepr::Cgi(v),
                UserLocation::Sai(v) => LocationRepr::Sai(v),
                UserLocation::Ecgi(v) => LocationRepr::Ecgi(v),
                UserLocation::Ncgi(v) => LocationRepr::Ncgi(v),
                UserLocation::Tai(v) => LocationRepr::Tai(v),
                UserLocation::Rai(v) => LocationRepr::Rai(v),
                UserLocation::Lai(v) => LocationRepr::Lai(v),
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
                LocationRepr::Rai(v) => UserLocation::Rai(v),
                LocationRepr::Lai(v) => UserLocation::Lai(v),
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
