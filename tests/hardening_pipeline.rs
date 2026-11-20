//! Regression tests for the conservation and dedup findings of the 2026-11-20
//! audit (C1–C16, `specs/002-audit-hardening/research.md`).
//!
//! Each test fails on the code the audit read and names its finding.

use mediation::{
    Batch, ChargedParty, ChargingId, ContainerSet, CorrelationKeys, DedupWindow, Disposition,
    Finding, HoldReason, Octets, OrphanReason, PartialFormat, PartialInfo, PartialRun,
    ProcessConfig, ProcessOutcome, ProcessState, RatingGroup, Reassembly, RecordClosureCause,
    RecordKey, RecordType, RejectReason, Rule, Seconds, SequenceObservation, SequenceSpace,
    SequenceTracker, ServiceDataContainer, SourceRef, SubscriberRef, UsageRecord, VolumePair,
    process,
};
use std::net::Ipv4Addr;
use time::{Duration, OffsetDateTime, macros::datetime};

const NOW: OffsetDateTime = datetime!(2026-07-28 12:00:00 UTC);
const OPENED: &str = "2026-07-28T10:00:00+00:00";

fn week() -> DedupWindow {
    DedupWindow::from_bound(Duration::days(7)).unwrap()
}

fn session(charging_id: u32, opened: &str, file: u32) -> UsageRecord {
    UsageRecord::new(
        RecordType::PduSession,
        ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
        opened.parse().unwrap(),
        SourceRef::new("pgw-01".parse().unwrap(), 0).with_file_sequence(file),
    )
    .with_volume(VolumePair::new(Octets::new(1_000), Octets::new(4_000)))
    .with_networks("262-01".parse().unwrap(), "262-01".parse().unwrap())
    .with_correlation(
        CorrelationKeys::default()
            .with_charging_id(ChargingId::new(charging_id))
            .with_gateway(Ipv4Addr::new(10, 0, 0, 1).into()),
    )
}

fn run(
    records: &[UsageRecord],
    carried: usize,
    state: &mut ProcessState,
    at: OffsetDateTime,
) -> ProcessOutcome {
    let outcome = process(
        &Batch::with_carry(records, carried).unwrap(),
        &ProcessConfig::default(),
        at,
        state,
    );
    assert!(outcome.reconciliation().balances());
    outcome
}

fn partial(n: u32, format: PartialFormat, cause: RecordClosureCause) -> UsageRecord {
    let mut record = session(77, OPENED, 1);
    record.partial = Some(PartialInfo::new(n, format, cause));
    record.correlation.record_sequence_number = Some(n);
    record
}

fn p1() -> UsageRecord {
    partial(
        1,
        PartialFormat::FullyQualified,
        RecordClosureCause::VolumeLimit,
    )
}
fn p2() -> UsageRecord {
    partial(2, PartialFormat::Reduced, RecordClosureCause::VolumeLimit)
}
fn p3() -> UsageRecord {
    partial(3, PartialFormat::Reduced, RecordClosureCause::NormalRelease)
}

fn count(outcome: &ProcessOutcome, rule: Rule) -> usize {
    outcome
        .findings()
        .findings()
        .iter()
        .filter(|f| f.rule == rule)
        .count()
}

/// C1: a late record is stamped with its arrival, so evicting after its
/// batch keeps its key and the resend is a duplicate.
///
/// audit-hardening/SR-001
#[test]
fn c1_a_late_record_resent_after_eviction_is_a_duplicate() {
    let mut state = ProcessState::new(week());
    let late = session(1, "2026-07-18T10:00:00+00:00", 1); // ten days old
    let first = run(std::slice::from_ref(&late), 0, &mut state, NOW);
    assert_eq!(first.reconciliation().released(), 1);

    assert_eq!(state.dedup_mut().evict(NOW), 0);
    let resend = run(&[late], 0, &mut state, NOW + Duration::hours(1));
    assert_eq!(resend.reconciliation().released(), 0);
    assert_eq!(resend.reconciliation().duplicate(), 1);
}

/// C2: a record held longer than the window keeps its key while carried, so
/// a resend arriving beside the carried copy is billed once.
///
/// audit-hardening/SR-001
#[test]
fn c2_a_carried_record_keeps_its_key_past_the_window() {
    let mut state = ProcessState::new(week());
    let a = session(1, OPENED, 1);
    let c = session(2, OPENED, 3); // file 2 is missing
    let first = run(&[a.clone(), c.clone()], 0, &mut state, NOW);
    assert_eq!(first.reconciliation().held(), 2);

    let later = NOW + Duration::days(8);
    state.dedup_mut().evict(later);
    let b = session(3, "2026-08-05T10:00:00+00:00", 2);
    let second = run(&[a.clone(), c, b, a], 2, &mut state, later);
    assert_eq!(second.reconciliation().released(), 3);
    assert_eq!(second.reconciliation().duplicate(), 1);
    assert!(
        !second.findings().has(Rule::SequenceGap),
        "nothing regressed"
    );
}

