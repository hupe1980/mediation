//! Who terminates a number, and what the traffic to it looks like.
//!
//! Both answers come from caller-supplied reference data, so these tests are
//! mostly about what happens when that data does not answer.

use mediation::{
    ChargedParty, Mcc, Msisdn, NumberRange, NumberRangeSet, PlmnId, RecordType, Seconds, Signal,
    SourceRef, SubscriberRef, TrafficShape, UniformTransit, UsageRecord, ait_signature,
    bypass_signature, impossible_travel, is_ported, premium_destination, terminating_plmn,
    wangiri_pattern,
};
use time::Duration;

fn range(digits: &str) -> NumberRange {
    digits.parse().expect("a valid range")
}

fn number(digits: &str) -> Msisdn {
    digits.parse().expect("a valid number")
}

/// Gives each record the destination `called` resolves for it.
fn resolve(
    records: &[UsageRecord],
    called: &impl Fn(usize, &UsageRecord) -> Option<Msisdn>,
) -> Vec<UsageRecord> {
    records
        .iter()
        .enumerate()
        .map(|(i, r)| match called(i, r) {
            Some(n) => r.clone().with_called(n.into()),
            None => r.clone(),
        })
        .collect()
}

fn record(kind: RecordType, opened: &str, serving: &str) -> UsageRecord {
    UsageRecord::new(
        kind,
        ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
        opened.parse().unwrap(),
        SourceRef::new("msc-01".parse().unwrap(), 0),
    )
    .with_networks(serving.parse().unwrap(), "262-02".parse().unwrap())
}

/// German mobile allocation, with one block carved out of a wider range.
fn allocation() -> NumberRangeSet<PlmnId> {
    let mut set = NumberRangeSet::new();
    set.insert(range("49"), "262-01".parse().unwrap());
    set.insert(range("49151"), "262-01".parse().unwrap());
    set.insert(range("4915112"), "262-23".parse().unwrap());
    set.insert(range("49170"), "262-02".parse().unwrap());
    set
}

// ── Settlement against the wrong partner ─────────────────────────────────────

/// The whole reason the module exists: interconnect settles per terminating
/// operator, and a number's range stopped answering that question the day
/// portability arrived.
#[test]
fn a_settlement_built_on_ranges_is_wrong_by_exactly_the_ported_numbers() {
    let allocation = allocation();

    // A national portability feed: two of these numbers have moved.
    let ported = |number: Msisdn| -> Option<PlmnId> {
        match number.to_string().as_str() {
            "491519000001" => Some("262-02".parse().unwrap()),
            "491709000002" => Some("262-01".parse().unwrap()),
            _ => None,
        }
    };

    let traffic = [
        number("491519000001"), // allocated 262-01, ported to 262-02
        number("491709000002"), // allocated 262-02, ported to 262-01
        number("491519000003"), // never ported
        number("491511200004"), // in the carved-out block
    ];

    let by_range: Vec<String> = traffic
        .iter()
        .map(|n| {
            allocation
                .longest_match(*n)
                .map(|(_, plmn)| plmn.to_string())
                .unwrap_or_default()
        })
        .collect();
    let by_termination: Vec<String> = traffic
        .iter()
        .map(|n| {
            terminating_plmn(*n, &allocation, &ported)
                .map(|plmn| plmn.to_string())
                .unwrap_or_default()
        })
        .collect();

    assert_eq!(by_range, ["262-01", "262-02", "262-01", "262-23"]);
    assert_eq!(by_termination, ["262-02", "262-01", "262-01", "262-23"]);

    // Exactly the ported numbers differ, and that is the size of the exposure.
    let wrong = traffic
        .iter()
        .filter(|n| is_ported(**n, &allocation, &ported))
        .count();
    assert_eq!(wrong, 2);
}

/// A number that resolves nowhere is unresolvable, never a default. Settling a
/// guess is a two-sided error that surfaces only when the partner's invoice
/// disagrees.
#[test]
fn an_unresolvable_number_is_reported_as_such() {
    let allocation = allocation();
    let nothing_ported = |_: Msisdn| None;

    assert_eq!(
        terminating_plmn(number("12125550100"), &allocation, &nothing_ported),
        None
    );
    assert!(!is_ported(
        number("12125550100"),
        &allocation,
        &nothing_ported
    ));
}

