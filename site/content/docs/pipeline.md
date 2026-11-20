+++
title = "One pass, and the order it runs in"
description = "How `process` assembles the pieces, and why the order matters."
weight = 60
+++

Each record reaches the first outcome that applies:

| # | Test | Outcome |
|---|---|---|
| 1 | no natural key | rejected, `NotKeyable` |
| 2 | key already billed, held or suppressed inside the window | duplicate |
| 3 | no charged party | rejected, `Unattributable` |
| 4 | opens after `as_of` | rejected, `FutureTimestamp` |
| 5 | past the inclusion deadline | rejected, `BeyondDeadline` |
| 6 | a blocking rule fired | rejected, `Malformed` |
| 7 | its partial run can never be reassembled (V19) | rejected, `Malformed` |
| 8 | its partial run is not finished, first record included (V20) | held, `AwaitingPartial` |
| 9 | the suppression policy says so | suppressed |
| 10 | its source has an unfilled sequence hole | held, `SequenceGap` |
| 11 | otherwise | billable |

- Keyability first: an unkeyed record could be billed twice.
- Deduplication before judgement: a malformed retransmission is one fault.
- Steps 4 and 5 use the `LatenessPolicy` in `ProcessConfig::validation`.
- Completeness last, per source: only the source with the hole is held, and
  only records that would otherwise be billed.

```rust
use mediation::{Batch, DedupWindow, Disposition, ProcessConfig, ProcessState, RejectReason, process};
use mediation::{ChargedParty, RecordType, SourceRef, SubscriberRef, UsageRecord};
use time::{Duration, macros::datetime};

// No correlator, so no natural key.
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

## Suppression

`process_with` takes your `Suppression` policy; `process` suppresses nothing.

## Held records come back

`Batch::with_carry` marks the leading records as carried in. They are counted
through `observe_carried` and refresh their dedup key instead of being checked
against it. Chain reconciliations through a `PeriodLedger`.

## The state is yours

`ProcessState` holds the dedup store and sequence trackers. `process` reads no
clock, so the same inputs give the same answer. Persist the state across
restarts (`DedupStore::iter` / `restore`, `SequenceTracker::snapshot` /
`restore`, or `serde`):

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

`process` does not correlate (that changes record cardinality; see
[correlation](@/docs/reassembly.md)), rate or round.
