//! Subscriber, subscription and equipment identifiers.
//!
//! | Type | Standard | Digits | Check digit |
//! |---|---|---|---|
//! | [`Imsi`] | ITU-T E.212 | 6–15 | — |
//! | [`Msisdn`] | ITU-T E.164 | 1–15 | — |
//! | [`Imei`] | 3GPP TS 23.003 | 15 | **Luhn** |
//! | [`Imeisv`] | 3GPP TS 23.003 | 16 | **none** |
//! | [`Iccid`] | ITU-T E.118 | 19–20 | Luhn, when present |
//! | [`Supi`] | 3GPP TS 23.003 | — | inherits |
//! | [`Gpsi`] | 3GPP TS 23.003 | — | inherits |
//! | [`Pei`] | 3GPP TS 23.003 | — | inherits |
//! | [`Suci`] | 3GPP TS 23.003 | — | — |
//!
//! # No heap
//!
//! Every numeric identifier stores its digits packed two per byte, inline. A
//! mediation layer holds millions of these in flight at once, and a heap
//! allocation per subscriber identity is the difference between fitting in
//! cache and not. All of them are [`Copy`].
//!
//! # IMEI is Luhn-checked and IMEISV is not
//!
//! The fifteenth position that carries the Luhn check digit in an IMEI is the
//! first software-version digit in an IMEISV. Running the check over an IMEISV
//! rejects valid equipment; not running it over an IMEI accepts corrupt records.
//! They are separate types so the compiler enforces the difference:
//!
//! ```rust
//! use mediation::{Imei, Imeisv};
//!
//! // A transposition breaks the checksum, which is the point of having one.
//! assert!("490154203237518".parse::<Imei>().is_ok());
//! assert!("490154203237581".parse::<Imei>().is_err());
//!
//! // The same equipment with a software version — no checksum to break.
//! let sv: Imeisv = "4901542032375120".parse().unwrap();
//! assert_eq!(sv.software_version().to_string(), "20");
//! ```
//!
//! # An IMSI cannot be split without a table
//!
//! [`Imsi::mcc`] is infallible; everything past it needs an
//! [`crate::plmn::MncWidths`] resolver, because the MNC is two digits
//! or three and the digit string does not say which. See [`crate::plmn`].
//!
//! # A SUCI is not an identity you can count
//!
//! [`Suci`] deliberately implements neither [`Hash`] nor [`Ord`], so it cannot
//! reach a deduplication key or a subscriber aggregate. It is designed to be
//! non-linkable and differs between registrations of the same subscriber;
//! counting it would split one subscriber into many. A record that only ever
//! carried a SUCI is unattributable, and must be dispositioned as such rather
//! than folded into a total.

use std::fmt;
use std::str::FromStr;

use crate::digits::DigitString;
use crate::error::{DecodeError, ParseError};
use crate::plmn::{Mcc, MncWidths, PlmnId};

/// Parses `s` into a run of `min..=max` digits, or produces a [`ParseError`].
fn digits_in_range(
    type_name: &'static str,
    shape: &'static str,
    s: &str,
    min: usize,
    max: usize,
) -> Result<DigitString, ParseError> {
    let err = || ParseError::format(type_name, s, shape);
    let d = DigitString::parse(s, max).ok_or_else(err)?;
    if d.len() < min { Err(err()) } else { Ok(d) }
}

// ── Imsi ─────────────────────────────────────────────────────────────────────

/// An ITU-T E.212 International Mobile Subscriber Identity.
///
/// Up to 15 digits: a three-digit [`Mcc`], a two- or three-digit MNC, and the
/// remaining MSIN. Only the MCC can be read without an assignment table — see
/// [`plmn`](crate::plmn).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Imsi {
    /// Resolved once, at parse time, so [`mcc`](Self::mcc) has no failure path
    /// and no fallback. A country code recovered on every call would need an
    /// unreachable-error arm, and an unreachable arm that returns a value is
    /// how a wrong country reaches a record.
    mcc: Mcc,
    digits: DigitString,
}

/// The shape [`Imsi`] accepts, as rendered in a [`ParseError`].
const IMSI_FORMAT: &str = "6 to 15 decimal digits beginning with a three-digit MCC";

impl Imsi {
    /// The country code, which is always the first three digits.
    ///
    /// Infallible by construction: an [`Imsi`] cannot exist without a valid MCC.
    #[must_use]
    pub const fn mcc(self) -> Mcc {
        self.mcc
    }

