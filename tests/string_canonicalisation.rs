//! One value, one string.
//!
//! Every type with a string form has exactly one of them, shared by `as_str`,
//! `Display`, `FromStr` and the serde tag. The rule is worth pinning because
//! the alternative fails silently: a value with two spellings produces two
//! database keys, two map entries and two "distinct" rows that mean the same
//! thing, and nothing anywhere reports an error.
//!
//! Four properties, checked over generated input:
//!
//! - **stability** — `parse(v.to_string()) == v` for every value
//! - **idempotence** — `parse(s).to_string()` is a fixed point for canonical `s`
//! - **totality** — every enum variant has a code and every code parses back
//! - **injectivity** — distinct values never share a string

use mediation::{
    ByteScale, CdrTimestamp, DigitString, DstAdjustment, Iccid, Imei, Imeisv, Imsi, Mcc, Mnc,
    MsTimeZone, Msisdn, NodeId, Octets, PartyId, PlmnId, QuarterHourOffset, RecordType, Seconds,
    ServiceDomain, TadigCode, UnattributableReason, VolumePair,
};
use proptest::prelude::*;
use std::collections::HashSet;
use std::fmt::Debug;
use std::fmt::Display;
use std::str::FromStr;

/// `parse(v.to_string()) == v`, and the string is a fixed point.
fn assert_round_trips<T>(value: &T)
where
    T: Display + FromStr + PartialEq + Debug,
    <T as FromStr>::Err: Debug,
{
    let s = value.to_string();
    let parsed = T::from_str(&s).expect("canonical string must parse");
    assert_eq!(&parsed, value, "round trip changed the value: {s:?}");
    assert_eq!(parsed.to_string(), s, "writing is not idempotent: {s:?}");
}

/// Every variant renders to its declared code, and every code parses back.
fn assert_codes_total<T>(all: &[T], codes: &[&str])
where
    T: Display + FromStr + PartialEq + Debug + Copy,
    <T as FromStr>::Err: Debug,
{
    assert_eq!(all.len(), codes.len(), "ALL and CODES disagree in length");
    let mut seen = HashSet::new();
    for (value, code) in all.iter().zip(codes) {
        assert_eq!(&value.to_string(), code);
        assert_eq!(&T::from_str(code).expect("code must parse"), value);
        assert!(seen.insert(*code), "duplicate code {code:?}");
    }
}

// ── Enumerated vocabularies ──────────────────────────────────────────────────

#[test]
fn every_enum_code_is_total_and_distinct() {
    assert_codes_total(ServiceDomain::ALL, ServiceDomain::CODES);
    assert_codes_total(RecordType::ALL, RecordType::CODES);
    assert_codes_total(DstAdjustment::ALL, DstAdjustment::CODES);
    assert_codes_total(UnattributableReason::ALL, UnattributableReason::CODES);
    assert_codes_total(ByteScale::ALL, ByteScale::CODES);
}

#[test]
fn unrecognised_input_is_an_error_and_never_a_silent_fallback() {
    assert!("TELEPATHY".parse::<ServiceDomain>().is_err());
    assert!("".parse::<RecordType>().is_err());
    assert!("PLUS_THREE_HOURS".parse::<DstAdjustment>().is_err());
    assert!("UNKNOWN".parse::<UnattributableReason>().is_err());
    assert!("PB".parse::<ByteScale>().is_err());
}

// ── Generated values ─────────────────────────────────────────────────────────

/// Any assigned UTC offset.
fn any_offset() -> impl Strategy<Value = QuarterHourOffset> {
    (-48i8..=56).prop_map(|q| QuarterHourOffset::new(q).expect("assigned"))
}

/// Any daylight-saving adjustment.
fn any_dst() -> impl Strategy<Value = DstAdjustment> {
    prop_oneof![
        Just(DstAdjustment::None),
        Just(DstAdjustment::PlusOneHour),
        Just(DstAdjustment::PlusTwoHours),
    ]
}

