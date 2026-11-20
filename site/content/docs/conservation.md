+++
title = "The conservation law"
description = "Every record reaches exactly one outcome, across batches and periods; corrections are signed events."
weight = 70
+++

## Every record reaches exactly one outcome

```text
carried_in + ingested == billable + suppressed + duplicate + rejected + held
```

`BatchReconciliation::balances()` checks this. `Held` is the only non-final
outcome.

## The law spans batches

A held record re-presented as a new arrival is counted twice, though each
batch balances. Count it with `observe_carried`; `PeriodLedger` checks that each
batch carries in what its predecessor held, and a break keeps `is_closable()`
false.

```rust
use mediation::{BatchReconciliation, ChainBreak, Disposition, HoldReason, PeriodLedger};

let mut monday = BatchReconciliation::EMPTY;
monday.observe(&Disposition::Held(HoldReason::AwaitingPartial));

// Tuesday re-presents it, but counts it as a new arrival.
let mut tuesday = BatchReconciliation::EMPTY;
tuesday.observe(&Disposition::Billable);
assert!(tuesday.balances());

let mut period = PeriodLedger::new();
period.append(monday).unwrap();
assert_eq!(
    period.append(tuesday),
    Err(ChainBreak::CarryMismatch { batch: 1, expected: 1, found: 0 }),
);
assert!(!period.is_closable());
```

`PeriodLedger::period_totals()` counts each record once, with `outstanding`
being what the last batch left held:

```text
ingested == billable + suppressed + duplicate + rejected + outstanding
```

```rust
use mediation::{BatchReconciliation, Disposition, HoldReason, PeriodLedger};

let mut first = BatchReconciliation::EMPTY;
first.observe(&Disposition::Billable);
first.observe(&Disposition::Held(HoldReason::AwaitingPartial));

let mut second = BatchReconciliation::EMPTY;
second.observe_carried(&Disposition::Billable);

let mut period = PeriodLedger::new();
period.append(first).unwrap();
period.append(second).unwrap();

let law = period.period_totals();
assert_eq!((law.ingested, law.billable, law.outstanding), (2, 2, 0));
assert!(law.balances());
```

Property tests:
[`tests/conservation.rs`](https://github.com/hupe1980/mediation/blob/main/tests/conservation.rs),
[`tests/period_and_correlation.rs`](https://github.com/hupe1980/mediation/blob/main/tests/period_and_correlation.rs).

## Corrections are new records, never edits

Editing a released record invalidates reported counts and the audit trail. A
correction is a `Restatement` carrying a signed `UsageDelta`:

```rust
use mediation::{RecordKey, RestateReason, Restatement, RestatementLog, UsageDelta};

let key = RecordKey::RecordingEntity {
    entity: "msc-01".parse().unwrap(),
    local_sequence: 1,
};
let correction = Restatement::new(
    key,
    RestateReason::LatePartial,
    UsageDelta::default().with_duration(300),
    "2026-08-02T09:00:00+02:00".parse().unwrap(),
);

// Withdrawing a correction is reversing it; both entries survive.
let log: RestatementLog =
    vec![correction.clone(), correction.checked_reverse().unwrap()]
        .into_iter()
        .collect();
assert_eq!(log.net(), Some(UsageDelta::ZERO));
assert_eq!(log.len(), 2);
```

`UsageDelta` holds octets as `i128`, since a correction can be negative.
`RestateReason` is a closed set (GSMA TD.57 permits resubmission only via RAP).
