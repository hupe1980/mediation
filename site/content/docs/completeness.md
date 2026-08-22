+++
title = "Completeness and sequence gaps"
description = "A lost record file leaves no trace in the surviving data. A hole in a per-source sequence number is the only evidence, and the wrap is the trap."
weight = 80
+++

A lost record file leaves **no trace in the surviving data**. Nothing that
arrives says the missing file existed, no total is visibly short, and no
subscriber complains — the usage simply was never billed.

The only evidence is a hole in a per-source sequence number, and the trap is the
wrap. A GSMA TAP file sequence runs `99999` then `00001`, at which point three
naive checks break, once per cycle: `next == previous + 1` reports a 99 998-file
gap, `next > previous` reports the entire next cycle as out of order, and
`next - previous` underflows.

`SequenceSpace` therefore carries the modulus and comparison follows the
serial-number arithmetic of **RFC 1982**:

```rust
use mediation::{SequenceObservation, SequenceSpace, SequenceTracker, SequenceValue};

let last = SequenceValue::new(SequenceSpace::TAP, 99_999).unwrap();
let next = SequenceValue::new(SequenceSpace::TAP, 1).unwrap();
assert!(last.is_successor(next));
assert_eq!(last.gap_to(next), Some(0));

// Gaps are identities, not a count: the operational response is a resend
// request for exactly these files.
let mut tracker = SequenceTracker::new(SequenceSpace::TAP);
tracker.observe_default(99_998);
assert_eq!(
    tracker.observe_default(2),
    SequenceObservation::Gap { missing: vec![99_999, 1], missing_count: 2 },
);
assert!(!tracker.is_complete());
```

One tracker per **scope**, and the scope is part of the standard: a TAP sequence
is per sender/recipient TADIG pair, a CDR file counter is per CGF node, a
`localSequenceNumber` is per recording entity. Sharing a tracker across scopes
manufactures gaps that do not exist.

## The list is bounded; the count never is

One corrupt sequence number can claim a jump of two billion files. Naming them
all is a two-billion-element allocation from a single bad field, so the list a
gap reports is capped — but the tracker's own arithmetic is not. Holes are held
as **runs**, so a hole of any size costs three numbers, the count stays exact,
and `is_complete()` cannot say "complete" about a source that is missing almost
everything:

```rust
use mediation::{SequenceSpace, SequenceTracker};
let mut tracker = SequenceTracker::new(SequenceSpace::U32);
tracker.observe_default(0);
tracker.observe_default(2_000_000_000);

assert_eq!(tracker.missing_upto(3), vec![1, 2, 3]);  // the list is capped
assert_eq!(tracker.missing_count(), 1_999_999_999);  // the count is not
assert_eq!(tracker.hole_count(), 1);                 // and it costs one run
assert!(!tracker.is_complete());
```

Truncating the count instead would be the one failure this module exists to
prevent, dressed as a memory optimisation: a source would read complete after a
hundred resends while the rest of the cycle was still missing, and billing over
that invoices an amount nobody can prove is right.

## A hole blocks its own source, not the whole batch

Completeness is a property of a **source**. When the Frankfurt gateway loses a
file, Frankfurt's records are held — `Disposition::Held(HoldReason::SequenceGap)`
— and Munich's are released, because Munich's completeness is provable and
Frankfurt's is not. Blocking the batch instead stops an operator's entire
billing run on one node's lost file.

Because nothing from an incomplete source is ever released, what a batch *does*
release is complete by construction. A hole a partner will never fill is retired
explicitly, with `SequenceTracker::abandon_older_than`, so the decision to give
up on a file is one somebody made rather than one the tracker made quietly.
