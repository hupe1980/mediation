//! Charging-record time: **local time plus an offset**, never a bare instant.
//!
//! This is the single most consequential thing the crate gets right, and it is
//! the one most often got wrong.
//!
//! # A CDR timestamp is not UTC
//!
//! The `TimeStamp` type of 3GPP TS 32.298 is nine BCD octets spelling
//! `YYMMDDhhmmssShhmm`: a **local** date and time, a sign, and a signed offset
//! to UTC. There is no timezone *name* anywhere in a CDR — only the numeric
//! offset that was true at the moment the record was written.
//!
//! Reading such a value as though it were UTC books every evening event of a
//! UTC+X network onto the wrong calendar day. Unlike a fixed-zone mistake it
//! cannot be corrected downstream by subtracting a constant, because the offset
//! varies from record to record.
//!
//! # Offsets are quarter-hours, not hours
//!
//! The offset originates in the MS Time Zone information element
//! (3GPP TS 29.060 §7.7.52, encoded per TS 24.008, sharing the semi-octet form
//! of the SMS service-centre timestamp in TS 23.040 §9.2.3.11). It counts
//! **quarter-hours**, and a great many real zones are not on an hour boundary:
//!
//! | Zone | Offset | Quarter-hours |
//! |---|---|---|
//! | India | UTC+05:30 | 22 |
//! | Nepal | UTC+05:45 | 23 |
//! | Australia (Eucla) | UTC+08:45 | 35 |
//! | Chatham Islands | UTC+12:45 | 51 |
//!
//! An `i8` of *hours* silently truncates every subscriber in those zones onto
//! the wrong hour, and therefore into the wrong tariff window.
//!
//! # The offset is DST-adjusted, and that is not enough
//!
//! The MS Time Zone value is the offset **already including** any daylight
//! saving adjustment, with a separate two-bit field stating how much of it is
//! DST. Both are needed: UTC+02:00 is Berlin in July and Athens in January, and
//! only the DST field tells them apart.
//!
//! ```rust
//! use mediation::{DstAdjustment, MsTimeZone, QuarterHourOffset};
//!
//! let berlin_summer = MsTimeZone::new(
//!     QuarterHourOffset::from_minutes(120).unwrap(),
//!     DstAdjustment::PlusOneHour,
//! );
//! let athens_winter = MsTimeZone::new(
//!     QuarterHourOffset::from_minutes(120).unwrap(),
//!     DstAdjustment::None,
//! );
//!
//! // Same instantaneous offset …
//! assert_eq!(berlin_summer.offset(), athens_winter.offset());
//! // … different zones, and only the DST field says so.
//! assert_eq!(berlin_summer.standard_offset().unwrap().to_string(), "+01:00");
//! assert_eq!(athens_winter.standard_offset().unwrap().to_string(), "+02:00");
//! ```
//!
//! # Types here
//!
//! | Type | Models | Standard |
//! |---|---|---|
//! | [`QuarterHourOffset`] | a signed offset in 15-minute steps | TS 23.040 §9.2.3.11 |
//! | [`DstAdjustment`] | the two-bit daylight-saving field | TS 24.008 §10.5.3.12 |
//! | [`MsTimeZone`] | offset + DST together — the zone as the network reported it | TS 29.060 §7.7.52 |
//! | [`CdrTimestamp`] | local date-time + offset — the record's timestamp | TS 32.298 |
//! | [`Seconds`] | an integer duration, never derived by subtraction | — |

use std::fmt;
use std::str::FromStr;

use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};

use crate::digits::DigitString;
use crate::error::{DecodeError, ParseError};

// ── QuarterHourOffset ────────────────────────────────────────────────────────

/// A UTC offset in 15-minute steps, as carried by 3GPP charging records.
///
/// Stored as a signed count of quarter-hours, which is exactly what the wire
/// carries — no rounding to hours, and no seconds component that the encoding
/// cannot express.
///
/// # Two ranges, deliberately
///
/// The semi-octet encoding can express `±79` quarter-hours — almost ±20 hours —
/// while only [`MIN_ASSIGNED`] … [`MAX_ASSIGNED`] (UTC−12:00 … UTC+14:00) are
/// assigned to real zones. [`new`] accepts the assigned range;
/// [`from_encoded`] accepts everything the wire can hold, so a decoder can
/// carry an out-of-range offset forward as a **finding** rather than panicking
/// or clamping it.
///
/// Clamping is the worst of the three: it turns corrupt input into a plausible
/// timestamp that no later stage can identify as wrong.
///
/// [`MIN_ASSIGNED`]: Self::MIN_ASSIGNED
/// [`MAX_ASSIGNED`]: Self::MAX_ASSIGNED
/// [`new`]: Self::new
/// [`from_encoded`]: Self::from_encoded
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct QuarterHourOffset(i8);

impl QuarterHourOffset {
    /// UTC itself — offset zero.
    pub const UTC: Self = Self(0);

    /// UTC−12:00, the westernmost assigned offset.
    pub const MIN_ASSIGNED: Self = Self(-48);

    /// UTC+14:00, the easternmost assigned offset.
    pub const MAX_ASSIGNED: Self = Self(56);

    /// The largest magnitude the semi-octet encoding can hold, in quarter-hours.
    ///
    /// Two BCD digits give 0…79; the sign occupies bit 3 of the first
    /// semi-octet, which is free because the tens digit never exceeds 7.
    pub const MAX_ENCODABLE_MAGNITUDE: i8 = 79;

    /// An offset from a count of quarter-hours, restricted to assigned zones.
    ///
    /// Returns `None` outside UTC−12:00 … UTC+14:00. Use [`from_encoded`] when
    /// decoding untrusted input that must be carried forward rather than
    /// rejected at the parser.
    ///
    /// [`from_encoded`]: Self::from_encoded
    #[must_use]
    pub const fn new(quarter_hours: i8) -> Option<Self> {
        if quarter_hours >= Self::MIN_ASSIGNED.0 && quarter_hours <= Self::MAX_ASSIGNED.0 {
            Some(Self(quarter_hours))
        } else {
            None
        }
    }

    /// An offset from anything the wire can encode, assigned or not.
    ///
    /// Returns `None` only beyond [`MAX_ENCODABLE_MAGNITUDE`], which no
    /// well-formed semi-octet can produce. Check [`is_assigned`] afterwards.
    ///
    /// [`MAX_ENCODABLE_MAGNITUDE`]: Self::MAX_ENCODABLE_MAGNITUDE
    /// [`is_assigned`]: Self::is_assigned
    #[must_use]
    pub const fn from_encoded(quarter_hours: i8) -> Option<Self> {
        if quarter_hours >= -Self::MAX_ENCODABLE_MAGNITUDE
            && quarter_hours <= Self::MAX_ENCODABLE_MAGNITUDE
        {
            Some(Self(quarter_hours))
        } else {
            None
        }
    }

