//! What the subscriber dialled, and what kind of destination it is.
//!
//! A fixed-line or IMS voice platform records the **dialled** string — `0151 1234567`,
//! `0049 151…`, `+49 151…`, `1234567`, `110`, `116117`, `0800 …` — not an E.164 number.
//! Rating, itemisation, fraud detection and deduplication all need one canonical
//! [`Destination`], and the conversion depends on facts the string does not carry: the
//! country it was dialled in, its prefixes, and the caller's own area code.
//!
//! ```rust
//! use mediation::{DestinationClass, DialContext, GermanNumberingPlan};
//!
//! let berlin = DialContext::germany().with_area_code("030")?;
//!
//! let mobile = berlin.normalise("0151 1234567")?;
//! assert_eq!(mobile.to_string(), "+491511234567");
//! assert_eq!(mobile.class(&GermanNumberingPlan), Some(DestinationClass::Mobile));
//!
//! // A local number takes the caller's area code — and is refused without one.
//! assert_eq!(berlin.normalise("2345678")?.to_string(), "+49302345678");
//! assert!(DialContext::germany().normalise("2345678").is_err());
//!
//! // A short code stays a short code: 110 is not +49110.
//! let police = berlin.normalise("110")?;
//! assert_eq!(police.to_string(), "110");
//! assert_eq!(police.class(&GermanNumberingPlan), Some(DestinationClass::Emergency));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Refuse, never guess
//!
//! A local number without an area code is [`DialError::AreaCodeRequired`], not a
//! number in whichever area the code happened to default to. A range the plan does
//! not list is *not classified*, never a default class. Errors name the reason and
//! the position, never the digits — a dial string is traffic data (§ 9 TDDDG).
//!
//! # Classification is replaceable
//!
//! [`NumberingPlan`] is a trait. [`GermanNumberingPlan`] is a dated example built from
//! the Federal Network Agency's numbering plan; a [`NumberRangeSet`] loaded from the
//! agency's published allocations, or a closure, works in its place.

use std::fmt;
use std::str::FromStr;

use crate::digits::DigitString;
use crate::error::ParseError;
use crate::identity::Msisdn;
use crate::numbering::NumberRangeSet;

// ── Destination ──────────────────────────────────────────────────────────────

/// Where a call went: an international number, or a national short code.
///
/// The canonical string form keeps the two apart: `+491511234567` for a number,
/// `110` for a short code. A short code is not an E.164 number and never becomes
/// one — `110` dialled in Berlin is the police, `+49110` is nothing.
///
/// ```rust
/// use mediation::Destination;
///
/// let number: Destination = "+49 30 1234567".parse()?;
/// assert_eq!(number.to_string(), "+49301234567");
/// let short: Destination = "116117".parse()?;
/// assert!(short.as_number().is_none());
/// # Ok::<(), mediation::ParseError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Destination {
    /// An international number, country code first.
    Number(Msisdn),
    /// A national short code such as `110`, `116117` or `11833`.
    Short(ShortCode),
}

impl Destination {
    /// The international number, when this is one.
    #[must_use]
    pub const fn as_number(&self) -> Option<Msisdn> {
        match self {
            Self::Number(n) => Some(*n),
            Self::Short(_) => None,
        }
    }

    /// The digits, without the `+` of an international number.
    #[must_use]
    pub const fn digits(&self) -> DigitString {
        match self {
            Self::Number(n) => n.digits(),
            Self::Short(s) => s.0,
        }
    }

    /// What kind of destination this is, according to `plan`.
    ///
    /// `None` means the plan does not classify it — not that it is harmless.
    #[must_use]
    pub fn class(&self, plan: &(impl NumberingPlan + ?Sized)) -> Option<DestinationClass> {
        plan.classify(self)
    }
}

impl From<Msisdn> for Destination {
    fn from(number: Msisdn) -> Self {
        Self::Number(number)
    }
}

impl From<ShortCode> for Destination {
    fn from(code: ShortCode) -> Self {
        Self::Short(code)
    }
}

impl fmt::Display for Destination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(n) => write!(f, "+{n}"),
            Self::Short(s) => s.fmt(f),
        }
    }
}

const DESTINATION_FORMAT: &str = "+ and a country code not starting with 0, 1 to 15 digits in all, \
    or a short code of 2 to 6 digits, such as +49301234567 or 110";

impl FromStr for Destination {
    type Err = ParseError;

    /// Parses the canonical form: `+` and digits for a number, bare digits for a
    /// short code. Spaces and `-` are ignored. A rejection does not keep the
    /// input ([`ParseError::is_withheld`]).
    ///
    /// A country code never starts with `0` (ITU-T E.164), so `+0…` is
    /// refused (audit D8).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim();
        if t.starts_with('+') {
            if t.bytes().find(u8::is_ascii_digit) == Some(b'0') {
                return Err(ParseError::withheld("Destination", DESTINATION_FORMAT));
            }
            t.parse()
                .map(Self::Number)
                .map_err(|_| ParseError::withheld("Destination", DESTINATION_FORMAT))
        } else {
            t.parse()
                .map(Self::Short)
                .map_err(|_| ParseError::withheld("Destination", DESTINATION_FORMAT))
        }
    }
}

