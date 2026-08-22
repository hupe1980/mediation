//! Reading a record as bytes.
//!
//! Decoding octets already in memory is a pure function — it reads no clock,
//! touches no socket, and given the same bytes returns the same value — so it
//! belongs in this crate. Fetching those octets does not.
//!
//! The suite exercises the encodings against the conventions a real record
//! mixes, because **one 3GPP record uses two different BCD orderings** and
//! reading either the wrong way round produces a well-formed, wrong value.

use mediation::{
    AddressString, CdrTimestamp, Century, ChargingCharacteristics, DigitString, Imei, Imsi, Msisdn,
    NatureOfAddress, NumberingPlan, QuarterHourOffset, SliceServiceType, Snssai,
};
use proptest::prelude::*;

// ── The two BCD conventions ──────────────────────────────────────────────────

/// The failure this suite exists for: the same octets, two conventions, two
/// well-formed and different answers.
#[test]
fn the_two_bcd_conventions_disagree_and_neither_reports_an_error() {
    let octets = [0x21, 0x43, 0x65];

    let tbcd = DigitString::from_tbcd(&octets).unwrap();
    let bcd = DigitString::from_bcd(&octets).unwrap();

    assert_eq!(tbcd.to_string(), "123456");
    assert_eq!(bcd.to_string(), "214365");
    assert_ne!(tbcd, bcd);
    // Both decoded cleanly, which is precisely why they are separate calls.
}

/// An even-length identity read with the wrong convention decodes **cleanly**
/// into a different subscriber's number. This is the dangerous case: nothing
/// anywhere reports an error.
#[test]
fn an_even_length_identity_misreads_silently() {
    let msisdn: Msisdn = "491701234567".parse().unwrap();
    let mut wire = [0u8; 8];
    let written = msisdn.to_tbcd(&mut wire).unwrap();
    assert_eq!(written, 6, "twelve digits need no filler");

    assert_eq!(Msisdn::from_tbcd(&wire[..written]).unwrap(), msisdn);

    let misread = DigitString::from_bcd(&wire[..written]).unwrap();
    assert_eq!(misread.to_string(), "947110325476");
    assert_ne!(misread.to_string(), msisdn.to_string());
}

/// An odd-length identity is luckier: the TBCD filler is not a decimal digit,
/// so a plain-BCD reader trips over it. Luck, not design — which is why the
/// two conventions are separate calls rather than one with a flag.
#[test]
fn an_odd_length_identity_happens_to_trip_the_wrong_reader() {
    let imsi: Imsi = "262021234567890".parse().unwrap();
    let mut wire = [0u8; 8];
    let written = imsi.to_tbcd(&mut wire).unwrap();
    assert_eq!(written, 8, "fifteen digits need a filler");

    assert_eq!(Imsi::from_tbcd(&wire[..written]).unwrap(), imsi);
    assert!(
        DigitString::from_bcd(&wire[..written]).is_err(),
        "the filler nibble is not a decimal digit"
    );
}

// ── TBCD identities ──────────────────────────────────────────────────────────

#[test]
fn identities_round_trip_through_the_wire() {
    let imsi: Imsi = "262021234567890".parse().unwrap();
    let msisdn: Msisdn = "491701234567".parse().unwrap();
    let imei: Imei = "490154203237518".parse().unwrap();

    let mut buffer = [0u8; 8];
    for (rendered, written) in [
        (imsi.to_string(), imsi.to_tbcd(&mut buffer).unwrap()),
        (
            msisdn.to_string(),
            msisdn.to_tbcd(&mut buffer.clone()).unwrap(),
        ),
    ] {
        assert_eq!(written, rendered.len().div_ceil(2));
    }

    let mut wire = [0u8; 8];
    let n = imsi.to_tbcd(&mut wire).unwrap();
    assert_eq!(Imsi::from_tbcd(&wire[..n]).unwrap(), imsi);

    let n = msisdn.to_tbcd(&mut wire).unwrap();
    assert_eq!(Msisdn::from_tbcd(&wire[..n]).unwrap(), msisdn);

    let n = imei.to_tbcd(&mut wire).unwrap();
    assert_eq!(Imei::from_tbcd(&wire[..n]).unwrap(), imei);
}

