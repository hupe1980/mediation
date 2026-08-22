//! A partner's batch, end to end.
//!
//! A roaming batch is the case where every layer of this crate has to agree at
//! once: the file states what it holds, the events are deduplicated and
//! validated, the batch is dispositioned, the total is summed, and the whole
//! thing is checked against the partner's own audit block — after which a RAP
//! return may undo part of it.
//!
//! Two failures live here that live nowhere else. A batch that disagrees with
//! its own audit block is corrupt however well it parsed. And a **fatal** RAP
//! return means the batch never counted at all, while a **severe** one leaves
//! it counted — conflating them breaks the conservation law in opposite
//! directions.

use mediation::{
    BatchDiscrepancy, BatchReconciliation, BatchTotals, CdrTimestamp, ChargedParty, ChargingId,
    CorrelationKeys, DedupDecision, DedupStore, DedupWindow, Disposition, Octets, PeriodLedger,
    RapReturn, RecordKey, RecordType, ScaledAmount, SequenceSpace, SequenceTracker, SequenceValue,
    SourceRef, SubscriberRef, TapBatch, TotalKind, UsageRecord, ValidationConfig, VolumePair,
    aggregate_released, score_batch, validate_records,
};
use std::net::Ipv4Addr;
use time::{Duration, OffsetDateTime, macros::datetime};

const NOW: OffsetDateTime = datetime!(2026-07-28 12:00:00 UTC);

fn seq(value: u32) -> SequenceValue {
    SequenceValue::new(SequenceSpace::TAP, value).expect("in space")
}

fn event(charging_id: u32, opened: &str, up: u64) -> UsageRecord {
    UsageRecord::new(
        RecordType::PduSession,
        ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
        opened.parse().unwrap(),
        SourceRef::new("vplmn-gw-01".parse().unwrap(), 0).with_file_sequence(42),
    )
    // Roaming: served abroad, billed home.
    .with_networks("234-15".parse().unwrap(), "262-02".parse().unwrap())
    .with_closed("2026-07-25T10:30:00+02:00".parse().unwrap())
    .with_volume(VolumePair::new(Octets::new(up), Octets::new(up * 4)))
    .with_correlation(
        CorrelationKeys::default()
            .with_charging_id(ChargingId::new(charging_id))
            .with_gateway(Ipv4Addr::new(10, 0, 0, 1).into()),
    )
}

fn batch(declared_events: u64, declared_charge: ScaledAmount) -> TapBatch {
    TapBatch {
        sender: "DEUD1".parse().unwrap(),
        recipient: "GBRVF".parse().unwrap(),
        file_sequence: seq(42),
        created: "2026-07-28T10:00:00+02:00".parse().unwrap(),
        declared_events,
        earliest_event: Some("2026-07-25T10:00:00+02:00".parse().unwrap()),
        latest_event: Some("2026-07-25T12:00:00+02:00".parse().unwrap()),
        declared_charge,
        declared_discount: None,
        declared_tax: None,
    }
}

/// Run a batch of events through the layer, returning the dispositions.
fn process(records: &[UsageRecord]) -> Vec<Disposition> {
    let findings = validate_records(records, &ValidationConfig::default(), NOW);
    let mut store = DedupStore::new(DedupWindow::from_bound(Duration::days(7)).unwrap());

    records
        .iter()
        .enumerate()
        .map(|(index, record)| {
            let key = match RecordKey::of(record) {
                Ok(key) => key,
                Err(reason) => {
                    return Disposition::Rejected(mediation::RejectReason::NotKeyable(reason));
                }
            };
            match store.observe(key.clone(), record.opened.to_utc()) {
                DedupDecision::Duplicate { .. } => Disposition::Duplicate(key),
                DedupDecision::First if findings.record_blocked(index) => {
                    Disposition::Rejected(mediation::RejectReason::Malformed)
                }
                DedupDecision::First => Disposition::Billable,
            }
        })
        .collect()
}

// ── A clean batch ────────────────────────────────────────────────────────────

/// The whole layer agreeing at once: the file describes what it holds, every
/// event is billable, the total ties to the reconciliation, and the batch is
/// releasable.
#[test]
fn a_clean_roaming_batch_reconciles_end_to_end() {
    let records: Vec<UsageRecord> = (1..=5)
        .map(|i| event(i, "2026-07-25T10:00:00+02:00", 1_000))
        .collect();
    let dispositions = process(&records);

    let released = aggregate_released(&records, &dispositions).unwrap();
    assert_eq!(released.total().records(), 5);
    assert_eq!(released.total().volume().uplink(), Octets::new(5_000));
    assert!(released.is_consistent());

    // The partner's audit block agrees with what arrived.
    let charge = ScaledAmount::new(12_500, 3);
    assert!(
        batch(5, charge)
            .audit(BatchTotals::new(released.total().records(), charge))
            .is_empty()
    );

    let mut tracker = SequenceTracker::new(SequenceSpace::TAP);
    tracker.observe_default(42);
    let findings = validate_records(&records, &ValidationConfig::default(), NOW);
    let quality = score_batch(&released.reconciliation(), &findings, tracker.is_complete());
    assert!(!quality.blocks_release(), "{quality}");
    assert!(quality.is_final());
}