/// A national short code: 2 to 6 digits, dialled without any prefix.
///
/// Emergency numbers (110, 112; FR 15, 17), the authority number 115,
/// harmonised services of social value (116xyz), directory enquiries (118xy).
/// Which strings are short codes is the [`DialContext`]'s business; what kind
/// a code is comes from a [`NumberingPlan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShortCode(DigitString);

impl ShortCode {
    /// The fewest digits a short code has (FR 15, 17, 18).
    pub const MIN_DIGITS: usize = 2;
    /// The most digits a short code has (116xyz).
    pub const MAX_DIGITS: usize = 6;

    /// How many digits the code has.
    #[must_use]
    pub const fn len(self) -> usize {
        self.0.len()
    }

    /// Always `false` — a short code is never empty.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        false
    }

    /// The digits.
    #[must_use]
    pub const fn digits(self) -> DigitString {
        self.0
    }

    fn from_digits(digits: DigitString) -> Option<Self> {
        (Self::MIN_DIGITS..=Self::MAX_DIGITS)
            .contains(&digits.len())
            .then_some(Self(digits))
    }
}

impl fmt::Display for ShortCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for ShortCode {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseError::withheld("ShortCode", "2 to 6 digits, such as 110 or 116117");
        DigitString::parse(s.trim(), Self::MAX_DIGITS)
            .and_then(Self::from_digits)
            .ok_or_else(err)
    }
}

// ── Dialling context ─────────────────────────────────────────────────────────

/// An area code without its trunk prefix: 1 to 5 digits, never starting with `0`.
///
/// Parses `30` and `030` alike — an area code never starts with `0`, so a leading
/// `0` can only be the trunk prefix and dropping it is not a guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AreaCode(DigitString);

impl AreaCode {
    /// The digits, without the trunk prefix.
    #[must_use]
    pub const fn digits(self) -> DigitString {
        self.0
    }
}

impl fmt::Display for AreaCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for AreaCode {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseError::format("AreaCode", s, "1 to 5 digits, such as 30 or 030");
        let t = s.trim();
        let t = t.strip_prefix('0').unwrap_or(t);
        let digits = DigitString::parse(t, 5).ok_or_else(err)?;
        if digits.is_empty() || digits.at(0) == 0 {
            return Err(err());
        }
        Ok(Self(digits))
    }
}

/// A short code a [`DialContext`] recognises: a digit prefix and the exact
/// total length.
///
/// `ShortCodePattern::new("116", 6)` matches `116117`; `("15", 2)` matches the
/// French SAMU. Checked when handed to [`DialContext::with_short_codes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShortCodePattern {
    prefix: &'static str,
    len: u8,
}

impl ShortCodePattern {
    /// The codes that start with `prefix` and have exactly `len` digits.
    #[must_use]
    pub const fn new(prefix: &'static str, len: u8) -> Self {
        Self { prefix, len }
    }

    /// The digit prefix.
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        self.prefix
    }

    /// The exact number of digits.
    #[must_use]
    pub const fn len(self) -> u8 {
        self.len
    }

    /// Always `false` — a valid pattern has at least one prefix digit.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        false
    }

    fn is_valid(self) -> bool {
        is_digits(self.prefix)
            && (ShortCode::MIN_DIGITS..=ShortCode::MAX_DIGITS).contains(&usize::from(self.len))
            && self.prefix.len() <= usize::from(self.len)
    }

    fn matches(self, digits: &[u8]) -> bool {
        digits.len() == usize::from(self.len) && strip(digits, self.prefix).is_some()
    }

    /// `true` when `digits` and the prefix agree as far as both go.
    fn overlaps(self, digits: &[u8]) -> bool {
        self.prefix
            .bytes()
            .zip(digits)
            .all(|(p, &d)| p.wrapping_sub(b'0') == d)
    }
}

const fn pattern(prefix: &'static str, len: u8) -> ShortCodePattern {
    ShortCodePattern::new(prefix, len)
}

/// BNetzA Nummernplan (Vfg 29/2015 as amended by Vfg 92/2025): 110 and 112
/// emergency, 115 authority, 116xyz harmonised, 118xy directory, 11x
/// (x = 1, 3, 4, 7, 9) network-internal.
const GERMAN_SHORT_CODES: &[ShortCodePattern] = &[
    pattern("110", 3),
    pattern("112", 3),
    pattern("115", 3),
    pattern("116", 6),
    pattern("118", 5),
    pattern("111", 3),
    pattern("113", 3),
    pattern("114", 3),
    pattern("117", 3),
    pattern("119", 3),
];

/// Italy: 112, 113, 115, 118 emergency; 116xyz harmonised.
const ITALIAN_SHORT_CODES: &[ShortCodePattern] = &[
    pattern("112", 3),
    pattern("113", 3),
    pattern("115", 3),
    pattern("118", 3),
    pattern("116", 6),
];

/// NANP N11 service codes.
const NANP_SHORT_CODES: &[ShortCodePattern] = &[
    pattern("211", 3),
    pattern("311", 3),
    pattern("411", 3),
    pattern("511", 3),
    pattern("611", 3),
    pattern("711", 3),
    pattern("811", 3),
    pattern("911", 3),
];