/// C3, C11: partials #2 and #3 one batch before #1 are held with V20, not
/// rejected, and the run bills once when #1 arrives.
///
/// audit-hardening/SR-002
#[test]
fn c3_partials_before_their_first_are_held_and_bill_once() {
    let mut state = ProcessState::new(week());
    let early = run(&[p2(), p3()], 0, &mut state, NOW);
    assert_eq!(
        early.dispositions(),
        vec![Disposition::Held(HoldReason::AwaitingPartial); 2]
    );
    assert_eq!(count(&early, Rule::IncompleteReassembly), 2);
    assert!(early.is_releasable(), "a hold is not a fault");

    let complete = run(&[p2(), p3(), p1()], 2, &mut state, NOW + Duration::hours(1));
    assert_eq!(complete.reconciliation().released(), 3);

    let resend = run(&[p1(), p2(), p3()], 0, &mut state, NOW + Duration::hours(2));
    assert_eq!(resend.reconciliation().duplicate(), 3);
}

/// C3, C11: a run whose sequence 1 is reduced can never complete; it is
/// rejected with V19.
#[test]
fn c3_a_reduced_first_partial_is_rejected_with_v19() {
    let mut state = ProcessState::new(week());
    let reduced_first = partial(1, PartialFormat::Reduced, RecordClosureCause::VolumeLimit);
    let outcome = run(&[reduced_first, p3()], 0, &mut state, NOW);
    assert_eq!(
        outcome.dispositions(),
        vec![Disposition::Rejected(RejectReason::Malformed); 2]
    );
    assert_eq!(count(&outcome, Rule::OrphanPartial), 2);
    assert!(state.dedup().is_empty(), "rejected records leave no key");
}

/// C4: partials without a `recordSequenceNumber` key on their partial
/// sequence, so a run of three bills three.
///
/// audit-hardening/SR-002
#[test]
fn c4_the_partial_sequence_comes_from_the_partial_info() {
    let parts: Vec<UsageRecord> = [p1(), p2(), p3()]
        .into_iter()
        .map(|mut r| {
            r.correlation.record_sequence_number = None;
            r
        })
        .collect();
    let keys: Vec<RecordKey> = parts.iter().map(|r| RecordKey::of(r).unwrap()).collect();
    assert_ne!(keys[0], keys[1]);
    assert_ne!(keys[1], keys[2]);

    let mut state = ProcessState::new(week());
    let outcome = run(&parts, 0, &mut state, NOW);
    assert_eq!(outcome.reconciliation().released(), 3);
}

fn call(record_type: RecordType) -> UsageRecord {
    let mut record = UsageRecord::new(
        record_type,
        ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
        OPENED.parse().unwrap(),
        SourceRef::new("msc-01".parse().unwrap(), 0),
    )
    .with_duration(Seconds::new(60))
    .with_closed("2026-07-28T10:01:00+00:00".parse().unwrap())
    .with_networks("262-01".parse().unwrap(), "262-01".parse().unwrap());
    record.correlation = CorrelationKeys::default()
        .with_recording_entity("msc-01".parse().unwrap())
        .with_call_id("+4915112345678@x".parse().unwrap());
    record
}

/// C5: the originating and terminating records of one on-net call share a
/// Call-ID on one node, and both are usage.
///
/// audit-hardening/SR-002
#[test]
fn c5_originating_and_terminating_records_of_one_call_both_bill() {
    let mo = call(RecordType::OriginatedCall);
    let mt = call(RecordType::TerminatedCall);
    let (mo_key, mt_key) = (RecordKey::of(&mo).unwrap(), RecordKey::of(&mt).unwrap());
    assert_ne!(mo_key, mt_key);
    assert_eq!(
        mo_key.to_string(),
        "CALL:msc-01/ORIGINATED_CALL/+4915112345678@x"
    );
    assert_eq!(mo_key.session_scope().unwrap(), mo_key);

    let mut state = ProcessState::new(week());
    let outcome = run(&[mo.clone(), mt], 0, &mut state, NOW);
    assert_eq!(outcome.reconciliation().released(), 2);

    // A duplicate finding still names only the node, never the Call-ID.
    assert_eq!(Finding::duplicate(0, &mo_key).detail, "CALL:msc-01");
}

