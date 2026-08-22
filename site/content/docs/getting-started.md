+++
title = "Getting started"
description = "Install the crate, run a batch through the one-pass driver, and see where every record went."
weight = 10
+++

Two things are worth knowing before the first line of code. The crate performs
**no I/O**: you hand it records you have already read, and it hands back
verdicts — there is no collector, no connection and no clock. And it computes
**usage, not money**: nothing here has a price, a tariff or a currency.

Everything else follows from those two.

## Installation

```bash
cargo add mediation
```

With `serde` support for all public types, in their canonical string forms:

```bash
cargo add mediation --features serde
```

**MSRV:** Rust `1.94` (edition 2024), pinned in
[`rust-toolchain.toml`](https://github.com/hupe1980/mediation/blob/main/rust-toolchain.toml) and verified by a dedicated CI lane.

**API stability:** pre-`1.0`, so breaking changes ship as minor releases.


## Quick start

One record, built field by field — nothing is inferred that the record did not
say:

```rust
use mediation::{
    ChargedParty, ChargingId, CorrelationKeys, Octets, RecordType, Seconds, ServiceDomain,
    SourceRef, SubscriberRef, UsageRecord, VolumePair,
};
use std::net::Ipv4Addr;

let record = UsageRecord::new(
    RecordType::PduSession,
    ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse()?)),
    "2026-07-28T10:00:00+02:00".parse()?,
    SourceRef::new("pgw-fra-01".parse()?, 0).with_file_sequence(42),
)
.with_closed("2026-07-28T10:30:00+02:00".parse()?)
.with_duration(Seconds::new(1_800))
.with_volume(VolumePair::new(Octets::new(4_096), Octets::new(65_536)))
.with_networks("310-260".parse()?, "262-02".parse()?)
.with_correlation(
    CorrelationKeys::default()
        .with_charging_id(ChargingId::new(1_234))
        .with_gateway(Ipv4Addr::new(10, 0, 0, 1).into()),
);

assert_eq!(record.service(), ServiceDomain::Data);   // derived, never stored twice
assert_eq!(record.is_roaming(), Some(true));
assert_eq!(record.local_date().to_string(), "2026-07-28");
Ok::<(), mediation::ParseError>(())
```

Then a whole batch, in one pass. Deduplication and gap detection are inherently
stateful, so the state is **yours** — which keeps [`process`] a pure mapping
from `(batch, config, as_of, state)` to `(outcome, state′)`, and a replay
reproducible:

```rust
use mediation::{Batch, DedupWindow, ProcessConfig, ProcessState, process};
use time::{Duration, macros::datetime};
use mediation::{ChargedParty, ChargingId, CorrelationKeys, Octets, RecordType,
                SourceRef, SubscriberRef, UsageRecord, VolumePair};
use std::net::Ipv4Addr;
fn session(id: u32) -> UsageRecord {
    UsageRecord::new(
        RecordType::PduSession,
        ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
        "2026-07-28T10:00:00+02:00".parse().unwrap(),
        SourceRef::new("pgw-fra-01".parse().unwrap(), 0).with_file_sequence(42),
    )
    .with_closed("2026-07-28T10:30:00+02:00".parse().unwrap())
    .with_volume(VolumePair::new(Octets::new(4_096), Octets::new(65_536)))
    .with_correlation(CorrelationKeys::default()
        .with_charging_id(ChargingId::new(id))
        .with_gateway(Ipv4Addr::new(10, 0, 0, 1).into()))
}
// Four distinct sessions, and one of them sent twice.
let mut records: Vec<UsageRecord> = (1..=4).map(session).collect();
records.push(session(1));

let mut state = ProcessState::new(DedupWindow::from_bound(Duration::days(7)).unwrap());

let outcome = process(
    &Batch::new(&records),
    &ProcessConfig::default(),
    datetime!(2026-07-28 12:00:00 UTC),   // "now" is a parameter, never sampled
    &mut state,
);

assert_eq!(outcome.reconciliation().released(), 4);    // billed once, never twice
assert_eq!(outcome.reconciliation().duplicate(), 1);
assert!(outcome.reconciliation().balances());          // nothing created, nothing lost
assert!(outcome.is_releasable());
assert_eq!(outcome.totals()?.total().records(), 4);
Ok::<(), mediation::AggregationError>(())
```

Every step of that pass is also usable on its own — `process` exists because the
*order* of the steps is a decision, and one that is easy to get wrong. It is
[documented in full](@/docs/pipeline.md).


## Feature flags

| Flag | Default | Effect |
|---|---|---|
| `serde` | ❌ | Derive `Serialize`/`Deserialize` on all public types |


## Modules at a glance

| Module | Key types | Purpose |
|---|---|---|
| `error` | `ParseError`, `DecodeError` | One error type per input kind — strings and bytes |
| `digits` | `DigitString` | Inline digits, plus the TBCD and plain-BCD codecs |
| `ber` | `BerReader`, `BerElement`, `BerTag`, `TagClass` | The X.690 framing a charging record arrives in |
| `cdrtime` | `CdrTimestamp`, `QuarterHourOffset`, `DstAdjustment`, `MsTimeZone`, `Seconds` | Local-time-plus-offset arithmetic, DST-boundary detection |
| `volume` | `Octets`, `VolumePair`, `ByteScale` | Integer octets, explicit conversion, explicit rounding primitives |
| `plmn` | `Mcc`, `Mnc`, `PlmnId`, `TadigCode`, `MncWidths` | Network identity; the 2-vs-3-digit MNC problem, handled explicitly |
| `identity` | `Imsi`, `Msisdn`, `Imei`, `Imeisv`, `Iccid`, `Supi`, `Gpsi`, `Pei`, `Suci`, `AddressString` | Validated identifiers with check digits, stored inline |
| `location` | `UserLocation`, `Uli`, `Cgi`, `Sai`, `Rai`, `Lai`, `Ecgi`, `Ncgi`, `Tai` | Where a record was produced, at every access generation, on the wire and as a string |
| `jurisdiction` | `usage_country()`, `customer_country_presumption()`, `is_abroad()`, `Ambiguity` | Which country's rules apply, and which question was asked |
| `numbering` | `NumberRange`, `NumberRangeSet`, `Porting`, `terminating_plmn` | Longest-prefix ranges, and who really terminates a number |
| `party` | `ChargedParty`, `SubscriberRef`, `TenantRef`, `UnattributableReason` | Who the usage is charged to |
| `correlation` | `CorrelationKeys`, `ChargingId`, `Icid`, `correlate()`, `CorrelatedGroup`, `LegChain`, `Unmatched` | The identifiers that tie a record to its session, and the join that uses them |
| `record` | `UsageRecord`, `RecordType`, `ServiceDomain`, `SourceRef` | The canonical usage type and its provenance |
| `lateness` | `LatenessPolicy`, `Lateness` | One concept behind every window |
| `sequence` | `SequenceSpace`, `SequenceValue`, `SequenceTracker` | Wrap-aware gap detection, RFC 1982 |
| `dedup` | `RecordKey`, `DedupStore`, `DedupWindow`, `NotKeyable` | Natural-key deduplication |
| `disposition` | `Disposition`, `Outcome`, `BatchReconciliation`, `PeriodLedger` | The conservation law |
| `pipeline` | `process()`, `Batch`, `ProcessState`, `ProcessOutcome` | The whole layer in one pass, in a documented order |
| `container` | `ServiceDataContainer`, `ContainerSet`, `RatingGroup`, `ChangeCondition` | Per-rating-group usage inside one record |
| `partial` | `PartialInfo`, `PartialRun`, `Reassembly`, `RecordClosureCause` | Partial-record reassembly |
| `validation` | `validate_records()`, `Rule` (V01–V21), `Finding`, `Severity` | The numbered rule catalogue |
| `session` | `Dnn`, `Snssai`, `SliceServiceType`, `RatType`, `ChargingCharacteristics` | The network, slice, access, location and charging profile a session ran on |
| `timeband` | `split_session()`, `split_measured()`, `apportion()`, `Apportionment` | Dividing usage at a tariff boundary, conservatively |
| `restatement` | `Restatement`, `UsageDelta`, `RestatementLog` | Corrections as signed, append-only events |
| `aggregation` | `aggregate_released()`, `UsageTotal`, `ReleasedTotals` | What the batch adds up to, with its evidence |
| `roaming` | `TapBatch`, `BatchTotals`, `ScaledAmount`, `RapReturn`, `BatchDiscrepancy` | A partner's batch, and whether it adds up |
| `quality` | `score_batch()`, `QualityGrade`, `ReleaseBlocker` | The release gate |
| `fairuse` | `assess()`, `FairUseAssessment`, `VolumeAllowance`, `Region` | EU 2016/2286 permanent-roaming assessment |
| `fraud` | `Signal`, `Indication`, `TrafficShape`, `TransitTimes` | Fraud evidence — never a score, never a verdict |
| `retention` | `RetentionPolicy`, `RetentionSchedule`, `RetentionBasis`, `Deletion` | How long a record may be kept, and from when — and when no date can be given |
| `privacy` | `Pseudonymiser`, `Pseudonym`, `Reduction`, `NumberPrefix` | Narrowing an identity to the question |
