//! Completeness and idempotence, under generated input.
//!
//! Two properties the crate stands on:
//!
//! - **A wrap is not a gap.** Sequence comparison follows RFC 1982 serial-number
//!   arithmetic, so a TAP file counter rolling from `99999` to `00001` is a
//!   successor rather than a 99 998-file hole.
//! - **Deduplication is idempotent.** Feeding the same record any number of
//!   times yields exactly one billable outcome — never two, never zero.

use mediation::{
    ChargedParty, ChargingId, CorrelationKeys, DedupDecision, DedupStore, DedupWindow, Disposition,
    NotKeyable, RecordKey, RecordType, SequenceObservation, SequenceSpace, SequenceTracker,
    SequenceValue, SourceRef, SubscriberRef, SuppressReason, UsageRecord,
};
use proptest::prelude::*;
use std::collections::BTreeSet;
use std::net::Ipv4Addr;
use time::{Duration, OffsetDateTime, macros::datetime};

const NOW: OffsetDateTime = datetime!(2026-07-28 00:00:00 UTC);

/// The retransmission bound these tests assume.
fn week() -> DedupWindow {
    DedupWindow::from_bound(Duration::days(7)).expect("positive")
}

fn tap(value: u32) -> SequenceValue {
    SequenceValue::new(SequenceSpace::TAP, value).expect("in space")
}

fn record(charging_id: u32, gateway: u8) -> UsageRecord {
    UsageRecord::new(
        RecordType::PduSession,
        ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
        "2026-07-28T10:00:00+02:00".parse().unwrap(),
        SourceRef::new("pgw-fra-01".parse().unwrap(), 0),
    )
    .with_correlation(
        CorrelationKeys::default()
            .with_charging_id(ChargingId::new(charging_id))
            .with_gateway(Ipv4Addr::new(10, 0, 0, gateway).into()),
    )
}

// ── Sequence arithmetic ──────────────────────────────────────────────────────

proptest! {
    /// Every value's successor is its successor, including across the wrap.
    #[test]
    fn successors_are_recognised_everywhere_in_the_space(value in 1u32..=99_999) {
        let current = tap(value);
        let next = current.successor();
        prop_assert!(current.is_successor(next), "{value} -> {next}");
        prop_assert_eq!(current.gap_to(next), Some(0));
        prop_assert!(!current.is_regression(next));
    }

    /// A forward jump reports exactly the values it skipped, wrap included.
    #[test]
    fn a_forward_jump_counts_the_values_it_skipped(
        from in 1u32..=99_999,
        skip in 1u64..1_000,
    ) {
        let start = tap(from);
        let mut cursor = start;
        for _ in 0..=skip {
            cursor = cursor.successor();
        }
        prop_assert_eq!(start.gap_to(cursor), Some(skip));
    }

    /// Moving forward and moving back are mutually exclusive and exhaustive for
    /// distinct values — there is no third answer for a comparison to fall into.
    #[test]
    fn forward_and_backward_partition_the_space(a in 1u32..=99_999, b in 1u32..=99_999) {
        let (x, y) = (tap(a), tap(b));
        if a == b {
            prop_assert_eq!(x.gap_to(y), None);
            prop_assert!(!x.is_regression(y));
        } else {
            prop_assert_ne!(x.gap_to(y).is_some(), x.is_regression(y));
        }
    }

    /// A tracker that receives a contiguous run, in order, reports no holes —
    /// whatever point in the space the run starts at.
    #[test]
    fn a_contiguous_run_never_reports_a_gap(start in 1u32..=99_999, length in 1u32..500) {
        let mut tracker = SequenceTracker::new(SequenceSpace::TAP);
        let mut cursor = tap(start);
        for _ in 0..length {
            let observation = tracker.observe_default(cursor.get());
            prop_assert!(
                matches!(
                    observation,
                    SequenceObservation::First | SequenceObservation::InOrder
                ),
                "{} was classified as {:?}",
                cursor,
                observation
            );
            cursor = cursor.successor();
        }
        prop_assert!(tracker.is_complete());
    }

    /// Withholding one file from the middle of a run leaves exactly that file
    /// outstanding, and delivering it later closes the hole.
    ///
    /// The hole is deliberately never the last file: see
    /// `a_hole_at_the_end_of_a_stream_is_invisible_until_the_next_file`.
    #[test]
    fn a_withheld_file_is_named_and_then_cleared(
        start in 1u32..=99_000,
        length in 3u32..200,
        offset in 0u32..1_000,
    ) {
        // 1 ..= length - 2: never the first file (nothing to compare against)
        // and never the last (nothing arrives afterwards to reveal it).
        let skip_at = 1 + offset % (length - 2);

        let mut tracker = SequenceTracker::new(SequenceSpace::TAP);
        let mut cursor = tap(start);
        let mut withheld = None;
        for i in 0..length {
            if i == skip_at {
                withheld = Some(cursor.get());
            } else {
                tracker.observe_default(cursor.get());
            }
            cursor = cursor.successor();
        }

        let withheld = withheld.expect("one file is always withheld");
        prop_assert_eq!(tracker.missing(), vec![withheld]);
        prop_assert!(!tracker.is_complete());

        prop_assert_eq!(
            tracker.observe_default(withheld),
            SequenceObservation::FilledGap
        );
        prop_assert!(tracker.is_complete());
    }
}

