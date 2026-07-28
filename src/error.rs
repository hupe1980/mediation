//! [`ParseError`] — the single error type every [`FromStr`] in this crate returns.
//!
//! One type rather than one per parsed type, so a caller decoding a record can
//! write
//!
//! ```rust
//! # use mediation::{Imsi, Msisdn, ParseError, PlmnId};
//! fn decode(imsi: &str, msisdn: &str, plmn: &str)
//!     -> Result<(Imsi, Msisdn, PlmnId), ParseError>
//! {
//!     Ok((imsi.parse()?, msisdn.parse()?, plmn.parse()?))
//! }
//! # assert!(decode("262021234567890", "491701234567", "262-02").is_ok());
//! # assert!(decode("262021234567890", "491701234567", "262-2").is_err());
//! ```
//!
//! without three `map_err` calls or a bespoke enum. The rejected input and the
//! type that rejected it are both carried, so the message stays specific enough
//! to name the offending field in a rejected record.
//!
//! [`DecodeError`] is the byte-level counterpart, returned by everything that
//! reads a wire encoding. It carries an **offset** rather than a rejected
//! string, because that is what a caller needs to find the field in a record
//! they cannot print.
//!
//! [`FromStr`]: std::str::FromStr

use std::fmt;

/// What a parser would have accepted.
///
/// Private so the variants can grow without a breaking change; render it
/// through [`ParseError`]'s [`Display`](fmt::Display) or read the
/// machine-usable parts via [`ParseError::expected_values`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum Expected {
    /// A closed set of codes, e.g. `["VOICE", "SMS", …]`.
    OneOf(&'static [&'static str]),
    /// A free-form shape, e.g. `"MCC-MNC, e.g. 262-02"`.
    Format(&'static str),
}

/// A string that could not be parsed into one of this crate's types.
///
/// Returned by every [`FromStr`](std::str::FromStr) implementation here.
/// Construct one only through the crate's parsers — the fields are private so
/// that added context is never a breaking change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    type_name: &'static str,
    input: String,
    expected: Expected,
}

impl ParseError {
    /// A value rejected against a closed set of codes.
    pub(crate) fn one_of(
        type_name: &'static str,
        input: &str,
        codes: &'static [&'static str],
    ) -> Self {
        Self {
            type_name,
            input: input.to_owned(),
            expected: Expected::OneOf(codes),
        }
    }

    /// A value rejected against a format description.
    pub(crate) fn format(type_name: &'static str, input: &str, shape: &'static str) -> Self {
        Self {
            type_name,
            input: input.to_owned(),
            expected: Expected::Format(shape),
        }
    }

    /// The type that rejected the input, e.g. `"Imsi"`.
    ///
    /// Useful when one `?` chain parses several fields of a record and the
    /// handler needs to report which one was bad.
    #[must_use]
    pub const fn type_name(&self) -> &'static str {
        self.type_name
    }

    /// The input that was rejected, verbatim.
    ///
    /// Note that this is subscriber-identifying data for the identifier types.
    /// Logging a [`ParseError`] verbatim writes a malformed IMSI or MSISDN to
    /// the log; prefer [`type_name`](Self::type_name) where the log is retained
    /// longer than the record would be.
    #[must_use]
    pub fn input(&self) -> &str {
        &self.input
    }

    /// The accepted codes, when the type has a closed set of them.
    ///
    /// `None` for types parsed by shape rather than by enumeration — an
    /// [`Imsi`](crate::Imsi) or a [`PlmnId`](crate::PlmnId) has a format, which
    /// [`Display`](fmt::Display) renders.
    #[must_use]
    pub const fn expected_values(&self) -> Option<&'static [&'static str]> {
        match self.expected {
            Expected::OneOf(codes) => Some(codes),
            Expected::Format(_) => None,
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid {} {:?}: expected ", self.type_name, self.input)?;
        match self.expected {
            Expected::OneOf(codes) => write!(f, "one of {}", codes.join(", ")),
            Expected::Format(shape) => f.write_str(shape),
        }
    }
}

impl std::error::Error for ParseError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Imsi, PlmnId, ServiceDomain};

    #[test]
    fn message_names_the_type_and_the_input() {
        let err = "TELEPATHY".parse::<ServiceDomain>().unwrap_err();
        assert_eq!(err.type_name(), "ServiceDomain");
        assert_eq!(err.input(), "TELEPATHY");
        let msg = err.to_string();
        assert!(msg.contains("ServiceDomain"), "{msg}");
        assert!(msg.contains("TELEPATHY"), "{msg}");
        assert!(msg.contains("VOICE"), "{msg}");
    }

    #[test]
    fn closed_sets_expose_their_codes_and_formats_do_not() {
        let closed = "nope".parse::<ServiceDomain>().unwrap_err();
        assert_eq!(closed.expected_values(), Some(ServiceDomain::CODES));

        let shaped = "nope".parse::<PlmnId>().unwrap_err();
        assert_eq!(shaped.expected_values(), None);
        assert!(shaped.to_string().contains("262-02"), "{shaped}");
    }

    /// One error type across every parser, so a decoder can use a single `?`
    /// chain — this is the reason the type is shared rather than per-type.
    #[test]
    fn every_parser_returns_the_same_error_type() {
        fn decode(d: &str, p: &str, i: &str) -> Result<(), ParseError> {
            let _: ServiceDomain = d.parse()?;
            let _: PlmnId = p.parse()?;
            let _: Imsi = i.parse()?;
            Ok(())
        }
        assert!(decode("DATA", "262-02", "262021234567890").is_ok());
        let err = decode("DATA", "262-02", "not-an-imsi").unwrap_err();
        assert_eq!(err.type_name(), "Imsi");
    }
}

// ── DecodeError ──────────────────────────────────────────────────────────────

/// Bytes that could not be decoded into one of this crate's types.
///
/// Carries the octet offset at which decoding failed, because a caller holding
/// a binary record cannot search it for a rejected string — they need to know
/// where to look.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError {
    at: usize,
    expected: &'static str,
}

impl DecodeError {
    /// A failure at `at` octets into the input.
    #[must_use]
    pub const fn new(at: usize, expected: &'static str) -> Self {
        Self { at, expected }
    }

    /// How many octets into the input the failure was found.
    #[must_use]
    pub const fn at(&self) -> usize {
        self.at
    }

    /// What the decoder would have accepted.
    #[must_use]
    pub const fn expected(&self) -> &'static str {
        self.expected
    }
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "cannot decode at octet {}: expected {}",
            self.at, self.expected
        )
    }
}

impl std::error::Error for DecodeError {}