/// C6: a malformed and a valid copy of one record, in either order, bill the
/// valid copy; a corrected resend after a rejected copy bills too.
///
/// audit-hardening/SR-001
#[test]
fn c6_a_rejected_copy_does_not_consume_the_key() {
    let mut bad = session(5, OPENED, 1);
    bad.closed = Some("2026-07-28T09:00:00+00:00".parse().unwrap());
    let good = session(5, OPENED, 1).with_closed("2026-07-28T10:10:00+00:00".parse().unwrap());

    for records in [[bad.clone(), good.clone()], [good.clone(), bad.clone()]] {
        let mut state = ProcessState::new(week());
        let outcome = run(&records, 0, &mut state, NOW);
        assert_eq!(outcome.reconciliation().released(), 1, "{records:?}");
    }

    let mut state = ProcessState::new(week());
    let rejected = run(std::slice::from_ref(&bad), 0, &mut state, NOW);
    assert_eq!(rejected.reconciliation().rejected(), 1);
    let corrected = run(&[good], 0, &mut state, NOW + Duration::hours(1));
    assert_eq!(corrected.reconciliation().released(), 1);
}

/// C7: a node's files 1 and 3, in either order, report file 2 missing — and
/// across the TAP wrap too.
///
/// audit-hardening/SR-002
#[test]
fn c7_gap_detection_does_not_depend_on_file_order() {
    for order in [[1, 3], [3, 1]] {
        let records: Vec<UsageRecord> = order
            .iter()
            .enumerate()
            .map(|(i, file)| session(10 + i as u32, OPENED, *file))
            .collect();
        let mut state = ProcessState::new(week());
        let outcome = run(&records, 0, &mut state, NOW);
        assert_eq!(outcome.sequence_gaps().len(), 1, "{order:?}");
        assert_eq!(outcome.sequence_gaps()[0].missing, vec![2]);
        assert_eq!(outcome.reconciliation().held(), 2);
    }

    for order in [[99_998, 2], [2, 99_998]] {
        let mut tracker = SequenceTracker::new(SequenceSpace::TAP);
        tracker.observe_all(order, 10);
        assert_eq!(tracker.missing(), vec![1, 99_999], "{order:?}");
        assert_eq!(tracker.high_water(), Some(2));
    }
}

/// C8: a counter reset raises V18 rather than passing silently.
#[test]
fn c8_a_counter_reset_raises_v18() {
    let mut state = ProcessState::in_space(week(), SequenceSpace::TAP);
    run(&[session(1, OPENED, 1_000)], 0, &mut state, NOW);
    let outcome = run(
        &[session(2, OPENED, 1), session(3, OPENED, 5)],
        0,
        &mut state,
        NOW,
    );
    let regressed: Vec<&Finding> = outcome
        .findings()
        .findings()
        .iter()
        .filter(|f| f.rule == Rule::SequenceGap && f.detail.contains("sequence regressed"))
        .collect();
    assert_eq!(regressed.len(), 2);
}

/// C8: a resent file whose every record is a duplicate is not a regression.
#[test]
fn c8_a_pure_resend_is_not_a_regression() {
    let mut state = ProcessState::new(week());
    run(
        &[session(1, OPENED, 1), session(2, OPENED, 2)],
        0,
        &mut state,
        NOW,
    );
    let outcome = run(&[session(1, OPENED, 1)], 0, &mut state, NOW);
    assert_eq!(outcome.reconciliation().duplicate(), 1);
    assert!(!outcome.findings().has(Rule::SequenceGap));
}

/// C9: a late fill closes its hole even after the high-water mark moved more
/// than half the space on.
#[test]
fn c9_a_late_fill_closes_its_hole() {
    let mut tracker = SequenceTracker::new(SequenceSpace::TAP);
    tracker.observe_default(1);
    tracker.observe_default(3);
    for value in 4..=50_003 {
        tracker.observe_default(value);
    }
    assert_eq!(tracker.observe_default(2), SequenceObservation::FilledGap);
    assert!(tracker.is_complete());
    assert_eq!(tracker.high_water(), Some(50_003));
}

/// C10: one unkeyable record is rejected without blocking the rest of the
/// batch, and its V09 is reported once.
#[test]
fn c10_a_rejected_record_does_not_block_release() {
    let mut records: Vec<UsageRecord> = (1..=100).map(|i| session(i, OPENED, 1)).collect();
    let mut unkeyable = session(999, OPENED, 1);
    unkeyable.correlation = CorrelationKeys::none();
    records.push(unkeyable);

    let mut state = ProcessState::new(week());
    let outcome = run(&records, 0, &mut state, NOW);
    assert_eq!(outcome.reconciliation().rejected(), 1);
    assert!(outcome.is_releasable(), "{}", outcome.quality());
    assert_eq!(count(&outcome, Rule::NotKeyable), 1);
}

