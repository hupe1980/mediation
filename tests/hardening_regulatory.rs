//! Regression tests for the regulatory and detector findings R1–R17 of the
//! audit-hardening review.
//!
//! Tests tagged `audit-hardening/SR-003` prove that no input panics; tests
//! tagged `audit-hardening/SR-007` prove that fair use, retention and lateness
//! never admit what the regulation forbids or act against the customer.

use mediation::{
    CdrTimestamp, ChangeCondition, ChargedParty, ContainerSet, FairUseAlert, FairUseAssessment,
    LatenessPolicy, Mcc, Msisdn, NumberPrefix, NumberRange, NumberRangeSet, ObservationWindow,
    Octets, PlmnId, PresenceDay, Pseudonym, Qos5qi, RatingGroup, RecordType, Region,
    RetentionBasis, RetentionPolicy, RetentionSchedule, Seconds, ServiceDataContainer, SourceRef,
    SplitError, SubscriberRef, TransitTimes, UniformTransit, UsageRecord, VolumePair,
    ait_signature, fairuse, impossible_travel, split_measured, wangiri_pattern,
};
use std::panic::catch_unwind;
use time::{
    Date, Duration, OffsetDateTime,
    macros::{date, datetime},
};

fn rec(kind: RecordType, imsi: &str, opened: &str) -> UsageRecord {
    UsageRecord::new(
        kind,
        ChargedParty::Subscriber(SubscriberRef::new(imsi.parse().unwrap())),
        opened.parse().unwrap(),
        SourceRef::new("n-01".parse().unwrap(), 0),
    )
}

fn ts(s: &str) -> CdrTimestamp {
    s.parse().unwrap()
}

fn home() -> PlmnId {
    "262-02".parse().unwrap()
}

// ── Fair use ─────────────────────────────────────────────────────────────────

/// R1: a day logged on at home is a day of domestic presence, even if the
/// subscriber was also seen abroad that day (EU 2016/2286 Art. 4(4)).
/// audit-hardening/SR-007
#[test]
fn r1_a_day_with_domestic_presence_is_domestic_only() {
    let window = ObservationWindow::minimum();
    let ending = date!(2026 - 04 - 30);
    let start = window.start(ending).unwrap();
    let mut presence = Vec::new();
    let mut day = start;
    while day <= ending {
        presence.push(PresenceDay::new(day, Region::Domestic));
        presence.push(PresenceDay::new(day, Region::Eea));
        day = day.next_day().unwrap();
    }
    let a = fairuse::assess(
        &[],
        &presence,
        home(),
        window,
        ending,
        &|_s: PlmnId, _h: PlmnId| Region::Eea,
    );
    assert_eq!(a.roaming_presence_days(), 0);
    assert_eq!(
        u64::from(a.domestic_presence_days()),
        u64::try_from((ending - start).whole_days() + 1).unwrap()
    );
    assert!(a.prevailing_domestic_presence());
}

/// R2: the window is four calendar months, with the end-of-month clamp, and
/// never shorter than four months.
/// audit-hardening/SR-007
#[test]
fn r2_the_window_is_counted_in_calendar_months() {
    let four = ObservationWindow::months(4).unwrap();
    assert_eq!(four, ObservationWindow::minimum());
    assert_eq!(ObservationWindow::months(3), None);
    assert_eq!(four.month_count(), 4);

    // 122 days, not 120.
    assert_eq!(
        four.start(date!(2026 - 07 - 28)),
        Some(date!(2026 - 03 - 29))
    );
    // A month ending on its last day starts on a first.
    assert_eq!(
        four.start(date!(2026 - 06 - 30)),
        Some(date!(2026 - 03 - 01))
    );
    // Clamping lengthens: 30 June minus four months is "30 February".
    assert_eq!(
        four.start(date!(2026 - 06 - 29)),
        Some(date!(2026 - 02 - 28))
    );
    assert_eq!(
        four.start(date!(2024 - 06 - 29)),
        Some(date!(2024 - 02 - 29))
    );
    // A year boundary.
    assert_eq!(
        four.start(date!(2026 - 02 - 15)),
        Some(date!(2025 - 10 - 16))
    );
    assert_eq!(
        ObservationWindow::months(12)
            .unwrap()
            .start(date!(2026 - 12 - 31)),
        Some(date!(2026 - 01 - 01))
    );
}

