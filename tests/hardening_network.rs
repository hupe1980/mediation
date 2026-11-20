//! Regression tests for the dial-plan, jurisdiction, location and session
//! findings of the 002 audit (D1–D9, N1, J1–J3, L1, S1–S3).

use mediation::dialplan::{DialContextError, ShortCodePattern};
use mediation::{
    Ambiguity, ChargedParty, ChargingCharacteristics, Destination, DestinationClass, DialContext,
    DialError, Dnn, Ecgi, Eci, GermanNumberingPlan, Jurisdiction, MacroEnbId, NumberRange,
    NumberRangeSet, NumberingPlan, RatType, RecordType, Rule, SessionContext, ShortCode, Snssai,
    SourceRef, SubscriberRef, Uli, UsageRecord, UserLocation, ValidationConfig,
    customer_country_presumption, is_abroad, usage_country, validate_records,
};
use time::macros::datetime;

fn berlin() -> DialContext {
    DialContext::germany().with_area_code("030").unwrap()
}

fn dial(context: &DialContext, dialled: &str) -> Result<String, DialError> {
    context.normalise(dialled).map(|d| d.to_string())
}

fn de_class(destination: &str) -> Option<DestinationClass> {
    destination
        .parse::<Destination>()
        .unwrap()
        .class(&GermanNumberingPlan)
}

// ── Dial plan ────────────────────────────────────────────────────────────────

/// D5: a German local number may start with 1. audit-hardening/SR-005
#[test]
fn d5_local_numbers_starting_with_1_take_the_area_code() {
    assert_eq!(dial(&berlin(), "19222").unwrap(), "+493019222");
    assert_eq!(dial(&berlin(), "1234567").unwrap(), "+49301234567");
    // Listed short codes stay short codes; a prefix of another length is refused.
    for code in ["110", "112", "115", "116117", "11833", "113"] {
        assert_eq!(dial(&berlin(), code).unwrap(), code);
    }
    for bad in ["11", "116", "1101", "118"] {
        assert_eq!(
            dial(&berlin(), bad),
            Err(DialError::InvalidShortCode),
            "{bad}"
        );
    }
}

/// D1: Italy's closed plan keeps the leading 0. audit-hardening/SR-005
#[test]
fn d1_italian_closed_plan() {
    let italy = DialContext::italy();
    assert_eq!(dial(&italy, "06 1234567").unwrap(), "+39061234567");
    assert_eq!(dial(&italy, "347 1234567").unwrap(), "+393471234567");
    assert_eq!(dial(&italy, "112").unwrap(), "112");
    assert_eq!(dial(&italy, "0049 30 1234567").unwrap(), "+49301234567");
    assert_eq!(
        dial(&italy, "+39 (0)6 1234567"),
        Err(DialError::MisplacedTrunkDigit { position: 4 })
    );
    assert_eq!(
        italy.with_area_code("06"),
        Err(DialContextError::ClosedPlan)
    );
}

/// D2: NANP strips the trunk 1 and wants ten digits. audit-hardening/SR-005
#[test]
fn d2_nanp_closed_plan() {
    let nanp = DialContext::nanp();
    assert_eq!(dial(&nanp, "1 212 555 1234").unwrap(), "+12125551234");
    assert_eq!(dial(&nanp, "(212) 555-1234").unwrap(), "+12125551234");
    assert_eq!(dial(&nanp, "011 49 30 1234567").unwrap(), "+49301234567");
    assert_eq!(dial(&nanp, "911").unwrap(), "911");
    assert_eq!(
        dial(&nanp, "555 1234"),
        Err(DialError::NationalNumberLength)
    );
    assert_eq!(
        dial(&nanp, "1 555 1234"),
        Err(DialError::NationalNumberLength)
    );
}

