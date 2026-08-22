+++
title = "The rule catalogue and the release gate"
description = "Twenty-one numbered, cited rules a downstream system can reference in a ticket — and the one call a release path makes."
weight = 110
+++

## The rule catalogue (V01–V21)

A stable, numbered rule set a downstream system can cite in a ticket: "V07 on
the Frankfurt gateway" means the same thing to everyone who reads it, this year
and in five years. Numbers are never reused; a retired rule keeps its number
reserved.

| Rule | Detects | Blocks |
|---|---|---|
| V01 `UnassignedOffset` | UTC offset outside UTC−12:00 … UTC+14:00 | ✅ |
| V02 `FutureTimestamp` | record opens after the evaluation instant | ✅ |
| V03 `ClosedBeforeOpened` | record closes before it opens | ✅ |
| V04 `DurationExceedsSpan` | chargeable duration longer than the wall-clock span | ✅ |
| V05 `ImplausibleVolume` | volume the elapsed time could not have carried | — |
| V06 `ZeroUsage` | no quantity at all | — |
| V07 `ContainerSumMismatch` | container sum disagrees with the record total | ✅ |
| V08 `UnattributableParty` | no charged party could be determined | ✅ |
| V09 `NotKeyable` | no natural key, so the record cannot be deduplicated | ✅ |
| V10 `BeyondDeadline` | past the inclusion deadline | ✅ |
| V11 `LateArrival` | late, still billable | — |
| V12 `ServingNetworkAbsent` | no serving network on subscriber-bound usage | — |
| V13 `QuantityDoesNotMatchService` | quantity absent for what the service measures | — |
| V14 `CheckDigitFailure` | identifier check digit does not verify | — |
| V15 `VolumeOverflow` | the record's own directions do not sum | ✅ |
| V16 `UnorderedSeries` | input not in ascending instant order | — |
| V17 `DuplicateRecord` | retransmission of a record already seen | — |
| V18 `SequenceGap` | source sequence has an unfilled hole | —¹ |
| V19 `OrphanPartial` | run of partials cannot be reassembled at all | ✅ |
| V20 `IncompleteReassembly` | run of partials is missing members | ✅ |
| V21 `JurisdictionConflict` | serving network and reported location disagree on the country | ✅ |

¹ V18 is the one row that needs a footnote. Its enforcement is *harder* than a
block, not softer: every record from that source is **held**, so none of it can
be billed. The finding is the evidence — the files to ask the partner to resend
— and blocking the batch on it as well would stop records from every other
source, whose completeness is not in question. Escalate it if you would rather
the whole batch waited.

Three properties carried over deliberately:

- **`validate_records` is pure.** The three inherently stateful rules — V17, V18
  and V19/V20 — live in `dedup`, `sequence` and `partial`, and their outcomes
  join the same catalogue through `Finding::duplicate`, `Finding::sequence_gap`
  and `Finding::reassembly`. A report speaks one vocabulary rather than four.
- **Order independence.** V16 evaluates against a sorted view and is reported
  **once** for the slice, so a shuffled input yields one finding rather than a
  cascade that buries the rules that matter.
- **Findings point into your slice.** `Finding::record_index` refers to the
  input you passed, not to an internal ordering.

Severity can be tightened but never loosened: `ValidationConfig::escalating`
raises a warning to blocking, and there is no call that does the reverse. The
defaults encode "billing this invoices an amount that cannot be defended", which
is not a local preference. Escalation reaches the **whole** report, stateful
rules included — a caller who escalates V17 gets it whichever stage found the
duplicate, which a report where half the catalogue ignored the configuration
could not give them.


## The release gate

Everything in this crate exists to answer one question: **may this batch be
invoiced?** `score_batch` answers it from three inputs, and reports every reason
it said no rather than only the first — fixing one blocker does not imply the
batch is releasable.

| Blocker | Meaning |
|---|---|
| `Unbalanced` | the conservation law does not hold |
| `IncompleteSources` | a source has an unfilled sequence hole, so completeness is **unknown** |
| `BlockingFindings` | at least one rule fired at blocking severity |

```rust
use mediation::{BatchReconciliation, Disposition, QualityGrade, ValidationReport, score_batch};

let counts: BatchReconciliation = vec![Disposition::Billable; 1_000_000].into_iter().collect();

// A million clean records still cannot outvote an unknown hole in a source.
let report = score_batch(&counts, &ValidationReport::new(), false);
assert_eq!(report.grade, QualityGrade::F);
assert!(report.blocks_release());
```

Unknown completeness is a **hard gate**, not a score. Nothing in such a batch is
wrong; what is wrong is that the batch cannot say what it is missing, and
releasing it invoices an amount nobody can prove is right.

The question is *"is every source whose records this batch **releases**
complete?"* — releases, not touches. `process` holds the records of a source
with a hole and releases everybody else's, so what it releases is complete by
construction and it answers `true`. A caller assembling their own batch, and
releasing records from a source they cannot prove complete, answers `false` and
the gate stops them.

The grade is derived rather than configurable, and its thresholds are
proportions — a batch of a hundred and a batch of a hundred million grade the
same way for the same defect rate. A per-deployment threshold would make two
teams' "grade B" mean different things.
