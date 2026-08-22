+++
title = "One pass, and the order it runs in"
description = "The assembly of every other piece, in a fixed order — because the order is itself a decision that is easy to get wrong quietly."
weight = 60
+++

Every piece of this crate works on its own. `process` is the assembly, and it
exists because the **order** of the pieces is itself a decision that is easy to
get wrong — and wrong quietly.

Each record reaches the first outcome that applies:

| # | Test | Outcome |
|---|---|---|
| 1 | no natural key | rejected — `NotKeyable` |
| 2 | key already seen inside the window | duplicate |
| 3 | no charged party | rejected — `Unattributable` |
| 4 | timestamped ahead of `as_of` | rejected — `FutureTimestamp` |
| 5 | past the inclusion deadline | rejected — `BeyondDeadline` |
| 6 | a blocking rule fired | rejected — `Malformed` |
| 7 | its run of partials cannot be reassembled | rejected — `Malformed` |
| 8 | its run of partials is not finished | **held** — `AwaitingPartial` |
| 9 | the suppression policy says so | suppressed |
| 10 | its source has an unfilled sequence hole | **held** — `SequenceGap` |
| 11 | otherwise | **billable** |

Three placements are load-bearing.

**Keyability before everything.** A record with no natural key cannot be
deduplicated, so billing it risks charging for it twice — the more expensive of
the two errors — and it is rejected before any test that might have let it
through.

**Deduplication before judgement.** A retransmission of a malformed record is a
retransmission. Rejecting it a second time as malformed would report two faults
where there was one, and would leave the sender's resend invisible in the counts.

**Completeness is per source, and it is the last test.** A gateway with a lost
file cannot prove what it is missing, so its records are held — but only *its*
records, and only the ones that were going to be billed. Blocking the whole
batch instead stops an operator's entire billing run on one node's hole; holding
a record the suppression policy had already ruled out re-presents a foregone
conclusion in every batch until the file arrives. Because nothing from an
incomplete source is ever *released*, what a batch does release is complete by
construction.

```rust
use mediation::{Batch, DedupWindow, Disposition, ProcessConfig, ProcessState, RejectReason, process};
use time::{Duration, macros::datetime};
use mediation::{ChargedParty, RecordType, SourceRef, SubscriberRef, UsageRecord};
// A record carrying no correlator at all cannot be deduplicated.
let bare = UsageRecord::new(
    RecordType::PduSession,
    ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
    "2026-07-28T10:00:00+02:00".parse().unwrap(),
    SourceRef::new("pgw-01".parse().unwrap(), 0),
);

let mut state = ProcessState::new(DedupWindow::from_bound(Duration::days(7)).unwrap());
let outcome = process(
    &Batch::new(&[bare]),
    &ProcessConfig::default(),
    datetime!(2026-07-28 12:00:00 UTC),
    &mut state,
);

assert!(matches!(
    outcome.dispositions()[0],
    Disposition::Rejected(RejectReason::NotKeyable(_)),
));
assert!(outcome.reconciliation().balances());
```

## Suppression is policy, and the crate holds none of it

A test SIM, a zero-rated emergency call, traffic netted off between an
operator's own networks — which records those are is a commercial fact, so
`process_with` takes a `Suppression` of yours. A crate that guessed which
identities were test SIMs would suppress real revenue.

## A held record comes back

`Batch::with_carry` marks the records an earlier batch held and this one
re-presents. They are counted through `observe_carried` rather than as new
arrivals — and they **skip deduplication**, because asking the store again would
recognise your own hold coming back as a duplicate of itself and the usage would
never be billed at all. A genuine retransmission arriving while the record is
held is still caught, because the key stayed in the store.

Chain the reconciliations through a `PeriodLedger` and the conservation law
telescopes across the whole period.

## The state is yours, and it round-trips

Deduplication and gap detection are inherently stateful — you cannot recognise a
retransmission without remembering, and you cannot see a hole without knowing
what came before. So `ProcessState` is caller-owned and passed in, which keeps
`process` a pure mapping from `(batch, config, as_of, state)` to
`(outcome, state′)`.

These two are the crate's load-bearing correctness controls, and a restart that
forgot them would re-bill a window of retransmissions and declare every source
complete. So the whole of the state is reachable and rebuildable —
`DedupStore::iter` / `restore`, `SequenceTracker::snapshot` / `restore`,
`ProcessState::trackers` / `with_tracker` — and with the `serde` feature the
whole thing serialises in one call:

```rust
#[cfg(feature = "serde")] {
use mediation::{DedupWindow, ProcessState};
use time::Duration;

let state = ProcessState::new(DedupWindow::from_bound(Duration::days(7)).unwrap());

let saved = serde_json::to_string(&state).unwrap();
let restored: ProcessState = serde_json::from_str(&saved).unwrap();
assert_eq!(restored.dedup().len(), state.dedup().len());
}
```

## What it does not do

`process` does not **correlate**, because correlation changes the cardinality of
the batch: two records describing one bearer become one usage, and the
conservation law counts records. Joining legs is `correlation::correlate`, run
before or after this pass depending on whether your feed carries both sides in
one file.

It does not **rate**, **aggregate by tariff** or **round**.