/// A checksum only some code paths apply is a checksum nobody can rely on: the
/// wire decoder verifies Luhn exactly as the string parser does.
#[test]
fn the_wire_decoder_applies_the_same_checksum_as_the_parser() {
    // 490154203237518 is valid; 490154203237512 is not.
    let good: Imei = "490154203237518".parse().unwrap();
    let mut wire = [0u8; 8];
    let n = good.to_tbcd(&mut wire).unwrap();
    assert!(Imei::from_tbcd(&wire[..n]).is_ok());

    // Corrupt the final digit on the wire and the decoder rejects it.
    let corrupted: DigitString = "490154203237512".parse().unwrap();
    let mut bad = [0u8; 8];
    let n = corrupted.to_tbcd(&mut bad).unwrap();
    assert!("490154203237512".parse::<Imei>().is_err());
    assert!(Imei::from_tbcd(&bad[..n]).is_err());
}

/// An out-of-range identity is rejected on the wire as it is in a string.
#[test]
fn the_wire_decoder_applies_the_same_length_and_range_rules() {
    // MCC 000 is unassigned, so this is not an IMSI however it arrived.
    let unassigned: DigitString = "000021234567890".parse().unwrap();
    let mut wire = [0u8; 8];
    let n = unassigned.to_tbcd(&mut wire).unwrap();
    assert!(Imsi::from_tbcd(&wire[..n]).is_err());

    // Sixteen digits is not an MSISDN.
    let too_long: DigitString = "1234567890123456".parse().unwrap();
    let n = too_long.to_tbcd(&mut wire).unwrap();
    assert!(Msisdn::from_tbcd(&wire[..n]).is_err());
}

proptest! {
    /// Every identity this crate accepts survives a wire round trip unchanged.
    #[test]
    fn every_imsi_survives_the_wire(rest in "[0-9]{12}") {
        let imsi: Imsi = format!("262{rest}").parse().unwrap();
        let mut wire = [0u8; 8];
        let written = imsi.to_tbcd(&mut wire).unwrap();
        prop_assert_eq!(written, 8, "15 digits pack into 8 octets with a filler");
        prop_assert_eq!(Imsi::from_tbcd(&wire[..written]).unwrap(), imsi);
    }

    #[test]
    fn every_msisdn_survives_the_wire(digits in "[0-9]{1,15}") {
        let msisdn: Msisdn = digits.parse().unwrap();
        let mut wire = [0u8; 8];
        let written = msisdn.to_tbcd(&mut wire).unwrap();
        prop_assert_eq!(Msisdn::from_tbcd(&wire[..written]).unwrap(), msisdn);
    }
}

// ── The BCD timestamp ────────────────────────────────────────────────────────

/// The nine-octet `TimeStamp` of 3GPP TS 32.298, hand-checked against its
/// layout: six plain-BCD octets, an ASCII sign, two plain-BCD octets.
#[test]
fn the_timestamp_layout_matches_the_specification() {
    let octets = [0x26, 0x07, 0x28, 0x10, 0x30, 0x45, b'+', 0x02, 0x00];
    let ts = CdrTimestamp::from_bcd(&octets, Century::TWENTY_FIRST).unwrap();

    assert_eq!(ts.to_string(), "2026-07-28T10:30:45+02:00");
    assert_eq!(ts.local_date().to_string(), "2026-07-28");
    assert_eq!(ts.offset(), QuarterHourOffset::from_minutes(120).unwrap());
    assert_eq!(ts.to_bcd(Century::TWENTY_FIRST).unwrap(), octets);
}

/// Every quarter-hour zone the crate models survives the encoding, including
/// the negative ones where the sign octet matters.
#[test]
fn every_quarter_hour_zone_survives_the_timestamp_encoding() {
    for minutes in (-720i16..=840).step_by(15) {
        let offset = QuarterHourOffset::from_minutes(minutes).unwrap();
        let local = "2026-07-28T10:30:45".to_owned() + &offset.to_string();
        let ts: CdrTimestamp = local.parse().unwrap();

        let octets = ts.to_bcd(Century::TWENTY_FIRST).unwrap();
        assert_eq!(
            octets[6],
            if minutes < 0 { b'-' } else { b'+' },
            "sign octet for {offset}"
        );
        assert_eq!(
            CdrTimestamp::from_bcd(&octets, Century::TWENTY_FIRST).unwrap(),
            ts,
            "round trip for {offset}"
        );
    }
}

