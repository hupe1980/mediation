+++
title = "Getting started"
description = "Install the crate, run a batch through the one-pass driver, and see where every record went."
weight = 10
+++

The crate performs **no I/O** (you pass records you have already read; there is
no collector, connection or clock) and computes **usage, not money** (no price,
tariff or currency).

## Installation

```bash
cargo add mediation
cargo add mediation --features serde   # Serialize/Deserialize in canonical string form
```

MSRV is Rust 1.94 (edition 2024). Pre-1.0: breaking changes ship as minor
releases.

## Quick start

One record, built field by field; nothing is inferred that the record did not say:

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

A whole batch in one pass. Deduplication and gap detection need state, so
`ProcessState` is the caller's: `process` is a pure function of
`(batch, config, as_of, state)`, and a replay gives the same verdicts.

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

[One pass, and the order it runs in](@/docs/pipeline.md) explains each step;
every step is also usable on its own. The module map is in the
[API reference](https://docs.rs/mediation).