/// The bug this module exists for, spelled out end to end: a partner whose file
/// counter rolls over must not look like a partner who lost 99 998 files.
#[test]
fn a_full_cycle_of_tap_sequences_never_manufactures_a_hole() {
    let mut tracker = SequenceTracker::new(SequenceSpace::TAP);
    let mut cursor = tap(99_990);
    for _ in 0..25 {
        let observation = tracker.observe_default(cursor.get());
        assert!(
            !observation.opened_a_gap(),
            "{cursor} opened a gap: {observation:?}"
        );
        cursor = cursor.successor();
    }
    assert!(tracker.is_complete());
    assert_eq!(tracker.missing_count(), 0);
}

/// A truth about sequence tracking worth stating plainly: a hole at the **end**
/// of what has arrived is indistinguishable from "nothing more has been sent
/// yet". Only the next file reveals it.
///
/// This is why completeness is a running assessment rather than a verdict a
/// batch can reach on its own, and why a source that has gone quiet needs a
/// separate liveness check that this crate deliberately does not provide — that
/// would require a clock.
#[test]
fn a_hole_at_the_end_of_a_stream_is_invisible_until_the_next_file() {
    let mut tracker = SequenceTracker::new(SequenceSpace::TAP);
    tracker.observe_default(1);
    tracker.observe_default(2);

    // File 3 never arrives. Nothing so far can show that.
    assert!(tracker.is_complete());
    assert_eq!(tracker.high_water(), Some(2));

    // File 4 arrives, and only now is file 3 known to be missing.
    assert_eq!(
        tracker.observe_default(4),
        SequenceObservation::Gap {
            missing: vec![3],
            missing_count: 1,
        }
    );
    assert_eq!(tracker.missing(), vec![3]);
}

/// Out-of-order delivery within a window is normal and must not be read as loss
/// followed by mystery arrivals.
#[test]
fn out_of_order_delivery_resolves_to_complete() {
    let mut tracker = SequenceTracker::new(SequenceSpace::TAP);
    for value in [1, 4, 2, 5, 3, 6] {
        tracker.observe_default(value);
    }
    assert!(tracker.is_complete(), "missing {:?}", tracker.missing());
    assert_eq!(tracker.high_water(), Some(6));
}

// ── Deduplication ────────────────────────────────────────────────────────────

proptest! {
    /// The idempotence property: however many copies of one record arrive, in
    /// whatever order, exactly one is billable and the rest are duplicates.
    #[test]
    fn any_number_of_copies_yields_exactly_one_billable(copies in 1usize..50) {
        let mut store = DedupStore::new(week());
        let key = RecordKey::of(&record(1_234, 1)).unwrap();

        let dispositions: Vec<Disposition> = (0..copies)
            .map(|i| {
                let at = NOW + Duration::seconds(i as i64);
                match store.observe(key.clone(), at) {
                    DedupDecision::First => Disposition::Billable,
                    DedupDecision::Duplicate { .. } => Disposition::Duplicate(key.clone()),
                }
            })
            .collect();

        let billable = dispositions.iter().filter(|d| d.is_released()).count();
        prop_assert_eq!(billable, 1, "exactly one copy may be billed");
        prop_assert_eq!(dispositions.len(), copies);
    }

    /// Distinct sessions are never merged, however similar their identifiers.
    #[test]
    fn distinct_sessions_keep_distinct_keys(a in any::<u32>(), b in any::<u32>()) {
        prop_assume!(a != b);
        let ka = RecordKey::of(&record(a, 1)).unwrap();
        let kb = RecordKey::of(&record(b, 1)).unwrap();
        prop_assert_ne!(&ka, &kb);

        // Same counter value, different gateway: also distinct, per TS 32.251.
        let same_id_other_gateway = RecordKey::of(&record(a, 2)).unwrap();
        prop_assert_ne!(&ka, &same_id_other_gateway);
    }

    /// A run of distinct records is billed once each — the dedup store must not
    /// suppress anything it has not actually seen.
    #[test]
    fn distinct_records_all_pass(count in 1usize..200) {
        let mut store = DedupStore::new(week());
        let mut keys = BTreeSet::new();

        for i in 0..count {
            let key = RecordKey::of(&record(u32::try_from(i).unwrap(), 1)).unwrap();
            prop_assert_eq!(store.observe(key.clone(), NOW), DedupDecision::First);
            keys.insert(key);
        }
        prop_assert_eq!(store.len(), count);
        prop_assert_eq!(keys.len(), count);
    }
}

