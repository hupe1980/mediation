//! The whole layer, on one batch.
//!
//! Every piece of this crate exists to answer one question — *may this batch be
//! invoiced?* — so the pieces are exercised here together rather than only
//! apart: sequence tracking, deduplication, validation, disposition and the
//! release gate, over a batch built to contain each interesting case exactly
//! once.

use mediation::{
    BatchReconciliation, ChangeCondition, ChargedParty, ChargingId, ContainerSet, CorrelationKeys,
    DedupDecision, DedupStore, DedupWindow, Disposition, Finding, HoldReason, Octets,
    PartialFormat, PartialInfo, PartialRun, QualityGrade, RatingGroup, Reassembly,
    RecordClosureCause, RecordKey, RecordType, RejectReason, Rule, Seconds, SequenceSpace,
    SequenceTracker, ServiceDataContainer, SourceRef, SubscriberRef, UsageRecord, ValidationConfig,
    ValidationReport, VolumePair, score_batch, validate_records,
};
use std::net::Ipv4Addr;
use time::{Duration, OffsetDateTime, macros::datetime};

const NOW: OffsetDateTime = datetime!(2026-07-28 12:00:00 UTC);

fn party() -> ChargedParty {
    ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap()))
}

fn session(charging_id: u32, opened: &str, closed: &str) -> UsageRecord {
    UsageRecord::new(
        RecordType::PduSession,
        party(),
        opened.parse().unwrap(),
        SourceRef::new("pgw-fra-01".parse().unwrap(), 0).with_file_sequence(1),
    )
    .with_networks("262-02".parse().unwrap(), "262-02".parse().unwrap())
    .with_closed(closed.parse().unwrap())
    .with_correlation(
        CorrelationKeys::default()
            .with_charging_id(ChargingId::new(charging_id))
            .with_gateway(Ipv4Addr::new(10, 0, 0, 1).into()),
    )
}

/// A session whose record total and container breakdown agree.
fn consistent_session(charging_id: u32) -> UsageRecord {
    session(
        charging_id,
        "2026-07-28T10:00:00+02:00",
        "2026-07-28T10:30:00+02:00",
    )
    .with_volume(VolumePair::new(Octets::new(1_500), Octets::new(6_000)))
    .with_containers(ContainerSet::from(vec![
        ServiceDataContainer::new(RatingGroup::new(10), ChangeCondition::TariffTime)
            .with_volume(VolumePair::new(Octets::new(1_000), Octets::new(4_000))),
        ServiceDataContainer::new(RatingGroup::new(10), ChangeCondition::RecordClosure)
            .with_volume(VolumePair::new(Octets::new(500), Octets::new(2_000))),
    ]))
}

/// Fold one record's stateful and stateless outcomes into a disposition.
fn disposition_for(
    index: usize,
    record: &UsageRecord,
    store: &mut DedupStore,
    findings: &ValidationReport,
    report_into: &mut ValidationReport,
) -> Disposition {
    let key = match RecordKey::of(record) {
        Ok(key) => key,
        Err(reason) => return Disposition::Rejected(RejectReason::NotKeyable(reason)),
    };
    match store.observe(key.clone(), record.opened.to_utc()) {
        DedupDecision::Duplicate { .. } => {
            report_into.push(Finding::duplicate(index, &key));
            Disposition::Duplicate(key)
        }
        DedupDecision::First if findings.record_blocked(index) => {
            Disposition::Rejected(RejectReason::Malformed)
        }
        DedupDecision::First => Disposition::Billable,
    }
}

/// A clean batch: distinct sessions, containers that agree, no holes.
#[test]
fn a_clean_batch_grades_a_and_releases() {
    let records: Vec<UsageRecord> = (1..=5).map(consistent_session).collect();

    let mut tracker = SequenceTracker::new(SequenceSpace::TAP);
    tracker.observe_default(1);

    let mut findings = validate_records(&records, &ValidationConfig::default(), NOW);
    let mut store = DedupStore::new(DedupWindow::from_bound(Duration::days(7)).unwrap());
    let mut counts = BatchReconciliation::EMPTY;

    let snapshot = findings.clone();
    for (index, record) in records.iter().enumerate() {
        counts.observe(&disposition_for(
            index,
            record,
            &mut store,
            &snapshot,
            &mut findings,
        ));
    }

    let quality = score_batch(&counts, &findings, tracker.is_complete());
    assert_eq!(quality.grade, QualityGrade::A, "{quality}");
    assert!(!quality.blocks_release());
    assert!(quality.is_final());
    assert_eq!(counts.released(), 5);
    assert!(counts.balances());
}

