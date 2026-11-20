//! Regression tests for the totality, wire and identity findings A1–A17 of
//! the audit (specs/002-audit-hardening/research.md).

use mediation::identity::{Nai, NatureOfAddress, NumberingPlanIndicator};
use mediation::{
    AddressString, BerReader, ByteScale, CdrTimestamp, Century, DigitString, Iccid, Imei, Imsi,
    MsTimeZone, Msisdn, QuarterHourOffset, Suci,
};
use std::panic::catch_unwind;

// ── A1 ───────────────────────────────────────────────────────────────────────

/// A1: the TS 29.002 `IMEI` field carries 14 digits, a spare `0` and a filler
/// (TS 23.003 §6.2.1); the check digit is computed, not read.
/// `audit-hardening/SR-004`
#[test]
fn a1_imei_wire_form_with_spare_digit_decodes_with_computed_check_digit() {
    let wire = [0x94, 0x10, 0x45, 0x02, 0x23, 0x73, 0x15, 0xF0];
    let imei = Imei::from_tbcd(&wire).unwrap();
    assert_eq!(imei.to_string(), "490154203237518");
    assert_eq!(imei.check_digit(), 8);

    // The encoder writes the same wire form back.
    let mut out = [0u8; 8];
    assert_eq!(imei.to_tbcd(&mut out).unwrap(), 8);
    assert_eq!(out, wire);

    // A transmitted (non-zero) check digit is verified, not trusted.
    let with_check = [0x94, 0x10, 0x45, 0x02, 0x23, 0x73, 0x15, 0xF8];
    assert_eq!(Imei::from_tbcd(&with_check).unwrap(), imei);
    let bad_check = [0x94, 0x10, 0x45, 0x02, 0x23, 0x73, 0x15, 0xF2];
    assert!(Imei::from_tbcd(&bad_check).is_err());

    // Sixteen digits is an IMEISV, not an IMEI; seven octets is not the field.
    let sixteen = [0x94, 0x10, 0x45, 0x02, 0x23, 0x73, 0x15, 0x02];
    assert!(Imei::from_tbcd(&sixteen).is_err());
    assert!(Imei::from_tbcd(&wire[..7]).is_err());
}

// ── A2 / A17 ─────────────────────────────────────────────────────────────────

/// A2: instants outside UTC years 1–9998 are refused at the parser, so
/// `to_utc`, `to_offset` and `cmp_instant` never panic.
/// `audit-hardening/SR-003`
#[test]
fn a2_timestamps_at_the_year_range_edge_are_refused_not_panicking() {
    for s in [
        "9999-12-31T23:59:59-12:00",
        "9999-06-01T00:00:00+00:00",
        "0000-01-01T00:00:00+14:00",
    ] {
        assert!(s.parse::<CdrTimestamp>().is_err(), "{s:?} must be refused");
    }
    assert!(CdrTimestamp::from_tap("99991231235959", "-1200").is_err());

    // The edges that remain are total.
    for s in ["9998-12-31T23:59:59+00:00", "0001-01-01T00:00:00+00:00"] {
        let ts: CdrTimestamp = s.parse().unwrap();
        let r = catch_unwind(|| {
            let _ = ts.to_utc();
            let east = ts.to_offset(QuarterHourOffset::from_encoded(79).unwrap());
            let west = ts.to_offset(QuarterHourOffset::from_encoded(-79).unwrap());
            assert_eq!(east.to_utc(), west.to_utc());
            assert_eq!(east.to_string().parse::<CdrTimestamp>().unwrap(), east);
            assert_eq!(west.to_string().parse::<CdrTimestamp>().unwrap(), west);
            CdrTimestamp::cmp_instant(&ts, &east)
        });
        assert!(r.is_ok(), "{s:?} must not panic");
    }
}

/// A2: the checked constructors refuse what `new` and `from_utc` cannot hold.
/// `audit-hardening/SR-003`
#[test]
fn a2_checked_constructors_refuse_out_of_range_instants() {
    let local = time::macros::datetime!(9999-12-31 23:00);
    assert!(CdrTimestamp::new(local, QuarterHourOffset::UTC).is_none());
    let instant = time::macros::datetime!(9999-12-31 23:00 UTC);
    assert!(CdrTimestamp::from_utc(instant, QuarterHourOffset::UTC).is_none());
    let instant = time::macros::datetime!(9998-12-31 23:00 UTC);
    let ts = CdrTimestamp::from_utc(instant, QuarterHourOffset::MAX_ASSIGNED).unwrap();
    assert_eq!(ts.to_utc(), instant);
}

