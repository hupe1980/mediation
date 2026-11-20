+++
title = "Partial records and correlation"
description = "Reassembling partial runs and joining multi-node records without inventing or double-billing usage."
weight = 100
+++

## Partial records

The crate reassembles the partials that arrived and never synthesises a
quantity. A reduced partial carries only what changed, so a run's first partial
must be fully qualified (TS 32.240).

| Assessment | Meaning | In `process` |
|---|---|---|
| `Complete` | first to terminal partial, no holes | billable |
| `Incomplete` | holes (the first record included) or no terminal record yet | held, `AwaitingPartial`; V20 if there are holes |
| `Orphan` | first partial is reduced, or a partial is numbered 0 | rejected, V19 |

```rust
use mediation::{PartialFormat, PartialInfo, PartialRun, Reassembly, RecordClosureCause};

// A reduced first record can never be completed.
let orphan = PartialRun::from_parts(vec![
    PartialInfo::new(1, PartialFormat::Reduced, RecordClosureCause::VolumeLimit),
]);
assert!(matches!(orphan.assess(), Reassembly::Orphan { .. }));
assert!(!orphan.assess().may_still_complete());

// A missing first record may still arrive.
let waiting = PartialRun::from_parts(vec![
    PartialInfo::new(2, PartialFormat::Reduced, RecordClosureCause::VolumeLimit),
]);
assert_eq!(waiting.assess().missing(), &[1]);
assert!(waiting.assess().may_still_complete());
```

Every member of a run takes the run's verdict. `PartialRun::is_explained_loss`
is true when a run simply stops after an abnormal release or management
intervention; a missing middle is never explained.

## Correlation

Billing both nodes' records double-charges; dropping an unmatched one hides
loss. `correlate` returns groups and unmatched records, and `accounts_for`
asserts they cover every input once.

```rust
use mediation::{CorrelationConfig, correlate};
use mediation::{ChargedParty, ChargingId, CorrelationKeys, Octets, RecordType, SourceRef,
                SubscriberRef, UsageRecord, VolumePair};
use std::net::Ipv4Addr;

fn record(node: &str) -> UsageRecord {
    let mut r = UsageRecord::new(
        RecordType::PduSession,
        ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
        "2026-07-28T10:00:00+02:00".parse().unwrap(),
        SourceRef::new(node.parse().unwrap(), 0),
    ).with_correlation(CorrelationKeys::default()
        .with_charging_id(ChargingId::new(42))
        .with_gateway(Ipv4Addr::new(10, 0, 0, 1).into()));
    r.volume = Some(VolumePair::new(Octets::new(100), Octets::new(900)));
    r
}

let records = vec![record("sgw-01"), record("pgw-01")];
let result = correlate(&records, &CorrelationConfig::packet_bearer());

assert_eq!(result.groups().len(), 1);
assert!(result.groups()[0].would_double_count(), "bill one side, not both");
assert!(result.accounts_for(records.len()));
```

A diverted IMS call links each leg's ICID to the previous one (TS 32.260). A leg
whose predecessor never arrived is reported in `broken_legs()`, not as
unmatched, so `accounts_for` still counts it once.