/// Retransmissions are suppressed, the batch still balances, and the grade
/// stays releasable — a duplicate is a recognised outcome, not a fault.
#[test]
fn retransmissions_suppress_without_blocking_the_release() {
    let mut records: Vec<UsageRecord> = (1..=3).map(consistent_session).collect();
    records.push(consistent_session(1));
    records.push(consistent_session(2));

    let mut findings = validate_records(&records, &ValidationConfig::default(), NOW);
    let snapshot = findings.clone();
    let mut store = DedupStore::new(DedupWindow::from_bound(Duration::days(7)).unwrap());
    let mut counts = BatchReconciliation::EMPTY;

    for (index, record) in records.iter().enumerate() {
        counts.observe(&disposition_for(
            index,
            record,
            &mut store,
            &snapshot,
            &mut findings,
        ));
    }

    assert_eq!(counts.ingested(), 5);
    assert_eq!(counts.released(), 3, "three distinct sessions");
    assert_eq!(counts.duplicate(), 2);
    assert!(counts.balances());

    let quality = score_batch(&counts, &findings, true);
    assert!(!quality.blocks_release(), "{quality}");
    assert_eq!(quality.blocking_findings, 0);
    assert_eq!(quality.warnings, 2, "each duplicate is a warning");
}

/// The hard gate: an unfilled sequence hole means unknown completeness, and no
/// number of clean records outvotes it.
#[test]
fn an_unfilled_sequence_hole_blocks_an_otherwise_perfect_batch() {
    let records: Vec<UsageRecord> = (1..=100).map(consistent_session).collect();

    let mut tracker = SequenceTracker::new(SequenceSpace::TAP);
    tracker.observe_default(1);
    tracker.observe_default(4); // files 2 and 3 never arrived

    let mut findings = validate_records(&records, &ValidationConfig::default(), NOW);
    assert!(findings.is_empty(), "the records themselves are clean");
    findings.push(Finding::sequence_gap("pgw-fra-01", &tracker.missing()));

    let mut store = DedupStore::new(DedupWindow::from_bound(Duration::days(7)).unwrap());
    let mut counts = BatchReconciliation::EMPTY;
    for record in &records {
        let key = RecordKey::of(record).unwrap();
        counts.observe(&match store.observe(key.clone(), record.opened.to_utc()) {
            DedupDecision::First => Disposition::Billable,
            DedupDecision::Duplicate { .. } => Disposition::Duplicate(key),
        });
    }

    let quality = score_batch(&counts, &findings, tracker.is_complete());
    assert_eq!(quality.grade, QualityGrade::F);
    assert!(quality.blocks_release());
    assert!(
        quality
            .blockers
            .contains(&mediation::ReleaseBlocker::IncompleteSources)
    );
    assert!(
        counts.balances(),
        "the law holds even when release does not"
    );
}

/// A record whose two statements of one fact disagree is blocked, and the rest
/// of the batch is not.
#[test]
fn a_container_mismatch_blocks_only_its_own_record() {
    let mut broken = consistent_session(9);
    broken.volume = Some(VolumePair::new(Octets::new(9_999), Octets::new(6_000)));

    let records = vec![consistent_session(1), broken, consistent_session(2)];
    let findings = validate_records(&records, &ValidationConfig::default(), NOW);

    assert!(findings.has(Rule::ContainerSumMismatch));
    assert!(findings.record_blocked(1));
    assert!(!findings.record_blocked(0));
    assert!(!findings.record_blocked(2));

    let mut store = DedupStore::new(DedupWindow::from_bound(Duration::days(7)).unwrap());
    let mut counts = BatchReconciliation::EMPTY;
    let mut mutable = findings.clone();
    for (index, record) in records.iter().enumerate() {
        counts.observe(&disposition_for(
            index,
            record,
            &mut store,
            &findings,
            &mut mutable,
        ));
    }

    assert_eq!(counts.released(), 2);
    assert_eq!(counts.rejected(), 1);
    assert!(counts.balances());
    assert!(score_batch(&counts, &findings, true).blocks_release());
}

