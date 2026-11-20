+++
title = "Fraud signals"
description = "Evidence, never verdicts: the measurements that fired and the records behind them."
weight = 150
+++

Detectors return an `Indication`: the signal, the records behind it and what
was measured. There is no score (weights depend on the operator's traffic), no
verdict (suspension has legal weight) and no shipped list (risky ranges are
commercial data).

| Signal | Shape |
|---|---|
| `ImpossibleTravel` | two countries closer in time than any journey allows |
| `PremiumDestination` | traffic to a caller-flagged range (IRSF) |
| `WangiriPattern` | many short inbound calls to one party within a window |
| `BypassSignature` | mostly outbound calls on one network (naive SIM box) |
| `AitSignature` | originated SMS concentrated on few destination ranges |

Only `ImpossibleTravel` is `is_arithmetic()`: impossible rather than unusual.

```rust
use mediation::{ChargedParty, RecordType, SourceRef, SubscriberRef, UsageRecord};
use mediation::{Signal, UniformTransit, impossible_travel};
use time::Duration;

fn at(opened: &str, serving: &str) -> UsageRecord {
    UsageRecord::new(
        RecordType::OriginatedCall,
        ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
        opened.parse().unwrap(),
        SourceRef::new("msc-01".parse().unwrap(), 0),
    )
    .with_networks(serving.parse().unwrap(), "262-02".parse().unwrap())
}

// Germany, then the United States, twenty minutes apart.
let records = vec![at("2026-07-28T10:00:00+02:00", "262-02"), at("2026-07-28T10:20:00+02:00", "310-260")];
let found = impossible_travel(&records, &UniformTransit::new(Duration::hours(4)));
assert_eq!(found.len(), 1);
assert_eq!(found[0].signal, Signal::ImpossibleTravel);

// 310 and 311 are one country (Mcc::same_country): no travel at all.
let home = vec![at("2026-07-28T10:00:00+02:00", "310-260"), at("2026-07-28T10:20:00+02:00", "311-480")];
assert!(impossible_travel(&home, &UniformTransit::new(Duration::hours(4))).is_empty());
```

`impossible_travel` compares countries per charged party, using
`Mcc::same_country` because a country can hold several MCCs. Minimum journey
times come from a caller's `TransitTimes` per country pair; an unmodelled pair
never fires. Travel time runs from the first record's close, and overlapping
sessions in two countries are reported as such (a cloned or shared SIM).

## Limits

Modern SIM boxes simulate human behaviour and evade CDR-based detection.
`bypass_signature` catches only the unsophisticated case and is a triage
signal, not a control.