    /// An offset from whole minutes, which must be a multiple of 15.
    ///
    /// Returns `None` for a minute count the encoding cannot express — a
    /// 20-minute offset is not representable in a CDR, and silently rounding it
    /// would invent a timestamp.
    #[must_use]
    pub const fn from_minutes(minutes: i16) -> Option<Self> {
        if minutes % 15 != 0 {
            return None;
        }
        let quarters = minutes / 15;
        if quarters < i8::MIN as i16 || quarters > i8::MAX as i16 {
            return None;
        }
        Self::new(quarters as i8)
    }

    /// The offset in quarter-hours, signed.
    #[must_use]
    pub const fn quarter_hours(self) -> i8 {
        self.0
    }

    /// The offset in whole minutes, signed.
    #[must_use]
    pub const fn total_minutes(self) -> i16 {
        self.0 as i16 * 15
    }

    /// `true` when the offset falls in the assigned range UTC−12:00 … UTC+14:00.
    ///
    /// A decoded offset outside it is well-formed but describes no real place,
    /// and is the signal for a corrupt or misaligned field.
    #[must_use]
    pub const fn is_assigned(self) -> bool {
        self.0 >= Self::MIN_ASSIGNED.0 && self.0 <= Self::MAX_ASSIGNED.0
    }

    /// Decode the single-octet semi-octet form of TS 23.040 §9.2.3.11.
    ///
    /// The nibbles are swapped — the low nibble holds the tens digit — and bit 3
    /// of that low nibble is the sign (set means negative). Returns `None` when
    /// the units nibble is not a decimal digit.
    ///
    /// ```rust
    /// use mediation::QuarterHourOffset;
    ///
    /// // +02:00 is 8 quarter-hours: tens 0, units 8 → 0x80.
    /// assert_eq!(QuarterHourOffset::from_semi_octet(0x80).unwrap().to_string(), "+02:00");
    /// // −05:00 is 20 quarter-hours: tens 2 with the sign bit → 0x0A, units 0.
    /// assert_eq!(QuarterHourOffset::from_semi_octet(0x0A).unwrap().to_string(), "-05:00");
    /// ```
    #[must_use]
    pub const fn from_semi_octet(octet: u8) -> Option<Self> {
        let units = octet >> 4;
        if units > 9 {
            return None;
        }
        let low = octet & 0x0F;
        let negative = low & 0x08 != 0;
        let tens = low & 0x07;
        let magnitude = (tens * 10 + units) as i8;
        Self::from_encoded(if negative { -magnitude } else { magnitude })
    }

    /// Encode to the single-octet semi-octet form of TS 23.040 §9.2.3.11.
    ///
    /// Round-trips with [`from_semi_octet`](Self::from_semi_octet) for every
    /// constructible value.
    #[must_use]
    pub const fn to_semi_octet(self) -> u8 {
        let negative = self.0 < 0;
        let magnitude = self.0.unsigned_abs();
        let tens = magnitude / 10;
        let units = magnitude % 10;
        let sign = if negative { 0x08 } else { 0x00 };
        (units << 4) | tens | sign
    }

    /// The equivalent [`UtcOffset`].
    ///
    /// # Panics
    ///
    /// Never, for any constructible value: the widest encodable offset is
    /// ±79 quarter-hours (±19:45), far inside the ±25:59:59 that [`UtcOffset`]
    /// accepts. The arm exists because the conversion is fallible in general,
    /// and it panics rather than substituting UTC — a substituted offset would
    /// silently move every timestamp built from it, which is precisely the
    /// class of failure this module exists to prevent.
    #[must_use]
    pub const fn to_utc_offset(self) -> UtcOffset {
        match UtcOffset::from_whole_seconds(self.total_minutes() as i32 * 60) {
            Ok(offset) => offset,
            Err(_) => panic!("quarter-hour offset outside the range UtcOffset accepts"),
        }
    }

    /// The offset of a [`UtcOffset`], if it is a whole number of quarter-hours.
    ///
    /// Returns `None` for anything a CDR cannot carry — an offset with a
    /// seconds component, or one outside the assigned range.
    #[must_use]
    pub const fn from_utc_offset(offset: UtcOffset) -> Option<Self> {
        let seconds = offset.whole_seconds();
        if seconds % 900 != 0 {
            return None;
        }
        let quarters = seconds / 900;
        if quarters < i8::MIN as i32 || quarters > i8::MAX as i32 {
            return None;
        }
        Self::new(quarters as i8)
    }

    /// This offset with a daylight-saving adjustment removed, giving the zone's
    /// standard (winter) offset.
    ///
    /// Returns `None` when the result leaves the assigned range, which means the
    /// pairing of offset and DST field was not self-consistent.
    #[must_use]
    pub const fn without_dst(self, dst: DstAdjustment) -> Option<Self> {
        match self.0.checked_sub(dst.quarter_hours()) {
            Some(q) => Self::new(q),
            None => None,
        }
    }
}

impl fmt::Display for QuarterHourOffset {
    /// Writes the ISO 8601 form — always signed, always `±HH:MM`.
    ///
    /// UTC is `+00:00` rather than `Z`, so that every value has exactly one
    /// spelling and sorting the strings groups the sign consistently.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let minutes = self.total_minutes();
        let sign = if minutes < 0 { '-' } else { '+' };
        let magnitude = minutes.unsigned_abs();
        write!(f, "{sign}{:02}:{:02}", magnitude / 60, magnitude % 60)
    }
}

/// The shape [`QuarterHourOffset`] accepts, as rendered in a [`ParseError`].
const OFFSET_FORMAT: &str =
    "a UTC offset in 15-minute steps such as +02:00, +0545, -03:30 or Z (UTC-12:00 to UTC+14:00)";

impl FromStr for QuarterHourOffset {
    type Err = ParseError;

    /// Parses `±HH:MM`, `±HHMM`, `±HH` and `Z`.
    ///
    /// Lenient on the way in, canonical on the way out: all four spellings of
    /// UTC+02:00 render as `+02:00`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseError::format("QuarterHourOffset", s, OFFSET_FORMAT);
        let t = s.trim();
        if t.eq_ignore_ascii_case("z") {
            return Ok(Self::UTC);
        }
        let (sign, rest) = match t.as_bytes().first() {
            Some(b'+') => (1i16, &t[1..]),
            Some(b'-') => (-1i16, &t[1..]),
            _ => return Err(err()),
        };
        let digits: &str = rest;
        let (hh, mm) = match digits.len() {
            2 => (digits, "00"),
            4 => digits.split_at(2),
            5 if digits.as_bytes()[2] == b':' => (&digits[..2], &digits[3..]),
            _ => return Err(err()),
        };
        let hours: i16 = hh.parse().map_err(|_| err())?;
        let minutes: i16 = mm.parse().map_err(|_| err())?;
        if minutes > 59 {
            return Err(err());
        }
        let total = sign * (hours.checked_mul(60).ok_or_else(err)? + minutes);
        Self::from_minutes(total).ok_or_else(err)
    }
}

