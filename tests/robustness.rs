//! No input, however malformed, may panic a parser or a decoder.
//!
//! A mediation layer reads records that arrived over a network from a partner's
//! equipment. Every byte and every character of them is untrusted, so a decoder
//! that aborts the process on a malformed field is not a validation failure —
//! it is a denial of service with a partner's file as the payload.
//!
//! These tests are deliberately dumb: throw arbitrary strings at every
//! `FromStr` and arbitrary bytes at every wire decoder, and require a `Result`
//! rather than a crash. They have already caught three real panics — a UTF-8
//! boundary in the offset parser, another in the timestamp parser, and a stack
//! overflow on nested indefinite BER lengths.

use mediation::*;
use proptest::prelude::*;

/// Every `FromStr` in the public API, hammered with one string.
macro_rules! every_parser {
    ($($t:ty),* $(,)?) => {
        fn parse_everything(s: &str) {
            $( let _ = s.parse::<$t>(); )*
        }
    };
}

every_parser!(
    AreaCode,
    ByteScale,
    CallDirection,
    CallId,
    Destination,
    DestinationClass,
    Itemisation,
    ShortCode,
    ShortenedNumber,
    CdrTimestamp,
    ChangeCondition,
    ChargingId,
    Cgi,
    DigitString,
    Dnn,
    DstAdjustment,
    Ecgi,
    EnbKind,
    ExtendedEnbKind,
    GnbIdLength,
    Gpsi,
    Icid,
    Iccid,
    Imei,
    Imeisv,
    Imsi,
    Lai,
    Mcc,
    Mnc,
    MsTimeZone,
    Msisdn,
    Nai,
    Ncgi,
    NfInstanceId,
    NodeId,
    NumberRange,
    Octets,
    Outcome,
    PacketSessionId,
    PartialFormat,
    PartyId,
    Pei,
    PlmnId,
    Qos5qi,
    QualityGrade,
    QuarterHourOffset,
    Rai,
    RatType,
    RatingGroup,
    RecordClosureCause,
    RecordType,
    RestateReason,
    RoutingIndicator,
    Sai,
    Seconds,
    ServiceDomain,
    Snssai,
    Suci,
    Supi,
    TacWidth,
    TadigCode,
    Tai,
    TenantRef,
    UnattributableReason,
    UserLocation,
    VolumePair,
);

/// Every decoder that reads bytes, hammered with one slice.
fn decode_everything(bytes: &[u8]) {
    let _ = DigitString::from_tbcd(bytes);
    let _ = DigitString::from_bcd(bytes);
    let _ = CdrTimestamp::from_bcd(bytes, Century::TWENTY_FIRST);
    let _ = Imsi::from_tbcd(bytes);
    let _ = Msisdn::from_tbcd(bytes);
    let _ = Imei::from_tbcd(bytes);
    let _ = AddressString::from_bytes(bytes);
    let _ = PlmnId::from_bcd(bytes);
    let _ = Snssai::from_bytes(bytes);
    let _ = Cgi::from_bytes(bytes);
    let _ = Sai::from_bytes(bytes);
    let _ = Rai::from_bytes(bytes);
    let _ = Lai::from_bytes(bytes);
    let _ = Tai::from_bytes(bytes);
    let _ = Ecgi::from_bytes(bytes);
    let _ = Ncgi::from_bytes(bytes);
    let _ = MacroEnbId::from_bytes(bytes);
    let _ = ExtendedMacroEnbId::from_bytes(bytes);
    let _ = Uli::decode(bytes);

    // The TAP timestamp reads two text fields rather than octets, and the same
    // rule applies: arbitrary input, a `Result`, never a panic.
    if let Ok(text) = std::str::from_utf8(bytes) {
        let _ = CdrTimestamp::from_tap(text, text);
        for split in 0..=text.len() {
            if text.is_char_boundary(split) {
                let (local, offset) = text.split_at(split);
                let _ = CdrTimestamp::from_tap(local, offset);
            }
        }
    }

    // Walk the BER framing, bounded, exactly as a caller would.
    for element in BerReader::new(bytes) {
        let Ok(element) = element else { break };
        let _ = element.as_u64();
        let _ = element.validate(32);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4_000))]

    /// Arbitrary text, including multi-byte characters at every offset.
    #[test]
    fn no_parser_panics_on_arbitrary_text(s in "\\PC{0,32}") {
        parse_everything(&s);
    }

    /// Text shaped like the values these types accept, so the generator
    /// actually reaches the code past the first length check.
    #[test]
    fn no_parser_panics_on_near_miss_text(
        s in "[-+0-9a-fA-FzZ:./ _\u{e9}\u{4e2d}\u{10000}]{0,32}"
    ) {
        parse_everything(&s);
    }

    /// A multi-byte character dropped into an otherwise valid timestamp, at
    /// every offset in turn. This is the exact shape that panicked.
    #[test]
    fn no_parser_panics_on_a_corrupted_timestamp(at in 0usize..26) {
        let canonical = "2026-07-28T10:00:00+02:00";
        let mut corrupted = String::new();
        for (index, ch) in canonical.chars().enumerate() {
            if index == at { corrupted.push('\u{e9}'); } else { corrupted.push(ch); }
        }
        parse_everything(&corrupted);
    }

    #[test]
    fn no_decoder_panics_on_arbitrary_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
        decode_everything(&bytes);
    }

    /// Bytes drawn from the small alphabet BER headers use, so the generator
    /// produces structures rather than noise the first read rejects.
    #[test]
    fn no_decoder_panics_on_ber_shaped_bytes(
        bytes in proptest::collection::vec(
            prop_oneof![Just(0x00u8), Just(0x02), Just(0x30), Just(0x80), Just(0xA0), Just(0xFF), any::<u8>()],
            0..96,
        )
    ) {
        decode_everything(&bytes);
    }
}

