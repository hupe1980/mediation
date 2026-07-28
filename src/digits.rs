//! [`DigitString`] — a short run of decimal digits, stored inline, and the two
//! binary-coded-decimal conventions a charging record uses.
//!
//! # One record, two BCD conventions
//!
//! A 3GPP charging record encodes digits in **two different ways**, and using
//! either convention where the other belongs produces a plausible, wrong value
//! that nothing downstream can identify as corrupt:
//!
//! | Encoding | Nibble order | Filler | Used for |
//! |---|---|---|---|
//! | **TBCD** (TS 29.002) | **swapped** — low nibble first | `0xF` | IMSI, MSISDN, IMEI, network identities |
//! | **Plain BCD** | natural — high nibble first | none | the `TimeStamp` of TS 32.298 |
//!
//! The digits `12345` are `21 43 F5` in TBCD and `01 23 45` in plain BCD. Read
//! one as the other and an IMSI becomes a different subscriber's, or a
//! timestamp jumps decades — silently, because both results are well-formed.
//!
//! ```rust
//! use mediation::DigitString;
//!
//! // TBCD: nibbles swapped, 0xF pads an odd count.
//! let tbcd = DigitString::from_tbcd(&[0x21, 0x43, 0xF5]).unwrap();
//! assert_eq!(tbcd.to_string(), "12345");
//!
//! // Plain BCD over the same digits is a different byte sequence entirely.
//! let bcd = DigitString::from_bcd(&[0x01, 0x23, 0x45]).unwrap();
//! assert_eq!(bcd.to_string(), "012345");
//!
//! // …and reading either with the wrong convention gives a wrong answer that
//! // is still a valid digit string, which is why the two are separate calls.
//! assert_eq!(DigitString::from_bcd(&[0x21, 0x43]).unwrap().to_string(), "2143");
//! ```
//!
//! # No heap
//!
//! Up to [`DigitString::MAX_DIGITS`] digits packed two per byte, inline and
//! [`Copy`]. A mediation layer holds millions of identities in flight at once,
//! and a heap allocation for each is the difference between fitting in cache
//! and not.

use std::fmt;
use std::str::FromStr;

use crate::error::{DecodeError, ParseError};

/// Bytes needed to hold [`DigitString::MAX_DIGITS`] nibbles.
const PACKED_BYTES: usize = DigitString::MAX_DIGITS.div_ceil(2);

/// The TBCD filler nibble, used when a digit count is odd (3GPP TS 29.002).
const TBCD_FILLER: u8 = 0x0F;

/// A short run of decimal digits, packed two per byte and stored inline.
///
/// Trailing nibbles beyond the length are always zero, so the derived
/// [`PartialEq`], [`Ord`] and [`Hash`] agree with the digit sequence rather than
/// with whatever happened to be in the buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct DigitString {
    packed: [u8; PACKED_BYTES],
    len: u8,
}

impl DigitString {
    /// The widest run this type stores — enough for a 20-digit ICCID.
    pub const MAX_DIGITS: usize = 20;

    /// An empty run.
    pub const EMPTY: Self = Self {
        packed: [0; PACKED_BYTES],
        len: 0,
    };

    /// Reads decimal digits from `s`, ignoring ASCII whitespace and `-`.
    ///
    /// `None` for a non-digit character or for more than `max` digits.
    #[must_use]
    pub fn parse(s: &str, max: usize) -> Option<Self> {
        let mut out = Self::EMPTY;
        for byte in s.bytes() {
            if byte.is_ascii_whitespace() || byte == b'-' {
                continue;
            }
            if !byte.is_ascii_digit() {
                return None;
            }
            out = out.checked_push(byte - b'0')?;
            if out.len() > max {
                return None;
            }
        }
        Some(out)
    }

    /// The digit at `index`, or 0 when out of range.
    #[must_use]
    pub const fn at(self, index: usize) -> u8 {
        if index >= self.len as usize {
            return 0;
        }
        (self.packed[index / 2] >> (4 * (index % 2))) & 0x0F
    }

    /// How many digits are held.
    #[must_use]
    pub const fn len(self) -> usize {
        self.len as usize
    }

