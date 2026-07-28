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
    BatchReconciliation, Disposition, HoldReason, NotKeyable, RecordKey, RejectReason,
    SuppressReason, UnattributableReason,
};
use proptest::prelude::*;

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
