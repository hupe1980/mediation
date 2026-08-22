+++
title = "The conservation law"
description = "Every record reaches exactly one outcome, the law telescopes across a billing period, and corrections are signed events rather than edits."
weight = 70
+++

## Every record reaches exactly one outcome

The crate's central invariant, and the difference between a mediation layer and
a data pipeline: a pipeline moves records, while a mediation layer can **prove**
it did not lose any.

```text
carried_in + ingested == billable + suppressed + duplicate + rejected + held
```

`BatchReconciliation::balances()` is that equation. A batch that fails it has
unknown completeness, and billing over unknown completeness invoices an amount
nobody can prove is right.

Four outcomes are final; `Held` is the one that may return — a partial record
whose successor has not arrived, a leg whose counterpart is still in flight, an
identity awaiting reference data.

### The law spans batches, not just one

A held record comes back. If the batch that re-presents it counts it as newly
ingested, the two batches together report it twice; if neither counts it, it
vanishes. **Both are the law failing at a seam, which is exactly where nobody
looks** — each batch balances perfectly on its own.

So a re-presented record is counted through `observe_carried` rather than
`observe`, and `PeriodLedger` checks that each batch carries in exactly what its
predecessor left held:

```rust
use mediation::{BatchReconciliation, ChainBreak, Disposition, HoldReason, PeriodLedger};

let mut monday = BatchReconciliation::EMPTY;
monday.observe(&Disposition::Held(HoldReason::AwaitingPartial));

// Tuesday re-presents it — but counts it as a new arrival.
let mut tuesday = BatchReconciliation::EMPTY;
tuesday.observe(&Disposition::Billable);
assert!(tuesday.balances(), "the batch is internally consistent");

let mut period = PeriodLedger::new();
period.append(monday).unwrap();
assert_eq!(
    period.append(tuesday),
    Err(ChainBreak::CarryMismatch { batch: 1, expected: 1, found: 0 }),
);
assert!(!period.is_closable());
```

Chained that way the law telescopes: across a whole period, everything ingested
reached a final outcome or is still explicitly outstanding and named as such. A
past break is never forgiven — `is_closable()` stays false even if the caller
continued.

**And the period has its own equation, which is not the batch one.** Summing the
batches counts a held record again in every batch that re-presented it, so
`PeriodLedger::period_totals()` states the law that survives the chaining:

```text
ingested == billable + suppressed + duplicate + rejected + outstanding
```

where `outstanding` is what the **last** batch left held rather than the sum of
every batch's holds:

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

// The batch sum counts the hold once per batch it waited in …
assert_eq!(period.totals().held(), 1);
assert_eq!(period.totals().carried_in(), 1);

// … and the period law counts each record exactly once.
let law = period.period_totals();
assert_eq!(law.ingested, 2);
assert_eq!(law.billable, 2);
assert_eq!(law.outstanding, 0);
assert!(law.balances());
```

Both laws are checked as `proptest`s over arbitrary batches and arbitrary
periods — see [`tests/conservation.rs`](https://github.com/hupe1980/mediation/blob/main/tests/conservation.rs) and
[`tests/period_and_correlation.rs`](https://github.com/hupe1980/mediation/blob/main/tests/period_and_correlation.rs).


## Corrections are new records, never edits

The past changes constantly: a late partial completes a run already released, a
partner returns records they had already booked, a reclassification arrives
after month-end. Going back and fixing the record **cannot balance** — once a
batch's counts have been reported, editing one of its records invalidates the
report with no way left to reconcile what was released against what is stored.
It also destroys the evidence, and the question at audit is *what did you bill,
and on what basis*.

So `UsageRecord` is immutable, `Disposition` is terminal, and a correction is an
explicit event carrying a **signed delta** — what changes, not what the value
becomes:

```rust
use mediation::{RestateReason, Restatement, RestatementLog, UsageDelta};
use mediation::RecordKey;
let key = RecordKey::RecordingEntity {
    entity: "msc-01".parse().unwrap(), local_sequence: 1 };
let correction = Restatement::new(
    key,
    RestateReason::LatePartial,
    UsageDelta::default().with_duration(300),
    "2026-08-02T09:00:00+02:00".parse().unwrap(),
);

// Withdrawing one is reversing it — and both entries survive.
let log: RestatementLog =
    vec![correction.clone(), correction.checked_reverse().unwrap()]
        .into_iter()
        .collect();
assert_eq!(log.net(), Some(UsageDelta::ZERO));
assert_eq!(log.len(), 2);
```

`UsageDelta` holds `i128` for octets. A `VolumePair` is unsigned because a
volume cannot be negative — but a *correction* to one can be, and as large as
the original. Narrowing here would silently cap the one number whose whole
purpose is to say how wrong the last one was.

`RestateReason` is a closed set rather than free text, because GSMA TD.57
permits a roaming event to be resubmitted in exactly one case: where it was
returned through the RAP process. A correction nobody can name the basis for is
the one nobody should accept.

`Restatement` refers to its target by `RecordKey` — stable across
retransmission, which is exactly the property a correction needs to find its
record months later.