// ── DstAdjustment ────────────────────────────────────────────────────────────

/// The daylight-saving adjustment contained in a reported offset.
///
/// Two bits on the wire (3GPP TS 24.008 §10.5.3.12): `00` no adjustment, `01`
/// plus one hour, `10` plus two hours, `11` reserved. It states how much of the
/// reported [`QuarterHourOffset`] is seasonal, which is the only way to recover
/// the zone's standard offset and therefore to identify the zone at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum DstAdjustment {
    /// No adjustment — the reported offset is the standard one.
    #[default]
    None,
    /// The reported offset includes one hour of daylight saving.
    PlusOneHour,
    /// The reported offset includes two hours of daylight saving.
    PlusTwoHours,
}

impl DstAdjustment {
    /// Every variant, for exhaustive iteration in tests and mappings.
    pub const ALL: &'static [Self] = &[Self::None, Self::PlusOneHour, Self::PlusTwoHours];

    /// The canonical codes, in the order of [`ALL`](Self::ALL).
    pub const CODES: &'static [&'static str] = &["NONE", "PLUS_ONE_HOUR", "PLUS_TWO_HOURS"];

    /// Decode the two-bit field. `None` for the reserved value `11`.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Option<Self> {
        match bits & 0b11 {
            0b00 => Some(Self::None),
            0b01 => Some(Self::PlusOneHour),
            0b10 => Some(Self::PlusTwoHours),
            _ => None,
        }
    }

    /// Encode to the two-bit field.
    #[must_use]
    pub const fn to_bits(self) -> u8 {
        match self {
            Self::None => 0b00,
            Self::PlusOneHour => 0b01,
            Self::PlusTwoHours => 0b10,
        }
    }

    /// The adjustment in whole hours: 0, 1 or 2.
    #[must_use]
    pub const fn hours(self) -> u8 {
        match self {
            Self::None => 0,
            Self::PlusOneHour => 1,
            Self::PlusTwoHours => 2,
        }
    }

    /// The adjustment in quarter-hours, for arithmetic against an offset.
    #[must_use]
    pub const fn quarter_hours(self) -> i8 {
        (self.hours() * 4) as i8
    }

    /// `true` for any non-zero adjustment.
    #[must_use]
    pub const fn is_active(self) -> bool {
        !matches!(self, Self::None)
    }

    /// The canonical string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "NONE",
            Self::PlusOneHour => "PLUS_ONE_HOUR",
            Self::PlusTwoHours => "PLUS_TWO_HOURS",
        }
    }
}

impl fmt::Display for DstAdjustment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DstAdjustment {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_uppercase().as_str() {
            "NONE" => Ok(Self::None),
            "PLUS_ONE_HOUR" => Ok(Self::PlusOneHour),
            "PLUS_TWO_HOURS" => Ok(Self::PlusTwoHours),
            _ => Err(ParseError::one_of("DstAdjustment", s, Self::CODES)),
        }
    }
}

// ── MsTimeZone ───────────────────────────────────────────────────────────────

/// The zone a network reported for a subscriber: offset plus DST adjustment.
///
/// Kept as a type of its own rather than folded into [`CdrTimestamp`] because
/// the wire keeps them apart — the record's timestamp carries only an offset,
/// while the DST field arrives in a separate information element and is often
/// simply absent. Merging them in the type would claim knowledge the decoder
/// does not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MsTimeZone {
    offset: QuarterHourOffset,
    dst: DstAdjustment,
}

impl MsTimeZone {
    /// A zone from a reported offset and its daylight-saving component.
    #[must_use]
    pub const fn new(offset: QuarterHourOffset, dst: DstAdjustment) -> Self {
        Self { offset, dst }
    }

    /// The offset as reported — already including any daylight saving.
    #[must_use]
    pub const fn offset(self) -> QuarterHourOffset {
        self.offset
    }

    /// The daylight-saving component of the reported offset.
    #[must_use]
    pub const fn dst(self) -> DstAdjustment {
        self.dst
    }

    /// The zone's standard (winter) offset — the reported offset less the DST
    /// component.
    ///
    /// This is what identifies the zone. Two subscribers can report the same
    /// offset from different continents; their standard offsets differ.
    ///
    /// `None` when subtracting the adjustment leaves the assigned range, which
    /// means the two fields disagreed.
    #[must_use]
    pub const fn standard_offset(self) -> Option<QuarterHourOffset> {
        self.offset.without_dst(self.dst)
    }

    /// `true` when the reported offset includes a daylight-saving adjustment.
    #[must_use]
    pub const fn is_dst(self) -> bool {
        self.dst.is_active()
    }

    /// `true` when both zones share a standard offset while their reported
    /// offsets differ — the signature of a session that crossed a daylight
    /// saving transition rather than one that moved between zones.
    ///
    /// ```rust
    /// use mediation::{DstAdjustment, MsTimeZone, QuarterHourOffset};
    ///
    /// let winter = MsTimeZone::new(QuarterHourOffset::from_minutes(60).unwrap(), DstAdjustment::None);
    /// let summer = MsTimeZone::new(
    ///     QuarterHourOffset::from_minutes(120).unwrap(),
    ///     DstAdjustment::PlusOneHour,
    /// );
    /// assert!(winter.is_same_zone_across_transition(summer));
    ///
    /// // A move from Berlin winter to Athens winter is not a transition.
    /// let athens = MsTimeZone::new(QuarterHourOffset::from_minutes(120).unwrap(), DstAdjustment::None);
    /// assert!(!winter.is_same_zone_across_transition(athens));
    /// ```
    #[must_use]
    pub const fn is_same_zone_across_transition(self, other: Self) -> bool {
        if self.offset.quarter_hours() == other.offset.quarter_hours() {
            return false;
        }
        match (self.standard_offset(), other.standard_offset()) {
            (Some(a), Some(b)) => a.quarter_hours() == b.quarter_hours(),
            _ => false,
        }
    }
}

impl fmt::Display for MsTimeZone {
    /// Writes `±HH:MM/D`, where `D` is the daylight-saving component in hours.
    ///
    /// The suffix is always present so that every value has one spelling;
    /// [`FromStr`] also accepts a bare offset and reads it as no adjustment.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.offset, self.dst.hours())
    }
}