    /// The network identity, given a resolver for the MNC's digit width.
    ///
    /// `None` when the resolver does not know the country, or when the IMSI is
    /// too short to hold an MSIN after the MNC. Both are unattributable records
    /// rather than defaults — guessing a width here misfiles the subscriber
    /// under a network that did not serve them.
    ///
    /// ```rust
    /// use mediation::{Imsi, NorthAmericanRule};
    ///
    /// let imsi: Imsi = "262021234567890".parse().unwrap();
    /// let plmn = imsi.plmn(&NorthAmericanRule).unwrap();
    /// assert_eq!(plmn.to_string(), "262-02");
    ///
    /// // The same digits split three-wide under a North American MCC.
    /// let us: Imsi = "310260123456789".parse().unwrap();
    /// assert_eq!(us.plmn(&NorthAmericanRule).unwrap().to_string(), "310-260");
    /// ```
    #[must_use]
    pub fn plmn(self, widths: &impl MncWidths) -> Option<PlmnId> {
        let width = widths.width_for(self.mcc)?.digits() as usize;
        if self.digits.len() <= 3 + width {
            return None;
        }
        let mnc = self.digits.slice(3, width);
        let mnc =
            crate::plmn::Mnc::new(u16::try_from(mnc.to_u64()).ok()?, u8::try_from(width).ok()?)?;
        Some(PlmnId::new(self.mcc, mnc))
    }

    /// The subscriber part, given a resolver for the MNC's digit width.
    #[must_use]
    pub fn msin(self, widths: &impl MncWidths) -> Option<Msin> {
        let width = widths.width_for(self.mcc)?.digits() as usize;
        let start = 3 + width;
        if self.digits.len() <= start {
            return None;
        }
        Some(Msin(self.digits.slice(start, self.digits.len() - start)))
    }

    /// How many digits the identity holds.
    #[must_use]
    pub fn len(self) -> usize {
        self.digits.len()
    }

    /// Always `false` — an [`Imsi`] cannot be constructed empty.
    ///
    /// Present because [`len`](Self::len) is, and a length accessor without one
    /// reads as an oversight.
    #[must_use]
    pub fn is_empty(self) -> bool {
        false
    }
}

impl Imsi {
    /// Decode from the Telephony BCD an `IMSI` field carries on the wire.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] when the octets are not well-formed TBCD, or when the
    /// digits they hold are not a valid IMSI.
    pub fn from_tbcd(bytes: &[u8]) -> Result<Self, DecodeError> {
        let digits = DigitString::from_tbcd(bytes)?;
        Self::from_digits(digits)
            .ok_or_else(|| DecodeError::new(0, "6 to 15 digits beginning with an assigned MCC"))
    }

    /// Encode to Telephony BCD, writing into `out` and returning the octet
    /// count.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] when `out` is shorter than eight octets.
    pub fn to_tbcd(self, out: &mut [u8]) -> Result<usize, DecodeError> {
        self.digits.to_tbcd(out)
    }

    /// The identity's digits, for callers that need the raw run.
    #[must_use]
    pub const fn digits(self) -> DigitString {
        self.digits
    }

    /// Shared by the string and wire constructors, so both apply the same rules.
    fn from_digits(digits: DigitString) -> Option<Self> {
        if digits.len() < 6 || digits.len() > 15 {
            return None;
        }
        let mcc = Mcc::new(u16::try_from(digits.slice(0, 3).to_u64()).ok()?)?;
        Some(Self { mcc, digits })
    }
}

impl fmt::Display for Imsi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.digits.fmt(f)
    }
}

impl FromStr for Imsi {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseError::format("Imsi", s, IMSI_FORMAT);
        // The MCC must be assigned; 000 is the shape an unparsed field takes.
        // Resolving it once at construction is what lets `mcc()` be a field
        // read with no failure path at all.
        Self::from_digits(digits_in_range("Imsi", IMSI_FORMAT, s, 6, 15)?).ok_or_else(err)
    }
}

/// The subscriber part of an [`Imsi`], after the MCC and MNC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Msin(DigitString);

impl fmt::Display for Msin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

// ── Msisdn ───────────────────────────────────────────────────────────────────

/// An ITU-T E.164 number, stored in international form.
///
/// No `+`, no separators, no national prefix — one number, one spelling. The
/// leading `+` is accepted on the way in and dropped, because a value that is
/// sometimes written with it and sometimes without produces two database keys
/// for one subscriber.
///
/// # This does not identify the terminating operator
///
/// A number's range records who *allocated* it, not who *terminates* it today.
/// Number portability broke that inference everywhere it was introduced, and
/// interconnect settlement is per terminating operator. Resolving the real
/// network needs portability data, which this crate does not carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Msisdn(DigitString);

