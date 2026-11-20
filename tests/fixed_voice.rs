//! Fixed-line voice, end to end: a fibre operator's voice platform exports A-number,
//! dialled B-number, answer time, chargeable seconds, node and Call-ID. Everything
//! between that row and the itemised bill is exercised here.
//!
//! Spec `001-fixed-line-voice`; each test names the requirement it proves.

use std::collections::{BTreeSet, HashSet};

use mediation::{
    Batch, CallDirection, CallId, CdrTimestamp, ChargedParty, DedupWindow, Destination,
    DestinationClass, DialContext, DialError, Disposition, GermanNumberingPlan, Itemisation,
    ItemisedDestination, Msisdn, NotKeyable, NumberPrefix, NumberRange, NumberRangeSet,
    NumberingPlan, ProcessConfig, ProcessState, QuarterHourOffset, RatType, RecordKey, RecordType,
    RejectReason, Rule, Seconds, ServiceDomain, SessionContext, SourceRef, SubscriberRef, Supi,
    UsageRecord, itemise_called, itemise_calling, premium_destination, process,
};
use time::{Duration, OffsetDateTime, macros::datetime};

const NOW: OffsetDateTime = datetime!(2026-07-28 20:00:00 UTC);

fn berlin() -> DialContext {
    DialContext::germany().with_area_code("030").unwrap()
}

fn line(id: &str) -> ChargedParty {
    ChargedParty::Subscriber(SubscriberRef::new(
        format!("nai-{id}@fibre.example").parse().unwrap(),
    ))
}

/// One row of the platform export, the way a consumer maps it.
fn row(node: &str, call_id: &str, dialled: &str, minute: u8, seconds: u32) -> UsageRecord {
    let cest = QuarterHourOffset::from_minutes(120).unwrap();
    let answered = CdrTimestamp::from_utc(
        datetime!(2026-07-28 16:00:00 UTC) + Duration::minutes(i64::from(minute)),
        cest,
    )
    .unwrap();
    UsageRecord::call(
        CallDirection::Originated,
        line("line-4711"),
        answered,
        Seconds::new(seconds),
        SourceRef::new(node.parse().unwrap(), u32::from(minute)).with_file_sequence(1),
    )
    .with_session(SessionContext::none().with_rat(RatType::Wireline))
    .with_calling("+49302345678".parse().unwrap())
    .with_called(berlin().normalise(dialled).unwrap())
    .with_call_id(node.parse().unwrap(), call_id.parse().unwrap())
}

fn run(records: &[UsageRecord]) -> mediation::ProcessOutcome {
    let mut state = ProcessState::new(DedupWindow::from_bound(Duration::days(7)).unwrap());
    process(
        &Batch::new(records),
        &ProcessConfig::default(),
        NOW,
        &mut state,
    )
}

// ── US1: a fixed-voice batch, end to end ─────────────────────────────────────

/// SC-001 / FR-007: balances, a resend is a duplicate, and nothing mobile-only fires.
#[test]
fn batch_balances_and_a_resend_is_a_duplicate() {
    let first = row(
        "sbc-ber-1",
        "a84b4c76e66710@sbc-ber-1",
        "0151 1234567",
        0,
        95,
    );
    let mut resend = first.clone();
    resend.provenance = SourceRef::new("sbc-ber-1".parse().unwrap(), 9).with_file_sequence(2);
    let other = row("sbc-ber-1", "f81d4fae7dec11d0@sbc-ber-1", "2345678", 5, 30);

    let outcome = run(&[first, resend, other]);

    assert!(outcome.reconciliation().balances());
    assert_eq!(outcome.reconciliation().released(), 2);
    assert_eq!(outcome.reconciliation().duplicate(), 1);
    assert!(matches!(
        outcome.dispositions()[1],
        Disposition::Duplicate(RecordKey::Call { .. })
    ));
    assert!(
        !outcome.findings().has(Rule::ServingNetworkAbsent),
        "V12 on wireline"
    );
    assert!(
        !outcome.findings().has(Rule::QuantityDoesNotMatchService),
        "V13 on voice"
    );

    // fixed-line-voice/SR-006: the V17 detail names the node, not the Call-ID.
    let v17: Vec<_> = outcome
        .findings()
        .findings()
        .iter()
        .filter(|f| f.rule == Rule::DuplicateRecord)
        .collect();
    assert_eq!(v17.len(), 1);
    assert_eq!(v17[0].detail, "CALL:sbc-ber-1");
    assert_eq!(
        outcome.findings().len(),
        1,
        "SC-001: nothing but the provoked V17"
    );
}