/// A17: no constructible timestamp renders a negative or five-digit year.
/// `audit-hardening/SR-003`
#[test]
fn a17_negative_years_are_not_constructible() {
    let date = time::Date::from_calendar_date(-5, time::Month::January, 1).unwrap();
    let local = time::PrimitiveDateTime::new(date, time::Time::MIDNIGHT);
    assert!(CdrTimestamp::new(local, QuarterHourOffset::UTC).is_none());
}

// ── A3 ───────────────────────────────────────────────────────────────────────

/// A3: a century is bounded to `0..=9900`, and its arithmetic is checked.
/// `audit-hardening/SR-003`
#[test]
fn a3_century_is_bounded() {
    assert!(Century::new(2_147_483_600).is_none());
    assert!(Century::new(-100).is_none());
    assert!(Century::new(10_000).is_none());
    let last = Century::new(9900).unwrap();
    assert_eq!(last.year(99), Some(9999));
    assert_eq!(last.year(100), None);
    assert!(last.contains(9999));

    // 9999 is outside the UTC year range, refused rather than panicking.
    let bcd = [0x99, 0x12, 0x31, 0x23, 0x00, 0x00, b'+', 0, 0];
    assert!(CdrTimestamp::from_bcd(&bcd, last).is_err());
    let ok = [0x98, 0x12, 0x31, 0x23, 0x00, 0x00, b'+', 0, 0];
    assert!(CdrTimestamp::from_bcd(&ok, last).is_ok());
}

// ── A4 / A12 ─────────────────────────────────────────────────────────────────

/// A4: a base offset that would overflow is a decode error, not a panic.
/// `audit-hardening/SR-003`
#[test]
fn a4_ber_base_offset_overflow_is_an_error() {
    let r = catch_unwind(|| {
        BerReader::at_offset(&[0x02, 0x01, 0x00], usize::MAX)
            .next()
            .map(|e| e.is_ok())
    });
    assert_eq!(r.ok(), Some(Some(false)));
    // The largest base that still fits works.
    let base = usize::MAX - 3;
    let element = BerReader::at_offset(&[0x02, 0x01, 0x00], base)
        .next()
        .unwrap()
        .unwrap();
    assert_eq!(element.offset(), base);
}

/// A12: BER lets the sender use the long length form, with leading zero
/// octets (X.690 §8.1.3.5); the reader accepts it.
/// `audit-hardening/SR-004`
#[test]
fn a12_long_form_lengths_are_accepted() {
    let b = [0x04, 0x81, 0x05, 1, 2, 3, 4, 5];
    let element = BerReader::new(&b).next().unwrap().unwrap();
    assert_eq!(element.content(), &[1, 2, 3, 4, 5]);
    assert_eq!(element.encoded_len(), b.len());

    let padded = [0x04, 0x83, 0x00, 0x00, 0x02, 7, 8];
    let element = BerReader::new(&padded).next().unwrap().unwrap();
    assert_eq!(element.content(), &[7, 8]);
}

// ── A5 / A6 ──────────────────────────────────────────────────────────────────

/// A5: every encodable offset (±79 quarter-hours) renders and parses back,
/// assigned or not.
/// `audit-hardening/SR-004`
#[test]
fn a5_every_encodable_offset_round_trips_through_its_string() {
    for q in -79i8..=79 {
        let o = QuarterHourOffset::from_encoded(q).unwrap();
        assert_eq!(o.to_string().parse::<QuarterHourOffset>(), Ok(o), "{o}");
        let z = MsTimeZone::new(o, mediation::DstAdjustment::None);
        assert_eq!(z.to_string().parse::<MsTimeZone>(), Ok(z), "{z}");
    }
    let o = QuarterHourOffset::from_semi_octet(0x97).unwrap();
    assert!(!o.is_assigned());
    assert_eq!(o.to_string().parse::<QuarterHourOffset>(), Ok(o));
    assert!("+20:00".parse::<QuarterHourOffset>().is_err());
}