/// There is no correct default century: a record from 1999 and one from 2099
/// are indistinguishable on the wire.
#[test]
fn the_century_is_the_callers_decision() {
    let octets = [0x99, 0x12, 0x31, 0x23, 0x59, 0x59, b'+', 0x00, 0x00];

    let twentieth = CdrTimestamp::from_bcd(&octets, Century::new(1900).unwrap()).unwrap();
    let twenty_first = CdrTimestamp::from_bcd(&octets, Century::TWENTY_FIRST).unwrap();

    assert_eq!(twentieth.local_date().year(), 1999);
    assert_eq!(twenty_first.local_date().year(), 2099);
    // Both encode back to the same nine octets, which is the whole problem —
    // and why the encoder makes the caller name the century too.
    assert_eq!(
        twentieth.to_bcd(Century::new(1900).unwrap()).unwrap(),
        twenty_first.to_bcd(Century::TWENTY_FIRST).unwrap()
    );
    // A timestamp cannot be written into a century it does not belong to.
    assert!(twentieth.to_bcd(Century::TWENTY_FIRST).is_err());
}

/// A decode error names the octet that failed, because a caller holding a
/// binary record cannot search it for a rejected string.
#[test]
fn a_decode_error_names_the_octet() {
    let bad_sign = [0x26, 0x07, 0x28, 0x10, 0x00, 0x00, b'*', 0x02, 0x00];
    let err = CdrTimestamp::from_bcd(&bad_sign, Century::TWENTY_FIRST).unwrap_err();
    assert_eq!(err.at(), 6);
    assert!(err.to_string().contains("octet 6"), "{err}");

    let odd_offset = [0x26, 0x07, 0x28, 0x10, 0x00, 0x00, b'+', 0x02, 0x10];
    let err = CdrTimestamp::from_bcd(&odd_offset, Century::TWENTY_FIRST).unwrap_err();
    assert_eq!(err.at(), 7);
}

proptest! {
    /// Any well-formed local time and assigned offset survives the encoding.
    #[test]
    fn any_timestamp_survives_the_encoding(
        year in 0u8..=99,
        month in 1u8..=12,
        day in 1u8..=28,
        hour in 0u8..=23,
        minute in 0u8..=59,
        second in 0u8..=59,
        quarters in -48i8..=56,
    ) {
        let offset = QuarterHourOffset::new(quarters).unwrap();
        let text = format!(
            "20{year:02}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}{offset}"
        );
        let ts: CdrTimestamp = text.parse().unwrap();

        let octets = ts.to_bcd(Century::TWENTY_FIRST).unwrap();
        prop_assert_eq!(octets.len(), CdrTimestamp::BCD_OCTETS);
        prop_assert_eq!(
            CdrTimestamp::from_bcd(&octets, Century::TWENTY_FIRST).unwrap(),
            ts
        );
    }
}

// ── Slice identities ─────────────────────────────────────────────────────────

/// One meaning, one encoding: a slice with no differentiator is written short,
/// never with the reserved value.
#[test]
fn a_slice_identity_has_one_encoding_per_meaning() {
    let bare = Snssai::new(SliceServiceType::EMBB);

    let mut wire = [0u8; 4];
    assert_eq!(bare.to_bytes(&mut wire).unwrap(), 1);

    // Both wire forms decode to the same slice …
    assert_eq!(Snssai::from_bytes(&[0x01]).unwrap(), bare);
    assert_eq!(Snssai::from_bytes(&[0x01, 0xFF, 0xFF, 0xFF]).unwrap(), bare);
    // … and only the short one is ever written.
    assert_eq!(&wire[..1], &[0x01]);
}

/// A differentiator of zero is a real slice, distinct from having none.
#[test]
fn a_zero_differentiator_is_not_an_absent_one() {
    let zero = Snssai::from_bytes(&[0x02, 0x00, 0x00, 0x00]).unwrap();
    let none = Snssai::from_bytes(&[0x02]).unwrap();

    assert_ne!(zero, none);
    assert_eq!(zero.sd(), Some(0));
    assert_eq!(none.sd(), None);

    let mut wire = [0u8; 4];
    assert_eq!(zero.to_bytes(&mut wire).unwrap(), 4, "zero is written long");
}

proptest! {
    /// Every representable slice identity survives a wire round trip.
    #[test]
    fn every_slice_identity_survives_the_wire(sst in any::<u8>(), sd in 0u32..0x00FF_FFFF) {
        let slice = Snssai::with_sd(SliceServiceType::new(sst), sd).unwrap();
        let mut wire = [0u8; 4];
        let written = slice.to_bytes(&mut wire).unwrap();
        prop_assert_eq!(written, 4);
        prop_assert_eq!(Snssai::from_bytes(&wire[..written]).unwrap(), slice);

        // …and through its string form too.
        prop_assert_eq!(slice.to_string().parse::<Snssai>().unwrap(), slice);
    }
}