/// FR-002: a call is voice, measured by duration, on every access.
#[test]
fn a_fixed_line_call_is_voice() {
    let call = row("sbc-ber-1", "c1@sbc", "2345678", 0, 60);
    assert_eq!(call.record_type, RecordType::OriginatedCall);
    assert_eq!(call.service(), ServiceDomain::Voice);
    assert_eq!(call.wall_clock_span(), Some(Seconds::new(60)));
    assert_eq!(
        RecordType::FixedAccessSession.service_domain(),
        ServiceDomain::FixedAccess,
        "broadband stays its own domain"
    );
}

/// fixed-line-voice/SR-007: two nodes may report the same Call-ID for different calls.
#[test]
fn the_same_call_id_on_two_nodes_is_two_calls() {
    let a = row("sbc-ber-1", "abc@host", "2345678", 0, 60);
    let b = row("sbc-ber-2", "abc@host", "2345679", 0, 60);
    let outcome = run(&[a, b]);
    assert_eq!(outcome.reconciliation().released(), 2);
    assert_eq!(outcome.reconciliation().duplicate(), 0);
}

/// FR-005 / US1 scenario 4: an unkeyable record says what is missing.
#[test]
fn an_unkeyable_call_names_the_missing_half() {
    let mut no_entity = row("sbc-ber-1", "abc@host", "2345678", 0, 60);
    no_entity.correlation.recording_entity = None;
    assert_eq!(RecordKey::of(&no_entity), Err(NotKeyable::IncompleteCallId));

    let mut bare = no_entity.clone();
    bare.correlation.call_id = None;
    assert_eq!(RecordKey::of(&bare), Err(NotKeyable::NoCorrelatorPresent));

    let outcome = run(&[bare]);
    assert_eq!(
        outcome.dispositions()[0],
        Disposition::Rejected(RejectReason::NotKeyable(NotKeyable::NoCorrelatorPresent))
    );
    assert!(outcome.reconciliation().balances());
}

/// fixed-line-voice/SR-010: the V12 exemption is for wireline voice, IMS and fixed access only.
#[test]
fn a_wireline_label_does_not_hide_a_mobile_only_record() {
    let registration = UsageRecord::new(
        RecordType::Registration,
        line("line-4711"),
        "2026-07-28T18:00:00+02:00".parse().unwrap(),
        SourceRef::new("amf-1".parse().unwrap(), 0),
    )
    .with_session(SessionContext::none().with_rat(RatType::Wireline));
    let report = mediation::validate_records(&[registration], &Default::default(), NOW);
    assert!(report.has(Rule::ServingNetworkAbsent));
}

/// FR-008: a fixed line is identified without an IMSI.
#[test]
fn wireline_subscriptions_have_their_own_supi_forms() {
    let gli: Supi = "gli-bGluZS00NzEx@5gc.mnc002.mcc262.3gppnetwork.org"
        .parse()
        .unwrap();
    assert!(gli.is_wireline());
    assert!(gli.as_imsi().is_none());
    assert!(gli.to_string().starts_with("gli-"));
    let gci: Supi = "gci-00-00-5E-00-53-00@cable.example".parse().unwrap();
    assert!(gci.is_wireline());
}

/// fixed-line-voice/SR-003: a Call-ID is bounded and shaped like RFC 3261's.
#[test]
fn call_ids_are_bounded() {
    assert!("a84b4c76e66710@pc33.example.com".parse::<CallId>().is_ok());
    assert!("x".repeat(CallId::MAX_LEN).parse::<CallId>().is_ok());
    assert!("x".repeat(CallId::MAX_LEN + 1).parse::<CallId>().is_err());
    for bad in ["", "a@b@c", "@host", "with space", "uni\u{e9}code"] {
        assert!(bad.parse::<CallId>().is_err(), "{bad:?}");
    }
}

