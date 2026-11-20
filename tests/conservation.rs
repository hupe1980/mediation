//! The conservation law, under generated input.
//!
//! ```text
//! ingested == billable + suppressed + duplicate + rejected + held
//! ```
//!
//! This is the crate's central claim, so it is checked as a property rather
//! than on a handful of examples: over arbitrary batches, in arbitrary
//! processing orders, split arbitrarily and recombined.

use mediation::{
    Batch, BatchReconciliation, ChargedParty, ChargingId, CorrelationKeys, DedupWindow,
    Disposition, HoldReason, NotKeyable, Octets, ProcessConfig, ProcessState, RecordKey,
    RecordType, RejectReason, SourceRef, SubscriberRef, SuppressReason, UnattributableReason,
    UsageRecord, VolumePair, apportion, process, split_session,
};
use proptest::prelude::*;
use std::net::Ipv4Addr;
use time::{Duration, OffsetDateTime, macros::datetime};

const NOW: OffsetDateTime = datetime!(2026-07-28 12:00:00 UTC);

/// A record that reaches `Billable` on its own.
fn billable(charging_id: u32) -> UsageRecord {
    UsageRecord::new(
        RecordType::PduSession,
        ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
        "2026-07-28T10:00:00+02:00".parse().unwrap(),
        SourceRef::new("pgw-fra-01".parse().unwrap(), 0),
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

/// Any outcome a record can reach.
fn any_disposition() -> impl Strategy<Value = Disposition> {
    let key = RecordKey::RecordingEntity {
        entity: "msc-01".parse().unwrap(),
        local_sequence: 1,
    };
    prop_oneof![
        Just(Disposition::Billable),
        prop_oneof![
            Just(SuppressReason::TestIdentity),
            Just(SuppressReason::ZeroRated),
            Just(SuppressReason::InternalTraffic),
            Just(SuppressReason::ZeroUsage),
            Just(SuppressReason::OperatorPolicy),
        ]
        .prop_map(Disposition::Suppressed),
        Just(Disposition::Duplicate(key)),
        prop_oneof![
            Just(RejectReason::BeyondDeadline),
            Just(RejectReason::FutureTimestamp),
            Just(RejectReason::Malformed),
            Just(RejectReason::Unattributable(
                UnattributableReason::IdentityAbsent
            )),
            Just(RejectReason::NotKeyable(NotKeyable::NoCorrelatorPresent)),
        ]
        .prop_map(Disposition::Rejected),
        prop_oneof![
            Just(HoldReason::AwaitingPartial),
            Just(HoldReason::AwaitingCorrelation),
            Just(HoldReason::AwaitingReferenceData),
            Just(HoldReason::SequenceGap),
        ]
        .prop_map(Disposition::Held),
    ]
}

/// A batch of up to 200 outcomes.
fn any_batch() -> impl Strategy<Value = Vec<Disposition>> {
    proptest::collection::vec(any_disposition(), 0..200)
}

proptest! {
    /// Every batch counted through the public path balances. Nothing is
    /// created and nothing is lost.
    #[test]
    fn every_batch_balances(batch in any_batch()) {
        let counts: BatchReconciliation = batch.iter().collect();
        prop_assert!(counts.balances(), "{counts}");
        prop_assert_eq!(counts.ingested(), batch.len() as u64);
    }

    /// The order records are processed in cannot change the totals — which is
    /// what makes parallel processing over disjoint key ranges safe.
    #[test]
    fn the_counts_are_order_independent(batch in any_batch(), rotation in 0usize..200) {
        let forward: BatchReconciliation = batch.iter().collect();

        let mut shuffled = batch.clone();
        let len = shuffled.len();
        if len > 0 {
            shuffled.rotate_left(rotation % len);
        }
        shuffled.reverse();
        let rotated: BatchReconciliation = shuffled.iter().collect();

        prop_assert_eq!(forward, rotated);
    }

    /// Splitting a batch and recombining the halves reproduces the whole,
    /// exactly. This is the property a caller relies on when they fan work out.
    #[test]
    fn splitting_and_merging_is_lossless(batch in any_batch(), at in 0usize..200) {
        let whole: BatchReconciliation = batch.iter().collect();

        let at = if batch.is_empty() { 0 } else { at % (batch.len() + 1) };
        let (left, right) = batch.split_at(at);
        let merged = BatchReconciliation::from_iter(left)
            .checked_add(BatchReconciliation::from_iter(right))
            .expect("counts this small cannot overflow");

        prop_assert_eq!(whole, merged);
        prop_assert!(merged.balances());
    }

    /// Merging is associative, so the shape of a fan-out does not matter.
    #[test]
    fn merging_is_associative(a in any_batch(), b in any_batch(), c in any_batch()) {
        let (ra, rb, rc) = (
            BatchReconciliation::from_iter(&a),
            BatchReconciliation::from_iter(&b),
            BatchReconciliation::from_iter(&c),
        );

        let left = ra.checked_add(rb).unwrap().checked_add(rc).unwrap();
        let right = ra.checked_add(rb.checked_add(rc).unwrap()).unwrap();
        prop_assert_eq!(left, right);
        prop_assert!(left.balances());
    }

    /// The empty reconciliation is the identity for merging.
    #[test]
    fn the_empty_batch_is_the_identity(batch in any_batch()) {
        let counts = BatchReconciliation::from_iter(&batch);
        prop_assert_eq!(counts.checked_add(BatchReconciliation::EMPTY), Some(counts));
        prop_assert_eq!(BatchReconciliation::EMPTY.checked_add(counts), Some(counts));
    }

    /// A batch is final exactly when it balances and nothing is held — the
    /// precondition for closing a period over it.
    #[test]
    fn finality_is_exactly_balanced_and_nothing_held(batch in any_batch()) {
        let counts = BatchReconciliation::from_iter(&batch);
        let held = batch.iter().filter(|d| d.may_return()).count() as u64;

        prop_assert_eq!(counts.held(), held);
        prop_assert_eq!(counts.is_final(), counts.balances() && held == 0);
    }

    /// Exactly one outcome is released, and it is the one that reaches rating.
    #[test]
    fn only_billable_records_are_released(batch in any_batch()) {
        let counts = BatchReconciliation::from_iter(&batch);
        let released = batch.iter().filter(|d| d.is_released()).count() as u64;
        prop_assert_eq!(counts.released(), released);

        for disposition in &batch {
            prop_assert_eq!(
                disposition.is_released(),
                matches!(disposition, Disposition::Billable)
            );
        }
    }
}

/// Every outcome is either final or may return, never both and never neither.
#[test]
fn each_outcome_is_final_or_returnable_and_not_both() {
    let key = RecordKey::RecordingEntity {
        entity: "msc-01".parse().unwrap(),
        local_sequence: 1,
    };
    let all = [
        Disposition::Billable,
        Disposition::Suppressed(SuppressReason::ZeroRated),
        Disposition::Duplicate(key),
        Disposition::Rejected(RejectReason::Malformed),
        Disposition::Held(HoldReason::SequenceGap),
    ];

    let returnable = all.iter().filter(|d| d.may_return()).count();
    assert_eq!(returnable, 1, "only a held record may return");

    // Every outcome has a distinct code, so a report cannot merge two of them.
    let mut kinds: Vec<&str> = all.iter().map(Disposition::kind).collect();
    kinds.sort_unstable();
    kinds.dedup();
    assert_eq!(kinds.len(), all.len());
}

/// A session that opened and closed in the same second still carries the octets
/// it reported. Dividing by a zero span used to hand every slice nothing —
/// silently deleting the whole volume through the one function whose stated
/// purpose is that usage is never created or destroyed.
#[test]
fn a_zero_length_session_does_not_lose_its_usage() {
    let record = UsageRecord::new(
        RecordType::PduSession,
        ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
        "2026-07-28T10:00:00+02:00".parse().unwrap(),
        SourceRef::new("pgw-01".parse().unwrap(), 0),
    )
    .with_closed("2026-07-28T10:00:00+02:00".parse().unwrap())
    .with_volume(VolumePair::new(Octets::new(1_000), Octets::new(500)));

    let split = split_session(&record, &[datetime!(2026-07-28 09:00 +2)]).unwrap();

    assert_eq!(split.len(), 1);
    assert_eq!(split.total_volume(), record.volume);
    assert!(split.conserves(&record));
}

/// The property beneath the whole module, over every shape of input: what goes
/// into an apportionment comes out of it.
#[test]
fn apportionment_conserves_for_every_weighting() {
    for total in [0u64, 1, 7, 1_000, u64::MAX] {
        for weights in [
            vec![0u64],
            vec![0, 0],
            vec![0, 0, 0, 0, 0],
            vec![1, 0, 1],
            vec![u64::MAX, u64::MAX],
            vec![3, 1, 4, 1, 5, 9, 2, 6],
        ] {
            let parts = apportion(total, &weights);
            assert_eq!(parts.len(), weights.len());
            assert_eq!(
                parts.iter().map(|p| u128::from(*p)).sum::<u128>(),
                u128::from(total),
                "{total} over {weights:?}"
            );
        }
    }
}

// ── The law through the one-pass driver ──────────────────────────────────────

/// A record built from generated parts, so the batch contains shapes nobody
/// thought to write a test for.
fn any_record() -> impl Strategy<Value = UsageRecord> {
    (
        0u32..4,                       // which correlator, if any
        0u32..6,                       // which correlator value
        proptest::option::of(0u32..4), // a partial sequence, sometimes
        proptest::bool::ANY,           // fully qualified, or reduced
        0u64..5_000,                   // uplink octets
        proptest::bool::ANY,           // attributable, or not
        0i64..90,                      // days before the evaluation instant
        proptest::option::of(0u32..3), // a file sequence, sometimes
    )
        .prop_map(
            |(shape, value, partial, fqpc, uplink, attributable, age, file)| {
                let party = if attributable {
                    ChargedParty::Subscriber(SubscriberRef::new(
                        "imsi-262021234567890".parse().unwrap(),
                    ))
                } else {
                    ChargedParty::Unattributable(UnattributableReason::IdentityAbsent)
                };
                let opened = datetime!(2026-07-28 10:00:00 UTC) - time::Duration::days(age);
                let opened =
                    mediation::CdrTimestamp::from_utc(opened, mediation::QuarterHourOffset::UTC)
                        .unwrap();
                let mut source = SourceRef::new("pgw-01".parse().unwrap(), 0);
                if let Some(file) = file {
                    source = source.with_file_sequence(file);
                }

                let mut record = UsageRecord::new(RecordType::PduSession, party, opened, source)
                    .with_closed(opened)
                    .with_volume(VolumePair::new(Octets::new(uplink), Octets::ZERO));

                let mut keys = mediation::CorrelationKeys::default();
                match shape {
                    0 => {}
                    1 => {
                        keys = keys
                            .with_charging_id(mediation::ChargingId::new(value))
                            .with_gateway(std::net::Ipv4Addr::new(10, 0, 0, 1).into());
                    }
                    2 => {
                        keys = keys.with_icid(format!("icid-{value}").parse().unwrap());
                    }
                    _ => {
                        keys.recording_entity = Some("pgw-01".parse().unwrap());
                        keys.local_sequence_number = Some(value);
                    }
                }
                if let Some(sequence) = partial {
                    keys.record_sequence_number = Some(sequence + 1);
                    record.partial = Some(mediation::PartialInfo::new(
                        sequence + 1,
                        if fqpc {
                            mediation::PartialFormat::FullyQualified
                        } else {
                            mediation::PartialFormat::Reduced
                        },
                        mediation::RecordClosureCause::VolumeLimit,
                    ));
                }
                record.with_correlation(keys)
            },
        )
}

proptest! {
    /// The crate's central claim, at the level a caller actually uses: whatever
    /// a batch contains, `process` accounts for every record exactly once.
    #[test]
    fn the_driver_conserves_over_any_batch(
        records in proptest::collection::vec(any_record(), 0..24),
        carried in 0usize..6,
    ) {
        let carried = carried.min(records.len());
        let batch = mediation::Batch::with_carry(&records, carried).expect("within the slice");

        let mut state = mediation::ProcessState::new(
            mediation::DedupWindow::from_bound(time::Duration::days(7)).unwrap(),
        );
        let outcome = mediation::process(
            &batch,
            &mediation::ProcessConfig::default(),
            datetime!(2026-07-28 12:00:00 UTC),
            &mut state,
        );

        prop_assert_eq!(outcome.dispositions().len(), records.len());
        prop_assert_eq!(outcome.reconciliation().carried_in(), carried as u64);
        prop_assert_eq!(
            outcome.reconciliation().presented(),
            Some(records.len() as u64)
        );
        prop_assert!(outcome.reconciliation().balances());

        // The totals are of exactly the records that were released.
        if let Ok(totals) = outcome.totals() {
            prop_assert!(totals.is_consistent());
        }
    }

    /// Replaying the same batch against fresh state reproduces the same answer,
    /// which is what a disputed billing cycle needs.
    #[test]
    fn the_driver_is_deterministic(
        records in proptest::collection::vec(any_record(), 0..16),
    ) {
        let run = || {
            let mut state = mediation::ProcessState::new(
                mediation::DedupWindow::from_bound(time::Duration::days(7)).unwrap(),
            );
            let outcome = mediation::process(
                &mediation::Batch::new(&records),
                &mediation::ProcessConfig::default(),
                datetime!(2026-07-28 12:00:00 UTC),
                &mut state,
            );
            (outcome.dispositions().to_vec(), *outcome.reconciliation())
        };
        prop_assert_eq!(run(), run());
    }

    /// Shuffling a batch changes neither what is billed nor which files are
    /// missing (audit C3, C6, C7).
    ///
    /// audit-hardening/SR-002
    #[test]
    fn shuffling_a_batch_changes_neither_billing_nor_gaps(
        (records, shuffled) in proptest::collection::vec(any_record(), 0..16)
            .prop_flat_map(|records| (Just(records.clone()), Just(records).prop_shuffle())),
    ) {
        let run = |records: &[UsageRecord]| {
            let mut state = ProcessState::new(DedupWindow::from_bound(Duration::days(7)).unwrap());
            let outcome = process(&Batch::new(records), &ProcessConfig::default(), NOW, &mut state);
            (outcome.reconciliation().released(), outcome.sequence_gaps().to_vec())
        };
        prop_assert_eq!(run(&records), run(&shuffled));
    }
}

/// Two reconciliations of one batch must not disagree. The pipeline's counts
/// and the ones travelling with the total are the same batch seen twice, and a
/// carried record counted as a new arrival in one of them is the seam where a
/// reconciliation stops being evidence.
#[test]
fn the_totals_carry_the_same_reconciliation_the_pass_reported() {
    let records: Vec<UsageRecord> = (1..=4).map(billable).collect();
    let batch = Batch::with_carry(&records, 2).expect("two carried");

    let mut state = ProcessState::new(DedupWindow::from_bound(Duration::days(7)).unwrap());
    let outcome = process(&batch, &ProcessConfig::default(), NOW, &mut state);

    let totals = outcome.totals().expect("aggregable");
    assert_eq!(&totals.reconciliation(), outcome.reconciliation());
    assert_eq!(totals.reconciliation().carried_in(), 2);
    assert_eq!(totals.reconciliation().ingested(), 2);
    assert!(totals.is_consistent());
}