/// The shape [`Msisdn`] accepts, as rendered in a [`ParseError`].
const MSISDN_FORMAT: &str =
    "1 to 15 decimal digits in international form, such as 491701234567 or +491701234567";

impl Msisdn {
    /// How many digits the number holds.
    #[must_use]
    pub fn len(self) -> usize {
        self.0.len()
    }

    /// Always `false` — an [`Msisdn`] cannot be constructed empty.
    #[must_use]
    pub fn is_empty(self) -> bool {
        false
    }

    /// `true` when the number begins with the given country-code digits.
    ///
    /// A prefix test rather than a country accessor: E.164 country codes are
    /// one, two or three digits and cannot be delimited without the assignment
    /// table, exactly as with an MNC.
    ///
    /// Compares digit by digit rather than through a formatted string, so it
    /// allocates nothing — this runs once per record.
    #[must_use]
    pub const fn starts_with_cc(self, cc: u16) -> bool {
        // No assigned country code begins with zero, so the digit count of the
        // number determines the prefix length unambiguously.
        let width = if cc >= 100 {
            3
        } else if cc >= 10 {
            2
        } else {
            1
        };
        if width > self.0.len() {
            return false;
        }
        let mut prefix = 0u16;
        let mut i = 0;
        while i < width {
            prefix = prefix * 10 + self.0.at(i) as u16;
            i += 1;
        }
        prefix == cc
    }

    /// The number written with a leading `+`, for display to a human.
    ///
    /// Not the canonical form: [`Display`](fmt::Display) writes the bare digits,
    /// which is what reaches storage.
    #[must_use]
    pub fn to_international(self) -> String {
        format!("+{}", self.0)
    }
}

impl Msisdn {
    /// Decode from the Telephony BCD an `MSISDN` field carries on the wire.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] when the octets are not well-formed TBCD, or hold more
    /// than the 15 digits E.164 allows.
    pub fn from_tbcd(bytes: &[u8]) -> Result<Self, DecodeError> {
        let digits = DigitString::from_tbcd(bytes)?;
        if digits.is_empty() || digits.len() > 15 {
            return Err(DecodeError::new(0, "1 to 15 digits"));
        }
        Ok(Self(digits))
    }

    /// Encode to Telephony BCD, writing into `out`.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] when `out` is shorter than eight octets.
    pub fn to_tbcd(self, out: &mut [u8]) -> Result<usize, DecodeError> {
        self.0.to_tbcd(out)
    }

    /// The number's digits, for callers that need the raw run.
    #[must_use]
    pub const fn digits(self) -> DigitString {
        self.0
    }
}

impl fmt::Display for Msisdn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for Msisdn {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim();
        let body = t.strip_prefix('+').unwrap_or(t);
        digits_in_range("Msisdn", MSISDN_FORMAT, body, 1, 15).map(Self)
    }
}

// ── Imei / Imeisv ────────────────────────────────────────────────────────────

/// A Type Allocation Code — the first eight digits of an [`Imei`] or
/// [`Imeisv`], identifying the equipment model.
///
/// The right granularity for "which device model", and the right truncation
/// when a full equipment identity is more personal data than the question needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tac(DigitString);

impl fmt::Display for Tac {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A 3GPP TS 23.003 International Mobile Equipment Identity: 15 digits with a
/// trailing Luhn check digit.
///
/// The checksum is verified at construction. An IMEI that fails it is a
/// truncated or transposed field, not a device, and rejecting it at the boundary
/// keeps a corrupt equipment identity out of every downstream aggregate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Imei(DigitString);

/// The shape [`Imei`] accepts, as rendered in a [`ParseError`].
const IMEI_FORMAT: &str = "15 decimal digits whose last digit is a valid Luhn check digit";

impl Imei {
    /// The eight-digit type allocation code.
    #[must_use]
    pub fn tac(self) -> Tac {
        Tac(self.0.slice(0, 8))
    }

    /// The six-digit serial number within the type allocation.
    #[must_use]
    pub fn serial(self) -> Digits6 {
        Digits6(self.0.slice(8, 6))
    }