/// D3: an inconsistent context is refused when built. audit-hardening/SR-005
#[test]
fn d3_contexts_are_built_fallibly() {
    use DialContextError as E;
    assert_eq!(DialContext::new(0, "00"), Err(E::CountryCode));
    assert_eq!(DialContext::new(1000, "00"), Err(E::CountryCode));
    assert_eq!(DialContext::new(49, ""), Err(E::InternationalPrefix));
    assert_eq!(DialContext::new(49, "0a"), Err(E::InternationalPrefix));

    let base = DialContext::new(49, "00").unwrap();
    assert_eq!(base.with_trunk_prefix(""), Err(E::TrunkPrefix));
    assert_eq!(base.with_trunk_prefix("+"), Err(E::TrunkPrefix));
    assert_eq!(base.with_trunk_prefix("00"), Err(E::PrefixesOverlap));
    assert_eq!(base.with_trunk_prefix("001"), Err(E::PrefixesOverlap));
    assert!(base.with_trunk_prefix("0").is_ok());

    static BAD: &[ShortCodePattern] = &[
        ShortCodePattern::new("110", 3),
        ShortCodePattern::new("1", 7),
    ];
    assert_eq!(
        base.with_short_codes(BAD),
        Err(E::ShortCodePattern { index: 1 })
    );
    static INVALID: [&[ShortCodePattern]; 4] = [
        &[ShortCodePattern::new("", 3)],
        &[ShortCodePattern::new("1", 1)],
        &[ShortCodePattern::new("1234", 3)],
        &[ShortCodePattern::new("1a", 3)],
    ];
    for invalid in INVALID {
        assert_eq!(
            base.with_short_codes(invalid),
            Err(E::ShortCodePattern { index: 0 }),
            "{invalid:?}"
        );
    }
    static SHADOWED: &[ShortCodePattern] = &[ShortCodePattern::new("01", 3)];
    assert_eq!(
        base.with_trunk_prefix("0")
            .unwrap()
            .with_short_codes(SHADOWED),
        Err(E::PrefixesOverlap)
    );
    assert_eq!(base.with_national_length(14), Err(E::NationalLength));
    assert_eq!(base.with_national_length(0), Err(E::NationalLength));
    assert_eq!(base.with_area_code("0x"), Err(E::AreaCode));
}

/// D9: two-digit short codes. audit-hardening/SR-005
#[test]
fn d9_two_digit_short_codes() {
    static FRANCE: &[ShortCodePattern] = &[
        ShortCodePattern::new("15", 2),
        ShortCodePattern::new("17", 2),
        ShortCodePattern::new("18", 2),
        ShortCodePattern::new("112", 3),
    ];
    let france = DialContext::new(33, "00")
        .unwrap()
        .with_trunk_prefix("0")
        .unwrap()
        .closed()
        .with_national_length(9)
        .unwrap()
        .with_short_codes(FRANCE)
        .unwrap();
    assert_eq!(dial(&france, "15").unwrap(), "15");
    assert_eq!(dial(&france, "01 23 45 67 89").unwrap(), "+33123456789");
    assert!("15".parse::<ShortCode>().is_ok());
    assert!("15".parse::<Destination>().unwrap().as_number().is_none());
}

/// D4: `(0)` is dropped only directly after the own country code.
/// audit-hardening/SR-005
#[test]
fn d4_bracketed_zero_only_after_own_country_code() {
    let berlin = berlin();
    assert_eq!(dial(&berlin, "+49 (0)30 2345678").unwrap(), "+49302345678");
    assert_eq!(dial(&berlin, "0049 (0)30 2345678").unwrap(), "+49302345678");
    assert_eq!(dial(&berlin, "(0)30 2345678").unwrap(), "+49302345678");
    for (dialled, position) in [
        ("+49 30 1234 (0)5", 12),
        ("+39 (0)6 1234567", 4),
        ("+(0)49 30 1", 1),
        ("030 (0)2345678", 4),
    ] {
        assert_eq!(
            dial(&berlin, dialled),
            Err(DialError::MisplacedTrunkDigit { position }),
            "{dialled}"
        );
    }
}