/// R3: the window start is checked arithmetic and never panics.
/// audit-hardening/SR-003
#[test]
fn r3_the_window_start_never_panics() {
    let huge = ObservationWindow::months(u32::MAX).unwrap();
    assert!(catch_unwind(|| huge.start(date!(2026 - 01 - 01))).is_ok());
    assert_eq!(huge.start(date!(2026 - 01 - 01)), None);
    for ending in [Date::MIN, Date::MAX] {
        assert!(catch_unwind(|| ObservationWindow::minimum().start(ending)).is_ok());
    }
    assert_eq!(ObservationWindow::minimum().start(Date::MIN), None);
    assert_eq!(
        ObservationWindow::minimum().start(Date::MAX),
        Some(
            Date::MAX
                .replace_day(1)
                .unwrap()
                .replace_month(time::Month::September)
                .unwrap()
        )
    );
    let region = |_s: PlmnId, _h: PlmnId| Region::Eea;
    for ending in [Date::MIN, Date::MAX] {
        assert!(catch_unwind(|| fairuse::assess(&[], &[], home(), huge, ending, &region)).is_ok());
    }
}

/// R4: an alert issued before the observation window does not rest on it.
/// audit-hardening/SR-007
#[test]
fn r4_an_alert_older_than_the_window_is_refused() {
    let start = date!(2026 - 03 - 01);
    let a = FairUseAssessment::from_counts(10, 110, Octets::new(1), Octets::new(9), 120, start);
    assert!(a.may_warn());
    assert!(!a.may_surcharge(
        Some(FairUseAlert::new(date!(2020 - 01 - 01))),
        date!(2026 - 07 - 01)
    ));
    assert!(!a.may_surcharge(
        Some(FairUseAlert::new(date!(2026 - 02 - 28))),
        date!(2026 - 07 - 01)
    ));
    assert!(a.may_surcharge(Some(FairUseAlert::new(start)), date!(2026 - 07 - 01)));
    assert_eq!(a.window_start(), start);
}

/// R4: `assess` records the start it observed from.
/// audit-hardening/SR-007
#[test]
fn r4_assess_carries_the_window_start() {
    let ending = date!(2026 - 07 - 28);
    let a = fairuse::assess(
        &[],
        &[],
        home(),
        ObservationWindow::minimum(),
        ending,
        &|_s: PlmnId, _h: PlmnId| Region::Eea,
    );
    assert_eq!(a.window_start(), date!(2026 - 03 - 29));
}

/// R4: the window start survives serde.
#[cfg(feature = "serde")]
#[test]
fn r4_the_window_start_round_trips() {
    let a = FairUseAssessment::from_counts(
        1,
        2,
        Octets::new(3),
        Octets::new(4),
        5,
        date!(2026 - 03 - 29),
    );
    let json = serde_json::to_string(&a).unwrap();
    assert!(json.contains("\"window_start\":\"2026-03-29\""), "{json}");
    assert_eq!(serde_json::from_str::<FairUseAssessment>(&json).unwrap(), a);
}

// ── Retention ────────────────────────────────────────────────────────────────

const FROM: OffsetDateTime = datetime!(2026-01-01 0:00 UTC);

/// R5: a deadline outside the calendar is refused at construction, so
/// `deadline` never panics.
/// audit-hardening/SR-003
#[test]
fn r5_retention_deadlines_are_checked() {
    let basis = RetentionBasis::OperatorPolicy { from: FROM };
    assert_eq!(RetentionPolicy::new(basis, Duration::MAX), None);
    assert_eq!(
        RetentionPolicy::new(basis, Duration::seconds(i64::MAX)),
        None
    );
    let edge = RetentionBasis::OperatorPolicy {
        from: Date::MAX.with_hms(23, 59, 59).unwrap().assume_utc(),
    };
    assert_eq!(RetentionPolicy::new(edge, Duration::seconds(1)), None);
}