    /// The trailing Luhn check digit.
    #[must_use]
    pub fn check_digit(self) -> u8 {
        self.0.at(14)
    }
}

impl Imei {
    /// Decode from the Telephony BCD an `IMEI` field carries on the wire.
    ///
    /// The Luhn check digit is verified, exactly as when parsing a string: a
    /// checksum that only some code paths apply is a checksum nobody can rely
    /// on.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] when the octets are not well-formed TBCD, do not hold
    /// 15 digits, or fail the checksum.
    pub fn from_tbcd(bytes: &[u8]) -> Result<Self, DecodeError> {
        let digits = DigitString::from_tbcd(bytes)?;
        if digits.len() != 15 {
            return Err(DecodeError::new(0, "exactly 15 digits"));
        }
        if !digits.luhn_valid() {
            return Err(DecodeError::new(7, "a valid Luhn check digit"));
        }
        Ok(Self(digits))
    }

    /// Encode to Telephony BCD, writing into `out`.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] when `out` is shorter than eight octets.
    pub fn to_tbcd(self, out: &mut [u8]) -> Result<usize, DecodeError> {
        self.0.to_tbcd(out)
    }

    /// The identity's digits, for callers that need the raw run.
    #[must_use]
    pub const fn digits(self) -> DigitString {
        self.0
    }
}

impl fmt::Display for Imei {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for Imei {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let d = digits_in_range("Imei", IMEI_FORMAT, s, 15, 15)?;
        if !d.luhn_valid() {
            return Err(ParseError::format("Imei", s, IMEI_FORMAT));
        }
        Ok(Self(d))
    }
}

/// A six-digit run — the serial part of an equipment identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Digits6(DigitString);

impl fmt::Display for Digits6 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A two-digit software version number, as carried by an [`Imeisv`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SoftwareVersion(DigitString);

impl fmt::Display for SoftwareVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A 3GPP TS 23.003 IMEI with Software Version: 16 digits, **no check digit**.
///
/// The fifteenth and sixteenth digits are the software version. The position
/// that would carry a Luhn check digit in an [`Imei`] is data here, so no
/// checksum is verified — doing so would reject valid equipment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Imeisv(DigitString);

/// The shape [`Imeisv`] accepts, as rendered in a [`ParseError`].
const IMEISV_FORMAT: &str = "16 decimal digits, the last two being the software version";

impl Imeisv {
    /// The eight-digit type allocation code.
    #[must_use]
    pub fn tac(self) -> Tac {
        Tac(self.0.slice(0, 8))
    }

    /// The six-digit serial number within the type allocation.
    #[must_use]
    pub fn serial(self) -> Digits6 {
        Digits6(self.0.slice(8, 6))
    }

    /// The two-digit software version.
    #[must_use]
    pub fn software_version(self) -> SoftwareVersion {
        SoftwareVersion(self.0.slice(14, 2))
    }

    /// The [`Imei`] for the same equipment, with the check digit computed.
    ///
    /// The two identities name one device, and a record set that mixes them
    /// double-counts handsets unless one form is derived from the other.
    ///
    /// ```rust
    /// use mediation::{Imei, Imeisv};
    ///
    /// let sv: Imeisv = "4901542032375120".parse().unwrap();
    /// assert_eq!(sv.to_imei(), "490154203237518".parse::<Imei>().unwrap());
    /// ```
    #[must_use]
    pub fn to_imei(self) -> Imei {
        let payload = self.0.slice(0, 14);
        Imei(
            payload
                .checked_push(payload.luhn_check_digit())
                // Unreachable: the payload is 14 digits and the limit is 20.
                // Panicking rather than returning `payload` is deliberate —
                // the fallback would hand back a 14-digit value wearing the
                // `Imei` type, which every downstream check assumes is 15 and
                // Luhn-valid.
                .expect("a 14-digit payload always admits a check digit"),
        )
    }
}

impl fmt::Display for Imeisv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for Imeisv {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        digits_in_range("Imeisv", IMEISV_FORMAT, s, 16, 16).map(Self)
    }
}

// ── Iccid ────────────────────────────────────────────────────────────────────

/// An ITU-T E.118 Integrated Circuit Card Identifier: the SIM's own number.
///
/// Begins with the major industry identifier `89`. Nineteen or twenty digits —
/// issuers ship both, with and without a trailing Luhn check digit, so the
/// checksum is **reported** by [`luhn_valid`](Self::luhn_valid) rather than
/// enforced at construction. A failing check on a 20-digit card is a finding
/// worth raising; on a 19-digit one it means nothing at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Iccid(DigitString);

/// The shape [`Iccid`] accepts, as rendered in a [`ParseError`].
const ICCID_FORMAT: &str = "19 or 20 decimal digits beginning with 89";

impl Iccid {
    /// How many digits the identifier holds: 19 or 20.
    #[must_use]
    pub fn len(self) -> usize {
        self.0.len()
    }