/// The shape [`MsTimeZone`] accepts, as rendered in a [`ParseError`].
const MS_TIME_ZONE_FORMAT: &str =
    "an offset with a daylight-saving suffix such as +02:00/1, or a bare offset such as +02:00";

impl FromStr for MsTimeZone {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseError::format("MsTimeZone", s, MS_TIME_ZONE_FORMAT);
        let t = s.trim();
        let (offset_part, dst) = match t.rsplit_once('/') {
            Some((head, tail)) => {
                let hours: u8 = tail.trim().parse().map_err(|_| err())?;
                let dst = match hours {
                    0 => DstAdjustment::None,
                    1 => DstAdjustment::PlusOneHour,
                    2 => DstAdjustment::PlusTwoHours,
                    _ => return Err(err()),
                };
                (head, dst)
            }
            None => (t, DstAdjustment::None),
        };
        let offset: QuarterHourOffset = offset_part.parse().map_err(|_| err())?;
        Ok(Self::new(offset, dst))
    }
}

// ── CdrTimestamp ─────────────────────────────────────────────────────────────

/// A charging-record timestamp: local wall-clock time plus its offset to UTC.
///
/// Models the `TimeStamp` of 3GPP TS 32.298 exactly. The local time is kept as
/// written, never silently normalised, so a record round-trips through this type
/// unchanged and a decoder cannot lose the wall-clock reading an operator will
/// quote back in a billing dispute.
///
/// # Ordering is deliberately absent
///
/// [`CdrTimestamp`] implements neither [`Ord`] nor [`PartialOrd`]. Two records
/// from different offsets compare correctly only as *instants*, and a derived
/// ordering would compare the local fields — silently interleaving a UTC+14:00
/// record ahead of a UTC−12:00 one that actually happened a day earlier. Sort by
/// [`to_utc`](Self::to_utc), or use [`cmp_instant`](Self::cmp_instant):
///
/// ```rust
/// use mediation::CdrTimestamp;
///
/// let mut records: Vec<CdrTimestamp> = vec![
///     "2026-07-28T01:00:00+14:00".parse().unwrap(),
///     "2026-07-27T23:00:00-12:00".parse().unwrap(),
/// ];
/// records.sort_by(CdrTimestamp::cmp_instant);
///
/// // The +14:00 record is the earlier instant, despite the later local date.
/// assert_eq!(records[0].to_string(), "2026-07-28T01:00:00+14:00");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CdrTimestamp {
    local: PrimitiveDateTime,
    offset: QuarterHourOffset,
}

impl CdrTimestamp {
    /// A timestamp from a local wall-clock reading and its offset.
    #[must_use]
    pub const fn new(local: PrimitiveDateTime, offset: QuarterHourOffset) -> Self {
        Self { local, offset }
    }

    /// The local wall-clock reading, exactly as the record carried it.
    #[must_use]
    pub const fn local(self) -> PrimitiveDateTime {
        self.local
    }

    /// The local calendar date — the correct key for grouping records into the
    /// days a subscriber would recognise on an itemised bill.
    #[must_use]
    pub const fn local_date(self) -> Date {
        self.local.date()
    }

    /// The offset to UTC that was in force where the record was written.
    #[must_use]
    pub const fn offset(self) -> QuarterHourOffset {
        self.offset
    }

    /// The instant this timestamp denotes, expressed in UTC.
    ///
    /// The returned value carries the UTC offset, not the record's, so
    /// `to_utc().date()` is the UTC date. That distinction is the whole point:
    /// a value that denoted the right instant while still reporting the local
    /// date would reintroduce the bug this type exists to prevent, in the one
    /// accessor a caller reaches for to escape it.
    #[must_use]
    pub const fn to_utc(self) -> OffsetDateTime {
        self.local
            .assume_offset(self.offset.to_utc_offset())
            .to_offset(UtcOffset::UTC)
    }

    /// The same instant re-expressed in another offset.
    ///
    /// The instant is preserved and the wall-clock reading changes — the
    /// opposite of [`new`](Self::new), which preserves the wall clock.
    #[must_use]
    pub const fn to_offset(self, offset: QuarterHourOffset) -> Self {
        let shifted = self.to_utc().to_offset(offset.to_utc_offset());
        Self {
            local: PrimitiveDateTime::new(shifted.date(), shifted.time()),
            offset,
        }
    }

    /// A timestamp for an instant, expressed in a given offset.
    #[must_use]
    pub const fn from_utc(instant: OffsetDateTime, offset: QuarterHourOffset) -> Self {
        let shifted = instant.to_offset(offset.to_utc_offset());
        Self {
            local: PrimitiveDateTime::new(shifted.date(), shifted.time()),
            offset,
        }
    }

    /// Compare two timestamps as instants.
    ///
    /// Pass to [`slice::sort_by`] to order a series correctly regardless of the
    /// offsets its records carry.
    ///
    /// [`slice::sort_by`]: slice::sort_by
    #[must_use]
    pub fn cmp_instant(a: &Self, b: &Self) -> std::cmp::Ordering {
        a.to_utc().cmp(&b.to_utc())
    }
}

/// The century a two-digit BCD year is read into.
///
/// The `TimeStamp` of TS 32.298 carries `YY`, not `YYYY`, so a decoder must
/// supply the century. There is no correct default: a record written in 1999
/// and one written in 2099 are indistinguishable on the wire, and picking a
/// pivot silently relocates one of them by a hundred years.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Century(i32);

impl Century {
    /// The 2000s — the century every record this crate will meet was written in.
    pub const TWENTY_FIRST: Self = Self(2000);

    /// A century from its first year, which must be a multiple of 100.
    #[must_use]
    pub const fn new(first_year: i32) -> Option<Self> {
        if first_year % 100 == 0 {
            Some(Self(first_year))
        } else {
            None
        }
    }

    /// The four-digit year for a two-digit one.
    #[must_use]
    pub const fn year(self, two_digit: u8) -> i32 {
        self.0 + two_digit as i32
    }
}

impl CdrTimestamp {
    /// The wire length of the 3GPP TS 32.298 `TimeStamp`, in octets.
    pub const BCD_OCTETS: usize = 9;