/// R5: serde refuses what `new` refuses, so no deserialized schedule panics.
/// audit-hardening/SR-003
#[cfg(feature = "serde")]
#[test]
fn r5_serde_refuses_an_overflowing_retention_period() {
    let json = r#"{"basis":{"kind":"OPERATOR_POLICY","from":"2026-01-01T00:00:00+00:00"},"duration_s":9223372036854775807}"#;
    let parsed = catch_unwind(|| serde_json::from_str::<RetentionPolicy>(json));
    assert!(parsed.expect("no panic").is_err());
    let negative = r#"{"basis":{"kind":"OPERATOR_POLICY","from":"2026-01-01T00:00:00+00:00"},"duration_s":-9223372036854775808}"#;
    assert!(serde_json::from_str::<RetentionPolicy>(negative).is_err());
}

/// R6: retention is whole seconds — duration and anchor — so the stored form
/// round-trips exactly.
/// audit-hardening/SR-007
#[test]
fn r6_sub_second_retention_is_refused() {
    let basis = RetentionBasis::OperatorPolicy { from: FROM };
    assert_eq!(
        RetentionPolicy::new(basis, Duration::milliseconds(500)),
        None
    );
    assert_eq!(
        RetentionPolicy::new(basis, Duration::milliseconds(1_500)),
        None
    );
    assert!(RetentionPolicy::new(basis, Duration::seconds(1)).is_some());

    let fractional = datetime!(2026-01-01 0:00:00.7 UTC);
    assert_eq!(
        RetentionPolicy::new(
            RetentionBasis::BillingTdddg10 {
                invoice_dispatched_at: fractional
            },
            Duration::days(1)
        ),
        None
    );
    assert_eq!(RetentionPolicy::billing_tdddg10(fractional), None);
    assert_eq!(
        RetentionPolicy::until_released(RetentionBasis::DisputeHold {
            raised_at: fractional
        }),
        None
    );
}

/// R6: every constructible policy survives serde unchanged.
/// audit-hardening/SR-007
#[cfg(feature = "serde")]
#[test]
fn r6_retention_round_trips_exactly() {
    let policies = [
        RetentionPolicy::billing_tdddg10(datetime!(2026-08-31 09:15:00 +02:00)).unwrap(),
        RetentionPolicy::new(
            RetentionBasis::LawfulAccess { recorded_at: FROM },
            Duration::days(90),
        )
        .unwrap(),
        RetentionPolicy::until_released(RetentionBasis::DisputeHold { raised_at: FROM }).unwrap(),
    ];
    for policy in policies {
        let json = serde_json::to_string(&policy).unwrap();
        let back: RetentionPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(back, policy, "{json}");
        assert_eq!(back.deadline(), policy.deadline());
    }
}

/// R7: § 10 Abs. 2 TDDDG is six calendar months from dispatch, and no longer.
/// audit-hardening/SR-007
#[test]
fn r7_billing_retention_is_at_most_six_calendar_months() {
    let cases = [
        (
            datetime!(2026-08-05 0:00 UTC),
            datetime!(2027-02-05 0:00 UTC),
        ),
        (
            datetime!(2026-08-31 0:00 UTC),
            datetime!(2027-02-28 0:00 UTC),
        ),
        (
            datetime!(2027-08-31 0:00 UTC),
            datetime!(2028-02-29 0:00 UTC),
        ),
        (
            datetime!(2026-12-31 12:00 UTC),
            datetime!(2027-06-30 12:00 UTC),
        ),
    ];
    for (dispatched, limit) in cases {
        let policy = RetentionPolicy::billing_tdddg10(dispatched).unwrap();
        assert_eq!(policy.deadline(), Some(limit));
        let basis = RetentionBasis::BillingTdddg10 {
            invoice_dispatched_at: dispatched,
        };
        assert!(RetentionPolicy::new(basis, limit - dispatched).is_some());
        assert_eq!(
            RetentionPolicy::new(basis, limit - dispatched + Duration::seconds(1)),
            None
        );
    }
    // Ten years under § 10 is not a § 10 policy.
    assert_eq!(
        RetentionPolicy::new(
            RetentionBasis::BillingTdddg10 {
                invoice_dispatched_at: FROM
            },
            Duration::days(3_650)
        ),
        None
    );
    // The earlier of the dispatch offset's and UTC's reading applies: local
    // 31 August is UTC 30 August, so the UTC reading (28 Feb) is not
    // exceeded via the local one (28 Feb local = 27 Feb 22:00 UTC).
    let local = datetime!(2026-08-31 01:00 +02:00);
    let policy = RetentionPolicy::billing_tdddg10(local).unwrap();
    assert_eq!(policy.deadline(), Some(datetime!(2027-02-27 23:00 UTC)));
}

