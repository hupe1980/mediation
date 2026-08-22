//! The whole layer, on one batch.
//!
//! Every piece of this crate exists to answer one question — *may this batch be
//! invoiced?* — so the pieces are exercised here together rather than only
//! apart: sequence tracking, deduplication, validation, disposition and the
//! release gate, over a batch built to contain each interesting case exactly
//! once.

use mediation::{Batch, PeriodLedger, ProcessConfig, ProcessState, process, process_with};
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
    findings.push(Finding::sequence_gap(
        &"pgw-fra-01",
        &tracker.missing(),
        tracker.missing_count(),
    ));

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

// ── The one-pass driver ──────────────────────────────────────────────────────

/// The same batch through `process`, which is what the hand-assembly above
/// exists to be checked against: every step of the pipeline is easy to order
/// wrongly, so the ordering lives in one place with the reasoning attached.
#[test]
fn the_driver_agrees_with_the_hand_assembly() {
    let records: Vec<UsageRecord> = (1..=5).map(consistent_session).collect();

    let mut state = ProcessState::new(DedupWindow::from_bound(Duration::days(7)).unwrap());
    let outcome = process(
        &Batch::new(&records),
        &ProcessConfig::default(),
        NOW,
        &mut state,
    );

    assert_eq!(outcome.quality().grade, QualityGrade::A);
    assert!(outcome.is_releasable());
    assert!(outcome.quality().is_final());
    assert_eq!(outcome.reconciliation().released(), 5);
    assert!(outcome.reconciliation().balances());
    assert_eq!(outcome.totals().unwrap().total().records(), 5);
    assert!(outcome.totals().unwrap().is_consistent());
}

/// A period built from `process` outcomes telescopes: each batch balances on
/// its own and carries in exactly what its predecessor held.
#[test]
fn successive_passes_chain_into_a_closable_period() {
    // Batch one: two whole sessions and an unfinished run of partials.
    let mut first = session(1, "2026-07-28T10:00:00+02:00", "2026-07-28T10:30:00+02:00");
    first.partial = Some(PartialInfo::new(
        1,
        PartialFormat::FullyQualified,
        RecordClosureCause::VolumeLimit,
    ));
    first.correlation.record_sequence_number = Some(1);

    let batch_one = vec![
        consistent_session(10),
        consistent_session(11),
        first.clone(),
    ];

    let mut state = ProcessState::new(DedupWindow::from_bound(Duration::days(7)).unwrap());
    let one = process(
        &Batch::new(&batch_one),
        &ProcessConfig::default(),
        NOW,
        &mut state,
    );
    assert_eq!(one.reconciliation().held(), 1);
    assert!(!one.quality().is_final(), "a held record reopens a period");

    // Batch two re-presents the held partial, and its successor arrives.
    let mut second = session(1, "2026-07-28T10:30:00+02:00", "2026-07-28T10:45:00+02:00");
    second.partial = Some(PartialInfo::new(
        2,
        PartialFormat::Reduced,
        RecordClosureCause::NormalRelease,
    ));
    second.correlation.record_sequence_number = Some(2);

    let batch_two = vec![first, second];
    let two = process(
        &Batch::with_carry(&batch_two, 1).unwrap(),
        &ProcessConfig::default(),
        NOW,
        &mut state,
    );

    assert_eq!(two.reconciliation().carried_in(), 1);
    assert_eq!(two.reconciliation().ingested(), 1);
    assert_eq!(two.reconciliation().held(), 0, "the run is finished");
    assert_eq!(two.reconciliation().released(), 2);

    let mut ledger = PeriodLedger::new();
    ledger.append(*one.reconciliation()).unwrap();
    ledger.append(*two.reconciliation()).unwrap();

    assert_eq!(ledger.outstanding(), 0);
    assert!(ledger.is_closable(), "{ledger}");
}