/// D6: +49 115 and 116xyz only at their exact length; 110/112/118 unclassified.
/// audit-hardening/SR-005
#[test]
fn d6_german_service_numbers_need_their_exact_length() {
    assert_eq!(de_class("+49115"), Some(DestinationClass::Authority));
    assert_eq!(de_class("+49116117"), Some(DestinationClass::Harmonised));
    for unclassified in [
        "+4911599999999",
        "+491161171",
        "+4911611",
        "+49110",
        "+49112",
        "+4911833",
    ] {
        assert_eq!(de_class(unclassified), None, "{unclassified}");
    }
    // Reached nationally as 0115.
    let authority = berlin().normalise("0115").unwrap();
    assert_eq!(
        authority.class(&GermanNumberingPlan),
        Some(DestinationClass::Authority)
    );
}

/// N1: disagreeing duplicate ranges classify nothing, in either order.
/// audit-hardening/SR-005
#[test]
fn n1_conflicting_duplicate_ranges_do_not_classify() {
    let range: NumberRange = "4930".parse().unwrap();
    let number: Destination = "+49302345678".parse().unwrap();
    for order in [
        [DestinationClass::Geographic, DestinationClass::Premium],
        [DestinationClass::Premium, DestinationClass::Geographic],
    ] {
        let mut plan = NumberRangeSet::new();
        plan.insert(range, order[0]);
        plan.insert(range, order[1]);
        assert_eq!(plan.classify(&number), None);
    }
    let mut agreeing = NumberRangeSet::new();
    agreeing.insert(range, DestinationClass::Geographic);
    agreeing.insert(range, DestinationClass::Geographic);
    assert_eq!(
        agreeing.classify(&number),
        Some(DestinationClass::Geographic)
    );
}

/// D8: no country code starts with 0. audit-hardening/SR-005
#[test]
fn d8_destination_refuses_country_code_zero() {
    for bad in ["+0049301234", "+ 0 49 30", "+0"] {
        assert!(bad.parse::<Destination>().is_err(), "{bad}");
    }
    assert!("+49 30 1234".parse::<Destination>().is_ok());
}

// ── Jurisdiction ─────────────────────────────────────────────────────────────

fn record(party: ChargedParty, serving: &str, home: &str) -> UsageRecord {
    UsageRecord::new(
        RecordType::PduSession,
        party,
        "2026-07-28T10:00:00+02:00".parse().unwrap(),
        SourceRef::new("pgw-01".parse().unwrap(), 0),
    )
    .with_networks(serving.parse().unwrap(), home.parse().unwrap())
}

fn subscriber(imsi: &str) -> ChargedParty {
    ChargedParty::Subscriber(SubscriberRef::new(imsi.parse().unwrap()))
}

/// J1: the MCCs of one country are one country. audit-hardening/SR-006
#[test]
fn j1_one_country_many_mccs() {
    let us = record(subscriber("imsi-310260123456789"), "311-480", "310-260");
    assert_eq!(is_abroad(&us), Some(false));
    let uk = record(subscriber("imsi-235011234567890"), "234-15", "235-01");
    assert_eq!(is_abroad(&uk), Some(false));
    let roamer = record(subscriber("imsi-262021234567890"), "214-07", "262-02");
    assert_eq!(is_abroad(&roamer), Some(true));

    let cell = UserLocation::Ecgi(Ecgi::new("311-480".parse().unwrap(), Eci::new(1).unwrap()));
    let us_cell = record(subscriber("imsi-310260123456789"), "310-260", "310-260")
        .with_session(SessionContext::none().with_location(cell));
    assert_eq!(
        usage_country(&us_cell),
        Jurisdiction::Country("310".parse().unwrap())
    );
    let report = validate_records(
        std::slice::from_ref(&us_cell),
        &ValidationConfig::default(),
        datetime!(2026-07-28 12:00 UTC),
    );
    assert_eq!(report.severity_of(Rule::JurisdictionConflict), None);
}