/// The conventions a subscriber dialled under.
///
/// The country code, the international and trunk prefixes, whether the
/// numbering plan is open (local numbers take the subscriber's area code) or
/// closed (every national number is dialled in full), and the short codes.
/// [`germany`](Self::germany), [`italy`](Self::italy) and [`nanp`](Self::nanp)
/// are ready-made; [`new`](Self::new) and the `with_…` methods build any
/// other, and refuse an inconsistent one (audit D1–D3, D9).
///
/// ```rust
/// use mediation::DialContext;
///
/// let rome = DialContext::italy();
/// assert_eq!(rome.normalise("06 1234567")?.to_string(), "+39061234567");
///
/// let new_york = DialContext::nanp();
/// assert_eq!(new_york.normalise("1 212 555 1234")?.to_string(), "+12125551234");
/// assert_eq!(new_york.normalise("911")?.to_string(), "911");
/// # Ok::<(), mediation::DialError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DialContext {
    country_code: u16,
    international_prefix: &'static str,
    trunk_prefix: Option<&'static str>,
    closed: bool,
    national_len: Option<u8>,
    short_codes: &'static [ShortCodePattern],
    area_code: Option<AreaCode>,
}

impl DialContext {
    /// The longest dialled string accepted, in bytes.
    ///
    /// Checked before anything else, so the work done on hostile input is bounded
    /// by a constant. Real dial strings, separators included, are far shorter.
    pub const MAX_DIALLED: usize = 64;

    /// Germany: country code 49, international prefix `00`, trunk prefix `0`,
    /// open numbering plan.
    ///
    /// Short codes are 110, 112, 115 (3 digits), 116xyz, 118xy and
    /// network-internal 11x with x = 1, 3, 4, 7, 9 (BNetzA Nummernplan, Vfg
    /// 29/2015 as amended by Vfg 92/2025). Any other string is a local number:
    /// subscriber numbers such as `19222` start with `1` (audit D5).
    ///
    /// Carrier selection (`010xy`, `0100yy`) ended on 31.12.2024 and the (0)10
    /// range is reserve (Vfg 87/2025); it is not stripped, and a `010…` string is
    /// an ordinary national number the plan leaves unclassified.
    #[must_use]
    pub const fn germany() -> Self {
        Self {
            country_code: 49,
            international_prefix: "00",
            trunk_prefix: Some("0"),
            closed: false,
            national_len: None,
            short_codes: GERMAN_SHORT_CODES,
            area_code: None,
        }
    }

    /// Italy: country code 39, international prefix `00`, no trunk prefix,
    /// closed numbering plan — the leading `0` of `06 …` is part of the number.
    ///
    /// Short codes are 112, 113, 115, 118 and 116xyz.
    #[must_use]
    pub const fn italy() -> Self {
        Self {
            country_code: 39,
            international_prefix: "00",
            trunk_prefix: None,
            closed: true,
            national_len: None,
            short_codes: ITALIAN_SHORT_CODES,
            area_code: None,
        }
    }

    /// The North American Numbering Plan: country code 1, international prefix
    /// `011`, trunk prefix `1`, closed plan of 10-digit national numbers, and
    /// the N11 short codes 211 … 911.
    #[must_use]
    pub const fn nanp() -> Self {
        Self {
            country_code: 1,
            international_prefix: "011",
            trunk_prefix: Some("1"),
            closed: true,
            national_len: Some(10),
            short_codes: NANP_SHORT_CODES,
            area_code: None,
        }
    }

    /// An open numbering plan with no trunk prefix and no short codes.
    ///
    /// # Errors
    ///
    /// [`DialContextError`] when `country_code` is not 1 to 999 or
    /// `international_prefix` is not one or more ASCII digits.
    pub fn new(
        country_code: u16,
        international_prefix: &'static str,
    ) -> Result<Self, DialContextError> {
        if !(1..=999).contains(&country_code) {
            return Err(DialContextError::CountryCode);
        }
        if !is_digits(international_prefix) {
            return Err(DialContextError::InternationalPrefix);
        }
        Ok(Self {
            country_code,
            international_prefix,
            trunk_prefix: None,
            closed: false,
            national_len: None,
            short_codes: &[],
            area_code: None,
        })
    }

    /// The same context with a trunk prefix.
    ///
    /// # Errors
    ///
    /// [`DialContextError`] when `trunk` is not one or more ASCII digits,
    /// starts with the international prefix, or starts like a short code.
    pub fn with_trunk_prefix(mut self, trunk: &'static str) -> Result<Self, DialContextError> {
        if !is_digits(trunk) {
            return Err(DialContextError::TrunkPrefix);
        }
        self.trunk_prefix = Some(trunk);
        self.checked()
    }

    /// The same context as a closed numbering plan: every national number is
    /// dialled in full, the trunk prefix (if any) is stripped, and there is no
    /// area code — one set before is removed.
    #[must_use]
    pub const fn closed(mut self) -> Self {
        self.closed = true;
        self.area_code = None;
        self
    }

