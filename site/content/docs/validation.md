+++
title = "The rule catalogue and the release gate"
description = "Twenty-one numbered rules, and the one call a release path makes."
weight = 110
+++

## The rule catalogue (V01–V21)

Codes are stable and never reused.

| Rule | Detects | Blocks |
|---|---|---|
| V01 `UnassignedOffset` | UTC offset outside UTC−12:00 … UTC+14:00 | ✅ |
| V02 `FutureTimestamp` | record opens after the evaluation instant | ✅ |
| V03 `ClosedBeforeOpened` | record closes before it opens | ✅ |
| V04 `DurationExceedsSpan` | duration longer than the wall-clock span | ✅ |
| V05 `ImplausibleVolume` | volume the elapsed time could not have carried | — |
| V06 `ZeroUsage` | no quantity at all | — |
| V07 `ContainerSumMismatch` | container sum disagrees with the record total | ✅ |
| V08 `UnattributableParty` | no charged party could be determined | ✅ |
| V09 `NotKeyable` | no natural key | ✅ |
| V10 `BeyondDeadline` | past the inclusion deadline | ✅ |
| V11 `LateArrival` | late, still billable | — |
| V12 `ServingNetworkAbsent` | no serving network (not for wireline, IMS, fixed access) | — |
| V13 `QuantityDoesNotMatchService` | quantity does not match the service | — |
| V14 `CheckDigitFailure` | identifier check digit does not verify | — |
| V15 `VolumeOverflow` | the record's directions or containers overflow when summed | ✅ |
| V16 `UnorderedSeries` | input not in ascending instant order | — |
| V17 `DuplicateRecord` | retransmission of a record already seen | — |
| V18 `SequenceGap` | source sequence has an unfilled hole, or regressed | —¹ |
| V19 `OrphanPartial` | run of partials can never be reassembled | ✅ |
| V20 `IncompleteReassembly` | run of partials is missing members, the first included | ✅ |
| V21 `JurisdictionConflict` | serving network and location disagree on country | ✅ |

¹ The source's records are held instead; the finding lists the files to
request. Escalate it to block the whole batch.

- `validate_records` is pure; V17–V20 come from the stateful `dedup`,
  `sequence` and `partial` stages.
- V16 is reported once per slice; in `process` it checks only ingested records.
- `ValidationConfig::escalating` raises a rule to blocking everywhere. Nothing
  lowers a severity.

## The release gate

`score_batch` answers "may this batch be invoiced?" with every blocker.

| Blocker | Meaning |
|---|---|
| `Unbalanced` | the conservation law does not hold |
| `IncompleteSources` | a released source has a sequence hole |
| `BlockingFindings` | a blocking finding |

```rust
use mediation::{BatchReconciliation, Disposition, QualityGrade, ValidationReport, score_batch};

let counts: BatchReconciliation = vec![Disposition::Billable; 1_000_000].into_iter().collect();

// Clean records cannot outvote unknown completeness.
let report = score_batch(&counts, &ValidationReport::new(), false);
assert_eq!(report.grade, QualityGrade::F);
assert!(report.blocks_release());
```

The third argument says whether every source the batch releases from is
complete. `score_dispositions`, used by `process`, also ignores blocking
findings on records already rejected or held. Grade thresholds are fixed
proportions.
