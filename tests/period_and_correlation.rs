//! The two boundaries where usage goes missing without anyone noticing.
//!
//! **Between batches.** A held record comes back. Counted as newly ingested it
//! is billed twice across two batches that each balance; counted by neither it
//! vanishes. Both are the conservation law failing at a seam.
//!
//! **Between nodes.** Two records describe one bearer. Billing both charges the
//! subscriber twice; discarding the unmatched one hides the loss of whichever
//! side was going to be billed.
//!
//! Neither failure is visible inside a single batch or a single record, which
//! is why both are exercised here rather than only in unit tests.

use mediation::{
    BatchReconciliation, ChainBreak, ChargedParty, ChargingId, CorrelationConfig, CorrelationKeys,
    Disposition, HoldReason, Octets, PeriodLedger, RecordKey, RecordType, RestateReason,
    Restatement, RestatementLog, SourceRef, SubscriberRef, UsageDelta, UsageRecord, VolumePair,
    correlate,
};
use proptest::prelude::*;
use std::net::Ipv4Addr;

fn party() -> ChargedParty {
    ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap()))
}

fn bearer_record(node: &str, charging_id: u32, gateway: u8, billable: bool) -> UsageRecord {
    let mut record = UsageRecord::new(
        RecordType::PduSession,
        party(),
        "2026-07-28T10:00:00+02:00".parse().unwrap(),
        SourceRef::new(node.parse().unwrap(), 0),
    )
    .with_correlation(
        CorrelationKeys::default()
            .with_charging_id(ChargingId::new(charging_id))
            .with_gateway(Ipv4Addr::new(10, 0, 0, gateway).into()),
    );
    if billable {
        record.volume = Some(VolumePair::new(Octets::new(100), Octets::new(900)));
    }
    record
}

fn key(serial: u32) -> RecordKey {
    RecordKey::RecordingEntity {
        entity: "msc-01".parse().unwrap(),
        local_sequence: serial,
    }
}

// ── The batch boundary ───────────────────────────────────────────────────────

/// A held record that comes back, handled correctly: the period telescopes and
/// closes.
#[test]
fn a_held_record_crosses_a_boundary_exactly_once() {
    let mut monday = BatchReconciliation::EMPTY;
    monday.observe(&Disposition::Billable);
    monday.observe(&Disposition::Held(HoldReason::AwaitingPartial));

    // Tuesday: the missing partial arrived, so the held record completes, and
    // one new record arrives alongside it.
    let mut tuesday = BatchReconciliation::EMPTY;
    tuesday.observe_carried(&Disposition::Billable);
    tuesday.observe(&Disposition::Billable);

    let mut period = PeriodLedger::new();
    period.append(monday).unwrap();
    period.append(tuesday).unwrap();

    assert!(period.is_closable());
    assert_eq!(period.outstanding(), 0);
    assert_eq!(period.totals().ingested(), 3, "three records ever arrived");
    assert_eq!(period.totals().released(), 3);
    assert_eq!(period.totals().carried_in(), 1);
}

/// The failure the ledger exists for: two batches that each balance, and a
/// record billed twice between them.
#[test]
fn double_counting_across_a_boundary_is_caught_although_each_batch_balances() {
    let mut monday = BatchReconciliation::EMPTY;
    monday.observe(&Disposition::Held(HoldReason::AwaitingCorrelation));

    let mut tuesday = BatchReconciliation::EMPTY;
    tuesday.observe(&Disposition::Billable); // wrong: it was carried, not new

    assert!(monday.balances());
    assert!(tuesday.balances(), "each batch is internally consistent");

    let mut period = PeriodLedger::new();
    period.append(monday).unwrap();
    assert_eq!(
        period.append(tuesday),
        Err(ChainBreak::CarryMismatch {
            batch: 1,
            expected: 1,
            found: 0,
        })
    );
    assert!(!period.is_closable());
}