    /// The same context with national numbers of exactly `digits` digits.
    ///
    /// # Errors
    ///
    /// [`DialContextError::NationalLength`] when the country code and `digits`
    /// together are not 2 to 15 digits (ITU-T E.164).
    pub fn with_national_length(mut self, digits: u8) -> Result<Self, DialContextError> {
        let total = usize::from(digits) + CountryCodeDigits::of(self.country_code).as_slice().len();
        if digits == 0 || total > Msisdn::MAX_DIGITS {
            return Err(DialContextError::NationalLength);
        }
        self.national_len = Some(digits);
        Ok(self)
    }

    /// The same context with these short codes, and no others.
    ///
    /// A string matching a pattern is a short code. A string that starts like
    /// one but has another length is refused ([`DialError::InvalidShortCode`]);
    /// any other string is a national or local number.
    ///
    /// # Errors
    ///
    /// [`DialContextError`] when a pattern is not 1 to 6 prefix digits for a
    /// code of 2 to 6 digits, or starts like the international or trunk prefix.
    pub fn with_short_codes(
        mut self,
        patterns: &'static [ShortCodePattern],
    ) -> Result<Self, DialContextError> {
        if let Some(index) = patterns.iter().position(|p| !p.is_valid()) {
            return Err(DialContextError::ShortCodePattern { index });
        }
        self.short_codes = patterns;
        self.checked()
    }

    /// The same context for a subscriber in `area` — `30` or `030` for Berlin.
    ///
    /// # Errors
    ///
    /// [`DialContextError::AreaCode`] when `area` is not 1 to 5 digits,
    /// optionally after one leading `0`; [`DialContextError::ClosedPlan`] when
    /// the plan is closed.
    pub fn with_area_code(mut self, area: &str) -> Result<Self, DialContextError> {
        if self.closed {
            return Err(DialContextError::ClosedPlan);
        }
        self.area_code = Some(area.parse().map_err(|_| DialContextError::AreaCode)?);
        Ok(self)
    }

    fn checked(self) -> Result<Self, DialContextError> {
        let overlap = |a: &str, b: &str| a.bytes().zip(b.bytes()).all(|(x, y)| x == y);
        let prefixes = [Some(self.international_prefix), self.trunk_prefix];
        // The international prefix is tried first, so a trunk prefix that
        // starts with it is never reached. `0` before `00` is fine.
        if let [Some(intl), Some(trunk)] = prefixes
            && trunk.starts_with(intl)
        {
            return Err(DialContextError::PrefixesOverlap);
        }
        for prefix in prefixes.into_iter().flatten() {
            if self.short_codes.iter().any(|p| overlap(p.prefix, prefix)) {
                return Err(DialContextError::PrefixesOverlap);
            }
        }
        Ok(self)
    }

    /// The subscriber's area code, if the context has one.
    #[must_use]
    pub const fn area_code(&self) -> Option<AreaCode> {
        self.area_code
    }

    /// The country code.
    #[must_use]
    pub const fn country_code(&self) -> u16 {
        self.country_code
    }

    /// `true` for a closed numbering plan.
    #[must_use]
    pub const fn is_closed(&self) -> bool {
        self.closed
    }

    /// Turns a dialled string into a [`Destination`].
    ///
    /// Accepts ASCII digits, one leading `+`, and the separators space, `-`, `/`,
    /// `(`, `)` and `.`. A bracketed `(0)` is the optional trunk prefix: dropped
    /// directly after the context's own country code (`+49 (0)30 …`), read as
    /// the trunk prefix at the start of a national string (`(0)30 …`), and
    /// refused anywhere else (audit D4). Then, in order:
    ///
    /// 1. `+…` — an international number.
    /// 2. the international prefix — an international number.
    /// 3. the trunk prefix — a national number in this country.
    /// 4. a short code of the context.
    /// 5. in a closed plan, a national number; in an open plan, a local number
    ///    in the context's area code.
    ///
    /// # Errors
    ///
    /// A [`DialError`] naming the reason; see its variants. None of them carries
    /// the dialled digits.
    pub fn normalise(&self, dialled: &str) -> Result<Destination, DialError> {
        if dialled.len() > Self::MAX_DIALLED {
            return Err(DialError::TooLong);
        }
        let zero_trunk = self.trunk_prefix == Some("0");
        let mut digits = [0u8; Self::MAX_DIALLED];
        let mut count = 0;
        let mut plus = false;
        let bytes = dialled.as_bytes();
        let mut position = 0;
        while position < bytes.len() {
            match bytes[position] {
                b @ b'0'..=b'9' => {
                    digits[count] = b - b'0';
                    count += 1;
                }
                b'(' if bytes[position..].starts_with(b"(0)") => {
                    if zero_trunk && self.after_own_country_code(plus, &digits[..count]) {
                        position += 3;
                        continue;
                    }
                    // `(0)30 …`: in national form the bracket holds the trunk
                    // prefix itself, read as a digit.
                    if !(zero_trunk && count == 0 && !plus) {
                        return Err(DialError::MisplacedTrunkDigit { position });
                    }
                }
                b' ' | b'-' | b'/' | b'(' | b')' | b'.' => {}
                b'+' if count == 0 && !plus => plus = true,
                b'+' => return Err(DialError::MisplacedPlus { position }),
                _ => return Err(DialError::InvalidCharacter { position }),
            }
            position += 1;
        }
        let digits = &digits[..count];
        if digits.is_empty() {
            return Err(DialError::Empty);
        }

        if plus {
            return international(digits);
        }
        if let Some(rest) = strip(digits, self.international_prefix) {
            return international(rest);
        }
        if let Some(rest) = self.trunk_prefix.and_then(|t| strip(digits, t)) {
            return self.national(rest);
        }
        if self.short_codes.iter().any(|p| p.matches(digits)) {
            let code = collect(digits, ShortCode::MAX_DIGITS)
                .and_then(ShortCode::from_digits)
                .ok_or(DialError::InvalidShortCode)?;
            return Ok(Destination::Short(code));
        }
        if self.short_codes.iter().any(|p| p.overlaps(digits)) {
            return Err(DialError::InvalidShortCode);
        }
        if self.closed {
            return self.national(digits);
        }
        match self.area_code {
            Some(area) => {
                let mut national = area.0;
                for &d in digits {
                    national = national.checked_push(d).ok_or(DialError::NumberTooLong)?;
                }
                self.national_digits(national)
            }
            None => Err(DialError::AreaCodeRequired),
        }
    }