/// A file that disagrees with its own audit block is corrupt however well it
/// parsed — and every discrepancy is reported, not just the first.
#[test]
fn a_batch_that_miscounts_itself_is_rejected_with_every_reason() {
    let records: Vec<UsageRecord> = (1..=5)
        .map(|i| event(i, "2026-07-25T10:00:00+02:00", 1_000))
        .collect();
    let dispositions = process(&records);
    let released = aggregate_released(&records, &dispositions).unwrap();

    let declared = ScaledAmount::new(99_999, 3);
    let actual = ScaledAmount::new(12_500, 3);
    let found = batch(9, declared).audit(BatchTotals::new(released.total().records(), actual));

    assert_eq!(found.len(), 2);
    assert!(found.contains(&BatchDiscrepancy::EventCountMismatch {
        declared: 9,
        actual: 5,
    }));
    assert!(found.iter().all(|d| d.rejects_the_file()));
}

/// Duplicates inside a partner's file are suppressed, and the audit block's
/// count is against what *arrived* rather than what was billed — the two
/// legitimately differ.
#[test]
fn a_partner_resend_inside_one_file_is_suppressed_without_breaking_the_audit() {
    let mut records: Vec<UsageRecord> = (1..=3)
        .map(|i| event(i, "2026-07-25T10:00:00+02:00", 1_000))
        .collect();
    records.push(event(1, "2026-07-25T10:00:00+02:00", 1_000));

    let dispositions = process(&records);
    let released = aggregate_released(&records, &dispositions).unwrap();

    assert_eq!(released.reconciliation().ingested(), 4);
    assert_eq!(released.reconciliation().released(), 3);
    assert_eq!(released.reconciliation().duplicate(), 1);
    assert!(released.is_consistent());

    // The file declared four events, and four arrived.
    let charge = ScaledAmount::zero(3);
    assert!(
        batch(4, charge)
            .audit(BatchTotals::new(4, charge))
            .is_empty()
    );
    // …while only three were billed, which is a different number by design.
    assert_eq!(released.total().records(), 3);
}

/// TD.57's 30-day rule: an event too old to be charged is caught by the batch
/// audit and by the record-level catalogue alike.
#[test]
fn an_event_past_the_inclusion_window_is_caught_at_both_levels() {
    let stale = event(1, "2026-01-01T10:00:00+02:00", 1_000);
    let findings = validate_records(
        std::slice::from_ref(&stale),
        &ValidationConfig::default(),
        NOW,
    );
    assert!(findings.has(mediation::Rule::BeyondDeadline));
    assert!(findings.blocks_billing());

    let mut aged = batch(1, ScaledAmount::zero(3));
    aged.earliest_event = Some("2026-01-01T10:00:00+02:00".parse().unwrap());
    assert!(
        aged.audit(BatchTotals::new(1, ScaledAmount::zero(3)))
            .iter()
            .any(|d| matches!(d, BatchDiscrepancy::EventBeyondInclusionWindow { .. }))
    );
}

// ── RAP returns ──────────────────────────────────────────────────────────────

/// A fatal return means the batch never counted: the period's carry must not
/// inherit anything from it, and the sequence number stays open.
#[test]
fn a_fatal_return_removes_the_batch_from_the_period() {
    let records: Vec<UsageRecord> = (1..=4)
        .map(|i| event(i, "2026-07-25T10:00:00+02:00", 1_000))
        .collect();
    let original: BatchReconciliation = process(&records).into_iter().collect();
    assert_eq!(original.released(), 4);

    let after = RapReturn::Fatal.effect_on(original).unwrap();
    assert_eq!(after, BatchReconciliation::EMPTY);
    assert!(after.balances());

    // A period built on the returned batch has nothing outstanding, because
    // nothing in it ever counted.
    let mut period = PeriodLedger::new();
    period.append(after).unwrap();
    assert!(period.is_closable());
    assert_eq!(period.totals().ingested(), 0);

    // …and the file's sequence number is still missing until it is resent.
    let mut tracker = SequenceTracker::new(SequenceSpace::TAP);
    tracker.observe_default(41);
    tracker.observe_default(43);
    assert_eq!(tracker.missing(), vec![42]);
    assert!(!tracker.is_complete());
}