/// A period stays open while anything is held, and a batch that resolves the
/// last held record closes it.
#[test]
fn a_period_closes_only_when_nothing_is_outstanding() {
    let mut period = PeriodLedger::new();

    let mut first = BatchReconciliation::EMPTY;
    first.observe(&Disposition::Held(HoldReason::SequenceGap));
    first.observe(&Disposition::Held(HoldReason::SequenceGap));
    period.append(first).unwrap();
    assert!(!period.is_closable());
    assert_eq!(period.outstanding(), 2);

    // One resolves, one is still waiting.
    let mut second = BatchReconciliation::EMPTY;
    second.observe_carried(&Disposition::Billable);
    second.observe_carried(&Disposition::Held(HoldReason::SequenceGap));
    period.append(second).unwrap();
    assert!(!period.is_closable());
    assert_eq!(period.outstanding(), 1);

    // The resend arrives and the last one is rejected as unbillable.
    let mut third = BatchReconciliation::EMPTY;
    third.observe_carried(&Disposition::Rejected(
        mediation::RejectReason::BeyondDeadline,
    ));
    period.append(third).unwrap();
    assert!(period.is_closable());
    assert!(period.breaks().is_empty());
}

proptest! {
    /// However many batches a period runs to, and however work is held between
    /// them, a correctly-chained period accounts for everything.
    #[test]
    fn a_correctly_chained_period_always_closes(
        arrivals in proptest::collection::vec(1usize..8, 1..30),
        holds in proptest::collection::vec(0usize..3, 1..30),
    ) {
        let mut period = PeriodLedger::new();
        let mut outstanding = 0u64;
        let mut ever_arrived = 0u64;

        for (index, arriving) in arrivals.iter().enumerate() {
            let mut batch = BatchReconciliation::EMPTY;

            // Everything held by the previous batch is re-presented here.
            let carried = outstanding;
            for _ in 0..carried {
                batch.observe_carried(&Disposition::Billable);
            }

            for _ in 0..*arriving {
                batch.observe(&Disposition::Billable);
            }
            ever_arrived += *arriving as u64;

            // Some of this batch's own arrivals are held back for the next one.
            let hold = (*holds.get(index).unwrap_or(&0)).min(*arriving) as u64;
            for _ in 0..hold {
                batch.observe(&Disposition::Held(HoldReason::AwaitingPartial));
                ever_arrived += 1;
            }

            prop_assert!(batch.balances(), "{}", batch);
            prop_assert_eq!(period.append(batch), Ok(()));
            outstanding = hold;
        }

        prop_assert!(period.breaks().is_empty());
        prop_assert_eq!(period.totals().ingested(), ever_arrived);
        prop_assert_eq!(period.outstanding(), outstanding);
        prop_assert_eq!(period.is_closable(), outstanding == 0);
    }
}

// ── The node boundary ────────────────────────────────────────────────────────

/// Two nodes describing one bearer: flagged before either is billed twice, and
/// neither record is discarded.
#[test]
fn one_bearer_seen_twice_is_joined_rather_than_billed_twice() {
    let records = vec![
        bearer_record("sgw-01", 42, 1, true),
        bearer_record("pgw-01", 42, 1, true),
        bearer_record("pgw-01", 43, 1, true),
    ];

    let result = correlate(&records, &CorrelationConfig::packet_bearer());
    assert!(result.accounts_for(records.len()));
    assert_eq!(result.groups().len(), 2);
    assert_eq!(result.double_counting_groups().len(), 1);

    // Billing one member of each group is three records reduced to two usages.
    let billable: usize = result.groups().len();
    assert_eq!(billable, 2);
}

/// Correlation composes with the conservation law: the counterpart of a joined
/// group is suppressed, not dropped, so the batch still balances.
#[test]
fn suppressing_a_counterpart_keeps_the_batch_balanced() {
    let records = vec![
        bearer_record("sgw-01", 42, 1, true),
        bearer_record("pgw-01", 42, 1, true),
    ];
    let result = correlate(&records, &CorrelationConfig::packet_bearer());

    let mut counts = BatchReconciliation::EMPTY;
    for group in result.groups() {
        for (position, _) in group.members.iter().enumerate() {
            // The first member is billed; every other member of the same usage
            // is suppressed rather than discarded.
            counts.observe(&if position == 0 {
                Disposition::Billable
            } else {
                Disposition::Suppressed(mediation::SuppressReason::InternalTraffic)
            });
        }
    }
    for _ in result.unmatched() {
        counts.observe(&Disposition::Held(HoldReason::AwaitingCorrelation));
    }

    assert_eq!(counts.ingested(), 2);
    assert_eq!(counts.released(), 1, "one usage, billed once");
    assert_eq!(counts.suppressed(), 1);
    assert!(counts.balances());
}