/// Any well-formed charging timestamp.
fn any_timestamp() -> impl Strategy<Value = CdrTimestamp> {
    (
        1970i32..=2200,
        1u8..=12,
        1u8..=28,
        0u8..=23,
        0u8..=59,
        0u8..=59,
        any_offset(),
    )
        .prop_map(|(y, m, d, hh, mm, ss, offset)| {
            let date = time::Date::from_calendar_date(y, time::Month::try_from(m).unwrap(), d)
                .expect("valid date");
            let clock = time::Time::from_hms(hh, mm, ss).expect("valid time");
            CdrTimestamp::new(time::PrimitiveDateTime::new(date, clock), offset)
        })
}

/// A digit string of exactly `n` characters.
fn digits(n: usize) -> impl Strategy<Value = String> {
    digits_between(n, n)
}

/// A digit string of `min..=max` characters.
fn digits_between(min: usize, max: usize) -> impl Strategy<Value = String> {
    proptest::collection::vec(0u8..=9, min..=max).prop_map(|ds| {
        ds.into_iter()
            .map(|d| char::from(b'0' + d))
            .collect::<String>()
    })
}

proptest! {
    #[test]
    fn offsets_round_trip(offset in any_offset()) {
        assert_round_trips(&offset);
    }

    /// The semi-octet form is the wire encoding; it must survive a round trip
    /// for every value the wire can hold, not merely the assigned ones.
    #[test]
    fn offsets_round_trip_through_the_wire_encoding(q in -79i8..=79) {
        let offset = QuarterHourOffset::from_encoded(q).expect("encodable");
        let decoded = QuarterHourOffset::from_semi_octet(offset.to_semi_octet());
        prop_assert_eq!(decoded, Some(offset));
    }

    #[test]
    fn zones_round_trip(offset in any_offset(), dst in any_dst()) {
        assert_round_trips(&MsTimeZone::new(offset, dst));
    }

    #[test]
    fn timestamps_round_trip(ts in any_timestamp()) {
        assert_round_trips(&ts);
    }

    /// Re-expressing a timestamp in another offset preserves the instant and
    /// only the instant — the wall clock is expected to move.
    #[test]
    fn re_expressing_a_timestamp_preserves_its_instant(
        ts in any_timestamp(),
        target in any_offset(),
    ) {
        let moved = ts.to_offset(target);
        prop_assert_eq!(moved.to_utc(), ts.to_utc());
        prop_assert_eq!(moved.offset(), target);
    }

    #[test]
    fn volumes_round_trip(up in any::<u64>(), down in any::<u64>()) {
        assert_round_trips(&Octets::new(up));
        assert_round_trips(&VolumePair::new(Octets::new(up), Octets::new(down)));
    }

    #[test]
    fn durations_round_trip(s in any::<u32>()) {
        assert_round_trips(&Seconds::new(s));
    }

    #[test]
    fn network_identities_round_trip(mcc in 1u16..=999, mnc in 0u16..=99, wide in any::<bool>()) {
        let mcc = Mcc::new(mcc).expect("assigned");
        let mnc = Mnc::new(mnc, if wide { 3 } else { 2 }).expect("fits");
        assert_round_trips(&mcc);
        assert_round_trips(&mnc);
        assert_round_trips(&PlmnId::new(mcc, mnc));
    }

    /// A leading zero carries information, so two MNCs that differ only in
    /// declared width must never share a string.
    #[test]
    fn mnc_width_is_injective(value in 0u16..=99) {
        let two = Mnc::new(value, 2).expect("fits");
        let three = Mnc::new(value, 3).expect("fits");
        prop_assert_ne!(two, three);
        prop_assert_ne!(two.to_string(), three.to_string());
    }

    #[test]
    fn subscriber_identities_round_trip(rest in digits(12)) {
        let imsi: Imsi = format!("262{rest}").parse().expect("valid IMSI");
        assert_round_trips(&imsi);
    }

    #[test]
    fn msisdns_round_trip(raw in digits_between(1, 15)) {
        let msisdn: Msisdn = raw.parse().expect("valid MSISDN");
        assert_round_trips(&msisdn);
        // The `+` form is accepted on the way in and never written out.
        prop_assert_eq!(msisdn.to_international().parse::<Msisdn>().unwrap(), msisdn);
    }

    /// Every IMEISV yields a valid IMEI, and both forms round-trip.
    #[test]
    fn equipment_identities_round_trip(body in digits(16)) {
        let sv: Imeisv = body.parse().expect("16 digits is always a valid IMEISV");
        assert_round_trips(&sv);
        assert_round_trips(&sv.to_imei());
        prop_assert_eq!(sv.tac(), sv.to_imei().tac());
    }

    /// The computed check digit is what makes the derived IMEI parse at all.
    #[test]
    fn a_derived_imei_always_validates(body in digits(16)) {
        let sv: Imeisv = body.parse().unwrap();
        let derived = sv.to_imei().to_string();
        prop_assert!(derived.parse::<Imei>().is_ok(), "{derived} must validate");
    }

    #[test]
    fn card_identifiers_round_trip(rest in digits(17)) {
        let iccid: Iccid = format!("89{rest}").parse().expect("valid ICCID");
        assert_round_trips(&iccid);
        prop_assert!(!iccid.has_check_digit());
    }

    #[test]
    fn opaque_identifiers_round_trip(n in 1usize..=64) {
        let id = "a".repeat(n);
        assert_round_trips(&id.parse::<PartyId>().unwrap());
        assert_round_trips(&id.parse::<NodeId>().unwrap());
    }

    #[test]
    fn tadig_codes_round_trip(body in "[A-Z]{3}[A-Z0-9]{2}") {
        let code: TadigCode = body.parse().expect("valid TADIG");
        assert_round_trips(&code);
        prop_assert_eq!(code.country().len(), 3);
        prop_assert_eq!(code.operator().len(), 2);
    }
}