    /// Always `false` — an [`Iccid`] cannot be constructed empty.
    #[must_use]
    pub fn is_empty(self) -> bool {
        false
    }

    /// `true` when the identifier is long enough to carry a check digit.
    #[must_use]
    pub fn has_check_digit(self) -> bool {
        self.0.len() == 20
    }

    /// Whether the final digit is a consistent Luhn check digit.
    ///
    /// Meaningful only when [`has_check_digit`](Self::has_check_digit) is true.
    #[must_use]
    pub fn luhn_valid(self) -> bool {
        self.0.luhn_valid()
    }
}

impl fmt::Display for Iccid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for Iccid {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let d = digits_in_range("Iccid", ICCID_FORMAT, s, 19, 20)?;
        if d.at(0) != 8 || d.at(1) != 9 {
            return Err(ParseError::format("Iccid", s, ICCID_FORMAT));
        }
        Ok(Self(d))
    }
}

// ── 5G identities ────────────────────────────────────────────────────────────

/// A 3GPP TS 23.003 Subscription Permanent Identifier.
///
/// Either an [`Imsi`] or a network-specific identifier in NAI form. Both spell
/// the same thing — a subscription — and both are stable, so both may key an
/// aggregate.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Supi {
    /// The IMSI form, used by every 3GPP access.
    Imsi(Imsi),
    /// A network-specific identifier in `user@realm` form.
    NetworkSpecific(Nai),
}

impl Supi {
    /// The IMSI, when this identity has one.
    #[must_use]
    pub fn as_imsi(&self) -> Option<Imsi> {
        match self {
            Self::Imsi(i) => Some(*i),
            Self::NetworkSpecific(_) => None,
        }
    }
}

impl fmt::Display for Supi {
    /// Writes the 3GPP prefixed form: `imsi-262021234567890` or `nai-user@realm`.
    ///
    /// The prefix is part of the value, because the bare forms are not mutually
    /// exclusive and a stored identity must say which it is.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Imsi(i) => write!(f, "imsi-{i}"),
            Self::NetworkSpecific(n) => write!(f, "nai-{n}"),
        }
    }
}

/// The shape [`Supi`] accepts, as rendered in a [`ParseError`].
const SUPI_FORMAT: &str = "imsi-<digits> or nai-<user@realm>";

impl FromStr for Supi {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseError::format("Supi", s, SUPI_FORMAT);
        let t = s.trim();
        if let Some(rest) = t.strip_prefix("imsi-") {
            return rest.parse().map(Self::Imsi).map_err(|_| err());
        }
        if let Some(rest) = t.strip_prefix("nai-") {
            return rest.parse().map(Self::NetworkSpecific).map_err(|_| err());
        }
        Err(err())
    }
}

/// A network access identifier in `user@realm` form (RFC 7542).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Nai(String);

impl Nai {
    /// The user part, before the `@`.
    #[must_use]
    pub fn user(&self) -> &str {
        self.0.split_once('@').map_or(&self.0[..], |(u, _)| u)
    }

    /// The realm part, after the `@`.
    #[must_use]
    pub fn realm(&self) -> &str {
        self.0.split_once('@').map_or("", |(_, r)| r)
    }
}

impl fmt::Display for Nai {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Nai {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseError::format("Nai", s, "a network access identifier such as user@realm");
        let t = s.trim();
        let (user, realm) = t.split_once('@').ok_or_else(err)?;
        if user.is_empty() || realm.is_empty() || t.contains(char::is_whitespace) {
            return Err(err());
        }
        Ok(Self(t.to_owned()))
    }
}

/// A 3GPP TS 23.003 Generic Public Subscription Identifier.
///
/// Either an [`Msisdn`] or an external identifier in NAI form — the identity a
/// party outside the network uses to name a subscription.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Gpsi {
    /// The MSISDN form.
    Msisdn(Msisdn),
    /// An external identifier in `user@realm` form.
    External(Nai),
}

impl Gpsi {
    /// The MSISDN, when this identity has one.
    #[must_use]
    pub fn as_msisdn(&self) -> Option<Msisdn> {
        match self {
            Self::Msisdn(m) => Some(*m),
            Self::External(_) => None,
        }
    }
}

impl fmt::Display for Gpsi {
    /// Writes the 3GPP prefixed form: `msisdn-491701234567` or `extid-user@realm`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Msisdn(m) => write!(f, "msisdn-{m}"),
            Self::External(n) => write!(f, "extid-{n}"),
        }
    }
}