/// A record whose counterpart never arrived is held, not dropped — dropping it
/// is how the loss becomes invisible.
#[test]
fn an_unmatched_record_is_held_rather_than_discarded() {
    let records = vec![bearer_record("smsc-01", 0, 1, true)];
    let mut stripped = records;
    stripped[0].correlation = CorrelationKeys::none();

    let result = correlate(&stripped, &CorrelationConfig::packet_bearer());
    assert!(result.groups().is_empty());
    assert_eq!(result.unmatched().len(), 1);
    assert!(result.accounts_for(1));

    let mut counts = BatchReconciliation::EMPTY;
    counts.observe(&Disposition::Held(HoldReason::AwaitingCorrelation));
    assert!(counts.balances());
    assert!(!counts.is_final(), "the loss stays visible");
}

// ── Corrections after the fact ───────────────────────────────────────────────

/// A correction discovered after a period closed reconciles in the period it
/// was found, and the original stays true.
#[test]
fn a_late_correction_does_not_disturb_a_closed_period() {
    let mut july = BatchReconciliation::EMPTY;
    july.observe(&Disposition::Billable);
    let mut period = PeriodLedger::new();
    period.append(july).unwrap();
    assert!(period.is_closable());

    // In August, a late partial reveals the July record was 300 seconds short.
    let mut log = RestatementLog::new();
    log.push(Restatement::new(
        key(1),
        RestateReason::LatePartial,
        UsageDelta::default().with_duration(300),
        "2026-08-02T09:00:00+02:00".parse().unwrap(),
    ));

    // July's counts are untouched — the correction is a separate event.
    assert_eq!(period.totals().released(), 1);
    assert!(period.is_closable());
    assert_eq!(
        log.net_for(&key(1)),
        Some(UsageDelta::default().with_duration(300))
    );
}

/// A correction issued in error is withdrawn by reversing it, and both entries
/// survive so the log stays an audit trail rather than a current state.
#[test]
fn a_withdrawn_correction_nets_to_nothing_and_leaves_a_trace() {
    let issued = Restatement::new(
        key(1),
        RestateReason::OperatorCorrection,
        UsageDelta::default().with_volume(-5_000, 0),
        "2026-08-02T09:00:00+02:00".parse().unwrap(),
    );
    let withdrawn = issued.clone().reverse();

    let log: RestatementLog = vec![issued, withdrawn].into_iter().collect();
    assert_eq!(log.net_for(&key(1)), Some(UsageDelta::ZERO));
    assert_eq!(log.len(), 2, "both entries remain");
    assert_eq!(log.corrected_records().len(), 1);
}

proptest! {
    /// Any sequence of corrections followed by their reversals, in any order,
    /// nets to nothing.
    #[test]
    fn corrections_and_their_reversals_always_cancel(
        deltas in proptest::collection::vec(-1_000_000i64..1_000_000, 1..40),
    ) {
        let mut entries: Vec<Restatement> = Vec::new();
        for (index, seconds) in deltas.iter().enumerate() {
            let entry = Restatement::new(
                key(u32::try_from(index % 5).unwrap()),
                RestateReason::Reclassification,
                UsageDelta::default().with_duration(*seconds),
                "2026-08-02T09:00:00+02:00".parse().unwrap(),
            );
            entries.push(entry.clone().reverse());
            entries.push(entry);
        }
        entries.reverse();

        let log: RestatementLog = entries.into_iter().collect();
        prop_assert_eq!(log.net(), Some(UsageDelta::ZERO));
        for serial in 0..5u32 {
            prop_assert_eq!(log.net_for(&key(serial)), Some(UsageDelta::ZERO));
        }
    }
}