/// The crate's central claim, at the level a caller actually uses: whatever a
/// batch contains, `process` accounts for every record exactly once.
#[test]
fn the_driver_conserves_over_an_awkward_batch() {
    let mut records: Vec<UsageRecord> = (1..=4).map(consistent_session).collect();

    // A retransmission.
    records.push(consistent_session(1));
    // A record with no natural key.
    records.push(UsageRecord::new(
        RecordType::PduSession,
        party(),
        "2026-07-28T10:00:00+02:00".parse().unwrap(),
        SourceRef::new("pgw-fra-01".parse().unwrap(), 99),
    ));
    // A record from before the inclusion deadline.
    let mut ancient = consistent_session(20);
    ancient.opened = "2026-01-01T10:00:00+02:00".parse().unwrap();
    ancient.closed = Some("2026-01-01T10:30:00+02:00".parse().unwrap());
    records.push(ancient);
    // A record that measured nothing.
    let mut empty = session(21, "2026-07-28T10:00:00+02:00", "2026-07-28T10:00:30+02:00");
    empty.volume = None;
    records.push(empty);

    let mut state = ProcessState::new(DedupWindow::from_bound(Duration::days(7)).unwrap());
    let outcome = process_with(
        &Batch::new(&records),
        &ProcessConfig::default().suppressing_zero_usage(),
        NOW,
        &mut state,
        &|record: &UsageRecord| {
            (record.correlation.charging_id == Some(ChargingId::new(4)))
                .then_some(mediation::SuppressReason::TestIdentity)
        },
    );

    let counts = outcome.reconciliation();
    assert_eq!(counts.presented(), Some(records.len() as u64));
    assert!(counts.balances(), "{counts}");
    assert_eq!(counts.duplicate(), 1);
    assert_eq!(
        counts.suppressed(),
        2,
        "the test identity and the empty record"
    );
    assert_eq!(
        counts.rejected(),
        2,
        "the unkeyable one and the ancient one"
    );
    assert_eq!(counts.released(), 3);
    assert_eq!(outcome.dispositions().len(), records.len());
}

/// A packet-data record commonly carries its volume **only** in its containers.
/// Reading the absent record-level total as zero would call a five-gigabyte
/// session empty — and a zero-usage suppression policy would then deliberately
/// not bill it.
#[test]
fn a_record_whose_volume_is_only_in_its_containers_is_not_empty() {
    let record = session(1, "2026-07-28T10:00:00+02:00", "2026-07-28T10:30:00+02:00")
        .with_containers(ContainerSet::from(vec![
            ServiceDataContainer::new(RatingGroup::new(10), ChangeCondition::RecordClosure)
                .with_volume(VolumePair::new(
                    Octets::new(1_000),
                    Octets::new(5_000_000_000),
                )),
        ]));

    assert_eq!(record.volume, None, "the record itself states no total");
    assert!(!record.is_zero_usage(), "but it is not an empty record");
    assert_eq!(
        record.billable_volume().unwrap().downlink(),
        Octets::new(5_000_000_000)
    );

    let records = vec![record];
    let mut state = ProcessState::new(DedupWindow::from_bound(Duration::days(7)).unwrap());
    let outcome = process(
        &Batch::new(&records),
        &ProcessConfig::default().suppressing_zero_usage(),
        NOW,
        &mut state,
    );

    assert_eq!(
        outcome.reconciliation().released(),
        1,
        "billed, not suppressed"
    );
    assert!(!outcome.findings().has(Rule::ZeroUsage));
}

/// A partial keyed on a recording entity and a serial number cannot be gathered
/// into a run at all: the key identifies the record, not the session above it.
/// Judging it against a run of one made every SGSN partial numbered anything but
/// 1 an orphan, and rejected it as malformed.
#[test]
fn a_partial_with_no_session_identity_is_not_an_orphan() {
    let mut record = UsageRecord::new(
        RecordType::PdpContext,
        party(),
        "2026-07-28T10:00:00+02:00".parse().unwrap(),
        SourceRef::new("sgsn-01".parse().unwrap(), 0).with_file_sequence(1),
    )
    .with_volume(VolumePair::new(Octets::new(100), Octets::new(900)));
    record.correlation.recording_entity = Some("sgsn-01".parse().unwrap());
    record.correlation.local_sequence_number = Some(77);
    record.partial = Some(PartialInfo::new(
        3,
        PartialFormat::Reduced,
        RecordClosureCause::VolumeLimit,
    ));

    // The key exists — it just names no session.
    let key = RecordKey::of(&record).unwrap();
    assert_eq!(key.session_scope(), None);

    let records = vec![record];
    let mut state = ProcessState::new(DedupWindow::from_bound(Duration::days(7)).unwrap());
    let outcome = process(
        &Batch::new(&records),
        &ProcessConfig::default(),
        NOW,
        &mut state,
    );

    assert_eq!(outcome.dispositions()[0], Disposition::Billable);
    assert!(outcome.reconciliation().balances());
}