/// The shape [`Gpsi`] accepts, as rendered in a [`ParseError`].
const GPSI_FORMAT: &str = "msisdn-<digits> or extid-<user@realm>";

impl FromStr for Gpsi {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseError::format("Gpsi", s, GPSI_FORMAT);
        let t = s.trim();
        if let Some(rest) = t.strip_prefix("msisdn-") {
            return rest.parse().map(Self::Msisdn).map_err(|_| err());
        }
        if let Some(rest) = t.strip_prefix("extid-") {
            return rest.parse().map(Self::External).map_err(|_| err());
        }
        Err(err())
    }
}

/// A 3GPP TS 23.003 Permanent Equipment Identifier: an [`Imei`] or an
/// [`Imeisv`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Pei {
    /// The 15-digit, Luhn-checked form.
    Imei(Imei),
    /// The 16-digit form carrying a software version, with no check digit.
    Imeisv(Imeisv),
}

impl Pei {
    /// The type allocation code, whichever form the identity takes.
    #[must_use]
    pub fn tac(self) -> Tac {
        match self {
            Self::Imei(i) => i.tac(),
            Self::Imeisv(i) => i.tac(),
        }
    }

    /// The equipment as an [`Imei`], computing the check digit where needed.
    ///
    /// The single form to key on when counting devices across a record set that
    /// mixes the two.
    #[must_use]
    pub fn to_imei(self) -> Imei {
        match self {
            Self::Imei(i) => i,
            Self::Imeisv(i) => i.to_imei(),
        }
    }
}

impl fmt::Display for Pei {
    /// Writes the 3GPP prefixed form: `imei-…` or `imeisv-…`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Imei(i) => write!(f, "imei-{i}"),
            Self::Imeisv(i) => write!(f, "imeisv-{i}"),
        }
    }
}

/// The shape [`Pei`] accepts, as rendered in a [`ParseError`].
const PEI_FORMAT: &str = "imei-<15 digits> or imeisv-<16 digits>";

impl FromStr for Pei {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseError::format("Pei", s, PEI_FORMAT);
        let t = s.trim();
        // The longer prefix first: "imeisv-" also starts with "imei".
        if let Some(rest) = t.strip_prefix("imeisv-") {
            return rest.parse().map(Self::Imeisv).map_err(|_| err());
        }
        if let Some(rest) = t.strip_prefix("imei-") {
            return rest.parse().map(Self::Imei).map_err(|_| err());
        }
        Err(err())
    }
}

// ── Suci ─────────────────────────────────────────────────────────────────────

/// A 3GPP TS 23.003 Subscription Concealed Identifier.
///
/// # Not an aggregation key
///
/// Implements neither [`Hash`] nor [`Ord`], deliberately, so it cannot reach a
/// deduplication key, a map key or a sort. A SUCI is designed to be
/// non-linkable and differs between registrations of the same subscriber;
/// treating it as an identity splits one subscriber into many and makes both
/// billing and fraud detection quietly wrong.
///
/// Resolving a SUCI to a [`Supi`] is the network's job. A record that only ever
/// carried a SUCI is unattributable and should be dispositioned as such.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suci {
    /// The home network the concealed identity belongs to.
    pub home_network: PlmnId,
    /// The protection scheme identifier: `0` is the null scheme.
    pub protection_scheme: u8,
    /// The home network public key identifier used for concealment.
    pub public_key_id: u8,
    /// The concealed output, as hex.
    pub scheme_output: String,
}

impl Suci {
    /// `true` for the null protection scheme, where the output is a plain MSIN.
    ///
    /// Even then the value is not an aggregation key here: a null-scheme SUCI
    /// should be read back as a [`Supi`] by the network rather than counted in
    /// this form.
    #[must_use]
    pub fn is_null_scheme(&self) -> bool {
        self.protection_scheme == 0
    }
}