/// C12: of two copies of partial #1, the fully qualified one is the basis,
/// in either order.
#[test]
fn c12_the_fully_qualified_first_partial_wins_in_any_order() {
    let reduced = PartialInfo::new(1, PartialFormat::Reduced, RecordClosureCause::VolumeLimit);
    let full = PartialInfo::new(
        1,
        PartialFormat::FullyQualified,
        RecordClosureCause::VolumeLimit,
    );
    let last = PartialInfo::new(2, PartialFormat::Reduced, RecordClosureCause::NormalRelease);
    let one = PartialRun::from_parts(vec![reduced, full, last]).assess();
    let other = PartialRun::from_parts(vec![full, reduced, last]).assess();
    assert_eq!(one, Reassembly::Complete { count: 2 });
    assert_eq!(one, other);

    let orphan = PartialRun::from_parts(vec![reduced, last]).assess();
    assert!(matches!(
        orphan,
        Reassembly::Orphan {
            reason: OrphanReason::FirstRecordNotFullyQualified,
            ..
        }
    ));
}

/// C13: containers whose sum overflows raise V15 and are rejected, not
/// suppressed as zero usage.
#[test]
fn c13_a_container_sum_overflow_raises_v15() {
    let big = VolumePair::new(Octets::new(u64::MAX - 1), Octets::new(0));
    let containers = ContainerSet::from(vec![
        ServiceDataContainer::new(RatingGroup::new(1), mediation::ChangeCondition::ALL[0])
            .with_volume(big),
        ServiceDataContainer::new(RatingGroup::new(1), mediation::ChangeCondition::ALL[0])
            .with_volume(big),
    ]);
    let mut record = session(1, OPENED, 1).with_containers(containers);
    record.volume = None;

    let mut state = ProcessState::new(week());
    let outcome = process(
        &Batch::new(&[record]),
        &ProcessConfig::default().suppressing_zero_usage(),
        NOW,
        &mut state,
    );
    assert!(outcome.findings().has(Rule::VolumeOverflow));
    assert_eq!(
        outcome.dispositions(),
        &[Disposition::Rejected(RejectReason::Malformed)]
    );
}

/// C14: carried records come first by construction, so V16 reads only the
/// ingested ones.
#[test]
fn c14_a_carried_prefix_does_not_raise_v16() {
    let mut state = ProcessState::new(week());
    let carried = session(1, "2026-07-28T11:00:00+00:00", 1);
    run(std::slice::from_ref(&carried), 0, &mut state, NOW);
    let outcome = run(&[carried, session(2, OPENED, 2)], 1, &mut state, NOW);
    assert!(!outcome.findings().has(Rule::UnorderedSeries));
}

/// C15: a snapshot whose holes are not behind its high-water mark is refused.
#[test]
fn c15_restore_refuses_holes_not_behind_the_high_water_mark() {
    let mut tracker = SequenceTracker::new(SequenceSpace::TAP);
    tracker.observe_default(1);
    tracker.observe_default(4);
    let good = tracker.snapshot();
    assert!(SequenceTracker::restore(good.clone()).is_some());

    let mut no_mark = good.clone();
    no_mark.high_water = None;
    assert!(SequenceTracker::restore(no_mark).is_none());

    let mut mark_in_hole = good;
    mark_in_hole.high_water = Some(2);
    assert!(SequenceTracker::restore(mark_in_hole).is_none());
}

/// C16: a sub-second dedup window is refused, so every window round-trips.
#[test]
fn c16_a_sub_second_window_is_refused() {
    assert!(DedupWindow::from_bound(Duration::milliseconds(500)).is_none());
    assert!(DedupWindow::from_bound(Duration::milliseconds(1_500)).is_none());
    assert!(DedupWindow::from_bound(Duration::seconds(2)).is_some());
}

/// C16: a dedup store survives serde with its window intact.
#[cfg(feature = "serde")]
#[test]
fn c16_a_dedup_store_round_trips_through_serde() {
    let store = mediation::DedupStore::new(DedupWindow::from_bound(Duration::seconds(90)).unwrap());
    let json = serde_json::to_string(&store).unwrap();
    let back: mediation::DedupStore = serde_json::from_str(&json).unwrap();
    assert_eq!(back.window(), store.window());
}

/// C5: the Call key's record type is part of its serde representation.
#[cfg(feature = "serde")]
#[test]
fn c5_the_call_key_serialises_its_record_type() {
    let key = RecordKey::of(&call(RecordType::TerminatedCall)).unwrap();
    let json = serde_json::to_value(&key).unwrap();
    assert_eq!(json["record_type"], "TERMINATED_CALL");
    assert_eq!(serde_json::from_value::<RecordKey>(json).unwrap(), key);
}