// ── Injectivity ──────────────────────────────────────────────────────────────

/// Distinct values must never share a string, or two rows that mean different
/// things collapse into one key.
#[test]
fn distinct_offsets_have_distinct_strings() {
    let mut seen = HashSet::new();
    for q in -48i8..=56 {
        let offset = QuarterHourOffset::new(q).unwrap();
        assert!(seen.insert(offset.to_string()), "collision at {q}");
    }
    assert_eq!(seen.len(), 105);
}

#[test]
fn distinct_zones_have_distinct_strings() {
    let mut seen = HashSet::new();
    for q in -48i8..=56 {
        for dst in DstAdjustment::ALL {
            let zone = MsTimeZone::new(QuarterHourOffset::new(q).unwrap(), *dst);
            assert!(seen.insert(zone.to_string()), "collision at {q}/{dst}");
        }
    }
    assert_eq!(seen.len(), 105 * 3);
}

/// Lenient in, canonical out: several accepted spellings, one written form.
#[test]
fn every_accepted_spelling_maps_onto_one_canonical_form() {
    for raw in ["+02:00", "+0200", "+02", " +02:00 "] {
        assert_eq!(
            raw.parse::<QuarterHourOffset>().unwrap().to_string(),
            "+02:00"
        );
    }
    for raw in [
        "491701234567",
        "+491701234567",
        "49-170-1234567",
        " 491701234567 ",
    ] {
        assert_eq!(raw.parse::<Msisdn>().unwrap().to_string(), "491701234567");
    }
    for raw in ["deud1", "DEUD1", " DeUd1 "] {
        assert_eq!(raw.parse::<TadigCode>().unwrap().to_string(), "DEUD1");
    }
    for raw in ["data", "DATA", " Data "] {
        assert_eq!(raw.parse::<ServiceDomain>().unwrap().to_string(), "DATA");
    }
}