// ── US2: dial-plan normalisation ─────────────────────────────────────────────

/// SC-002: the acceptance table. fixed-line-voice/SR-001
#[test]
fn dial_table() {
    let berlin = berlin();
    let no_area = DialContext::germany();
    let ok: &[(&str, &str)] = &[
        ("0151 1234567", "+491511234567"),
        ("0151-1234567", "+491511234567"),
        ("(0151) 123 45 67", "+491511234567"),
        ("0049 151 1234567", "+491511234567"),
        ("+49 151 1234567", "+491511234567"),
        // The bracketed trunk prefix of the international spelling, and the
        // bracketed area code of the national one.
        ("+49 (0)30 2345678", "+49302345678"),
        ("0049 (0)30 2345678", "+49302345678"),
        ("(030) 2345678", "+49302345678"),
        ("(0)30 2345678", "+49302345678"),
        ("030 2345678", "+49302345678"),
        ("030/2345678", "+49302345678"),
        ("2345678", "+49302345678"),
        ("0800 1110111", "+498001110111"),
        ("0900 0123456", "+499000123456"),
        ("0043 1 2345678", "+4312345678"),
        ("+1 212 555 1234", "+12125551234"),
        ("110", "110"),
        ("112", "112"),
        ("115", "115"),
        ("116117", "116117"),
        ("116 123", "116123"),
        ("11833", "11833"),
        // Only the listed short codes are short codes; a local number may
        // start with 1 (audit D5).
        ("1234567", "+49301234567"),
        ("19222", "+493019222"),
        ("0110", "+49110"),
        ("010 123456", "+4910123456"),
    ];
    for &(dialled, expected) in ok {
        assert_eq!(
            berlin.normalise(dialled).unwrap().to_string(),
            expected,
            "{dialled:?}"
        );
    }

    let refused: &[(&DialContext, &str, DialError)] = &[
        (&no_area, "2345678", DialError::AreaCodeRequired),
        (&berlin, "", DialError::Empty),
        (&berlin, " - / ", DialError::Empty),
        (
            &berlin,
            "030+2345678",
            DialError::MisplacedPlus { position: 3 },
        ),
        (&berlin, "++49", DialError::MisplacedPlus { position: 1 }),
        (
            &berlin,
            "0\u{ff13}0 2345678",
            DialError::InvalidCharacter { position: 1 },
        ),
        (
            &berlin,
            "030 CALL-NOW",
            DialError::InvalidCharacter { position: 4 },
        ),
        (
            &berlin,
            "*31#0302345678",
            DialError::InvalidCharacter { position: 0 },
        ),
        (&berlin, "+", DialError::Empty),
        (&berlin, "00", DialError::MissingNumber),
        (&berlin, "0", DialError::MissingNumber),
        (&berlin, "+049", DialError::InvalidCountryCode),
        (&berlin, "11", DialError::InvalidShortCode),
        (&berlin, "1101", DialError::InvalidShortCode),
        (
            &berlin,
            "+49 30 1234 (0)5",
            DialError::MisplacedTrunkDigit { position: 12 },
        ),
        (&berlin, "+1234567890123456", DialError::NumberTooLong),
        (&berlin, &"1".repeat(65), DialError::TooLong),
        // Former carrier selection: an ordinary national number, here too long.
        (&berlin, "01013 030 2345678", DialError::NumberTooLong),
    ];
    for (context, dialled, error) in refused {
        assert_eq!(context.normalise(dialled), Err(*error), "{dialled:?}");
    }
}

/// fixed-line-voice/SR-006: a refusal can be logged — it never contains the digits.
#[test]
fn errors_do_not_carry_the_dialled_digits() {
    let dialled = "0309876543X";
    let err = berlin().normalise(dialled).unwrap_err();
    for rendered in [err.to_string(), format!("{err:?}")] {
        assert!(!rendered.contains("9876543"), "{rendered}");
    }
    let parse = "+49 30 9876543 ext".parse::<Destination>().unwrap_err();
    assert!(parse.is_withheld());
    for rendered in [parse.to_string(), format!("{parse:?}")] {
        assert!(!rendered.contains("9876543"), "{rendered}");
    }
    let call_id = "+4930987654321@sbc host".parse::<CallId>().unwrap_err();
    assert!(call_id.is_withheld());
    assert!(!format!("{call_id} {call_id:?}").contains("987654321"));
}