    /// Decode the nine-octet `TimeStamp` of 3GPP TS 32.298.
    ///
    /// The layout is `YYMMDDhhmmssShhmm`: six octets of **plain** BCD for the
    /// local date and time, one ASCII octet for the sign (`+` or `-`), then two
    /// plain-BCD octets for the offset's hours and minutes.
    ///
    /// Note the two conventions in one field: the date-time digits are plain
    /// BCD, while the identifier fields of the same record are swapped-nibble
    /// TBCD. See [`crate::digits`].
    ///
    /// ```rust
    /// use mediation::{CdrTimestamp, Century};
    ///
    /// // 2026-07-28 10:00:00, offset +02:00.
    /// let octets = [0x26, 0x07, 0x28, 0x10, 0x00, 0x00, b'+', 0x02, 0x00];
    /// let ts = CdrTimestamp::from_bcd(&octets, Century::TWENTY_FIRST).unwrap();
    ///
    /// assert_eq!(ts.to_string(), "2026-07-28T10:00:00+02:00");
    /// assert_eq!(ts.to_bcd().unwrap(), octets);
    /// ```
    ///
    /// # Errors
    ///
    /// [`DecodeError`] for the wrong length, a non-decimal nibble, a sign octet
    /// that is neither `+` nor `-`, an impossible date or time, or an offset
    /// that is not a multiple of 15 minutes.
    pub fn from_bcd(bytes: &[u8], century: Century) -> Result<Self, DecodeError> {
        if bytes.len() != Self::BCD_OCTETS {
            return Err(DecodeError::new(0, "exactly 9 octets"));
        }
        let stamp = DigitString::from_bcd(&bytes[..6])?;
        let sign: i16 = match bytes[6] {
            b'+' => 1,
            b'-' => -1,
            _ => return Err(DecodeError::new(6, "an ASCII '+' or '-' sign octet")),
        };
        let offset_digits = DigitString::from_bcd(&bytes[7..9])
            .map_err(|e| DecodeError::new(7 + e.at(), e.expected()))?;

        let pair = |digits: DigitString, index: usize| -> u8 {
            digits.at(index * 2) * 10 + digits.at(index * 2 + 1)
        };
        let (year, month, day) = (pair(stamp, 0), pair(stamp, 1), pair(stamp, 2));
        let (hour, minute, second) = (pair(stamp, 3), pair(stamp, 4), pair(stamp, 5));
        let (offset_hours, offset_minutes) = (pair(offset_digits, 0), pair(offset_digits, 1));

        let month = Month::try_from(month).map_err(|_| DecodeError::new(1, "a month in 1..=12"))?;
        let date = Date::from_calendar_date(century.year(year), month, day)
            .map_err(|_| DecodeError::new(0, "a date that exists"))?;
        let time = Time::from_hms(hour, minute, second)
            .map_err(|_| DecodeError::new(3, "a time that exists"))?;

        let total = sign * (i16::from(offset_hours) * 60 + i16::from(offset_minutes));
        let offset = QuarterHourOffset::from_minutes(total)
            .ok_or_else(|| DecodeError::new(7, "an assigned offset in 15-minute steps"))?;

        Ok(Self::new(PrimitiveDateTime::new(date, time), offset))
    }

    /// Encode to the nine-octet `TimeStamp` of 3GPP TS 32.298.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] when the year falls outside the two digits the encoding
    /// carries. Refusing is deliberate: writing `YY` for a year the reader will
    /// resolve into a different century is how a record moves a hundred years
    /// without anything reporting an error.
    pub fn to_bcd(self) -> Result<[u8; Self::BCD_OCTETS], DecodeError> {
        let date = self.local.date();
        let year = date.year();
        if !(0..=9999).contains(&year) {
            return Err(DecodeError::new(0, "a year the two-digit field can carry"));
        }
        let time = self.local.time();
        let minutes = self.offset.total_minutes();
        let magnitude = minutes.unsigned_abs();

        let bcd = |value: u8| -> u8 { ((value / 10) << 4) | (value % 10) };
        Ok([
            bcd((year % 100) as u8),
            bcd(u8::from(date.month())),
            bcd(date.day()),
            bcd(time.hour()),
            bcd(time.minute()),
            bcd(time.second()),
            if minutes < 0 { b'-' } else { b'+' },
            bcd((magnitude / 60) as u8),
            bcd((magnitude % 60) as u8),
        ])
    }
}

impl fmt::Display for CdrTimestamp {
    /// Writes the RFC 3339 form with whole seconds: `2026-07-28T06:00:00+05:45`.
    ///
    /// Whole seconds because the 3GPP encoding has no sub-second field — writing
    /// a fractional part would claim precision the record never carried.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let d = self.local.date();
        let t = self.local.time();
        write!(
            f,
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{}",
            d.year(),
            u8::from(d.month()),
            d.day(),
            t.hour(),
            t.minute(),
            t.second(),
            self.offset,
        )
    }
}

/// The shape [`CdrTimestamp`] accepts, as rendered in a [`ParseError`].
const TIMESTAMP_FORMAT: &str =
    "an RFC 3339 date-time with a 15-minute offset, such as 2026-07-28T06:00:00+05:45";

impl FromStr for CdrTimestamp {
    type Err = ParseError;

    /// Parses `YYYY-MM-DDThh:mm:ss` followed by an offset.
    ///
    /// The date-time separator may be `T`, `t` or a space. The offset accepts
    /// every spelling [`QuarterHourOffset`] does.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseError::format("CdrTimestamp", s, TIMESTAMP_FORMAT);
        let t = s.trim();
        if t.len() < 20 {
            return Err(err());
        }
        let bytes = t.as_bytes();
        if !matches!(bytes[10], b'T' | b't' | b' ') {
            return Err(err());
        }
        let (date_part, rest) = t.split_at(10);
        let rest = &rest[1..];
        // The time is `hh:mm:ss`; everything after it is the offset.
        if rest.len() < 8 {
            return Err(err());
        }
        let (time_part, offset_part) = rest.split_at(8);

        let year: i32 = date_part[..4].parse().map_err(|_| err())?;
        let month: u8 = date_part[5..7].parse().map_err(|_| err())?;
        let day: u8 = date_part[8..10].parse().map_err(|_| err())?;
        if date_part.as_bytes()[4] != b'-' || date_part.as_bytes()[7] != b'-' {
            return Err(err());
        }

        let hour: u8 = time_part[..2].parse().map_err(|_| err())?;
        let minute: u8 = time_part[3..5].parse().map_err(|_| err())?;
        let second: u8 = time_part[6..8].parse().map_err(|_| err())?;
        if time_part.as_bytes()[2] != b':' || time_part.as_bytes()[5] != b':' {
            return Err(err());
        }

        let month = Month::try_from(month).map_err(|_| err())?;
        let date = Date::from_calendar_date(year, month, day).map_err(|_| err())?;
        let time = Time::from_hms(hour, minute, second).map_err(|_| err())?;
        let offset: QuarterHourOffset = offset_part.parse().map_err(|_| err())?;
        Ok(Self::new(PrimitiveDateTime::new(date, time), offset))
    }
}