/// A run of partials that is still open holds rather than rejects: the batch
/// balances, is releasable, and is not final.
#[test]
fn an_open_partial_run_holds_without_blocking() {
    let run = PartialRun::from_parts(vec![
        PartialInfo::new(
            1,
            PartialFormat::FullyQualified,
            RecordClosureCause::VolumeLimit,
        ),
        PartialInfo::new(2, PartialFormat::Reduced, RecordClosureCause::VolumeLimit),
    ]);
    let assessment = run.assess();
    assert!(assessment.may_still_complete());
    assert_eq!(
        Finding::reassembly(0, &assessment),
        None,
        "open is not a fault"
    );

    let mut counts = BatchReconciliation::EMPTY;
    counts.observe(&Disposition::Held(HoldReason::AwaitingPartial));
    counts.observe(&Disposition::Held(HoldReason::AwaitingPartial));
    counts.observe(&Disposition::Billable);

    let quality = score_batch(&counts, &ValidationReport::new(), true);
    assert!(!quality.blocks_release(), "held is not a fault");
    assert!(!quality.is_final(), "…but the period cannot close");
    assert_eq!(quality.held, 2);
}

/// A run that cannot be reassembled at all blocks, and waiting will not help.
#[test]
fn an_orphaned_partial_run_blocks_the_release() {
    let run = PartialRun::from_parts(vec![PartialInfo::new(
        1,
        PartialFormat::Reduced,
        RecordClosureCause::VolumeLimit,
    )]);
    let assessment = run.assess();
    assert!(matches!(assessment, Reassembly::Orphan { .. }));
    assert!(!assessment.may_still_complete());

    let mut findings = ValidationReport::new();
    findings.push(Finding::reassembly(0, &assessment).expect("an orphan is a finding"));
    assert!(findings.has(Rule::OrphanPartial));

    let counts: BatchReconciliation = vec![Disposition::Billable].into_iter().collect();
    let quality = score_batch(&counts, &findings, true);
    assert_eq!(quality.grade, QualityGrade::F);
    assert!(quality.blocks_release());
}

/// The whole layer is deterministic: the same inputs, replayed, give the same
/// answer — which is what makes a disputed billing period re-runnable.
#[test]
fn the_same_inputs_replay_to_the_same_answer() {
    let records: Vec<UsageRecord> = (1..=20).map(consistent_session).collect();

    let evaluate = || {
        let findings = validate_records(&records, &ValidationConfig::default(), NOW);
        let mut store = DedupStore::new(DedupWindow::from_bound(Duration::days(7)).unwrap());
        let mut counts = BatchReconciliation::EMPTY;
        for record in &records {
            let key = RecordKey::of(record).unwrap();
            counts.observe(&match store.observe(key.clone(), record.opened.to_utc()) {
                DedupDecision::First => Disposition::Billable,
                DedupDecision::Duplicate { .. } => Disposition::Duplicate(key),
            });
        }
        let quality = score_batch(&counts, &findings, true);
        (findings, counts, quality)
    };

    assert_eq!(evaluate(), evaluate());
}

/// Replaying an archive against its own evaluation instant reproduces the
/// verdicts it originally received; judging it against today rejects it all.
#[test]
fn an_archive_replays_to_its_original_verdicts() {
    let archived = session(1, "2019-03-01T10:00:00+01:00", "2019-03-01T10:30:00+01:00")
        .with_volume(VolumePair::new(Octets::new(100), Octets::new(900)))
        .with_duration(Seconds::new(1_800));
    let then = datetime!(2019-03-02 12:00:00 UTC);

    let at_the_time = validate_records(
        std::slice::from_ref(&archived),
        &ValidationConfig::default(),
        then,
    );
    assert!(!at_the_time.has(Rule::BeyondDeadline));
    assert!(!at_the_time.blocks_billing());

    let today = validate_records(
        std::slice::from_ref(&archived),
        &ValidationConfig::default(),
        NOW,
    );
    assert!(today.has(Rule::BeyondDeadline));
    assert!(today.blocks_billing());
}