    /// `true` when no digits are held.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.len == 0
    }

    /// A sub-run of `count` digits starting at `start`.
    ///
    /// [`EMPTY`](Self::EMPTY) when the range does not fit — the callers here
    /// slice at offsets their own validation already established, and an
    /// out-of-range slice is a programming error rather than bad input.
    #[must_use]
    pub fn slice(self, start: usize, count: usize) -> Self {
        let mut out = Self::EMPTY;
        if start + count > self.len() {
            return out;
        }
        for i in 0..count {
            // Unreachable: `count <= self.len() <= MAX_DIGITS`, checked above.
            // Panicking rather than truncating is deliberate — a silently
            // shortened slice is a different identifier that still looks valid.
            out = out
                .checked_push(self.at(start + i))
                .expect("a sub-run of a valid run always fits");
        }
        out
    }

    /// The digits read as a number.
    ///
    /// **Saturates.** A `u64` covers about 1.8 × 10^19, so a run of 20 digits
    /// can exceed it and a 20-digit ICCID sometimes does. Saturating rather
    /// than wrapping is the lesser evil — a wrapped value is a small plausible
    /// number, while [`u64::MAX`] is visibly wrong.
    ///
    /// Use it only for the short runs where overflow is impossible: a
    /// three-digit MCC, a two- or three-digit MNC. For anything longer, compare
    /// the [`DigitString`] itself, which is exact and cheaper.
    #[must_use]
    pub fn to_u64(self) -> u64 {
        let mut value = 0u64;
        for i in 0..self.len() {
            value = value
                .saturating_mul(10)
                .saturating_add(u64::from(self.at(i)));
        }
        value
    }

    /// Appends a digit, `None` past [`MAX_DIGITS`](Self::MAX_DIGITS) or for a
    /// value above 9.
    #[must_use]
    pub const fn checked_push(mut self, digit: u8) -> Option<Self> {
        let i = self.len as usize;
        if i >= Self::MAX_DIGITS || digit > 9 {
            return None;
        }
        self.packed[i / 2] |= digit << (4 * (i % 2));
        self.len += 1;
        Some(self)
    }

    // ── Check digits ─────────────────────────────────────────────────────────

    /// `true` when the final digit is a consistent Luhn check digit
    /// (ISO/IEC 7812-1 Annex B).
    #[must_use]
    pub fn luhn_valid(self) -> bool {
        if self.len() < 2 {
            return false;
        }
        let mut sum = 0u32;
        // Walk right to left; the rightmost digit is the check digit and is
        // never doubled.
        for (position, index) in (0..self.len()).rev().enumerate() {
            let mut d = u32::from(self.at(index));
            if position % 2 == 1 {
                d *= 2;
                if d > 9 {
                    d -= 9;
                }
            }
            sum += d;
        }
        sum.is_multiple_of(10)
    }

    /// The Luhn check digit that would make `self` — treated as a payload with
    /// no check digit — valid.
    #[must_use]
    pub fn luhn_check_digit(self) -> u8 {
        let mut sum = 0u32;
        // The check digit would occupy position 0, so the last payload digit
        // sits at position 1 and is doubled.
        for (position, index) in (0..self.len()).rev().enumerate() {
            let mut d = u32::from(self.at(index));
            if position % 2 == 0 {
                d *= 2;
                if d > 9 {
                    d -= 9;
                }
            }
            sum += d;
        }
        ((10 - (sum % 10)) % 10) as u8
    }

    // ── TBCD ─────────────────────────────────────────────────────────────────

    /// Decode the Telephony BCD of 3GPP TS 29.002: **swapped nibbles**, `0xF`
    /// filler.
    ///
    /// The low nibble of each octet is the earlier digit. A trailing `0xF` high
    /// nibble marks an odd digit count and ends the run.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] for a nibble that is not a decimal digit, for a filler
    /// that appears anywhere but the final high nibble, or for more than
    /// [`MAX_DIGITS`](Self::MAX_DIGITS) digits.
    ///
    /// The extended TBCD values `*`, `#`, `a`, `b` and `c` are **rejected**
    /// rather than mapped: they cannot appear in the identifiers this crate
    /// models, and silently accepting one would put a non-digit into a value
    /// that every downstream comparison assumes is numeric.
    pub fn from_tbcd(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut out = Self::EMPTY;
        for (offset, octet) in bytes.iter().enumerate() {
            let low = octet & 0x0F;
            let high = octet >> 4;

            if low == TBCD_FILLER {
                return Err(DecodeError::new(offset, "a decimal digit, not a filler"));
            }
            out = out.checked_push(low).ok_or_else(|| {
                DecodeError::new(offset, "a decimal digit within the length limit")
            })?;

            if high == TBCD_FILLER {
                if offset + 1 != bytes.len() {
                    return Err(DecodeError::new(
                        offset,
                        "a filler only in the final high nibble",
                    ));
                }
                return Ok(out);
            }
            out = out.checked_push(high).ok_or_else(|| {
                DecodeError::new(offset, "a decimal digit within the length limit")
            })?;
        }
        Ok(out)
    }

    /// Encode to Telephony BCD, writing into `out`.
    ///
    /// Returns how many octets were written: `len().div_ceil(2)`.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] when `out` is too short. Writing into a caller's buffer
    /// rather than returning one keeps the encoder allocation-free, which
    /// matters when it runs once per identifier per record.
    pub fn to_tbcd(self, out: &mut [u8]) -> Result<usize, DecodeError> {
        let needed = self.len().div_ceil(2);
        if out.len() < needed {
            return Err(DecodeError::new(0, "a buffer of at least len()/2 octets"));
        }
        for octet in out.iter_mut().take(needed) {
            *octet = 0;
        }
        for i in 0..self.len() {
            let digit = self.at(i);
            if i % 2 == 0 {
                out[i / 2] |= digit;
            } else {
                out[i / 2] |= digit << 4;
            }
        }
        if !self.len().is_multiple_of(2) {
            out[needed - 1] |= TBCD_FILLER << 4;
        }
        Ok(needed)
    }

    // ── Plain BCD ────────────────────────────────────────────────────────────

    /// Decode plain binary-coded decimal: **natural nibble order**, no filler.
    ///
    /// Always yields an even digit count, two per octet. This is the convention
    /// the `TimeStamp` of 3GPP TS 32.298 uses — *not* the swapped-nibble TBCD
    /// of the identifier fields in the same record.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] for a nibble above 9, or for more than
    /// [`MAX_DIGITS`](Self::MAX_DIGITS) digits.
    pub fn from_bcd(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut out = Self::EMPTY;
        for (offset, octet) in bytes.iter().enumerate() {
            for digit in [octet >> 4, octet & 0x0F] {
                out = out.checked_push(digit).ok_or_else(|| {
                    DecodeError::new(offset, "a decimal digit within the length limit")
                })?;
            }
        }
        Ok(out)
    }

    /// Encode to plain binary-coded decimal, writing into `out`.
    ///
    /// Returns how many octets were written.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] when the digit count is odd — plain BCD has no filler,
    /// so an odd run cannot be represented and padding it would invent a digit —
    /// or when `out` is too short.
    pub fn to_bcd(self, out: &mut [u8]) -> Result<usize, DecodeError> {
        if !self.len().is_multiple_of(2) {
            return Err(DecodeError::new(
                0,
                "an even digit count; plain BCD has no filler",
            ));
        }
        let needed = self.len() / 2;
        if out.len() < needed {
            return Err(DecodeError::new(0, "a buffer of at least len()/2 octets"));
        }
        for (index, octet) in out.iter_mut().enumerate().take(needed) {
            *octet = (self.at(index * 2) << 4) | self.at(index * 2 + 1);
        }
        Ok(needed)
    }
}

