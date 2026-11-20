+++
title = "Octets, seconds and events"
description = "Every chargeable quantity is an integer, uplink and downlink stay apart, and rounding is named but never applied."
weight = 30
+++

## Octets, not megabytes, and never a float

A float loses integers beyond 2^53 octets (about 9 PB). `Octets` and `Seconds`
are integers and every sum is checked.

Byte scales follow IEC 80000-13: `kB` is 1 000, `KiB` is 1 024. `KB` (1 024 under
JEDEC) and the bit spellings (`kb`) are refused.

```rust
use mediation::{ByteScale, Octets, VolumePair};

assert_eq!(Octets::from_scaled(5, ByteScale::Mega).unwrap().get(), 5_000_000);
assert_eq!(Octets::from_scaled(5, ByteScale::Mebi).unwrap().get(), 5_242_880);
assert!("KB".parse::<ByteScale>().is_err());

// A wrapped total is a plausible wrong number.
assert_eq!(Octets::new(u64::MAX).checked_add(Octets::new(1)), None);

// Uplink and downlink stay apart; the total is derived.
let v = VolumePair::new(Octets::new(100), Octets::new(900));
assert_eq!(v.checked_total(), Some(Octets::new(1_000)));
```

`VolumePair` has no total-only constructor: a sum cannot be split again.
Rounding (`Octets::units_ceil`, `Seconds::units_ceil`) is offered but never
applied.


## Containers: the list is the data

A packet-data record carries a list of containers (TS 32.298
`ListOfServiceData`), each scoped to a rating group and the condition that
closed it. Flattening them at ingest loses what makes the session ratable.

```rust
use mediation::{ChangeCondition, ContainerSet, Octets, RatingGroup, ServiceDataContainer,
                VolumePair};

let containers = ContainerSet::from(vec![
    ServiceDataContainer::new(RatingGroup::new(10), ChangeCondition::TariffTime)
        .with_volume(VolumePair::new(Octets::new(1_000), Octets::new(4_000))),
    ServiceDataContainer::new(RatingGroup::new(10), ChangeCondition::RecordClosure)
        .with_volume(VolumePair::new(Octets::new(500), Octets::new(2_000))),
]);

// One rating group split by a priced boundary: do not merge before rating.
assert_eq!(containers.rating_groups().len(), 1);
assert!(containers.crosses_priced_boundary());

// Record total and container sum disagree: reported (V07), not resolved.
let declared = VolumePair::new(Octets::new(1_550), Octets::new(6_000));
assert_eq!(containers.agrees_with(Some(declared)), Some(false));
assert_eq!(containers.discrepancy(Some(declared)), Some((50, 0)));
```

Either number may be wrong, so the crate reports the signed discrepancy and
leaves the choice to the operator.


## What the batch adds up to

`aggregate_released` returns the total together with the reconciliation that
produced it, so a total can be checked against what was ingested.

```rust
use mediation::{Disposition, Octets, RejectReason, aggregate_released};
use mediation::{ChargedParty, RecordType, SourceRef, SubscriberRef, UsageRecord, VolumePair};
fn record(up: u64) -> UsageRecord {
    UsageRecord::new(
        RecordType::PduSession,
        ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
        "2026-07-28T10:00:00+02:00".parse().unwrap(),
        SourceRef::new("pgw-01".parse().unwrap(), 0),
    ).with_volume(VolumePair::new(Octets::new(up), Octets::ZERO))
}
let records = vec![record(100), record(200), record(400)];
let dispositions = vec![
    Disposition::Billable,
    Disposition::Billable,
    Disposition::Rejected(RejectReason::Malformed),
];

let released = aggregate_released(&records, &dispositions).unwrap();

assert_eq!(released.total().volume().uplink(), Octets::new(300));
assert_eq!(released.reconciliation().ingested(), 3);
assert!(released.is_consistent());
```

- `UsageTotal::unquantified()` counts records that contributed nothing, which
  is how a dropped field shows up.
- A container disagreement or an overflow stops aggregation and names the record.
- `aggregate_by` groups by a key closure; `aggregate_rating_groups` reads
  containers only and puts container-less records under `None`.