/// Reference data that answers one question twice is data the loader should
/// have rejected.
#[test]
fn a_duplicated_range_is_visible_before_it_decides_anything() {
    let mut set = NumberRangeSet::new();
    set.insert(range("49151"), "262-01".parse::<PlmnId>().unwrap());
    set.insert(range("49151"), "262-02".parse::<PlmnId>().unwrap());

    assert_eq!(set.duplicates(), vec![range("49151")]);
    assert!(allocation().duplicates().is_empty());
}

// ── Fraud signals over the same reference data ───────────────────────────────

/// One range set, two questions: which operator terminates this, and is the
/// destination one an operator flagged.
#[test]
fn the_same_structure_answers_allocation_and_risk() {
    let mut high_risk: NumberRangeSet<&str> = NumberRangeSet::new();
    high_risk.insert(range("88216"), "satellite");
    high_risk.insert(range("2392"), "IPRN");

    let records = vec![
        record(
            RecordType::OriginatedCall,
            "2026-07-28T10:00:00+02:00",
            "262-02",
        ),
        record(
            RecordType::OriginatedCall,
            "2026-07-28T11:00:00+02:00",
            "262-02",
        ),
        record(
            RecordType::OriginatedCall,
            "2026-07-28T12:00:00+02:00",
            "262-02",
        ),
    ];
    let called = |index: usize, _: &UsageRecord| -> Option<Msisdn> {
        match index {
            0 => Some(number("882161234567")),
            1 => Some(number("491701234567")),
            _ => None, // the decoder could not resolve this one
        }
    };

    let found = premium_destination(&resolve(&records, &called), &high_risk);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].signal, Signal::PremiumDestination);
    assert_eq!(found[0].records, vec![0]);
    assert!(found[0].detail.contains("88216"));
}

/// An unresolvable destination is not evidence of anything, in either
/// direction.
#[test]
fn an_unresolved_destination_is_never_evidence() {
    let mut high_risk: NumberRangeSet<&str> = NumberRangeSet::new();
    high_risk.insert(range("88216"), "satellite");

    let records: Vec<UsageRecord> = (0..5)
        .map(|i| {
            record(
                RecordType::OriginatedCall,
                &format!("2026-07-28T1{i}:00:00+02:00"),
                "262-02",
            )
        })
        .collect();
    let nothing_resolves = |_: usize, _: &UsageRecord| None;

    assert!(premium_destination(&resolve(&records, &nothing_resolves), &high_risk).is_empty());
}

/// A day of one-ring inbound calls, then the callbacks that pay for them.
#[test]
fn a_wangiri_wave_shows_up_as_volume_not_as_any_single_call() {
    let wave: Vec<UsageRecord> = (0..30)
        .map(|i| {
            record(
                RecordType::TerminatedCall,
                &format!("2026-07-28T10:{i:02}:00+02:00"),
                "262-02",
            )
            .with_duration(Seconds::new(3))
        })
        .collect();

    let found = wangiri_pattern(&wave, Seconds::new(5), 20, Duration::hours(1));
    assert_eq!(found.len(), 1, "thirty one-ring calls");
    let found = &found[0];
    assert_eq!(found.signal, Signal::WangiriPattern);
    assert_eq!(found.records.len(), 30);

    // The same wave against a stricter volume floor says nothing.
    assert!(wangiri_pattern(&wave, Seconds::new(5), 50, Duration::hours(1)).is_empty());
    // Nor does it if the calls were not short.
    assert!(wangiri_pattern(&wave, Seconds::new(1), 20, Duration::hours(1)).is_empty());

    // A call with no recorded duration is not a short call.
    let unmetered: Vec<UsageRecord> = (0..30)
        .map(|i| {
            record(
                RecordType::TerminatedCall,
                &format!("2026-07-28T11:{i:02}:00+02:00"),
                "262-02",
            )
        })
        .collect();
    assert!(wangiri_pattern(&unmetered, Seconds::new(5), 20, Duration::hours(1)).is_empty());
}