// ── US3: destination class ───────────────────────────────────────────────────

fn class(destination: &str) -> Option<DestinationClass> {
    destination
        .parse::<Destination>()
        .unwrap()
        .class(&GermanNumberingPlan)
}

/// FR-022 / US3: the shipped German example plan. fixed-line-voice/SR-009
#[test]
fn the_german_plan_classifies_by_range() {
    use DestinationClass::*;
    let table: &[(&str, Option<DestinationClass>)] = &[
        ("+4915112345678", Some(Mobile)),
        ("+491701234567", Some(Mobile)),
        ("+491601234567", Some(Mobile)),
        ("+491641234567", Some(Paging)),
        ("+498001234567", Some(Freephone)),
        ("+4918051234567", Some(SharedCost)),
        ("+4990001234567", Some(Premium)),
        ("+4990081234567", Some(Premium)),
        ("+4913761234567", Some(MassTraffic)),
        ("+4970012345678", Some(Personal)),
        ("+4932123456789", Some(NationalSubscriber)),
        ("+491811234567", Some(Vpn)),
        ("+49192123", Some(OnlineService)),
        ("+49301234567", Some(Geographic)),
        ("+49891234567", Some(Geographic)),
        ("+4312345678", Some(International)),
        ("110", Some(Emergency)),
        ("112", Some(Emergency)),
        ("115", Some(Authority)),
        ("116117", Some(Harmonised)),
        ("11833", Some(Directory)),
        ("113", Some(NetworkInternal)),
        // SR-009: reserve and withdrawn ranges are not classified.
        ("+4990091234567", None), // 09009 dialer range, cancelled 2022
        ("+4913812345678", None), // 0138, removed
        ("+49121234567", None),   // 012, reserve
        ("+4910123456", None),    // 010 carrier selection, withdrawn 2024
        ("+4918001234567", None), // 0180-0 reserve
        ("+499011234567", None),  // 0901 reserve
        ("+49311234567", None),   // 031 test / reserve
        ("11803", None),          // 1180x reserve
        ("1234", None),
    ];
    for &(destination, expected) in table {
        assert_eq!(class(destination), expected, "{destination}");
    }
    assert_eq!(GermanNumberingPlan::AS_OF, "2025-11-06");
    assert!(GermanNumberingPlan::SOURCE.contains("Bundesnetzagentur"));
}

/// FR-021: a caller's own plan works through the same call.
#[test]
fn a_caller_plan_replaces_the_shipped_one() {
    let destination: Destination = "+491511234567".parse().unwrap();

    let closure = |d: &Destination| d.as_number().map(|_| DestinationClass::Premium);
    assert_eq!(destination.class(&closure), Some(DestinationClass::Premium));

    let mut ranges: NumberRangeSet<DestinationClass> = NumberRangeSet::new();
    ranges.insert(
        "4915".parse::<NumberRange>().unwrap(),
        DestinationClass::Mobile,
    );
    assert_eq!(destination.class(&ranges), Some(DestinationClass::Mobile));
    assert_eq!(ranges.classify(&"110".parse().unwrap()), None);
}

// ── US4: the itemised bill ───────────────────────────────────────────────────

/// fixed-line-voice/SR-005 / FR-031.
#[test]
fn shortening_hides_the_last_three_digits() {
    let none: [Destination; 0] = [];
    let shown = |d: &str| match itemise_called(&d.parse().unwrap(), Itemisation::Shortened, &none) {
        ItemisedDestination::Shortened(n) => n.to_string(),
        other => panic!("{other:?}"),
    };
    assert_eq!(shown("+49301234567"), "+49301234xxx");
    assert_eq!(shown("116117"), "116xxx");
    assert_eq!(shown("110"), "xxx", "a three-digit code is hidden entirely");

    let full = itemise_called(&"+49301234567".parse().unwrap(), Itemisation::Full, &none);
    assert_eq!(
        full,
        ItemisedDestination::Full("+49301234567".parse().unwrap())
    );
}