/// Where the key *does* name a session, the run is gathered and every member of
/// it takes the same verdict — a run is complete or it is not, and holding half
/// of one leaves the session partly billed.
#[test]
fn a_session_scoped_run_is_gathered_and_judged_as_one() {
    let mut first = session(7, "2026-07-28T10:00:00+02:00", "2026-07-28T10:15:00+02:00");
    first.partial = Some(PartialInfo::new(
        1,
        PartialFormat::FullyQualified,
        RecordClosureCause::VolumeLimit,
    ));
    first.correlation.record_sequence_number = Some(1);

    let mut second = session(7, "2026-07-28T10:15:00+02:00", "2026-07-28T10:30:00+02:00");
    second.partial = Some(PartialInfo::new(
        2,
        PartialFormat::Reduced,
        RecordClosureCause::VolumeLimit,
    ));
    second.correlation.record_sequence_number = Some(2);

    let records = vec![first, second];
    let mut state = ProcessState::new(DedupWindow::from_bound(Duration::days(7)).unwrap());
    let outcome = process(
        &Batch::new(&records),
        &ProcessConfig::default(),
        NOW,
        &mut state,
    );

    assert_eq!(
        outcome.reconciliation().held(),
        2,
        "all of it, or none of it"
    );
    assert_eq!(outcome.reconciliation().released(), 0);
}

// ── Completeness is per source ───────────────────────────────────────────────

/// A record from the same node but a different file, so a batch can carry two
/// sources at once.
fn session_from(node: &str, charging_id: u32, file: u32) -> UsageRecord {
    UsageRecord::new(
        RecordType::PduSession,
        party(),
        "2026-07-28T10:00:00+02:00".parse().unwrap(),
        SourceRef::new(node.parse().unwrap(), 0).with_file_sequence(file),
    )
    .with_networks("262-02".parse().unwrap(), "262-02".parse().unwrap())
    .with_closed("2026-07-28T10:30:00+02:00".parse().unwrap())
    .with_volume(VolumePair::new(Octets::new(1_000), Octets::new(4_000)))
    .with_correlation(
        CorrelationKeys::default()
            .with_charging_id(ChargingId::new(charging_id))
            .with_gateway(Ipv4Addr::new(10, 0, 0, 1).into()),
    )
}

/// One gateway losing a file must not stop an operator's whole billing run.
/// Frankfurt's records are held because Frankfurt cannot prove what it is
/// missing; Munich's are released because Munich can.
#[test]
fn a_hole_at_one_gateway_does_not_block_another() {
    let mut state = ProcessState::new(DedupWindow::from_bound(Duration::days(7)).unwrap());

    // Frankfurt file 1 and Munich file 1 both arrive.
    let first = vec![
        session_from("pgw-fra-01", 1, 1),
        session_from("pgw-muc-01", 2, 1),
    ];
    let outcome = process(
        &Batch::new(&first),
        &ProcessConfig::default(),
        NOW,
        &mut state,
    );
    assert!(outcome.is_releasable());
    assert_eq!(outcome.reconciliation().released(), 2);

    // Frankfurt jumps to file 4 — files 2 and 3 are lost. Munich carries on.
    let second = vec![
        session_from("pgw-fra-01", 3, 4),
        session_from("pgw-muc-01", 4, 2),
    ];
    let outcome = process(
        &Batch::new(&second),
        &ProcessConfig::default(),
        NOW,
        &mut state,
    );

    assert_eq!(
        outcome.dispositions(),
        &[
            Disposition::Held(HoldReason::SequenceGap),
            Disposition::Billable,
        ],
        "Frankfurt waits; Munich bills"
    );
    assert!(outcome.reconciliation().balances());
    assert!(
        outcome.is_releasable(),
        "what is released is complete, so the batch may go"
    );
    assert_eq!(outcome.sequence_gaps().len(), 1);
    assert_eq!(outcome.sequence_gaps()[0].missing, vec![2, 3]);
    assert_eq!(outcome.sequence_gaps()[0].missing_count, 2);

    // The resend closes the hole and the held record can be re-presented.
    let filled = vec![
        session_from("pgw-fra-01", 3, 4),
        session_from("pgw-fra-01", 5, 2),
        session_from("pgw-fra-01", 6, 3),
    ];
    let batch = Batch::with_carry(&filled, 1).expect("one carried record");
    let outcome = process(&batch, &ProcessConfig::default(), NOW, &mut state);
    assert_eq!(outcome.reconciliation().released(), 3);
    assert_eq!(outcome.reconciliation().held(), 0);
    assert!(outcome.reconciliation().balances());
}