/// R7: a dispute hold ends when the dispute is resolved, never on a date.
/// audit-hardening/SR-007
#[test]
fn r7_a_dated_dispute_hold_is_refused() {
    let hold = RetentionBasis::DisputeHold { raised_at: FROM };
    assert_eq!(RetentionPolicy::new(hold, Duration::days(1)), None);
    let open = RetentionPolicy::until_released(hold).unwrap();
    let schedule: RetentionSchedule = vec![open].into_iter().collect();
    assert!(schedule.is_held());
    assert!(!schedule.is_deletable(datetime!(2099-01-01 0:00 UTC)));
}

/// R7: serde refuses a dated hold and an over-long § 10 period.
/// audit-hardening/SR-007
#[cfg(feature = "serde")]
#[test]
fn r7_serde_refuses_what_the_regulation_forbids() {
    let dated_hold = r#"{"basis":{"kind":"DISPUTE_HOLD","raised_at":"2026-01-01T00:00:00+00:00"},"duration_s":86400}"#;
    assert!(serde_json::from_str::<RetentionPolicy>(dated_hold).is_err());
    let ten_years = r#"{"basis":{"kind":"BILLING_TDDDG_10","invoice_dispatched_at":"2026-01-01T00:00:00+00:00"},"duration_s":315360000}"#;
    assert!(serde_json::from_str::<RetentionPolicy>(ten_years).is_err());
    let six_months = r#"{"basis":{"kind":"BILLING_TDDDG_10","invoice_dispatched_at":"2026-01-01T00:00:00+00:00"},"duration_s":15638400}"#;
    assert_eq!(
        serde_json::from_str::<RetentionPolicy>(six_months)
            .unwrap()
            .deadline(),
        Some(datetime!(2026-07-01 0:00 UTC))
    );
}

// ── Lateness ─────────────────────────────────────────────────────────────────

/// R8: lateness instants are checked, for any deserialized bound.
/// audit-hardening/SR-003
#[cfg(feature = "serde")]
#[test]
fn r8_lateness_instants_never_panic() {
    let policy: LatenessPolicy = serde_json::from_str(
        r#"{"late_after_s":9223372036854775807,"deadline_after_s":9223372036854775807}"#,
    )
    .unwrap();
    let event = datetime!(2026-01-01 0:00 UTC);
    assert_eq!(
        catch_unwind(|| policy.late_at(event)).expect("no panic"),
        None
    );
    assert_eq!(
        catch_unwind(|| policy.deadline_at(event)).expect("no panic"),
        None
    );
    assert!(catch_unwind(|| policy.classify(event, event)).is_ok());
}

/// R8: in range, the instants are what they were.
/// audit-hardening/SR-003
#[test]
fn r8_lateness_instants_inside_the_calendar() {
    let policy = LatenessPolicy::tap3();
    let event = datetime!(2026-07-01 0:00 UTC);
    assert_eq!(policy.late_at(event), Some(datetime!(2026-07-31 0:00 UTC)));
    let edge = Date::MAX.with_hms(0, 0, 0).unwrap().assume_utc();
    assert_eq!(policy.late_at(edge), None);
    assert_eq!(policy.deadline_at(edge), None);
}