/// Dial strings, Call-IDs and itemisation (fixed-line-voice/SR-003, SC-003): up to
/// 64 KiB, ten thousand cases, a value or a typed refusal — never a panic.
fn dial_everything(s: &str) {
    for context in [
        DialContext::germany(),
        DialContext::germany().with_area_code("30").unwrap(),
        DialContext::nanp(),
        DialContext::italy(),
        DialContext::new(999, "9").unwrap(),
    ] {
        if let Ok(destination) = context.normalise(s) {
            let _ = destination.class(&GermanNumberingPlan);
            for choice in Itemisation::ALL {
                let _ = itemise_called(&destination, *choice, &[destination]);
                let _ = itemise_called(&destination, *choice, &[] as &[Destination]);
            }
            if let Some(number) = destination.as_number() {
                let _ = itemise_calling(number);
                for hide in 0..20 {
                    let _ = NumberPrefix::without_last(number, hide);
                }
            }
        }
    }
    let _ = s.parse::<CallId>();
    let _ = s.parse::<Destination>();
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(10_000))]

    #[test]
    fn no_dial_string_panics(s in "[-+0-9 ()/.*#a-z\u{ff10}-\u{ff19}]{0,80}") {
        dial_everything(&s);
    }

    #[test]
    fn no_arbitrary_text_panics_the_dial_plan(s in "\\PC{0,80}") {
        dial_everything(&s);
    }
}

#[test]
fn huge_input_is_refused_in_constant_work() {
    let huge = "9".repeat(64 * 1024);
    assert_eq!(
        DialContext::germany().normalise(&huge),
        Err(DialError::TooLong)
    );
    assert!(huge.parse::<CallId>().is_err());
    assert!(huge.parse::<Destination>().is_err());
}

/// The three panics these tests were written for, kept as named regressions so
/// a rewrite cannot quietly reintroduce one.
#[test]
fn the_known_panics_stay_fixed() {
    // A multi-byte character on the boundary the offset parser sliced at.
    assert!("+1\u{e9}3".parse::<QuarterHourOffset>().is_err());
    assert!(
        "202\u{e9}-7-28T06:00:00+02:00"
            .parse::<CdrTimestamp>()
            .is_err()
    );
    assert!("+0\u{e9}:00/1".parse::<MsTimeZone>().is_err());

    // Indefinite lengths nested deeply enough to exhaust a recursive scanner.
    let mut nested = Vec::new();
    for _ in 0..100_000 {
        nested.extend_from_slice(&[0x30, 0x80]);
    }
    for _ in 0..100_000 {
        nested.extend_from_slice(&[0x00, 0x00]);
    }
    assert!(BerReader::new(&nested).next().unwrap().is_err());
}

/// A corrupt partial-record sequence number must not ask for a list of holes
/// proportional to the number it claims.
#[test]
fn a_corrupt_partial_sequence_does_not_allocate_the_whole_range() {
    let run = PartialRun::from_parts(vec![
        PartialInfo::new(
            1,
            PartialFormat::FullyQualified,
            RecordClosureCause::VolumeLimit,
        ),
        PartialInfo::new(
            u32::MAX,
            PartialFormat::Reduced,
            RecordClosureCause::NormalRelease,
        ),
    ]);

    let assessment = run.assess();
    // The count is exact …
    assert_eq!(assessment.missing_count(), u64::from(u32::MAX) - 2);
    // … and the list of identities is not.
    assert_eq!(assessment.missing().len(), PartialRun::MAX_REPORTED_MISSING);
    assert!(assessment.missing_is_truncated());
}

/// The two-field TAP timestamp reads text at fixed offsets, which is the shape
/// that panicked twice already in the RFC 3339 parser.
#[test]
fn the_tap_timestamp_survives_a_corrupted_field() {
    let canonical = "20260728103045";
    for at in 0..canonical.chars().count() {
        let corrupted: String = canonical
            .chars()
            .enumerate()
            .map(|(index, ch)| if index == at { '\u{e9}' } else { ch })
            .collect();
        assert!(CdrTimestamp::from_tap(&corrupted, "+0200").is_err());
    }
    for at in 0..5 {
        let corrupted: String = "+0200"
            .chars()
            .enumerate()
            .map(|(index, ch)| if index == at { '\u{4e2d}' } else { ch })
            .collect();
        assert!(CdrTimestamp::from_tap(canonical, &corrupted).is_err());
    }
}
