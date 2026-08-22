+++
title = "Fraud signals"
description = "Evidence, never verdicts: no score, no decision, no built-in lists — the measurements that fired and the records behind them."
weight = 150
+++

Mediation is where fraud is visible — NRTRDE exists because a roaming extract
reaching the home network within four hours is the difference between catching a
fraud and paying for it.

What the detectors return is an `Indication`: the signal, the records behind it,
and what was measured. **No score**, because the weight of an indicator depends
on an operator's traffic mix, partners and tariffs. **No verdict**, because
suspending a subscriber has legal weight. **No shipped lists**, because
high-risk destination ranges are commercial data that changes weekly.

```rust
use mediation::{Signal, UniformTransit, impossible_travel};
use mediation::{ChargedParty, RecordType, SourceRef, SubscriberRef, UsageRecord};
use time::Duration;
fn at(opened: &str, serving: &str) -> UsageRecord {
    UsageRecord::new(
        RecordType::MobileOriginatedCall,
        ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
        opened.parse().unwrap(),
        SourceRef::new("msc-01".parse().unwrap(), 0),
    ).with_networks(serving.parse().unwrap(), "262-02".parse().unwrap())
}

// Germany, then the United States, twenty minutes apart.
let records = vec![
    at("2026-07-28T10:00:00+02:00", "262-02"),
    at("2026-07-28T10:20:00+02:00", "310-260"),
];

let found = impossible_travel(&records, &UniformTransit::new(Duration::hours(4)));
assert_eq!(found.len(), 1);
assert_eq!(found[0].signal, Signal::ImpossibleTravel);
assert!(found[0].signal.is_arithmetic());
```

`impossible_travel` compares **countries, not cells** — a country change is a
lower bound on distance that needs no geography and no coordinate table — and it
compares **per charged party**, so a whole batch can be passed without one
subscriber's movement being read against another's.

The minimum comes from a caller-supplied `TransitTimes`, per country pair.
`UniformTransit` is the flat rule, and it is the right starting point rather
than the right answer: four hours is a sensible floor for an intercontinental
hop and absurd for Germany to Austria, where a train ride is ninety minutes. One
number for every pair either misses real fraud or accuses real travellers,
depending which way it is set. An unmodelled pair returns `None` and **never
fires** — a signal raised out of ignorance costs an analyst the same time as a
real one and teaches them to discount the next.

Travel time runs from the first record's **close**, not its open: the subscriber
was demonstrably in that country until the record closed, so that is the
earliest the journey could begin. Measuring from the open instant grants a
four-hour call four extra hours of travel — and hides the clearest evidence
there is, two sessions that *overlap* in two countries, which is a cloned or
shared SIM rather than a fast journey. Those say so in the detail line.

| Signal | Shape |
|---|---|
| `ImpossibleTravel` | Two countries closer together in time than any journey allows |
| `PremiumDestination` | Traffic to a caller-flagged range — the IRSF shape |
| `WangiriPattern` | Many short inbound calls, of the kind meant to provoke a callback |
| `BypassSignature` | Mostly outbound calls, one network — the naive SIM-box shape |
| `AitSignature` | Message traffic concentrated on few destination ranges |

Only `ImpossibleTravel` is `is_arithmetic()`: it is impossible rather than
unusual, and it is the only one a single record justifies acting on.

## What CDR-based detection cannot catch

Worth stating plainly, because the industry rarely does. Modern SIM boxes run
*human behaviour simulation*: they generate incoming calls, vary durations, and
move SIMs between gateways in different cells, so the network records
human-looking mobility at no cost to the fraudster. Published research finds
these evade CDR-based detection at a high rate.

`bypass_signature` therefore catches the **unsophisticated** case and is a
triage signal for an analyst's queue, not a control. An operator relying on it
alone will conclude they have no bypass problem. Anything stronger needs
signal-level data — latency at the cellular edge, radio measurements — that
never reaches a charging record.