/// R9: bounds are whole seconds, and the deadline is compared in full.
/// audit-hardening/SR-003
#[test]
fn r9_lateness_bounds_are_whole_seconds() {
    assert_eq!(LatenessPolicy::advisory(Duration::milliseconds(500)), None);
    assert_eq!(
        LatenessPolicy::deadline(Duration::milliseconds(1_500)),
        None
    );
    // A 1.2 s deadline below a 1.5 s threshold was accepted on whole seconds.
    assert_eq!(
        LatenessPolicy::new(Duration::milliseconds(1_500), Duration::milliseconds(1_200)),
        None
    );
    assert_eq!(
        LatenessPolicy::new(Duration::seconds(2), Duration::seconds(1)),
        None
    );
    assert!(LatenessPolicy::new(Duration::seconds(1), Duration::seconds(1)).is_some());
}

// ── Privacy ──────────────────────────────────────────────────────────────────

/// R10: only 64 lower-case hex characters parse, and no input panics.
/// audit-hardening/SR-003
#[test]
fn r10_a_pseudonym_parses_only_its_rendered_form() {
    let good = "0123456789abcdef".repeat(4);
    let parsed: Pseudonym = good.parse().unwrap();
    assert_eq!(parsed.to_string(), good);

    let multibyte = format!("a{}", "€".repeat(21)); // 64 bytes, not 64 chars
    assert_eq!(multibyte.len(), 64);
    let plus = format!("+f{}", "0".repeat(62));
    let upper = "0123456789ABCDEF".repeat(4);
    let spaced = format!(" {}", &good[1..]);
    for bad in [multibyte.as_str(), &plus, &upper, &spaced, "", &good[..62]] {
        let result = catch_unwind(|| bad.parse::<Pseudonym>()).expect("no panic");
        assert!(result.is_err(), "{bad:?}");
        assert!(result.unwrap_err().is_withheld());
    }
}

/// R10: serde uses the same parser.
/// audit-hardening/SR-003
#[cfg(feature = "serde")]
#[test]
fn r10_pseudonym_deserialization_never_panics() {
    let multibyte = format!("\"a{}\"", "€".repeat(21));
    let plus = format!("\"+f{}\"", "0".repeat(62));
    for bad in [multibyte, plus] {
        let result = catch_unwind(|| serde_json::from_str::<Pseudonym>(&bad)).expect("no panic");
        assert!(result.is_err());
    }
    let p = Pseudonym::from_digest([0xab; 32]);
    let json = serde_json::to_string(&p).unwrap();
    assert_eq!(serde_json::from_str::<Pseudonym>(&json).unwrap(), p);
}

/// R11: a number prefix parses its rendering.
/// audit-hardening/SR-003
#[test]
fn r11_a_number_prefix_round_trips() {
    let n: Msisdn = "491701234567".parse().unwrap();
    let prefix = NumberPrefix::without_last(n, 3).unwrap();
    assert_eq!(prefix.to_string().parse::<NumberPrefix>().unwrap(), prefix);
    assert_eq!("4".parse::<NumberPrefix>().unwrap().len(), 1);
    assert_eq!("49170123456789".parse::<NumberPrefix>().unwrap().len(), 14);
    for bad in ["", "491701234567890", "+49", "49 170", "4x", "٤٩"] {
        let result = catch_unwind(|| bad.parse::<NumberPrefix>()).expect("no panic");
        assert!(result.is_err(), "{bad:?}");
    }
}

/// R11: and deserializes it.
/// audit-hardening/SR-003
#[cfg(feature = "serde")]
#[test]
fn r11_a_number_prefix_deserializes() {
    let n: Msisdn = "491701234567".parse().unwrap();
    let prefix = NumberPrefix::of(n, 5).unwrap();
    let json = serde_json::to_string(&prefix).unwrap();
    assert_eq!(json, "\"49170\"");
    assert_eq!(serde_json::from_str::<NumberPrefix>(&json).unwrap(), prefix);
    assert!(serde_json::from_str::<NumberPrefix>("\"491701234567890\"").is_err());
}