/// Abandoning a hole is the caller's explicit decision, and it unblocks the
/// source it was blocking.
#[test]
fn abandoning_a_hole_releases_the_source_it_held() {
    let mut state = ProcessState::new(DedupWindow::from_bound(Duration::days(7)).unwrap());
    let opening = vec![session_from("pgw-fra-01", 1, 1)];
    let _ = process(
        &Batch::new(&opening),
        &ProcessConfig::default(),
        NOW,
        &mut state,
    );

    let jumped = vec![session_from("pgw-fra-01", 2, 40)];
    let outcome = process(
        &Batch::new(&jumped),
        &ProcessConfig::default(),
        NOW,
        &mut state,
    );
    assert_eq!(outcome.reconciliation().held(), 1);
    assert_eq!(state.incomplete_sources().count(), 1);

    // The partner will not resend; the operator says so.
    let node: mediation::NodeId = "pgw-fra-01".parse().unwrap();
    let mut tracker = state.tracker(&node).unwrap().clone();
    // Files 2..=39 are missing and the high-water mark is 40, so a bound of
    // zero steps abandons every one of them.
    assert_eq!(tracker.abandon_older_than(0), 38);
    state = state.with_tracker(node, tracker).expect("same space");
    assert_eq!(state.incomplete_sources().count(), 0);

    let again = vec![session_from("pgw-fra-01", 3, 41)];
    let outcome = process(
        &Batch::new(&again),
        &ProcessConfig::default(),
        NOW,
        &mut state,
    );
    assert_eq!(outcome.reconciliation().released(), 1);
}

// ── State survives a restart ─────────────────────────────────────────────────

/// The two load-bearing correctness controls are caller-owned state, and a
/// restart that lost them would re-bill a window of retransmissions and declare
/// every source complete. So they round-trip.
#[test]
#[cfg(feature = "serde")]
fn the_whole_state_survives_a_round_trip() {
    let mut state = ProcessState::new(DedupWindow::from_bound(Duration::days(7)).unwrap());
    let opening = vec![session_from("pgw-muc-01", 9, 1)];
    let _ = process(
        &Batch::new(&opening),
        &ProcessConfig::default(),
        NOW,
        &mut state,
    );

    // Frankfurt's first file, and Munich jumping from 1 to 3.
    let records = vec![
        session_from("pgw-fra-01", 1, 1),
        session_from("pgw-muc-01", 2, 3),
    ];
    let _ = process(
        &Batch::new(&records),
        &ProcessConfig::default(),
        NOW,
        &mut state,
    );

    let saved = serde_json::to_string(&state).expect("serialisable");
    let mut restored: ProcessState = serde_json::from_str(&saved).expect("deserialisable");

    assert_eq!(restored.dedup().len(), state.dedup().len());
    assert_eq!(restored.trackers().count(), state.trackers().count());
    for (node, tracker) in state.trackers() {
        assert_eq!(restored.tracker(node), Some(tracker));
    }

    // And it behaves the same: the same records are still duplicates, and
    // Munich's hole is still a hole.
    let outcome = process(
        &Batch::new(&records),
        &ProcessConfig::default(),
        NOW,
        &mut restored,
    );
    assert_eq!(outcome.reconciliation().duplicate(), 2);
    assert_eq!(
        restored
            .tracker(&"pgw-muc-01".parse().unwrap())
            .unwrap()
            .missing_runs(),
        vec![(2, 2)],
    );
}