    /// `true` when the digits so far are exactly the context's own country
    /// code in international form.
    fn after_own_country_code(&self, plus: bool, digits: &[u8]) -> bool {
        let rest = if plus {
            Some(digits)
        } else {
            strip(digits, self.international_prefix)
        };
        rest == Some(CountryCodeDigits::of(self.country_code).as_slice())
    }

    fn national(&self, rest: &[u8]) -> Result<Destination, DialError> {
        if rest.is_empty() {
            return Err(DialError::MissingNumber);
        }
        let national = collect(rest, Msisdn::MAX_DIGITS).ok_or(DialError::NumberTooLong)?;
        self.national_digits(national)
    }

    fn national_digits(&self, national: DigitString) -> Result<Destination, DialError> {
        if self
            .national_len
            .is_some_and(|len| usize::from(len) != national.len())
        {
            return Err(DialError::NationalNumberLength);
        }
        let mut out = DigitString::EMPTY;
        for &d in CountryCodeDigits::of(self.country_code).as_slice() {
            out = out.checked_push(d).ok_or(DialError::NumberTooLong)?;
        }
        for i in 0..national.len() {
            out = out
                .checked_push(national.at(i))
                .ok_or(DialError::NumberTooLong)?;
        }
        number(out)
    }
}

/// The digits of a country code (1 to 999), without leading zeros.
struct CountryCodeDigits([u8; 3], usize);

impl CountryCodeDigits {
    fn of(cc: u16) -> Self {
        let all = [(cc / 100 % 10) as u8, (cc / 10 % 10) as u8, (cc % 10) as u8];
        let start = all.iter().position(|&d| d != 0).unwrap_or(2);
        Self(all, start)
    }

    fn as_slice(&self) -> &[u8] {
        &self.0[self.1..]
    }
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn strip<'a>(digits: &'a [u8], prefix: &str) -> Option<&'a [u8]> {
    if prefix.is_empty() || prefix.len() > digits.len() {
        return None;
    }
    let matches = prefix
        .bytes()
        .zip(digits)
        .all(|(p, &d)| p.is_ascii_digit() && p - b'0' == d);
    matches.then(|| &digits[prefix.len()..])
}

fn collect(digits: &[u8], max: usize) -> Option<DigitString> {
    if digits.len() > max {
        return None;
    }
    digits
        .iter()
        .try_fold(DigitString::EMPTY, |acc, &d| acc.checked_push(d))
}

fn international(digits: &[u8]) -> Result<Destination, DialError> {
    match digits.first() {
        None => Err(DialError::MissingNumber),
        // ITU-T E.164: no country code begins with 0.
        Some(0) => Err(DialError::InvalidCountryCode),
        Some(_) => number(collect(digits, Msisdn::MAX_DIGITS).ok_or(DialError::NumberTooLong)?),
    }
}

fn number(digits: DigitString) -> Result<Destination, DialError> {
    if digits.len() > Msisdn::MAX_DIGITS {
        return Err(DialError::NumberTooLong);
    }
    Ok(Destination::Number(Msisdn::from_digits(digits)))
}