// ── Measured split ───────────────────────────────────────────────────────────

fn sdc(rg: u32, cc: ChangeCondition, up: u64, at: &str) -> ServiceDataContainer {
    ServiceDataContainer::new(RatingGroup::new(rg), cc)
        .with_volume(VolumePair::new(Octets::new(up), Octets::ZERO))
        .with_closed(ts(at))
}

fn two_hour_session(containers: Vec<ServiceDataContainer>) -> UsageRecord {
    rec(
        RecordType::PduSession,
        "imsi-262021234567890",
        "2026-07-28T07:00:00+00:00",
    )
    .with_closed(ts("2026-07-28T09:00:00+00:00"))
    .with_containers(ContainerSet::from(containers))
}

/// R12: containers of several rating groups are grouped by close instant, in
/// any order, and the split conserves.
#[test]
fn r12_measured_split_groups_rating_groups_by_close_instant() {
    let interleaved = vec![
        sdc(
            1,
            ChangeCondition::TariffTime,
            100,
            "2026-07-28T08:00:00+00:00",
        ),
        sdc(
            2,
            ChangeCondition::TariffTime,
            200,
            "2026-07-28T08:00:00+00:00",
        ),
        sdc(
            1,
            ChangeCondition::RecordClosure,
            10,
            "2026-07-28T09:00:00+00:00",
        ),
        sdc(
            2,
            ChangeCondition::RecordClosure,
            20,
            "2026-07-28T09:00:00+00:00",
        ),
    ];
    let by_group = vec![
        interleaved[0].clone(),
        interleaved[2].clone(),
        interleaved[1].clone(),
        interleaved[3].clone(),
    ];
    let mut reversed = interleaved.clone();
    reversed.reverse();

    for containers in [interleaved, by_group, reversed] {
        let record = two_hour_session(containers);
        let split = split_measured(&record).unwrap();
        assert_eq!(split.len(), 2);
        let up: Vec<u64> = split
            .slices()
            .iter()
            .map(|s| s.volume.unwrap().uplink().get())
            .collect();
        assert_eq!(up, vec![300, 30]);
        let wall: Vec<u32> = split.slices().iter().map(|s| s.wall_clock.get()).collect();
        assert_eq!(wall, vec![3_600, 3_600]);
        assert_eq!(split.total_volume(), record.containers.checked_total());
        assert_eq!(split.slices()[0].end, split.slices()[1].start);
    }
}

/// R13: a container closing after the record closed is refused.
#[test]
fn r13_a_container_after_the_record_close_is_refused() {
    let record = two_hour_session(vec![sdc(
        1,
        ChangeCondition::RecordClosure,
        10,
        "2026-07-29T09:00:00+00:00",
    )]);
    assert_eq!(
        split_measured(&record),
        Err(SplitError::ContainerAfterRecordClose)
    );
    // At the record's close is inside it.
    let record = two_hour_session(vec![sdc(
        1,
        ChangeCondition::RecordClosure,
        10,
        "2026-07-28T09:00:00+00:00",
    )]);
    assert_eq!(
        split_measured(&record).unwrap().slices()[0].wall_clock,
        Seconds::new(7_200)
    );
}

// ── Fraud ────────────────────────────────────────────────────────────────────

/// R14: two MCCs of one country are no journey.
#[test]
fn r14_impossible_travel_compares_countries_not_codes() {
    let records = vec![
        rec(
            RecordType::OriginatedCall,
            "imsi-310260123456789",
            "2026-07-28T10:00:00+00:00",
        )
        .with_networks("310-260".parse().unwrap(), "310-260".parse().unwrap()),
        rec(
            RecordType::OriginatedCall,
            "imsi-310260123456789",
            "2026-07-28T10:05:00+00:00",
        )
        .with_networks("311-480".parse().unwrap(), "310-260".parse().unwrap()),
    ];
    let transit = UniformTransit::new(Duration::hours(4));
    assert!(impossible_travel(&records, &transit).is_empty());
    let (us, us2, de): (Mcc, Mcc, Mcc) = (
        "310".parse().unwrap(),
        "311".parse().unwrap(),
        "262".parse().unwrap(),
    );
    assert_eq!(transit.minimum_transit(us, us2), None);
    assert_eq!(transit.minimum_transit(us, de), Some(Duration::hours(4)));
}