// ── Seconds ──────────────────────────────────────────────────────────────────

/// An integer duration in seconds.
///
/// Used for the **chargeable** duration a record carries, which is not the same
/// thing as the span between its open and close timestamps: chargeable duration
/// runs from answer, so a 45-second ring followed by 10 seconds of conversation
/// is a 10-second call across a 55-second wall-clock span. Deriving one from the
/// other over-bills every answered call and invents duration for every
/// unanswered one — which is why this type is constructed from a value, never
/// from a subtraction.
///
/// `u32` holds just over 136 years, well beyond any session, and keeps the type
/// four bytes so a record stays cache-friendly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Seconds(u32);

impl Seconds {
    /// Zero duration.
    pub const ZERO: Self = Self(0);

    /// A duration from a count of seconds.
    #[must_use]
    pub const fn new(seconds: u32) -> Self {
        Self(seconds)
    }

    /// The count of seconds.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }

    /// Sum, `None` on overflow.
    ///
    /// Checked rather than wrapping because summing partial records is exactly
    /// where an overflow would appear, and a wrapped total is a plausible-looking
    /// wrong number on an invoice.
    #[must_use]
    pub const fn checked_add(self, other: Self) -> Option<Self> {
        match self.0.checked_add(other.0) {
            Some(v) => Some(Self(v)),
            None => None,
        }
    }

    /// The equivalent [`time::Duration`].
    #[must_use]
    pub const fn to_duration(self) -> time::Duration {
        time::Duration::seconds(self.0 as i64)
    }

    /// The number of whole `step`-second units this duration occupies, rounding
    /// up — one chargeable minute for a one-second call.
    ///
    /// A rating primitive, offered because per-minute and per-30-second
    /// rounding are ubiquitous. Nothing in this crate calls it: what leaves here
    /// is the measured duration.
    ///
    /// `None` for a zero step.
    ///
    /// ```rust
    /// use mediation::Seconds;
    ///
    /// assert_eq!(Seconds::new(1).units_ceil(60), Some(1));
    /// assert_eq!(Seconds::new(60).units_ceil(60), Some(1));
    /// assert_eq!(Seconds::new(61).units_ceil(60), Some(2));
    /// assert_eq!(Seconds::ZERO.units_ceil(60), Some(0));
    /// ```
    #[must_use]
    pub const fn units_ceil(self, step: u32) -> Option<u32> {
        if step == 0 {
            return None;
        }
        Some(self.0.div_ceil(step))
    }

    /// This duration rounded up to a whole multiple of `step` seconds.
    ///
    /// A rating primitive; `None` for a zero step or on overflow.
    #[must_use]
    pub const fn round_up_to(self, step: u32) -> Option<Self> {
        match self.units_ceil(step) {
            Some(units) => match units.checked_mul(step) {
                Some(v) => Some(Self(v)),
                None => None,
            },
            None => None,
        }
    }

    /// `true` for a zero duration.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for Seconds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Seconds {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.trim()
            .parse::<u32>()
            .map(Self)
            .map_err(|_| ParseError::format("Seconds", s, "a non-negative whole number of seconds"))
    }
}

impl From<Seconds> for u32 {
    fn from(s: Seconds) -> Self {
        s.0
    }
}

impl From<u32> for Seconds {
    fn from(s: u32) -> Self {
        Self(s)
    }
}