impl fmt::Display for DigitString {
    /// Writes the digits with no separators and no allocation.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for i in 0..self.len() {
            write!(f, "{}", self.at(i))?;
        }
        Ok(())
    }
}

impl FromStr for DigitString {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s, Self::MAX_DIGITS).ok_or_else(|| {
            ParseError::format(
                "DigitString",
                s,
                "up to 20 decimal digits, optionally separated by spaces or hyphens",
            )
        })
    }
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::DigitString;
    use serde::de::{self, Visitor};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::fmt;

    impl Serialize for DigitString {
        /// A string, so leading zeros survive.
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.collect_str(self)
        }
    }

    impl<'de> Deserialize<'de> for DigitString {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct V;
            impl Visitor<'_> for V {
                type Value = DigitString;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("a run of up to 20 decimal digits")
                }
                fn visit_str<E: de::Error>(self, v: &str) -> Result<DigitString, E> {
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

    #[test]
    fn packed_digits_compare_by_sequence() {
        let a: DigitString = "0123456789".parse().unwrap();
        assert_eq!(a.to_string(), "0123456789");
        assert_eq!(a.len(), 10);
        assert_eq!(a.at(0), 0);
        assert_eq!(a.at(9), 9);

        // Equal sequences compare equal regardless of construction path.
        assert_eq!(a, DigitString::parse("01-23 456789", 20).unwrap());

        // A leading zero is a digit, not padding.
        assert_ne!(
            DigitString::parse("01", 20).unwrap(),
            DigitString::parse("1", 20).unwrap()
        );
    }

    #[test]
    fn digits_reject_non_numeric_and_overlong_input() {
        assert!(DigitString::parse("12a4", 20).is_none());
        assert!(DigitString::parse("123456789012345678901", 20).is_none());
        assert!(DigitString::parse("1234", 3).is_none());
        assert!(DigitString::EMPTY.is_empty());
    }

    // ── TBCD ─────────────────────────────────────────────────────────────────

    /// The worked example from 3GPP TS 29.002: `12345` is `21 43 F5`.
    #[test]
    fn tbcd_matches_the_published_example() {
        let decoded = DigitString::from_tbcd(&[0x21, 0x43, 0xF5]).unwrap();
        assert_eq!(decoded.to_string(), "12345");

        let mut buffer = [0u8; 3];
        assert_eq!(decoded.to_tbcd(&mut buffer).unwrap(), 3);
        assert_eq!(buffer, [0x21, 0x43, 0xF5]);
    }

    /// The bug the two separate calls exist to prevent: the same octets read
    /// with the other convention give a different, well-formed answer.
    #[test]
    fn the_two_conventions_disagree_on_the_same_octets() {
        let octets = [0x21, 0x43];
        assert_eq!(DigitString::from_tbcd(&octets).unwrap().to_string(), "1234");
        assert_eq!(DigitString::from_bcd(&octets).unwrap().to_string(), "2143");
    }

    #[test]
    fn tbcd_round_trips_at_every_length() {
        for length in 1..=DigitString::MAX_DIGITS {
            let text: String = (0..length)
                .map(|i| char::from(b'0' + (i % 10) as u8))
                .collect();
            let digits: DigitString = text.parse().unwrap();

            let mut buffer = [0u8; 10];
            let written = digits.to_tbcd(&mut buffer).unwrap();
            assert_eq!(written, length.div_ceil(2));
            assert_eq!(
                DigitString::from_tbcd(&buffer[..written]).unwrap(),
                digits,
                "length {length}"
            );
        }
    }

    /// The extended TBCD values cannot appear in an identifier this crate
    /// models, and accepting one would put a non-digit into a numeric value.
    #[test]
    fn tbcd_rejects_the_extended_values() {
        // 0xA is `*`, 0xB is `#`, 0xC..0xE are `a`..`c`.
        for nibble in 0xA..=0xE {
            assert!(
                DigitString::from_tbcd(&[nibble]).is_err(),
                "low nibble {nibble:#X} must be rejected"
            );
            assert!(
                DigitString::from_tbcd(&[0x01 | (nibble << 4)]).is_err(),
                "high nibble {nibble:#X} must be rejected"
            );
        }
    }

    /// A filler anywhere but the final high nibble means the field was
    /// truncated or misaligned.
    #[test]
    fn tbcd_rejects_a_misplaced_filler() {
        // Low nibble is the *first* digit, so a filler there is out of place.
        assert!(
            DigitString::from_tbcd(&[0x1F]).is_err(),
            "filler in a low nibble"
        );
        assert!(
            DigitString::from_tbcd(&[0x2F, 0x43]).is_err(),
            "filler before the end"
        );
        // …while 0xF1 is a well-formed single digit: 1, then the filler.
        assert_eq!(DigitString::from_tbcd(&[0xF1]).unwrap().to_string(), "1");
        assert!(
            DigitString::from_tbcd(&[0x21, 0xF3, 0x45]).is_err(),
            "high-nibble filler mid-run"
        );
        // …and one in the right place is fine.
        assert!(DigitString::from_tbcd(&[0x21, 0xF3]).is_ok());
    }

    #[test]
    fn tbcd_rejects_an_overlong_field_and_a_short_buffer() {
        let too_long = [0x12u8; 11]; // 22 digits
        assert!(DigitString::from_tbcd(&too_long).is_err());

        let digits: DigitString = "12345".parse().unwrap();
        let mut small = [0u8; 2];
        assert!(digits.to_tbcd(&mut small).is_err());
    }

    // ── Plain BCD ────────────────────────────────────────────────────────────

    #[test]
    fn plain_bcd_round_trips() {
        let digits: DigitString = "260728100000".parse().unwrap();
        let mut buffer = [0u8; 6];
        assert_eq!(digits.to_bcd(&mut buffer).unwrap(), 6);
        assert_eq!(buffer, [0x26, 0x07, 0x28, 0x10, 0x00, 0x00]);
        assert_eq!(DigitString::from_bcd(&buffer).unwrap(), digits);
    }

    /// Plain BCD has no filler, so an odd run cannot be written — padding it
    /// would invent a digit.
    #[test]
    fn plain_bcd_refuses_an_odd_digit_count() {
        let odd: DigitString = "12345".parse().unwrap();
        let mut buffer = [0u8; 3];
        assert!(odd.to_bcd(&mut buffer).is_err());
    }

    #[test]
    fn plain_bcd_rejects_a_non_decimal_nibble() {
        assert!(DigitString::from_bcd(&[0xA1]).is_err());
        assert!(DigitString::from_bcd(&[0x1F]).is_err());
    }

    // ── Check digits ─────────────────────────────────────────────────────────

    #[test]
    fn luhn_verification_and_generation_agree() {
        for seed in 0u64..200 {
            let payload = format!("{:014}", seed * 7_919);
            let digits: DigitString = payload.parse().unwrap();
            let full = digits.checked_push(digits.luhn_check_digit()).unwrap();
            assert!(full.luhn_valid(), "{payload} + check digit must validate");
        }
        // Too short to carry a check digit at all.
        assert!(!DigitString::parse("7", 20).unwrap().luhn_valid());
    }

    #[test]
    fn a_slice_that_does_not_fit_is_empty_rather_than_wrong() {
        let digits: DigitString = "12345".parse().unwrap();
        assert_eq!(digits.slice(0, 3).to_string(), "123");
        assert_eq!(digits.slice(3, 2).to_string(), "45");
        assert!(digits.slice(3, 9).is_empty());
    }

    /// A wrapped value would be a small plausible number; the saturated one is
    /// visibly wrong, which is the point.
    #[test]
    fn to_u64_saturates_rather_than_wrapping() {
        let twenty: DigitString = "99999999999999999999".parse().unwrap();
        assert_eq!(twenty.to_u64(), u64::MAX);

        // The runs it is meant for cannot overflow at all.
        assert_eq!("262".parse::<DigitString>().unwrap().to_u64(), 262);
        assert_eq!("02".parse::<DigitString>().unwrap().to_u64(), 2);

        // Comparing the digits themselves is exact where the number is not.
        let a: DigitString = "99999999999999999998".parse().unwrap();
        let b: DigitString = "99999999999999999999".parse().unwrap();
        assert_eq!(a.to_u64(), b.to_u64(), "both saturate");
        assert_ne!(a, b, "…but the digit strings are distinct");
    }
}