/// R15: Wangiri is many short calls to one subscriber within a window.
#[test]
fn r15_wangiri_groups_by_party_within_a_window() {
    // Five subscribers, one short call each, a month apart: not a wave.
    let spread: Vec<_> = (0..5)
        .map(|i| {
            rec(
                RecordType::TerminatedCall,
                &format!("imsi-26202123456789{i}"),
                &format!("2026-0{}-01T10:00:00+00:00", i + 1),
            )
            .with_duration(Seconds::new(2))
        })
        .collect();
    assert!(wangiri_pattern(&spread, Seconds::new(3), 5, Duration::days(365)).is_empty());

    // One subscriber, five calls in five minutes: a wave.
    let wave: Vec<_> = (0..5)
        .map(|i| {
            rec(
                RecordType::TerminatedCall,
                "imsi-262021234567890",
                &format!("2026-07-28T10:0{i}:00+00:00"),
            )
            .with_duration(Seconds::new(2))
        })
        .collect();
    let found = wangiri_pattern(&wave, Seconds::new(3), 5, Duration::minutes(10));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].records, vec![0, 1, 2, 3, 4]);
    // The same calls against a two-minute window are not five together.
    assert!(wangiri_pattern(&wave, Seconds::new(3), 5, Duration::minutes(2)).is_empty());
    // A non-positive window finds nothing.
    assert!(wangiri_pattern(&wave, Seconds::new(3), 1, Duration::ZERO).is_empty());

    // Two victims at once give two indications.
    let mut both = wave.clone();
    both.extend((0..5).map(|i| {
        rec(
            RecordType::TerminatedCall,
            "imsi-262021234567891",
            &format!("2026-07-28T10:0{i}:30+00:00"),
        )
        .with_duration(Seconds::new(1))
    }));
    assert_eq!(
        wangiri_pattern(&both, Seconds::new(3), 5, Duration::minutes(10)).len(),
        2
    );
}

/// R17: AIT counts the messages a sender originates, not ones it receives.
#[test]
fn r17_ait_counts_only_originated_messages() {
    let mut ranges: NumberRangeSet<&str> = NumberRangeSet::new();
    ranges.insert("2392".parse::<NumberRange>().unwrap(), "IPRN");
    let to: Msisdn = "239212345678".parse().unwrap();
    let message = |kind, i: usize| {
        rec(
            kind,
            "imsi-262021234567890",
            &format!("2026-07-28T10:{:02}:00+00:00", i % 60),
        )
        .with_called(to.into())
    };
    let terminated: Vec<_> = (0..100)
        .map(|i| message(RecordType::SmsTerminated, i))
        .collect();
    assert!(ait_signature(&terminated, &ranges, 50, 80).is_none());

    let originated: Vec<_> = (0..100)
        .map(|i| message(RecordType::SmsOriginated, i))
        .collect();
    let mut mixed = originated.clone();
    mixed.extend(terminated);
    let found = ait_signature(&mixed, &ranges, 50, 80).expect("100 originated to one range");
    assert_eq!(found.records.len(), 100);
    assert!(found.records.iter().all(|i| *i < 100));
}

/// 5QI 255 is reserved, not operator specific.
#[test]
fn qos_5qi_255_is_reserved() {
    let reserved = Qos5qi::new(255).unwrap();
    assert!(reserved.is_reserved());
    assert!(!reserved.is_standardised());
    assert!(!reserved.is_operator_specific());
    assert!(Qos5qi::new(254).unwrap().is_operator_specific());
    assert!(Qos5qi::new(128).unwrap().is_operator_specific());
    assert!(!Qos5qi::new(127).unwrap().is_operator_specific());
    assert!(!Qos5qi::new(9).unwrap().is_reserved());
}