/// Why a [`DialContext`] could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum DialContextError {
    /// The country code is not 1 to 999 (ITU-T E.164).
    #[error("a country code is 1 to 999 (ITU-T E.164)")]
    CountryCode,
    /// The international prefix is not one or more ASCII digits.
    #[error("the international prefix must be one or more digits")]
    InternationalPrefix,
    /// The trunk prefix is not one or more ASCII digits.
    #[error("the trunk prefix must be one or more digits")]
    TrunkPrefix,
    /// The trunk prefix starts with the international prefix, or a short code
    /// starts like either prefix, so one of them is never reached.
    #[error(
        "the trunk prefix starts with the international prefix, or a short code starts like a prefix"
    )]
    PrefixesOverlap,
    /// A short-code pattern is not 1 to 6 prefix digits for a code of 2 to 6
    /// digits.
    #[error("short-code pattern {index} is not a digit prefix for a code of 2 to 6 digits")]
    ShortCodePattern {
        /// Index of the pattern in the slice.
        index: usize,
    },
    /// The national length and the country code are not 2 to 15 digits.
    #[error("the country code and the national number exceed 15 digits (ITU-T E.164)")]
    NationalLength,
    /// The area code is not 1 to 5 digits, optionally after one leading `0`.
    #[error("an area code is 1 to 5 digits, such as 30 or 030")]
    AreaCode,
    /// A closed numbering plan has no area codes.
    #[error("a closed numbering plan has no area code")]
    ClosedPlan,
}

/// Why a dialled string could not be normalised.
///
/// Carries a reason and, where useful, a byte position — never the dialled
/// digits, so it can be logged without putting traffic data in the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum DialError {
    /// No digits at all.
    #[error("the dialled string holds no digits")]
    Empty,
    /// Longer than [`DialContext::MAX_DIALLED`] bytes.
    #[error("the dialled string is longer than {} bytes", DialContext::MAX_DIALLED)]
    TooLong,
    /// A character that is neither a digit, a leading `+` nor a separator.
    #[error("invalid character at byte {position} of the dialled string")]
    InvalidCharacter {
        /// Byte offset of the character.
        position: usize,
    },
    /// A `+` that is not the first non-separator character, or a second one.
    #[error("'+' at byte {position} is not at the start of the dialled string")]
    MisplacedPlus {
        /// Byte offset of the `+`.
        position: usize,
    },
    /// A `(0)` that is neither directly after the context's own country code
    /// nor the trunk prefix at the start of a national string.
    #[error("'(0)' at byte {position} is not directly after the country code")]
    MisplacedTrunkDigit {
        /// Byte offset of the `(`.
        position: usize,
    },
    /// A prefix with nothing after it.
    #[error("the dialled string ends after its prefix")]
    MissingNumber,
    /// An international number whose country code starts with `0`.
    #[error("no country code starts with 0 (ITU-T E.164)")]
    InvalidCountryCode,
    /// More than 15 digits once normalised (ITU-T E.164).
    #[error("the normalised number is longer than 15 digits (ITU-T E.164)")]
    NumberTooLong,
    /// A national number of another length than the context's plan fixes.
    #[error("the national number does not have the length the numbering plan fixes")]
    NationalNumberLength,
    /// A local number, and the context has no area code to put in front of it.
    #[error("a local number needs the subscriber's area code, and the dialling context has none")]
    AreaCodeRequired,
    /// Starts like one of the context's short codes but has another length.
    #[error("starts like a short code but has another length")]
    InvalidShortCode,
}

// ── Destination class ────────────────────────────────────────────────────────

/// What kind of service a destination is.
///
/// The vocabulary is common to European numbering plans; which ranges fall in
/// which class is a [`NumberingPlan`]'s business. A price book maps class to
/// usage kind, so it holds no prefixes of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DestinationClass {
    /// A geographic number — a subscriber line in an area.
    Geographic,
    /// A mobile number.
    Mobile,
    /// A paging number.
    Paging,
    /// Free to the caller (DE: 0800).
    Freephone,
    /// Shared-cost service number (DE: 0180).
    SharedCost,
    /// Premium-rate service (DE: 0900).
    Premium,
    /// Mass-traffic / televoting (DE: 0137).
    MassTraffic,
    /// Personal number, routed to wherever its holder chooses (DE: 0700).
    Personal,
    /// National subscriber number without a geographic area (DE: 032).
    NationalSubscriber,
    /// Virtual private network (DE: 018x, 0181 international VPN).
    Vpn,
    /// Online-service dial-in (DE: 0191–0194).
    OnlineService,
    /// Directory enquiries (DE: 118xy).
    Directory,
    /// Harmonised service of social value (EU: 116xyz).
    Harmonised,
    /// Emergency call (DE: 110, 112).
    Emergency,
    /// Public authority number (DE: 115).
    Authority,
    /// A code that routes inside the network and is not a subscriber destination.
    NetworkInternal,
    /// A number in another country.
    International,
}