/// Deduplication and the conservation law compose: a batch of originals and
/// retransmissions still balances, and the billable count equals the number of
/// distinct records.
#[test]
fn dedup_and_reconciliation_compose() {
    let mut store = DedupStore::new(week());
    let mut counts = mediation::BatchReconciliation::EMPTY;

    // Three distinct sessions, delivered with duplicates interleaved.
    let arrivals = [1, 2, 1, 3, 2, 1, 3, 3];
    for charging_id in arrivals {
        let key = RecordKey::of(&record(charging_id, 1)).unwrap();
        let disposition = match store.observe(key.clone(), NOW) {
            DedupDecision::First => Disposition::Billable,
            DedupDecision::Duplicate { .. } => Disposition::Duplicate(key),
        };
        counts.observe(&disposition);
    }

    assert_eq!(counts.ingested(), 8);
    assert_eq!(counts.released(), 3, "three distinct sessions");
    assert_eq!(counts.duplicate(), 5);
    assert!(counts.balances());
    assert!(counts.is_final());
}

/// Eviction is the window's stated consequence, and it is visible in the
/// counts rather than hidden: a duplicate past the window is billed again.
#[test]
fn a_duplicate_past_the_window_is_billed_again_and_the_batch_still_balances() {
    // A deliberately short bound, so the window closes inside the test.
    let mut store = DedupStore::new(DedupWindow::from_bound(Duration::hours(1)).unwrap());
    let mut counts = mediation::BatchReconciliation::EMPTY;
    let key = RecordKey::of(&record(1, 1)).unwrap();

    counts.observe(&match store.observe(key.clone(), NOW) {
        DedupDecision::First => Disposition::Billable,
        DedupDecision::Duplicate { .. } => Disposition::Duplicate(key.clone()),
    });

    let later = NOW + Duration::hours(2);
    assert_eq!(store.evict(later), 1);

    counts.observe(&match store.observe(key.clone(), later) {
        DedupDecision::First => Disposition::Billable,
        DedupDecision::Duplicate { .. } => Disposition::Duplicate(key),
    });

    assert_eq!(counts.released(), 2, "the window's documented consequence");
    assert!(counts.balances());
}

/// A record that cannot be keyed cannot be deduplicated, so it is rejected
/// rather than billed — and the batch still balances.
#[test]
fn an_unkeyable_record_is_rejected_and_still_counts() {
    let unkeyable = UsageRecord::new(
        RecordType::SmsOriginated,
        ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
        "2026-07-28T10:00:00+02:00".parse().unwrap(),
        SourceRef::new("smsc-01".parse().unwrap(), 0),
    );

    let disposition = match RecordKey::of(&unkeyable) {
        Ok(_) => Disposition::Billable,
        Err(reason) => Disposition::Rejected(mediation::RejectReason::NotKeyable(reason)),
    };
    assert!(matches!(disposition, Disposition::Rejected(_)));

    let mut counts = mediation::BatchReconciliation::EMPTY;
    counts.observe(&disposition);
    counts.observe(&Disposition::Suppressed(SuppressReason::ZeroUsage));
    assert_eq!(counts.ingested(), 2);
    assert!(counts.balances());
    assert!(counts.is_final());
}

/// A record is keyed on the most specific **complete** correlator it carries.
/// A half one is a reason to refuse the record only when nothing else keyed it:
/// an IMS record that happens to carry a bearer's Charging ID without its
/// gateway is still perfectly keyable on its ICID, and rejecting it for the
/// stray field would lose the usage entirely.
#[test]
fn a_stray_half_correlator_does_not_defeat_a_complete_one() {
    let base = UsageRecord::new(
        RecordType::ImsSession,
        ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
        "2026-07-28T10:00:00+02:00".parse().unwrap(),
        SourceRef::new("scscf-01".parse().unwrap(), 0),
    );

    // A complete ICID, plus a Charging ID whose gateway the decoder dropped.
    let mut keys = CorrelationKeys::default().with_icid("icid-1".parse().unwrap());
    keys.charging_id = Some(ChargingId::new(42));
    let record = base.clone().with_correlation(keys);

    let key = RecordKey::of(&record).expect("the ICID keys it");
    assert_eq!(key.kind(), "IMS");

    // With nothing else to key on, the half identity is the reason, because it
    // points at a decoder dropping a field rather than at a record type that
    // never carries one.
    let only_half = CorrelationKeys {
        charging_id: Some(ChargingId::new(42)),
        ..Default::default()
    };
    assert_eq!(
        RecordKey::of(&base.clone().with_correlation(only_half)),
        Err(NotKeyable::IncompletePacketSessionId)
    );

    // And a record carrying nothing says so differently again.
    assert_eq!(RecordKey::of(&base), Err(NotKeyable::NoCorrelatorPresent));
}