// ── Serde: the canonical strings, not the Rust field names ───────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::{CdrTimestamp, DstAdjustment, MsTimeZone, QuarterHourOffset, Seconds};
    use serde::de::{self, Visitor};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::fmt;

    /// Serialise a `Display`/`FromStr` pair as its canonical string.
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

    string_serde!(QuarterHourOffset, "a UTC offset string such as \"+05:45\"");
    string_serde!(DstAdjustment, "a daylight-saving code such as \"NONE\"");
    string_serde!(MsTimeZone, "a zone string such as \"+02:00/1\"");
    string_serde!(
        CdrTimestamp,
        "an RFC 3339 timestamp such as \"2026-07-28T06:00:00+05:45\""
    );

    impl Serialize for Seconds {
        /// A JSON number, not a string — a duration is a quantity.
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_u32(self.get())
        }
    }

    impl<'de> Deserialize<'de> for Seconds {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            u32::deserialize(deserializer).map(Seconds::new)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    // ── QuarterHourOffset ────────────────────────────────────────────────────

    /// The bug this type exists to prevent: an hour-granular offset truncates
    /// every quarter-hour zone onto the wrong hour.
    #[test]
    fn quarter_hour_zones_survive() {
        for (minutes, rendered) in [
            (330i16, "+05:30"), // India
            (345, "+05:45"),    // Nepal
            (525, "+08:45"),    // Eucla
            (765, "+12:45"),    // Chatham
            (-210, "-03:30"),   // Newfoundland
        ] {
            let o = QuarterHourOffset::from_minutes(minutes).expect("assigned zone");
            assert_eq!(o.to_string(), rendered);
            assert_eq!(o.total_minutes(), minutes);
        }
    }

    #[test]
    fn offsets_off_the_quarter_hour_are_not_representable() {
        assert_eq!(QuarterHourOffset::from_minutes(20), None);
        assert_eq!(QuarterHourOffset::from_minutes(1), None);
        assert!(QuarterHourOffset::from_minutes(15).is_some());
    }

    /// The encodable range is wider than the assigned one, and a decoder must be
    /// able to carry the difference forward rather than clamp it.
    #[test]
    fn encodable_range_exceeds_assigned_range() {
        let bogus = QuarterHourOffset::from_encoded(79).expect("encodable");
        assert!(!bogus.is_assigned());
        assert_eq!(QuarterHourOffset::new(79), None);

        assert!(QuarterHourOffset::MAX_ASSIGNED.is_assigned());
        assert!(QuarterHourOffset::MIN_ASSIGNED.is_assigned());
        assert_eq!(QuarterHourOffset::MAX_ASSIGNED.to_string(), "+14:00");
        assert_eq!(QuarterHourOffset::MIN_ASSIGNED.to_string(), "-12:00");
    }

    #[test]
    fn semi_octet_round_trips_every_encodable_value() {
        for q in
            -QuarterHourOffset::MAX_ENCODABLE_MAGNITUDE..=QuarterHourOffset::MAX_ENCODABLE_MAGNITUDE
        {
            let o = QuarterHourOffset::from_encoded(q).expect("encodable");
            let octet = o.to_semi_octet();
            assert_eq!(
                QuarterHourOffset::from_semi_octet(octet),
                Some(o),
                "0x{octet:02X} must decode back to {q}"
            );
        }
    }

    #[test]
    fn semi_octet_rejects_non_decimal_units() {
        // Units nibble 0xA is not a decimal digit.
        assert_eq!(QuarterHourOffset::from_semi_octet(0xA0), None);
    }

    #[test]
    fn offset_parses_every_spelling_and_writes_one() {
        for s in ["+02:00", "+0200", "+02", " +02:00 "] {
            assert_eq!(
                s.parse::<QuarterHourOffset>().unwrap().to_string(),
                "+02:00"
            );
        }
        assert_eq!(
            "Z".parse::<QuarterHourOffset>().unwrap(),
            QuarterHourOffset::UTC
        );
        assert_eq!(QuarterHourOffset::UTC.to_string(), "+00:00");
    }

    #[test]
    fn offset_rejects_malformed_and_unassigned_input() {
        for s in ["", "02:00", "+2:00", "+02:60", "+99:00", "+00:20", "-13:00"] {
            assert!(
                s.parse::<QuarterHourOffset>().is_err(),
                "{s:?} must not parse"
            );
        }
    }

    #[test]
    fn utc_offset_conversion_rejects_sub_quarter_hour_offsets() {
        let odd = UtcOffset::from_whole_seconds(3661).unwrap();
        assert_eq!(QuarterHourOffset::from_utc_offset(odd), None);

        let ok = UtcOffset::from_whole_seconds(5 * 3600 + 45 * 60).unwrap();
        assert_eq!(
            QuarterHourOffset::from_utc_offset(ok).unwrap().to_string(),
            "+05:45"
        );
    }

    // ── DstAdjustment / MsTimeZone ───────────────────────────────────────────

    #[test]
    fn dst_bits_round_trip_and_reserved_is_rejected() {
        for d in DstAdjustment::ALL {
            assert_eq!(DstAdjustment::from_bits(d.to_bits()), Some(*d));
        }
        assert_eq!(DstAdjustment::from_bits(0b11), None);
    }

    /// The reason the DST field is kept at all: the same reported offset
    /// belongs to different zones depending on it.
    #[test]
    fn identical_offsets_are_different_zones() {
        let plus_two = QuarterHourOffset::from_minutes(120).unwrap();
        let berlin_summer = MsTimeZone::new(plus_two, DstAdjustment::PlusOneHour);
        let athens_winter = MsTimeZone::new(plus_two, DstAdjustment::None);

        assert_eq!(berlin_summer.offset(), athens_winter.offset());
        assert_ne!(
            berlin_summer.standard_offset(),
            athens_winter.standard_offset()
        );
        assert_eq!(
            berlin_summer.standard_offset().unwrap().to_string(),
            "+01:00"
        );
        assert!(berlin_summer.is_dst() && !athens_winter.is_dst());
    }

    #[test]
    fn a_dst_transition_is_distinguishable_from_a_zone_change() {
        let berlin_winter = MsTimeZone::new(
            QuarterHourOffset::from_minutes(60).unwrap(),
            DstAdjustment::None,
        );
        let berlin_summer = MsTimeZone::new(
            QuarterHourOffset::from_minutes(120).unwrap(),
            DstAdjustment::PlusOneHour,
        );
        let athens_winter = MsTimeZone::new(
            QuarterHourOffset::from_minutes(120).unwrap(),
            DstAdjustment::None,
        );

        assert!(berlin_winter.is_same_zone_across_transition(berlin_summer));
        assert!(!berlin_winter.is_same_zone_across_transition(athens_winter));
        // An unchanged offset is not a transition either.
        assert!(!berlin_winter.is_same_zone_across_transition(berlin_winter));
    }

    #[test]
    fn inconsistent_offset_and_dst_yield_no_standard_offset() {
        // UTC-12:00 cannot also contain two hours of daylight saving: the
        // implied standard offset would be UTC-14:00, which is unassigned.
        let broken = MsTimeZone::new(QuarterHourOffset::MIN_ASSIGNED, DstAdjustment::PlusTwoHours);
        assert_eq!(broken.standard_offset(), None);
    }

    #[test]
    fn ms_time_zone_round_trips_and_accepts_a_bare_offset() {
        let z = MsTimeZone::new(
            QuarterHourOffset::from_minutes(120).unwrap(),
            DstAdjustment::PlusOneHour,
        );
        assert_eq!(z.to_string(), "+02:00/1");
        assert_eq!("+02:00/1".parse::<MsTimeZone>().unwrap(), z);
        // Lenient in: a bare offset means no adjustment.
        assert_eq!(
            "+02:00".parse::<MsTimeZone>().unwrap(),
            MsTimeZone::new(
                QuarterHourOffset::from_minutes(120).unwrap(),
                DstAdjustment::None
            )
        );
        assert!("+02:00/3".parse::<MsTimeZone>().is_err());
    }

    // ── CdrTimestamp ─────────────────────────────────────────────────────────

    /// The bug this type exists to prevent: reading a local timestamp as UTC.
    #[test]
    fn local_time_is_not_utc() {
        let ts: CdrTimestamp = "2026-07-28T00:30:00+05:45".parse().unwrap();
        // The subscriber's day is the 28th …
        assert_eq!(ts.local_date().to_string(), "2026-07-28");
        // … while the instant falls on the 27th in UTC.
        assert_eq!(ts.to_utc(), datetime!(2026-07-27 18:45:00 UTC));
    }

    /// `to_utc` must re-express the instant in UTC, not merely denote it: a
    /// value that compared equal to the right instant while still reporting the
    /// local date would smuggle the local day back into every caller that
    /// reached for this accessor to escape it.
    #[test]
    fn to_utc_reports_the_utc_calendar_fields() {
        let ts: CdrTimestamp = "2026-07-28T00:30:00+05:45".parse().unwrap();
        let utc = ts.to_utc();
        assert_eq!(utc.offset(), UtcOffset::UTC);
        assert_eq!(utc.date().to_string(), "2026-07-27");
        assert_eq!(utc.hour(), 18);
        assert_eq!(utc.minute(), 45);
    }

    #[test]
    fn timestamp_round_trips_through_its_canonical_string() {
        for s in [
            "2026-07-28T06:00:00+05:45",
            "2026-01-01T00:00:00+00:00",
            "2025-12-31T23:59:59-12:00",
            "2026-06-15T12:34:56+14:00",
        ] {
            let ts: CdrTimestamp = s.parse().unwrap();
            assert_eq!(ts.to_string(), s);
        }
    }

    #[test]
    fn timestamp_accepts_lenient_separators_and_offset_spellings() {
        let canonical: CdrTimestamp = "2026-07-28T06:00:00+02:00".parse().unwrap();
        for s in [
            "2026-07-28t06:00:00+02:00",
            "2026-07-28 06:00:00+02:00",
            "2026-07-28T06:00:00+0200",
            " 2026-07-28T06:00:00+02 ",
        ] {
            assert_eq!(s.parse::<CdrTimestamp>().unwrap(), canonical, "{s:?}");
        }
    }

    #[test]
    fn timestamp_rejects_malformed_input() {
        for s in [
            "",
            "2026-07-28",
            "2026-07-28T06:00:00",       // no offset
            "2026-13-01T00:00:00+00:00", // month 13
            "2026-02-30T00:00:00+00:00", // impossible day
            "2026-07-28X06:00:00+00:00", // bad separator
            "2026-07-28T25:00:00+00:00", // hour 25
            "2026/07/28T06:00:00+00:00", // wrong date separators
        ] {
            assert!(s.parse::<CdrTimestamp>().is_err(), "{s:?} must not parse");
        }
    }

    /// Sorting by the struct would interleave records from distant offsets;
    /// there is no `Ord`, and the instant comparator is the supported way.
    #[test]
    fn ordering_is_by_instant_not_by_wall_clock() {
        let far_east: CdrTimestamp = "2026-07-28T01:00:00+14:00".parse().unwrap();
        let far_west: CdrTimestamp = "2026-07-27T23:00:00-12:00".parse().unwrap();
        assert!(far_east.to_utc() < far_west.to_utc());

        let mut series = vec![far_west, far_east];
        series.sort_by(CdrTimestamp::cmp_instant);
        assert_eq!(series, vec![far_east, far_west]);
    }

    #[test]
    fn re_expressing_an_instant_preserves_it() {
        let ts: CdrTimestamp = "2026-07-28T06:00:00+05:45".parse().unwrap();
        let utc = ts.to_offset(QuarterHourOffset::UTC);
        assert_eq!(utc.to_utc(), ts.to_utc());
        assert_eq!(utc.to_string(), "2026-07-28T00:15:00+00:00");
        assert_ne!(utc.local(), ts.local());
    }

    #[test]
    fn from_utc_and_to_utc_are_inverse() {
        let offset = QuarterHourOffset::from_minutes(345).unwrap();
        let instant = datetime!(2026-03-29 01:30:00 UTC);
        let ts = CdrTimestamp::from_utc(instant, offset);
        assert_eq!(ts.to_utc(), instant);
        assert_eq!(ts.offset(), offset);
    }

    // ── Seconds ──────────────────────────────────────────────────────────────

    // ── BCD codec ────────────────────────────────────────────────────────────

    #[test]
    fn the_bcd_timestamp_round_trips() {
        let octets = [0x26, 0x07, 0x28, 0x10, 0x00, 0x00, b'+', 0x02, 0x00];
        let ts = CdrTimestamp::from_bcd(&octets, Century::TWENTY_FIRST).unwrap();
        assert_eq!(ts.to_string(), "2026-07-28T10:00:00+02:00");
        assert_eq!(ts.to_bcd().unwrap(), octets);
    }

    /// A negative offset uses the ASCII `-`, and quarter-hour zones survive.
    #[test]
    fn the_sign_octet_is_ascii_and_quarter_hours_survive() {
        let nepal = [0x26, 0x07, 0x28, 0x09, 0x30, 0x00, b'+', 0x05, 0x45];
        let ts = CdrTimestamp::from_bcd(&nepal, Century::TWENTY_FIRST).unwrap();
        assert_eq!(ts.offset().to_string(), "+05:45");

        let newfoundland = [0x26, 0x07, 0x28, 0x09, 0x30, 0x00, b'-', 0x03, 0x30];
        let ts = CdrTimestamp::from_bcd(&newfoundland, Century::TWENTY_FIRST).unwrap();
        assert_eq!(ts.offset().to_string(), "-03:30");
        assert_eq!(ts.to_bcd().unwrap(), newfoundland);
    }

    /// There is no correct default century, so the decoder takes one.
    #[test]
    fn the_century_is_supplied_rather_than_guessed() {
        let octets = [0x99, 0x12, 0x31, 0x23, 0x59, 0x59, b'+', 0x00, 0x00];
        let twentieth = CdrTimestamp::from_bcd(&octets, Century::new(1900).unwrap()).unwrap();
        let twenty_first = CdrTimestamp::from_bcd(&octets, Century::TWENTY_FIRST).unwrap();

        assert_eq!(twentieth.local_date().year(), 1999);
        assert_eq!(twenty_first.local_date().year(), 2099);
        assert_ne!(twentieth, twenty_first);
        assert_eq!(Century::new(1950), None, "a century starts on a hundred");
    }

    #[test]
    fn a_malformed_timestamp_names_the_octet_that_failed() {
        // Wrong length.
        assert!(CdrTimestamp::from_bcd(&[0x26; 8], Century::TWENTY_FIRST).is_err());

        // A sign octet that is neither '+' nor '-'.
        let bad_sign = [0x26, 0x07, 0x28, 0x10, 0x00, 0x00, 0x00, 0x02, 0x00];
        let err = CdrTimestamp::from_bcd(&bad_sign, Century::TWENTY_FIRST).unwrap_err();
        assert_eq!(err.at(), 6, "{err}");

        // An offset that is not a multiple of 15 minutes.
        let odd_offset = [0x26, 0x07, 0x28, 0x10, 0x00, 0x00, b'+', 0x02, 0x20];
        let err = CdrTimestamp::from_bcd(&odd_offset, Century::TWENTY_FIRST).unwrap_err();
        assert_eq!(err.at(), 7, "{err}");

        // A month that does not exist.
        let bad_month = [0x26, 0x13, 0x28, 0x10, 0x00, 0x00, b'+', 0x00, 0x00];
        assert!(CdrTimestamp::from_bcd(&bad_month, Century::TWENTY_FIRST).is_err());

        // A day that does not exist in that month.
        let bad_day = [0x26, 0x02, 0x30, 0x10, 0x00, 0x00, b'+', 0x00, 0x00];
        assert!(CdrTimestamp::from_bcd(&bad_day, Century::TWENTY_FIRST).is_err());
    }

    #[test]
    fn seconds_sum_is_checked() {
        assert_eq!(
            Seconds::new(30).checked_add(Seconds::new(12)),
            Some(Seconds::new(42))
        );
        assert_eq!(Seconds::new(u32::MAX).checked_add(Seconds::new(1)), None);
    }

    #[test]
    fn seconds_round_trip() {
        assert_eq!("900".parse::<Seconds>().unwrap(), Seconds::new(900));
        assert_eq!(Seconds::new(900).to_string(), "900");
        assert!("-1".parse::<Seconds>().is_err());
        assert!("1.5".parse::<Seconds>().is_err());
    }
}