/// A severe return leaves the batch counted and moves the returned events from
/// billable to rejected — and the law still holds.
#[test]
fn a_severe_return_adjusts_the_batch_without_unwinding_it() {
    let records: Vec<UsageRecord> = (1..=10)
        .map(|i| event(i, "2026-07-25T10:00:00+02:00", 1_000))
        .collect();
    let original: BatchReconciliation = process(&records).into_iter().collect();

    let after = RapReturn::Severe { returned_events: 3 }
        .effect_on(original)
        .unwrap();

    assert_eq!(after.ingested(), 10, "the file still counted");
    assert_eq!(after.released(), 7);
    assert_eq!(after.rejected(), 3);
    assert!(after.balances());
    assert_eq!(
        RapReturn::Severe { returned_events: 3 }.accepted_events(10),
        Some(7)
    );

    // The period sees a balanced, final batch.
    let mut period = PeriodLedger::new();
    period.append(after).unwrap();
    assert!(period.is_closable());
}

/// Returning more than was released means the two sides disagree about the
/// file, which is not arithmetic to absorb quietly.
#[test]
fn a_return_larger_than_the_batch_is_a_disagreement_not_a_correction() {
    let records: Vec<UsageRecord> = (1..=2)
        .map(|i| event(i, "2026-07-25T10:00:00+02:00", 1_000))
        .collect();
    let original: BatchReconciliation = process(&records).into_iter().collect();

    assert_eq!(
        RapReturn::Severe { returned_events: 5 }.effect_on(original),
        None
    );
    assert_eq!(
        RapReturn::Severe { returned_events: 5 }.accepted_events(2),
        None
    );
}

// ── Money ────────────────────────────────────────────────────────────────────

/// A settlement amount is an integer with a scale, and it never becomes a float
/// on the way through.
#[test]
fn charges_sum_exactly_at_one_scale() {
    let charges = [
        ScaledAmount::new(1_234, 3),
        ScaledAmount::new(5, 3),
        ScaledAmount::new(999_999, 3),
    ];
    let total = charges
        .iter()
        .try_fold(ScaledAmount::zero(3), |acc, c| acc.checked_add(*c))
        .unwrap();

    assert_eq!(total, ScaledAmount::new(1_001_238, 3));
    assert_eq!(total.to_string(), "1001.238");

    // A partner sending one line at a different precision is a decoding fault,
    // not a rounding one — so it stops the sum.
    assert!(total.checked_add(ScaledAmount::new(12, 2)).is_none());
}

/// A credit is an amount like any other, and renders with its sign in front of
/// the whole number.
#[test]
fn a_credit_nets_against_a_charge() {
    let charge = ScaledAmount::new(1_234, 3);
    let credit = charge.checked_negate().unwrap();

    assert_eq!(credit.to_string(), "-1.234");
    assert_eq!(charge.checked_add(credit).unwrap(), ScaledAmount::zero(3));
    assert!(charge.checked_add(credit).unwrap().is_zero());
}

/// A severe return moves the returned events out of `billable` and touches
/// nothing else. Rebuilding the reconciliation record by record used to drop
/// the carry and turn every duplicate into a suppression — an unbalanced period
/// and a wrong revenue-assurance report, from a partner's routine rejection.
#[test]
fn a_severe_return_moves_only_the_returned_events() {
    let mut batch = BatchReconciliation::EMPTY;
    batch.observe_carried(&Disposition::Billable);
    batch.observe(&Disposition::Billable);
    batch.observe(&Disposition::Billable);
    batch.observe(&Disposition::Duplicate(RecordKey::RecordingEntity {
        entity: "msc-01".parse().unwrap(),
        local_sequence: 1,
    }));
    batch.observe(&Disposition::Held(mediation::HoldReason::AwaitingPartial));

    let after = RapReturn::Severe { returned_events: 1 }
        .effect_on(batch)
        .unwrap();

    assert_eq!(after.carried_in(), 1, "the carry survives");
    assert_eq!(after.ingested(), batch.ingested());
    assert_eq!(after.released(), 2);
    assert_eq!(after.rejected(), 1);
    assert_eq!(after.duplicate(), 1, "a duplicate is not a suppression");
    assert_eq!(after.held(), 1);
    assert!(after.balances());

    // More back than went out is a disagreement with the partner, not
    // arithmetic to absorb.
    assert!(
        RapReturn::Severe {
            returned_events: 99
        }
        .effect_on(batch)
        .is_none()
    );
}