impl DestinationClass {
    /// Every variant, for exhaustive iteration in tests and mappings.
    pub const ALL: &'static [Self] = &[
        Self::Geographic,
        Self::Mobile,
        Self::Paging,
        Self::Freephone,
        Self::SharedCost,
        Self::Premium,
        Self::MassTraffic,
        Self::Personal,
        Self::NationalSubscriber,
        Self::Vpn,
        Self::OnlineService,
        Self::Directory,
        Self::Harmonised,
        Self::Emergency,
        Self::Authority,
        Self::NetworkInternal,
        Self::International,
    ];

    /// The canonical codes, in the order of [`ALL`](Self::ALL).
    pub const CODES: &'static [&'static str] = &[
        "GEOGRAPHIC",
        "MOBILE",
        "PAGING",
        "FREEPHONE",
        "SHARED_COST",
        "PREMIUM",
        "MASS_TRAFFIC",
        "PERSONAL",
        "NATIONAL_SUBSCRIBER",
        "VPN",
        "ONLINE_SERVICE",
        "DIRECTORY",
        "HARMONISED",
        "EMERGENCY",
        "AUTHORITY",
        "NETWORK_INTERNAL",
        "INTERNATIONAL",
    ];

    /// The canonical string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Geographic => "GEOGRAPHIC",
            Self::Mobile => "MOBILE",
            Self::Paging => "PAGING",
            Self::Freephone => "FREEPHONE",
            Self::SharedCost => "SHARED_COST",
            Self::Premium => "PREMIUM",
            Self::MassTraffic => "MASS_TRAFFIC",
            Self::Personal => "PERSONAL",
            Self::NationalSubscriber => "NATIONAL_SUBSCRIBER",
            Self::Vpn => "VPN",
            Self::OnlineService => "ONLINE_SERVICE",
            Self::Directory => "DIRECTORY",
            Self::Harmonised => "HARMONISED",
            Self::Emergency => "EMERGENCY",
            Self::Authority => "AUTHORITY",
            Self::NetworkInternal => "NETWORK_INTERNAL",
            Self::International => "INTERNATIONAL",
        }
    }
}

impl fmt::Display for DestinationClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DestinationClass {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .find(|c| c.as_str().eq_ignore_ascii_case(s.trim()))
            .copied()
            .ok_or_else(|| ParseError::one_of("DestinationClass", s, Self::CODES))
    }
}

/// A mapping from destination to [`DestinationClass`].
///
/// `None` means *not classified*. A plan must never fall back to a default
/// class: an unlisted range rated as geographic is how a premium number gets
/// billed at the local rate.
///
/// Implemented for [`GermanNumberingPlan`], for a [`NumberRangeSet`] of classes
/// (numbers only — load the regulator's published allocations into one), and for
/// any `Fn(&Destination) -> Option<DestinationClass>`.
pub trait NumberingPlan {
    /// The class of `destination`, or `None` when the plan does not know it.
    fn classify(&self, destination: &Destination) -> Option<DestinationClass>;
}

impl<F> NumberingPlan for F
where
    F: Fn(&Destination) -> Option<DestinationClass>,
{
    fn classify(&self, destination: &Destination) -> Option<DestinationClass> {
        self(destination)
    }
}

/// Classifies by the longest matching range. Duplicate ranges that disagree
/// classify nothing, whatever their insertion order (audit N1).
impl NumberingPlan for NumberRangeSet<DestinationClass> {
    fn classify(&self, destination: &Destination) -> Option<DestinationClass> {
        let number = destination.as_number()?;
        let (_, classes) = self.longest_match_all(number)?;
        let (first, rest) = classes.split_first()?;
        rest.iter().all(|c| c == first).then_some(*first)
    }
}

/// The German numbering plan, as an **example** table.
///
/// Built from the Federal Network Agency's *Nummernplan Nummernraum für die
/// öffentliche Telekommunikation* (Vfg 29/2015, last amended by Vfg 92/2025, in
/// force since [`AS_OF`](Self::AS_OF)) and the per-range plans it refers to. It
/// knows **classes of ranges**, not allocations: a number in an allocated-looking
/// range that nobody holds is still classified. Reserve and withdrawn ranges —
/// (0)10, (0)12, (0)138, (0)161, (0)165–(0)167, (0)1800, (0)1808–(0)1809, (0)31,
/// (0)500–(0)501, (0)701, (0)801, (0)9009, (0)901–(0)905 — are not classified,
/// nor are +49 110, 112 and 118xy, and +49 115 and 116xyz are classified only
/// at their exact length.
///
/// Numbering plans change. For production rating, implement [`NumberingPlan`]
/// over the agency's current data, and treat this table as a starting point
/// whose date you check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct GermanNumberingPlan;

impl GermanNumberingPlan {
    /// The source of the table.
    pub const SOURCE: &'static str = "Bundesnetzagentur, Nummernplan Nummernraum für die \
        öffentliche Telekommunikation (Vfg 29/2015 as amended by Vfg 92/2025), \
        with the range plans for mobile, 0180, 0900, 0137, 032, 0700, 116 and 118";

    /// The date the amendment the table reflects came into force.
    pub const AS_OF: &'static str = "2025-11-06";
}