// ── Ordering ─────────────────────────────────────────────────────────────────

/// Digits are packed two per byte with the *earlier* digit in the low nibble,
/// so a derived `Ord` compares the second digit before the first: `"19"` sorts
/// after `"21"`. Every prefix table, sorted report and `BTreeMap` keyed on an
/// identifier would then be in an order no reader could predict.
#[test]
fn digit_runs_sort_the_way_their_strings_do() {
    let mut runs: Vec<DigitString> = ["21", "19", "123", "12", "9", "1", "100", "0", "00"]
        .iter()
        .map(|s| s.parse().unwrap())
        .collect();
    runs.sort();

    let rendered: Vec<String> = runs.iter().map(ToString::to_string).collect();
    assert_eq!(
        rendered,
        ["0", "00", "1", "100", "12", "123", "19", "21", "9"]
    );

    // The same order a `str` comparison gives, over every pair.
    for a in &runs {
        for b in &runs {
            assert_eq!(
                Ord::cmp(a, b),
                a.to_string().cmp(&b.to_string()),
                "{a} vs {b}"
            );
        }
    }
}

/// The identifiers built on `DigitString` inherit the ordering, so a sorted
/// subscriber report reads the way a human expects.
#[test]
fn identifiers_sort_by_their_digits() {
    let mut numbers: Vec<Msisdn> = ["4919", "4921", "49123"]
        .iter()
        .map(|s| s.parse().unwrap())
        .collect();
    numbers.sort();
    assert_eq!(
        numbers.iter().map(ToString::to_string).collect::<Vec<_>>(),
        ["49123", "4919", "4921"]
    );
}

/// A prefix is not a shorter number that happens to compare small; the test a
/// longest-prefix lookup actually needs.
#[test]
fn a_prefix_test_is_not_an_ordering_test() {
    let number: DigitString = "49151234".parse().unwrap();
    assert!(number.starts_with("49151".parse().unwrap()));
    assert!(!number.starts_with("49152".parse().unwrap()));
    // A prefix longer than the run is not a prefix of it.
    assert!(!number.starts_with("491512345".parse().unwrap()));
    assert_eq!(number.prefix(5).to_string(), "49151");
    assert_eq!(number.prefix(99), DigitString::EMPTY);
}

/// IEC 80000-13 gives `B` to the byte and `b` to the bit, so `kb` is a kilobit.
/// Accepting it as a kilobyte is a factor-of-eight error in exactly the
/// direction this crate exists to prevent — a silent unit confusion producing a
/// plausible number.
#[test]
fn a_bit_spelling_is_refused_rather_than_helpfully_coerced() {
    for bits in ["b", "kb", "mb", "gb", "tb", "Kb", "Mb"] {
        assert!(
            bits.parse::<ByteScale>().is_err(),
            "{bits:?} is a bit, not a byte"
        );
    }

    // The byte spellings still parse, including the ubiquitous capital `KB`.
    for (text, expected) in [
        ("B", ByteScale::Byte),
        ("bytes", ByteScale::Byte),
        ("kB", ByteScale::Kilo),
        ("KB", ByteScale::Kilo),
        ("MB", ByteScale::Mega),
    ] {
        assert_eq!(text.parse::<ByteScale>().unwrap(), expected);
    }

    // The binary prefixes carry an `i`, which disambiguates them on its own.
    for text in ["KiB", "kib", "KIB", "MiB", "GiB", "TiB"] {
        assert!(text.parse::<ByteScale>().unwrap().is_binary(), "{text}");
    }

    // And the two kilobytes are still a factor of 1.024 apart, not the same.
    assert_ne!(
        Octets::from_scaled(5, ByteScale::Kilo),
        Octets::from_scaled(5, ByteScale::Kibi)
    );
}