/// The unsophisticated bypass shape, and the honest limit of it.
#[test]
fn bypass_triage_catches_the_naive_case_and_says_so() {
    let rack: Vec<UsageRecord> = (0..80)
        .map(|i| {
            record(
                RecordType::OriginatedCall,
                &format!("2026-07-28T{:02}:{:02}:00+02:00", i / 4, (i % 4) * 15),
                "262-02",
            )
        })
        .collect();

    let shape = TrafficShape::of(&rack);
    assert_eq!(shape.calls(), 80);
    assert_eq!(shape.outbound_percent(), Some(100));
    assert_eq!(shape.distinct_networks, 1);

    let found = bypass_signature(&rack, 50, 90).expect("the naive shape fires");
    assert_eq!(found.signal, Signal::BypassSignature);
    assert_eq!(found.records.len(), 80);

    // Now simulate human behaviour: incoming calls and movement between cells
    // on two networks. Nothing about the fraud changed; the signal is gone.
    let simulated: Vec<UsageRecord> = (0..80)
        .map(|i| {
            record(
                if i % 3 == 0 {
                    RecordType::TerminatedCall
                } else {
                    RecordType::OriginatedCall
                },
                &format!("2026-07-28T{:02}:{:02}:00+02:00", i / 4, (i % 4) * 15),
                if i % 5 == 0 { "262-01" } else { "262-02" },
            )
        })
        .collect();
    assert!(bypass_signature(&simulated, 50, 90).is_none());
}

/// A data session is not voice traffic and must not move the ratio.
#[test]
fn non_call_records_do_not_shape_voice_traffic() {
    let mut mixed: Vec<UsageRecord> = (0..10)
        .map(|i| {
            record(
                RecordType::OriginatedCall,
                &format!("2026-07-28T1{i}:00:00+02:00"),
                "262-02",
            )
        })
        .collect();
    mixed.push(record(
        RecordType::PduSession,
        "2026-07-28T23:00:00+02:00",
        "262-02",
    ));

    let shape = TrafficShape::of(&mixed);
    assert_eq!(shape.calls(), 10, "the data session is not a call");

    let found = bypass_signature(&mixed, 10, 90).expect("ten outbound calls on one network");
    assert_eq!(
        found.records.len(),
        10,
        "the indication points at the calls, not the data session"
    );
}

/// An inflated one-time-password flood concentrates on a few ranges; the
/// denominator is every message, so a sender whose traffic is mostly elsewhere
/// does not look concentrated.
#[test]
fn inflated_message_traffic_is_measured_against_all_of_it() {
    let mut ranges: NumberRangeSet<&str> = NumberRangeSet::new();
    ranges.insert(range("2392"), "IPRN");

    let messages: Vec<UsageRecord> = (0..100)
        .map(|i| {
            record(
                RecordType::SmsOriginated,
                &format!("2026-07-28T{:02}:{:02}:00+02:00", i / 10, i % 60),
                "262-02",
            )
        })
        .collect();

    // Eighty-five of a hundred to one range.
    let concentrated = |index: usize, _: &UsageRecord| -> Option<Msisdn> {
        if index < 85 {
            Some(number("239212345678"))
        } else {
            Some(number("491701234567"))
        }
    };
    let found = ait_signature(&resolve(&messages, &concentrated), &ranges, 50, 80)
        .expect("eighty-five percent");
    assert_eq!(found.signal, Signal::AitSignature);
    assert_eq!(found.records.len(), 85);
    assert!(found.detail.contains("85%"), "{}", found.detail);

    // The same eighty-five messages inside a much larger stream are not a
    // concentration, and counting only the matched ones would claim they were.
    let diluted = |index: usize, _: &UsageRecord| -> Option<Msisdn> {
        if index < 25 {
            Some(number("239212345678"))
        } else {
            Some(number("491701234567"))
        }
    };
    assert!(ait_signature(&resolve(&messages, &diluted), &ranges, 50, 80).is_none());
}

// ── Determinism ──────────────────────────────────────────────────────────────

