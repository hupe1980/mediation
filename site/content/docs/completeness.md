+++
title = "Completeness and sequence gaps"
description = "A hole in a per-source sequence number is the only evidence of a lost file."
weight = 80
+++

A lost file leaves no trace except a hole in a per-source sequence number.
Sequences wrap (TAP runs `99999` then `00001`), so `SequenceSpace` compares with
RFC 1982 serial-number arithmetic.

```rust
use mediation::{SequenceObservation, SequenceSpace, SequenceTracker, SequenceValue};

let last = SequenceValue::new(SequenceSpace::TAP, 99_999).unwrap();
let next = SequenceValue::new(SequenceSpace::TAP, 1).unwrap();
assert!(last.is_successor(next));

// Gaps are the files to request again, not a count.
let mut tracker = SequenceTracker::new(SequenceSpace::TAP);
tracker.observe_default(99_998);
assert_eq!(
    tracker.observe_default(2),
    SequenceObservation::Gap { missing: vec![99_999, 1], missing_count: 2 },
);
assert!(!tracker.is_complete());
```

Use one tracker per scope (TADIG pair, CGF node, recording entity); a shared
tracker reports false gaps.

## The list is bounded; the count is not

A corrupt field can claim a two-billion-file jump. The list is capped; the
count stays exact, so `is_complete()` never misreports.

```rust
use mediation::{SequenceSpace, SequenceTracker};

let mut tracker = SequenceTracker::new(SequenceSpace::U32);
tracker.observe_default(0);
tracker.observe_default(2_000_000_000);

assert_eq!(tracker.missing_upto(3), vec![1, 2, 3]);
assert_eq!(tracker.missing_count(), 1_999_999_999);
assert_eq!(tracker.hole_count(), 1);
```

## In the pipeline

| Situation | Result |
|---|---|
| files listed out of order | observed in serial order; same holes either way |
| an older file fills no hole and brings new records | V18 "sequence regressed" |
| a source has a hole | its billable records `Held(SequenceGap)`; other sources release |

Retire a hole that will never be filled with
`SequenceTracker::abandon_older_than`.