/// A fatal return unwinds what the batch ingested. What it *carried in* was a
/// predecessor's hold and is outstanding again — dropping it would break the
/// period chain at the boundary nobody audits.
#[test]
fn a_fatal_return_leaves_the_carry_outstanding() {
    let mut batch = BatchReconciliation::EMPTY;
    batch.observe_carried(&Disposition::Billable);
    batch.observe_carried(&Disposition::Billable);
    batch.observe(&Disposition::Billable);

    let after = RapReturn::Fatal.effect_on(batch).unwrap();

    assert_eq!(after.ingested(), 0, "nothing this batch ingested counted");
    assert_eq!(after.carried_in(), 2);
    assert_eq!(after.held(), 2, "still owed by the period");
    assert!(after.balances());
}

/// TD.57's audit block declares three totals, and a discrepancy in each sends
/// someone to a different place.
#[test]
fn each_declared_total_is_checked_and_names_itself() {
    let charge = ScaledAmount::new(1_000, 3);
    let mut batch = batch(2, charge);
    batch.declared_discount = Some(ScaledAmount::new(100, 3));
    batch.declared_tax = Some(ScaledAmount::new(190, 3));

    // Everything agrees.
    let agreeing = BatchTotals::new(2, charge)
        .with_discount(ScaledAmount::new(100, 3))
        .with_tax(ScaledAmount::new(190, 3));
    assert!(batch.audit(agreeing).is_empty());

    // The tax is out, and the finding says so rather than saying "totals".
    let wrong_tax = BatchTotals::new(2, charge)
        .with_discount(ScaledAmount::new(100, 3))
        .with_tax(ScaledAmount::new(200, 3));
    let found = batch.audit(wrong_tax);
    assert_eq!(found.len(), 1);
    assert!(matches!(
        found[0],
        BatchDiscrepancy::ChargeMismatch {
            kind: TotalKind::Tax,
            ..
        }
    ));
    assert!(
        found[0].to_string().contains("totalTaxValue"),
        "{}",
        found[0]
    );
}

/// A total the batch did not declare is not a disagreement. Comparing an absent
/// optional field against zero raises a discrepancy on every well-formed file
/// from a partner who simply omits it.
#[test]
fn an_undeclared_total_is_not_a_discrepancy() {
    let charge = ScaledAmount::new(1_000, 3);
    let batch = batch(2, charge); // no discount, no tax declared

    let with_extras = BatchTotals::new(2, charge)
        .with_discount(ScaledAmount::new(100, 3))
        .with_tax(ScaledAmount::new(190, 3));
    assert!(batch.audit(with_extras).is_empty());
}

/// TAP carries local time and its offset as **two separate fields**, usually
/// with the offset behind a code table declared once per file. A decoder that
/// reads the local timestamp and forgets to resolve the code produces a value
/// that looks complete, parses cleanly, and is wrong by up to fourteen hours
/// for every event in the file — so there is no constructor that takes the
/// local half alone.
#[test]
fn a_tap_timestamp_needs_both_of_its_fields() {
    let ts = CdrTimestamp::from_tap("20260728103045", "+0545").unwrap();
    assert_eq!(ts.to_string(), "2026-07-28T10:30:45+05:45");
    assert_eq!(
        ts.to_tap(),
        ("20260728103045".to_owned(), "+0545".to_owned())
    );

    // Unlike the 3GPP BCD form, TAP carries the century — so no pivot, and no
    // hundred-year relocation.
    let last_century = CdrTimestamp::from_tap("19990101000000", "+0000").unwrap();
    assert_eq!(last_century.local_date().year(), 1999);

    // Every quarter-hour zone survives both directions.
    for offset in [
        "-1200", "-0330", "+0000", "+0530", "+0545", "+0845", "+1400",
    ] {
        let ts = CdrTimestamp::from_tap("20260728103045", offset).unwrap();
        assert_eq!(ts.to_tap().1, offset);
    }
}

#[test]
fn a_malformed_tap_timestamp_is_refused() {
    for (local, offset) in [
        ("2026072810304", "+0200"),   // thirteen digits
        ("202607281030455", "+0200"), // fifteen
        ("2026072810304X", "+0200"),  // not a digit
        ("20260230103045", "+0200"),  // 30 February
        ("20260728253045", "+0200"),  // hour 25
        ("20260728103045", "+02:00"), // the RFC 3339 spelling is not TAP's
        ("20260728103045", "+0220"),  // not a multiple of 15 minutes
        ("20260728103045", "+1500"),  // beyond the assigned range
    ] {
        assert!(
            CdrTimestamp::from_tap(local, offset).is_err(),
            "{local:?}/{offset:?} must not decode"
        );
    }
}