/// Every detector takes its evaluation inputs as arguments and reads no clock,
/// so the same batch yields the same indications however often it is replayed.
#[test]
fn detection_is_replayable() {
    let records = vec![
        record(
            RecordType::OriginatedCall,
            "2026-07-28T10:00:00+02:00",
            "262-02",
        ),
        record(
            RecordType::OriginatedCall,
            "2026-07-28T10:20:00+02:00",
            "310-260",
        ),
        record(
            RecordType::OriginatedCall,
            "2026-07-28T10:40:00+02:00",
            "262-02",
        ),
    ];

    let first = impossible_travel(&records, &UniformTransit::new(Duration::hours(4)));
    let second = impossible_travel(&records, &UniformTransit::new(Duration::hours(4)));
    assert_eq!(first, second);
    assert_eq!(first.len(), 2, "there and back again");
    assert!(first.iter().all(|i| i.signal.is_arithmetic()));
}

/// A national numbering plan is hundreds of thousands of ranges and every call
/// needs one lookup. A linear scan is fine on the toy set a test builds and is
/// why a run that worked in staging takes a week in production — so the lookup
/// probes the number's own prefixes rather than walking the ranges.
#[test]
fn a_lookup_is_independent_of_how_many_ranges_are_loaded() {
    let mut ranges: NumberRangeSet<u32> = NumberRangeSet::new();
    // 100 000 ranges, none of which matches the number under test.
    for block in 0..100_000u32 {
        let prefix = format!("1{block:06}");
        ranges.insert(prefix.parse().unwrap(), block);
    }
    ranges.insert("49151".parse().unwrap(), 1);
    ranges.insert("4915123".parse().unwrap(), 2);

    let number: Msisdn = "491512345678".parse().unwrap();
    let (range, value) = ranges.longest_match(number).unwrap();

    assert_eq!(range.to_string(), "4915123", "longest prefix wins");
    assert_eq!(*value, 2);
    assert_eq!(ranges.len(), 100_002);
}

/// A duplicated range in reference data means two answers to one question, and
/// which one wins is otherwise an accident of load order.
#[test]
fn a_duplicated_range_keeps_both_values_and_is_reported() {
    let mut ranges: NumberRangeSet<&str> = NumberRangeSet::new();
    ranges.insert("49151".parse().unwrap(), "first");
    ranges.insert("49151".parse().unwrap(), "second");
    ranges.insert("49152".parse().unwrap(), "only");

    let number: Msisdn = "491511234567".parse().unwrap();
    assert_eq!(*ranges.longest_match(number).unwrap().1, "first");
    assert_eq!(
        ranges.longest_match_all(number).unwrap().1,
        ["first", "second"]
    );

    assert_eq!(
        ranges
            .duplicates()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["49151"]
    );
    assert_eq!(ranges.ranges(), 2);
    assert_eq!(ranges.len(), 3);
}

/// A range at least as long as the number does not contain it: `49151` is not a
/// block that `4915` belongs to.
#[test]
fn a_range_longer_than_the_number_does_not_match() {
    let mut ranges: NumberRangeSet<&str> = NumberRangeSet::new();
    ranges.insert("49151".parse().unwrap(), "mobile");

    assert!(ranges.longest_match("4915".parse().unwrap()).is_none());
    assert!(ranges.longest_match("49151".parse().unwrap()).is_some());
}

/// An unmodelled country pair is not evidence of anything. A fraud signal
/// raised out of ignorance costs an analyst the same time as a real one and
/// teaches them to discount the next.
#[test]
fn an_unknown_country_pair_never_fires() {
    let records = vec![
        record(
            RecordType::OriginatedCall,
            "2026-07-28T10:00:00+02:00",
            "262-02",
        ),
        record(
            RecordType::OriginatedCall,
            "2026-07-28T10:20:00+02:00",
            "310-260",
        ),
    ];

    // A resolver that models nothing.
    let silent = |_: Mcc, _: Mcc| None;
    assert!(impossible_travel(&records, &silent).is_empty());

    // One that models this pair generously does not fire either.
    let generous = |_: Mcc, _: Mcc| Some(Duration::minutes(10));
    assert!(impossible_travel(&records, &generous).is_empty());

    // And one that knows the journey is long does.
    let realistic = |from: Mcc, to: Mcc| {
        if from.is_north_american() || to.is_north_american() {
            Some(Duration::hours(8))
        } else {
            Some(Duration::hours(1))
        }
    };
    assert_eq!(impossible_travel(&records, &realistic).len(), 1);
}