/// A6: `-00:00`, `Z` and TAP `-0000` say the local offset is unknown
/// (RFC 3339 §4.3, RFC 9557 §2.2) and are refused; UTC is `+00:00`.
/// `audit-hardening/SR-004`
#[test]
fn a6_unknown_local_offset_is_refused() {
    for s in [
        "2026-01-01T00:00:00-00:00",
        "2026-01-01T00:00:00-0000",
        "2026-01-01T00:00:00-00",
        "2026-01-01T00:00:00Z",
        "2026-01-01T00:00:00z",
    ] {
        assert!(s.parse::<CdrTimestamp>().is_err(), "{s:?}");
    }
    for s in ["-00:00", "Z", "-00"] {
        assert!(s.parse::<QuarterHourOffset>().is_err(), "{s:?}");
    }
    assert!(CdrTimestamp::from_tap("20260101000000", "-0000").is_err());
    assert!(CdrTimestamp::from_tap("20260101000000", "+0000").is_ok());
    assert!("2026-01-01T00:00:00+00:00".parse::<CdrTimestamp>().is_ok());
}

// ── A7 / A8 ──────────────────────────────────────────────────────────────────

/// A7: the scheme identifier is one hex digit (TS 29.503), the null scheme
/// carries key 0, and each value has one spelling.
/// `audit-hardening/SR-004`
#[test]
fn a7_suci_scheme_is_one_hex_digit_with_one_spelling() {
    let s10 = Suci::new(
        "262-02".parse().unwrap(),
        "0678".parse().unwrap(),
        10,
        1,
        "ab",
    )
    .unwrap();
    assert_eq!(s10.to_string(), "suci-0-262-02-0678-a-1-ab");
    assert_eq!(s10.to_string().parse::<Suci>().unwrap(), s10);
    assert_eq!(
        "suci-0-262-02-0678-A-1-ab".parse::<Suci>().unwrap(),
        s10,
        "TS 29.503 admits upper-case hex"
    );

    for s in [
        "suci-0-262-02-0678-01-1-ab",         // two digits
        "suci-0-262-02-0678-+1-1-ab",         // a sign
        "suci-0-262-02-0678-10-1-ab",         // decimal ten
        "suci-0-262-02-0678-1-+1-ab",         // a signed key
        "suci-0-262-02-0678-1-01-ab",         // a padded key
        "suci-0-262-02-0678-1-0-ab",          // key 0 outside the null scheme
        "suci-0-262-02-0678-0-27-0123456789", // null scheme with key 27
        "suci-0-262-02-0678-1-27-ab-cd",      // trailing field
    ] {
        assert!(s.parse::<Suci>().is_err(), "{s:?} must not parse");
    }
    assert!(Suci::new("262-02".parse().unwrap(), "0".parse().unwrap(), 0, 27, "12").is_none());
    assert!(Suci::new("262-02".parse().unwrap(), "0".parse().unwrap(), 1, 0, "12").is_none());
    assert!(
        Suci::new(
            "262-02".parse().unwrap(),
            "0".parse().unwrap(),
            200,
            1,
            "12"
        )
        .is_none()
    );
}

/// A7: the fields are read through accessors, so no value bypasses `new`.
#[test]
fn a7_suci_fields_are_accessors() {
    let suci: Suci = "suci-0-262-02-0678-1-27-0072b7d4c1e8".parse().unwrap();
    assert_eq!(suci.home_network().to_string(), "262-02");
    assert_eq!(suci.routing_indicator().to_string(), "0678");
    assert_eq!(suci.protection_scheme(), 1);
    assert_eq!(suci.public_key_id(), 27);
    assert_eq!(suci.scheme_output(), "0072b7d4c1e8");
}

/// A8: a SUCI made of separators is refused without splitting it per `-`.
/// `audit-hardening/SR-003`
#[test]
fn a8_suci_with_many_separators_is_refused() {
    let many = format!("suci-0-262-02-0678-1-1-{}", "-".repeat(100_000));
    assert!(many.parse::<Suci>().is_err());
}

// ── A9 ───────────────────────────────────────────────────────────────────────

/// A9: an NAI is at most 253 octets (RFC 7542 §2.2).
/// `audit-hardening/SR-004`
#[test]
fn a9_nai_is_bounded() {
    let at_limit = format!("{}@realm", "a".repeat(253 - 6));
    assert_eq!(at_limit.len(), 253);
    assert!(at_limit.parse::<Nai>().is_ok());
    let over = format!("{}@realm", "a".repeat(253 - 5));
    assert!(over.parse::<Nai>().is_err());
    let huge = format!("{}@realm", "a".repeat(1_000_000));
    assert!(huge.parse::<Nai>().is_err());
}