impl fmt::Display for Suci {
    /// Writes the 3GPP form: `suci-0-262-02-<pki>-<scheme>-<output>`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "suci-0-{}-{}-{}-{}-{}",
            self.home_network.mcc(),
            self.home_network.mnc(),
            self.public_key_id,
            self.protection_scheme,
            self.scheme_output
        )
    }
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::{Gpsi, Iccid, Imei, Imeisv, Imsi, Msisdn, Nai, Pei, Supi, Tac};
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

    string_serde!(Imsi, "an IMSI such as \"262021234567890\"");
    string_serde!(Msisdn, "an MSISDN such as \"491701234567\"");
    string_serde!(Imei, "an IMEI such as \"490154203237518\"");
    string_serde!(Imeisv, "an IMEISV such as \"4901542032375120\"");
    string_serde!(Iccid, "an ICCID such as \"8949012345678901234\"");
    string_serde!(Nai, "a network access identifier such as \"user@realm\"");
    string_serde!(Supi, "a SUPI such as \"imsi-262021234567890\"");
    string_serde!(Gpsi, "a GPSI such as \"msisdn-491701234567\"");
    string_serde!(Pei, "a PEI such as \"imei-490154203237518\"");

    /// Write-only: a [`Tac`] is derived from an equipment identity and has no
    /// parser of its own.
    impl Serialize for Tac {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.collect_str(self)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plmn::{Mcc, MncWidth, NorthAmericanRule};

    // ── IMSI ─────────────────────────────────────────────────────────────────

    /// The bug this API shape exists to prevent: splitting an IMSI at a fixed
    /// offset attributes a whole region to networks that do not exist.
    #[test]
    fn the_mnc_width_comes_from_the_resolver_not_the_digits() {
        let de: Imsi = "262021234567890".parse().unwrap();
        let us: Imsi = "310260123456789".parse().unwrap();

        assert_eq!(de.plmn(&NorthAmericanRule).unwrap().to_string(), "262-02");
        assert_eq!(us.plmn(&NorthAmericanRule).unwrap().to_string(), "310-260");

        // Same digits, a different table, a different network.
        let always_three = |_: Mcc| Some(MncWidth::Three);
        assert_eq!(de.plmn(&always_three).unwrap().to_string(), "262-021");
    }

    #[test]
    fn an_unknown_country_yields_no_network_rather_than_a_default() {
        let imsi: Imsi = "262021234567890".parse().unwrap();
        let empty_table = |_: Mcc| None;
        assert_eq!(imsi.plmn(&empty_table), None);
        assert_eq!(imsi.msin(&empty_table), None);
        // The country code needs no table at all.
        assert_eq!(imsi.mcc(), Mcc::GERMANY);
    }

    #[test]
    fn imsi_splits_into_three_parts() {
        let imsi: Imsi = "262021234567890".parse().unwrap();
        assert_eq!(imsi.mcc().to_string(), "262");
        assert_eq!(
            imsi.plmn(&NorthAmericanRule).unwrap().mnc().to_string(),
            "02"
        );
        assert_eq!(
            imsi.msin(&NorthAmericanRule).unwrap().to_string(),
            "1234567890"
        );
        assert_eq!(imsi.len(), 15);
        assert_eq!(imsi.to_string(), "262021234567890");
    }

    #[test]
    fn malformed_imsis_are_rejected() {
        for s in [
            "",
            "12345",            // too short to hold MCC + MNC + MSIN
            "1234567890123456", // 16 digits
            "000021234567890",  // MCC 000 is unassigned
            "26202123456789a",
        ] {
            assert!(s.parse::<Imsi>().is_err(), "{s:?} must not parse");
        }
    }

    // ── MSISDN ───────────────────────────────────────────────────────────────

    #[test]
    fn msisdn_normalises_to_one_spelling() {
        let plain: Msisdn = "491701234567".parse().unwrap();
        for s in ["+491701234567", " 491701234567 ", "49-170-1234567"] {
            assert_eq!(s.parse::<Msisdn>().unwrap(), plain, "{s:?}");
        }
        assert_eq!(plain.to_string(), "491701234567");
        assert_eq!(plain.to_international(), "+491701234567");
        assert!(plain.starts_with_cc(49));
        assert!(!plain.starts_with_cc(48));
    }

    #[test]
    fn msisdn_rejects_over_fifteen_digits() {
        assert!("1234567890123456".parse::<Msisdn>().is_err());
        assert!("123456789012345".parse::<Msisdn>().is_ok());
        assert!("".parse::<Msisdn>().is_err());
    }

    // ── IMEI / IMEISV ────────────────────────────────────────────────────────

    /// The whole point of a check digit: a transposition is caught.
    #[test]
    fn imei_luhn_rejects_transpositions() {
        let good = "490154203237518";
        assert!(good.parse::<Imei>().is_ok());

        let transposed = "490154203237581";
        assert!(transposed.parse::<Imei>().is_err());
    }

    #[test]
    fn imei_splits_into_tac_serial_and_check_digit() {
        let imei: Imei = "490154203237518".parse().unwrap();
        assert_eq!(imei.tac().to_string(), "49015420");
        assert_eq!(imei.serial().to_string(), "323751");
        assert_eq!(imei.check_digit(), 8);
        assert_eq!(imei.to_string(), "490154203237518");
    }

    /// The bug this type split exists to prevent: an IMEISV has no check digit,
    /// and Luhn-checking one rejects valid equipment.
    #[test]
    fn imeisv_carries_a_software_version_where_an_imei_carries_a_checksum() {
        let sv: Imeisv = "4901542032375120".parse().unwrap();
        assert_eq!(sv.tac().to_string(), "49015420");
        assert_eq!(sv.serial().to_string(), "323751");
        assert_eq!(sv.software_version().to_string(), "20");

        // Digit 15 is software-version data here; as an IMEI it would fail Luhn.
        assert!("490154203237512".parse::<Imei>().is_err());
        // …yet the IMEISV is perfectly valid.
        assert_eq!(sv.to_string(), "4901542032375120");
    }

    #[test]
    fn an_imeisv_yields_the_imei_of_the_same_device() {
        let sv: Imeisv = "4901542032375120".parse().unwrap();
        let imei: Imei = "490154203237518".parse().unwrap();
        assert_eq!(sv.to_imei(), imei);
        assert_eq!(sv.tac(), imei.tac());

        // Both PEI forms collapse onto one device identity.
        assert_eq!(Pei::Imeisv(sv).to_imei(), Pei::Imei(imei).to_imei());
    }

    // ── ICCID ────────────────────────────────────────────────────────────────

    #[test]
    fn iccid_requires_the_major_industry_identifier() {
        assert!("8949012345678901234".parse::<Iccid>().is_ok());
        assert!("1249012345678901234".parse::<Iccid>().is_err());
        assert!("894901234567890123".parse::<Iccid>().is_err()); // 18 digits
    }

    /// Issuers ship both lengths, so the checksum is reported rather than
    /// enforced — a failing check on a 19-digit card means nothing.
    #[test]
    fn iccid_reports_its_checksum_rather_than_enforcing_it() {
        let nineteen: Iccid = "8949012345678901234".parse().unwrap();
        assert!(!nineteen.has_check_digit());
        assert_eq!(nineteen.len(), 19);

        let twenty: Iccid = "89490123456789012341".parse().unwrap();
        assert!(twenty.has_check_digit());
        assert_eq!(twenty.len(), 20);
    }

    // ── 5G identities ────────────────────────────────────────────────────────

    #[test]
    fn supi_and_gpsi_round_trip_through_their_prefixed_forms() {
        for s in ["imsi-262021234567890", "nai-user@example.org"] {
            assert_eq!(s.parse::<Supi>().unwrap().to_string(), s);
        }
        for s in ["msisdn-491701234567", "extid-device@example.org"] {
            assert_eq!(s.parse::<Gpsi>().unwrap().to_string(), s);
        }
        assert!("262021234567890".parse::<Supi>().is_err());
        assert!("imsi-".parse::<Supi>().is_err());
    }

    /// `imeisv-` also begins with `imei`, so prefix order matters.
    #[test]
    fn pei_prefixes_do_not_shadow_each_other() {
        let imei: Pei = "imei-490154203237518".parse().unwrap();
        let imeisv: Pei = "imeisv-4901542032375120".parse().unwrap();
        assert_eq!(imei.to_string(), "imei-490154203237518");
        assert_eq!(imeisv.to_string(), "imeisv-4901542032375120");
        assert_eq!(imei.tac(), imeisv.tac());
    }

    #[test]
    fn nai_splits_at_the_at_sign() {
        let n: Nai = "device-42@iot.example.org".parse().unwrap();
        assert_eq!(n.user(), "device-42");
        assert_eq!(n.realm(), "iot.example.org");
        for s in ["noatsign", "@realm", "user@", "user @realm"] {
            assert!(s.parse::<Nai>().is_err(), "{s:?} must not parse");
        }
    }

    /// A compile-time guarantee, asserted here so the intent is recorded: a
    /// SUCI must not become a map key or a sort key.
    #[test]
    fn a_suci_is_not_hashable_or_orderable() {
        fn assert_not_key<T>() {}
        // These would fail to compile if `Suci` gained `Hash` or `Ord`:
        //   let _: std::collections::HashSet<Suci>;
        //   let _: std::collections::BTreeSet<Suci>;
        assert_not_key::<Suci>();

        let suci = Suci {
            home_network: "262-02".parse().unwrap(),
            protection_scheme: 0,
            public_key_id: 0,
            scheme_output: "1234567890".into(),
        };
        assert!(suci.is_null_scheme());
        assert_eq!(suci.to_string(), "suci-0-262-02-0-0-1234567890");
    }
}