// ── The period law ───────────────────────────────────────────────────────────

/// The batch law counts a held record again in every batch that re-presents it.
/// The period law does not, and that is the whole point of chaining.
#[test]
fn the_period_law_telescopes_the_carries_away() {
    let mut ledger = PeriodLedger::new();

    // One record billed, one held, across three batches — the hold coming back
    // twice before it finally bills.
    let mut first = BatchReconciliation::EMPTY;
    first.observe(&Disposition::Billable);
    first.observe(&Disposition::Held(HoldReason::AwaitingPartial));
    ledger.append(first).unwrap();

    let mut second = BatchReconciliation::EMPTY;
    second.observe_carried(&Disposition::Held(HoldReason::AwaitingPartial));
    ledger.append(second).unwrap();

    let mut third = BatchReconciliation::EMPTY;
    third.observe_carried(&Disposition::Billable);
    ledger.append(third).unwrap();

    // The batch sum counts the hold in two batches …
    assert_eq!(ledger.totals().held(), 2);
    assert_eq!(ledger.totals().carried_in(), 2);

    // … and the period law counts each record exactly once.
    let period = ledger.period_totals();
    assert_eq!(period.ingested, 2);
    assert_eq!(period.billable, 2);
    assert_eq!(period.outstanding, 0);
    assert!(period.balances());
    assert!(period.is_closable());
    assert!(ledger.is_closable());
}

/// A period with something still in flight balances and is not closable — the
/// outstanding record is named rather than lost.
#[test]
fn an_outstanding_record_keeps_the_period_open_without_breaking_it() {
    let mut ledger = PeriodLedger::new();
    let mut batch = BatchReconciliation::EMPTY;
    batch.observe(&Disposition::Billable);
    batch.observe(&Disposition::Held(HoldReason::AwaitingCorrelation));
    ledger.append(batch).unwrap();

    let period = ledger.period_totals();
    assert_eq!(period.ingested, 2);
    assert_eq!(period.outstanding, 1);
    assert!(period.balances(), "nothing is lost");
    assert!(!period.is_closable(), "but nothing is finished either");
}

/// A record nobody was going to bill does not wait on its source's
/// completeness. Holding it would re-present a foregone conclusion in every
/// batch until the missing file arrived, and the answer would not change.
#[test]
fn suppression_outranks_the_completeness_hold() {
    let mut state = ProcessState::new(DedupWindow::from_bound(Duration::days(7)).unwrap());
    let _ = process(
        &Batch::new(&[session_from("pgw-fra-01", 1, 1)]),
        &ProcessConfig::default(),
        NOW,
        &mut state,
    );

    // A hole opens, and the batch carries one billable record and one the
    // policy suppresses.
    let records = vec![
        session_from("pgw-fra-01", 2, 5),
        session_from("pgw-fra-01", 3, 5),
    ];
    let suppress_second = |record: &UsageRecord| {
        (record.correlation.charging_id == Some(ChargingId::new(3)))
            .then_some(mediation::SuppressReason::TestIdentity)
    };
    let outcome = process_with(
        &Batch::new(&records),
        &ProcessConfig::default(),
        NOW,
        &mut state,
        &suppress_second,
    );

    assert_eq!(
        outcome.dispositions(),
        &[
            Disposition::Held(HoldReason::SequenceGap),
            Disposition::Suppressed(mediation::SuppressReason::TestIdentity),
        ]
    );
    assert!(outcome.reconciliation().balances());
    assert_eq!(outcome.reconciliation().suppressed(), 1);
    assert_eq!(outcome.reconciliation().held(), 1);
}