/// National-number prefixes after `+49`, longest match wins. `None` marks a
/// reserve or withdrawn range inside an otherwise classified block.
const GERMAN_RANGES: &[(&str, Option<DestinationClass>)] = {
    use DestinationClass::{
        Freephone, MassTraffic, Mobile, NationalSubscriber, NetworkInternal, OnlineService, Paging,
        Personal, Premium, SharedCost, Vpn,
    };
    &[
        ("10", None),
        // (0)115 and (0)116xyz: exact lengths, checked in `classify`.
        // (0)110, (0)112 and (0)118xy are not reachable as national numbers.
        ("11", None),
        ("12", None),
        ("13", None),
        ("137", Some(MassTraffic)),
        ("1370", None),
        ("14", None),
        ("15", Some(Mobile)),
        ("16", None),
        ("160", Some(Mobile)),
        ("162", Some(Mobile)),
        ("163", Some(Mobile)),
        ("164", Some(Paging)),
        ("168", Some(Paging)),
        ("169", Some(Paging)),
        ("17", Some(Mobile)),
        ("18", Some(Vpn)),
        ("180", None),
        ("1801", Some(SharedCost)),
        ("1802", Some(SharedCost)),
        ("1803", Some(SharedCost)),
        ("1804", Some(SharedCost)),
        ("1805", Some(SharedCost)),
        ("1806", Some(SharedCost)),
        ("1807", Some(SharedCost)),
        ("19", None),
        ("191", Some(OnlineService)),
        ("192", Some(OnlineService)),
        ("193", Some(OnlineService)),
        ("194", Some(OnlineService)),
        ("199", Some(NetworkInternal)),
        ("31", None),
        ("32", Some(NationalSubscriber)),
        ("500", None),
        ("501", None),
        ("700", Some(Personal)),
        ("701", None),
        ("800", Some(Freephone)),
        ("801", None),
        ("900", Some(Premium)),
        ("9009", None),
        ("901", None),
        ("902", None),
        ("903", None),
        ("904", None),
        ("905", None),
    ]
};

fn digits_start_with(digits: DigitString, offset: usize, prefix: &str) -> bool {
    prefix.len() + offset <= digits.len()
        && prefix
            .bytes()
            .enumerate()
            .all(|(i, p)| digits.at(offset + i) == p - b'0')
}

impl NumberingPlan for GermanNumberingPlan {
    fn classify(&self, destination: &Destination) -> Option<DestinationClass> {
        match destination {
            Destination::Short(code) => german_short_code(code.0),
            Destination::Number(number) => {
                let digits = number.digits();
                if !digits_start_with(digits, 0, "49") {
                    return Some(DestinationClass::International);
                }
                // +49 115 and +49 116xyz only at their exact length (audit D6).
                match digits.len().saturating_sub(2) {
                    3 if digits_start_with(digits, 2, "115") => {
                        return Some(DestinationClass::Authority);
                    }
                    6 if digits_start_with(digits, 2, "116") => {
                        return Some(DestinationClass::Harmonised);
                    }
                    _ => {}
                }
                let longest = GERMAN_RANGES
                    .iter()
                    .filter(|(prefix, _)| digits_start_with(digits, 2, prefix))
                    .max_by_key(|(prefix, _)| prefix.len());
                match longest {
                    Some((_, class)) => *class,
                    // Outside every listed block: area codes are (0)2…(0)9.
                    None if (2..=9).contains(&digits.at(2)) && digits.len() > 2 => {
                        Some(DestinationClass::Geographic)
                    }
                    None => None,
                }
            }
        }
    }
}

fn german_short_code(code: DigitString) -> Option<DestinationClass> {
    let starts = |p: &str| digits_start_with(code, 0, p);
    match code.len() {
        3 if starts("110") || starts("112") => Some(DestinationClass::Emergency),
        3 if starts("115") => Some(DestinationClass::Authority),
        3 if ["111", "113", "114", "117", "119"]
            .iter()
            .any(|p| starts(p)) =>
        {
            Some(DestinationClass::NetworkInternal)
        }
        6 if starts("116") => Some(DestinationClass::Harmonised),
        5 if starts("118") && !starts("1180") => Some(DestinationClass::Directory),
        _ => None,
    }
}

// ── serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::{AreaCode, Destination, DestinationClass, ShortCode};
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

    string_serde!(
        Destination,
        "a destination such as \"+49301234567\" or \"110\""
    );
    string_serde!(ShortCode, "a short code such as \"116117\"");
    string_serde!(AreaCode, "an area code such as \"30\"");
    string_serde!(
        DestinationClass,
        "a destination class code such as \"MOBILE\""
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn de(area: &str) -> DialContext {
        DialContext::germany().with_area_code(area).unwrap()
    }

    #[test]
    fn plus_double_zero_and_trunk_agree() {
        let c = de("30");
        let a = c.normalise("+49 151 1234567").unwrap();
        assert_eq!(c.normalise("0049-151-1234567").unwrap(), a);
        assert_eq!(c.normalise("(0151) 1234567").unwrap(), a);
    }

    #[test]
    fn plus_after_digit_is_refused() {
        assert_eq!(
            de("30").normalise("030+1234567"),
            Err(DialError::MisplacedPlus { position: 3 })
        );
    }

    #[test]
    fn country_code_without_leading_zeros() {
        let us = DialContext::new(1, "011")
            .unwrap()
            .with_area_code("212")
            .unwrap();
        assert_eq!(us.normalise("5551234").unwrap().to_string(), "+12125551234");
    }

    #[test]
    fn unknown_ranges_stay_unclassified() {
        for n in ["+49101234567", "+49900912345", "+491381234567", "+49121234"] {
            let d: Destination = n.parse().unwrap();
            assert_eq!(GermanNumberingPlan.classify(&d), None, "{n}");
        }
    }
}