/// fixed-line-voice/SR-004: every spelling of a protected number is omitted, under both choices.
#[test]
fn protected_destinations_are_omitted_whatever_the_spelling() {
    let berlin = berlin();
    let protected: BTreeSet<Destination> = ["+498001110111".parse().unwrap()].into();
    let hashed: HashSet<Destination> = protected.iter().copied().collect();
    for dialled in ["0800 111 0 111", "+49 800 1110111", "0049800-1110111"] {
        let destination = berlin.normalise(dialled).unwrap();
        for choice in Itemisation::ALL {
            assert_eq!(
                itemise_called(&destination, *choice, &protected),
                ItemisedDestination::Omitted,
                "{dialled} {choice}"
            );
            assert_eq!(
                itemise_called(&destination, *choice, &hashed),
                ItemisedDestination::Omitted
            );
        }
    }
}

/// fixed-line-voice/SR-011: an incoming caller's number is always shortened.
#[test]
fn an_incoming_caller_is_always_shortened() {
    let caller: Msisdn = "49301234567".parse().unwrap();
    assert_eq!(itemise_calling(caller).to_string(), "+49301234xxx");
}

/// fixed-line-voice/SR-008 / SC-004: itemising a processed batch changes nothing it reconciled.
#[test]
fn itemisation_never_touches_the_reconciliation() {
    let records = [
        row("sbc-ber-1", "c1@sbc", "0800 1110111", 0, 300),
        row("sbc-ber-1", "c2@sbc", "0151 1234567", 10, 60),
    ];
    let outcome = run(&records);
    let before = *outcome.reconciliation();
    let protected = ["+498001110111".parse::<Destination>().unwrap()];

    let lines: Vec<_> = records
        .iter()
        .zip(outcome.dispositions())
        .filter(|(_, d)| d.is_released())
        .map(|(r, _)| itemise_called(&r.called.unwrap(), Itemisation::Shortened, &protected))
        .collect();

    assert_eq!(lines[0], ItemisedDestination::Omitted);
    assert!(matches!(lines[1], ItemisedDestination::Shortened(_)));
    assert_eq!(*outcome.reconciliation(), before);
    assert_eq!(
        outcome.reconciliation().released(),
        2,
        "the protected call is still billed"
    );
}

/// FR-042: no caller arithmetic, no underflow.
#[test]
fn a_prefix_without_the_last_digits() {
    let n: Msisdn = "49301234567".parse().unwrap();
    assert_eq!(
        NumberPrefix::without_last(n, 3).unwrap().to_string(),
        "49301234"
    );
    assert_eq!(NumberPrefix::without_last(n, 0), None);
    assert_eq!(NumberPrefix::without_last(n, 11), None);
    let short: Msisdn = "12".parse().unwrap();
    assert_eq!(NumberPrefix::without_last(short, 3), None);
}

// ── US5: fraud reads the record ──────────────────────────────────────────────

#[test]
fn premium_destination_reads_the_called_number() {
    let mut high_risk: NumberRangeSet<&str> = NumberRangeSet::new();
    high_risk.insert("88216".parse().unwrap(), "satellite");
    let records = [
        row("sbc-ber-1", "c1@sbc", "00 882 16 1234567", 0, 60),
        row("sbc-ber-1", "c2@sbc", "110", 1, 60),
    ];
    let found = premium_destination(&records, &high_risk);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].records, vec![0]);
}

// ── Q-4: a call across a tariff boundary ─────────────────────────────────────

/// The parts of a split call sum to its chargeable duration, to the second.
#[test]
fn a_call_splits_at_a_tariff_boundary_and_conserves_seconds() {
    // Answered 18:00 CEST (16:00 UTC), 95 s; the band changes at 16:01:00 UTC.
    let call = row("sbc-ber-1", "c1@sbc", "2345678", 0, 95);
    let split = mediation::split_session(&call, &[datetime!(2026-07-28 16:01:00 UTC)]).unwrap();
    let parts: Vec<u32> = split
        .slices()
        .iter()
        .map(|s| s.duration.unwrap().get())
        .collect();
    assert_eq!(parts, vec![60, 35]);
    assert!(split.conserves(&call));
}