// ── A10 / A11 ────────────────────────────────────────────────────────────────

/// A10: an international E.212 number is not an MSISDN.
/// `audit-hardening/SR-004`
#[test]
fn a10_to_msisdn_requires_the_isdn_plan() {
    let digits: DigitString = "262021234567890".parse().unwrap();
    let e212 = AddressString::new(
        NatureOfAddress::International,
        NumberingPlanIndicator::LandMobile,
        digits,
    );
    assert_eq!(e212.to_msisdn(), None);
    let e164 = AddressString::new(
        NatureOfAddress::International,
        NumberingPlanIndicator::Isdn,
        "491701234567".parse().unwrap(),
    );
    assert_eq!(e164.to_msisdn().unwrap().to_string(), "491701234567");
}

/// A11: a hand-built `Reserved` holding an assigned code is normalised, so
/// one wire octet has one value.
/// `audit-hardening/SR-004`
#[test]
fn a11_reserved_aliases_are_normalised() {
    let digits: DigitString = "49".parse().unwrap();
    let x = AddressString::new(
        NatureOfAddress::Reserved(1),
        NumberingPlanIndicator::Reserved(1),
        digits,
    );
    let y = AddressString::new(
        NatureOfAddress::International,
        NumberingPlanIndicator::Isdn,
        digits,
    );
    assert_eq!(x, y);
    assert_eq!(x.nature(), NatureOfAddress::International);
    assert_eq!(x.plan(), NumberingPlanIndicator::Isdn);

    // A genuinely reserved code stays reserved.
    let r = AddressString::new(
        NatureOfAddress::Reserved(5),
        NumberingPlanIndicator::Reserved(2),
        digits,
    );
    assert_eq!(r.nature(), NatureOfAddress::Reserved(5));
    assert_eq!(r.to_msisdn(), None);
}

/// A11: serde refuses `RESERVED_<n>` for an assigned or out-of-range code.
#[cfg(feature = "serde")]
#[test]
fn a11_serde_reserved_only_for_unassigned_codes() {
    assert!(serde_json::from_str::<NatureOfAddress>("\"RESERVED_1\"").is_err());
    assert!(serde_json::from_str::<NatureOfAddress>("\"RESERVED_13\"").is_err());
    assert_eq!(
        serde_json::from_str::<NatureOfAddress>("\"RESERVED_5\"").unwrap(),
        NatureOfAddress::Reserved(5)
    );
}

// ── A14 / A15 / A16 ──────────────────────────────────────────────────────────

/// A14: `KB` is 1 024 under JEDEC and 1 000 to an IEC reader, so it is refused.
/// `audit-hardening/SR-004`
#[test]
fn a14_kb_is_refused() {
    assert!("KB".parse::<ByteScale>().is_err());
    assert_eq!("kB".parse::<ByteScale>(), Ok(ByteScale::Kilo));
    assert_eq!("KiB".parse::<ByteScale>(), Ok(ByteScale::Kibi));
}

/// A15: the ICCID length is a hint; a 19-digit card may carry a valid check
/// digit.
#[test]
fn a15_iccid_length_is_only_a_hint() {
    let i: Iccid = "8944110068256270054".parse().unwrap();
    assert!(!i.has_check_digit());
    assert!(i.luhn_valid());
}

/// A16: separators are allowed only between digits; a leading `-` does not
/// vanish.
/// `audit-hardening/SR-004`
#[test]
fn a16_separators_only_between_digits() {
    assert!("-491701234567".parse::<Msisdn>().is_err());
    assert!("491701234567-".parse::<Msisdn>().is_err());
    assert!("--".parse::<DigitString>().is_err());
    assert!("-".parse::<DigitString>().is_err());
    assert_eq!(
        "49 170 1234-567".parse::<Msisdn>().unwrap().to_string(),
        "491701234567"
    );
    assert_eq!(
        " 491701234567 ".parse::<Msisdn>().unwrap().to_string(),
        "491701234567"
    );
    assert_eq!(
        "262-02-1234-5".parse::<Imsi>().unwrap().to_string(),
        "2620212345"
    );
}