/// J2: the VAT presumption is about a subscriber. audit-hardening/SR-006
#[test]
fn j2_api_consumer_has_no_vat_presumption() {
    let api = record(
        ChargedParty::ApiConsumer {
            developer: "dev-fr".parse().unwrap(),
            subject: Some(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
        },
        "262-02",
        "262-02",
    );
    assert_eq!(
        customer_country_presumption(&api),
        Jurisdiction::Unresolved(Ambiguity::NoEvidence)
    );
}

/// J3: the VAT presumption needs 3GPP radio access. audit-hardening/SR-006
#[test]
fn j3_vat_presumption_only_over_radio() {
    let base = record(subscriber("imsi-262021234567890"), "262-02", "262-02");
    for rat in [RatType::Wireline, RatType::Wlan] {
        let fixed = base
            .clone()
            .with_session(SessionContext::none().with_rat(rat));
        assert_eq!(
            customer_country_presumption(&fixed),
            Jurisdiction::Unresolved(Ambiguity::NoEvidence),
            "{rat}"
        );
    }
    let germany = Jurisdiction::Country("262".parse().unwrap());
    for rat in [RatType::Nr, RatType::Eutran, RatType::NonTerrestrial] {
        let radio = base
            .clone()
            .with_session(SessionContext::none().with_rat(rat));
        assert_eq!(customer_country_presumption(&radio), germany, "{rat}");
    }
    // No access technology recorded: the presumption stands.
    assert_eq!(customer_country_presumption(&base), germany);
}

// ── Location and session ─────────────────────────────────────────────────────

/// L1: eNodeB identities take part in the country check.
#[test]
fn l1_uli_country_includes_enb_identities() {
    let mut uli = Uli::new();
    uli.macro_enb = MacroEnbId::new("214-07".parse().unwrap(), 5);
    assert_eq!(uli.mcc(), Ok(Some("214".parse().unwrap())));
    uli.ecgi = Some(Ecgi::new("262-02".parse().unwrap(), Eci::new(1).unwrap()));
    assert!(uli.mcc().is_err());
}

/// S1: charging characteristics alone are a recorded context.
#[test]
fn s1_charging_characteristics_count_as_context() {
    let cc: ChargingCharacteristics = "0800".parse().unwrap();
    let context = SessionContext::none().with_charging_characteristics(cc);
    assert!(!context.is_empty());
    assert!(context.to_string().contains("0800"), "{context}");
}

/// S2: the APN network identifier keeps all its labels.
#[test]
fn s2_network_identifier_keeps_every_label() {
    let dnn: Dnn = "corp.example.com.mnc002.mcc262.gprs".parse().unwrap();
    assert_eq!(dnn.network_identifier(), "corp.example.com");
    let plain: Dnn = "corp.example.com".parse().unwrap();
    assert_eq!(plain.network_identifier(), "corp.example.com");
    let ims: Dnn = "ims.mnc002.mcc262.gprs".parse().unwrap();
    assert_eq!(ims.network_identifier(), "ims");
}

/// S3: S-NSSAI parses only its canonical form; DNN labels are at most 63.
#[test]
fn s3_canonical_snssai_and_dnn_label_length() {
    for bad in [
        "+1-0000a1",
        "1-+000a1",
        "01-0000a1",
        "001",
        "+1",
        "1-0x00a1",
    ] {
        assert!(bad.parse::<Snssai>().is_err(), "{bad}");
    }
    for good in ["0", "1", "255", "1-0000a1"] {
        assert_eq!(good.parse::<Snssai>().unwrap().to_string(), good);
    }
    assert!("a".repeat(64).parse::<Dnn>().is_err());
    assert!("a".repeat(63).parse::<Dnn>().is_ok());
}