// ── AddressString ────────────────────────────────────────────────────────────

/// The single most common decoding error in this area, and the one that is
/// invisible without a reference decode: a `servedMSISDN` is an
/// `ISDN-AddressString`, whose first octet is a qualifier rather than a digit
/// pair. Reading it as TBCD prepends two digits and produces a valid-looking
/// number belonging to somebody else.
#[test]
fn a_served_msisdn_is_not_a_bare_tbcd_field() {
    // 0x91: extension bit set, international, E.164 — then 491701234567.
    let field = [0x91, 0x94, 0x71, 0x10, 0x32, 0x54, 0x76];

    let address = AddressString::from_bytes(&field).unwrap();
    assert_eq!(address.nature(), NatureOfAddress::International);
    assert_eq!(address.plan(), NumberingPlan::Isdn);
    assert_eq!(address.to_msisdn().unwrap().to_string(), "491701234567");

    // The wrong reading parses, validates and bills the wrong subscriber.
    let wrong = Msisdn::from_tbcd(&field).unwrap();
    assert_eq!(wrong.to_string(), "19491701234567");
    assert_ne!(wrong, address.to_msisdn().unwrap());
}

/// An MSISDN is international by definition. Turning a national number into one
/// needs the operator's dialling plan, and prefixing a country code by
/// assumption settles the call against the wrong partner.
#[test]
fn only_an_international_address_becomes_an_msisdn() {
    let national = [0xA1, 0x03, 0x21, 0x43, 0x65];
    let address = AddressString::from_bytes(&national).unwrap();
    assert_eq!(address.nature(), NatureOfAddress::NationalSignificant);
    assert_eq!(address.to_msisdn(), None);
    // The digits are still available to a caller who does hold a dialling plan.
    assert_eq!(address.digits().to_string(), "30123456");
}

#[test]
fn an_address_string_round_trips() {
    let field = [0x91, 0x94, 0x71, 0x10, 0x32, 0x54, 0x76];
    let address = AddressString::from_bytes(&field).unwrap();

    let mut out = [0u8; 16];
    let written = address.to_bytes(&mut out).unwrap();
    assert_eq!(&out[..written], &field);
    assert_eq!(address.to_string(), "INTERNATIONAL/ISDN:491701234567");
}

/// TS 29.002 fixes the extension bit at 1. A zero there means the octets are
/// not an `AddressString` at all — most often, that a decoder is one field out.
#[test]
fn a_qualifier_without_the_extension_bit_is_refused() {
    assert!(AddressString::from_bytes(&[0x11, 0x94, 0x71]).is_err());
    assert!(AddressString::from_bytes(&[]).is_err());
}

/// Sixteen operator-defined behaviours, and the crate reads none of them.
/// TS 32.251 §A.1: the bits are "freely defined by Operators", so `0x0800` is
/// prepaid at one operator and a lab profile at the next.
#[test]
fn charging_characteristics_carry_the_bits_and_interpret_nothing() {
    let profile = ChargingCharacteristics::from_bits(0x0800);

    assert_eq!(profile.to_bytes(), [0x08, 0x00]);
    assert_eq!(
        ChargingCharacteristics::from_bytes(&[0x08, 0x00]).unwrap(),
        profile
    );
    assert_eq!(profile.to_string(), "0800");

    // Behaviour 11, and nothing else.
    assert!(profile.behaviour(11));
    assert_eq!(profile.count(), 1);
    assert_eq!(profile.set_behaviours().collect::<Vec<_>>(), vec![11]);
    // An index past the field is not selected, and does not panic.
    assert!(!profile.behaviour(16));
    assert!(!profile.behaviour(255));

    // Nothing selected is not the same as no field at all.
    assert!(ChargingCharacteristics::NONE.is_empty());
    assert_eq!(ChargingCharacteristics::NONE.to_string(), "0000");

    // Every value round-trips through both forms.
    for bits in [0x0000u16, 0x0001, 0x8000, 0xFFFF, 0x1234] {
        let profile = ChargingCharacteristics::from_bits(bits);
        assert_eq!(
            ChargingCharacteristics::from_bytes(&profile.to_bytes()).unwrap(),
            profile
        );
        assert_eq!(
            profile
                .to_string()
                .parse::<ChargingCharacteristics>()
                .unwrap(),
            profile
        );
    }

    // Any length but two is refused rather than padded.
    assert!(ChargingCharacteristics::from_bytes(&[0x08]).is_err());
    assert!(ChargingCharacteristics::from_bytes(&[0x08, 0x00, 0x00]).is_err());
}