/// A run of partials shares a session scope and does **not** share a key —
/// which is the point: neither partial deduplicates the other, and the run is
/// gathered by the scope.
#[test]
fn partials_of_one_session_share_a_scope_and_not_a_key() {
    let keyed = |sequence: u32| {
        let keys = CorrelationKeys::default()
            .with_charging_id(ChargingId::new(7))
            .with_gateway(std::net::Ipv4Addr::new(10, 0, 0, 1).into());
        let mut keys = keys;
        keys.record_sequence_number = Some(sequence);
        RecordKey::of(
            &UsageRecord::new(
                RecordType::PduSession,
                ChargedParty::Subscriber(SubscriberRef::new(
                    "imsi-262021234567890".parse().unwrap(),
                )),
                "2026-07-28T10:00:00+02:00".parse().unwrap(),
                SourceRef::new("pgw-01".parse().unwrap(), 0),
            )
            .with_correlation(keys),
        )
        .unwrap()
    };

    let (first, second) = (keyed(1), keyed(2));
    assert_ne!(first, second, "neither deduplicates the other");
    assert_eq!(first.session_scope(), second.session_scope());
    assert!(first.session_scope().is_some());
}

/// RFC 1982 leaves the ordering of two values exactly half a space apart
/// undefined. The crate resolves it forward, and says so: a hole that turns out
/// to be a regression costs a resend request, while a regression read as an
/// arrival silently abandons half a cycle of files.
#[test]
fn the_undefined_half_space_distance_is_read_forward() {
    let space = SequenceSpace::TAP;
    let half = space.size() / 2; // 49 999 for the TAP space

    let from = SequenceValue::new(space, 1).unwrap();
    let to = SequenceValue::new(space, 1 + half as u32).unwrap();

    assert_eq!(from.gap_to(to), Some(half - 1), "forward, not backward");
    assert!(!from.is_regression(to));

    // One step further is unambiguously a regression.
    let beyond = SequenceValue::new(space, 2 + half as u32).unwrap();
    assert_eq!(from.gap_to(beyond), None);
    assert!(from.is_regression(beyond));
}

// ── State survives a restart ─────────────────────────────────────────────────

/// A deduplication store you cannot read out is one you cannot persist, and a
/// store you cannot persist forgets a window of retransmissions on every
/// deployment. So the store round-trips.
#[test]
fn a_dedup_store_round_trips_through_its_own_accessors() {
    let window = DedupWindow::from_bound(Duration::days(7)).unwrap();
    let mut store = DedupStore::new(window);

    let first = RecordKey::of(&record(1, 1)).unwrap();
    let second = RecordKey::of(&record(2, 1)).unwrap();
    store.observe(first.clone(), datetime!(2026-07-28 10:00 UTC));
    store.observe(second.clone(), datetime!(2026-07-28 11:00 UTC));

    let saved: Vec<_> = store.iter().map(|(k, at)| (k.clone(), at)).collect();
    let restored = DedupStore::restore(window, saved);

    assert_eq!(restored.len(), 2);
    assert_eq!(restored.first_seen(&first), store.first_seen(&first));
    assert_eq!(restored.first_seen(&second), store.first_seen(&second));

    // And it still recognises the retransmission it was remembering.
    let mut restored = restored;
    assert!(matches!(
        restored.observe(first, datetime!(2026-07-28 12:00 UTC)),
        DedupDecision::Duplicate { .. }
    ));
}

/// A key restored twice keeps its earliest instant, so concatenating snapshots
/// cannot extend a key's life past its window.
#[test]
fn restoring_a_repeated_key_keeps_the_earliest_instant() {
    let window = DedupWindow::from_bound(Duration::days(7)).unwrap();
    let k = RecordKey::of(&record(7, 1)).unwrap();
    let store = DedupStore::restore(
        window,
        [
            (k.clone(), datetime!(2026-07-28 12:00 UTC)),
            (k.clone(), datetime!(2026-07-28 10:00 UTC)),
        ],
    );
    assert_eq!(store.first_seen(&k), Some(datetime!(2026-07-28 10:00 UTC)));
}
