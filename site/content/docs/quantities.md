+++
title = "Octets, seconds and events"
description = "Every chargeable quantity is an integer, uplink and downlink stay apart, and rounding is a rating decision the crate names but never applies."
weight = 30
+++

## Octets, not megabytes — and never a float

A data volume is inherently an integer count of octets, so a float is not merely
imprecise but categorically wrong: beyond 2^53 octets (≈ 9.01 PB) consecutive
integers are no longer representable, and a national monthly aggregate crosses
that. Counts would be dropped silently, in the one direction nobody audits.

| Kind | Type | Examples |
|---|---|---|
| Chargeable quantity | `u64` / `u32` | `Octets`, `Seconds`, event counts |
| Ratio / score / indicator | *(none yet)* | reserved for later modules |

A CI lane greps the source for float types, so the guarantee cannot regress
silently.

```rust
use mediation::{ByteScale, Octets, VolumePair};

// IEC 80000-13, without exception: kB is 1 000 octets and KiB is 1 024.
assert_eq!(Octets::from_scaled(5, ByteScale::Mega).unwrap().get(), 5_000_000);
assert_eq!(Octets::from_scaled(5, ByteScale::Mebi).unwrap().get(), 5_242_880);

// Every sum is checked — a wrapped total is a plausible wrong number.
assert_eq!(Octets::new(u64::MAX).checked_add(Octets::new(1)), None);

// Uplink and downlink stay apart; a total is derived, never stored.
let v = VolumePair::new(Octets::new(100), Octets::new(900));
assert_eq!(v.checked_total(), Some(Octets::new(1_000)));
```

`VolumePair` offers **no constructor taking only a total**. Asymmetric tariffs,
zero-rated uplink and interconnect settlement all need the two directions
separately, and once summed the split cannot be recovered.

Rounding up to whole chargeable units is priced behaviour, so it is offered as a
named primitive (`Octets::units_ceil`, `Seconds::units_ceil`) and applied by
nothing. What leaves the crate is the measured quantity.


## Containers: the list is the data

A packet-data record does not carry one volume. It carries a list of containers
(3GPP TS 32.298 `ListOfServiceData`), each scoped to a rating group, a quality
class, and the condition that closed it. Flattening them at ingest destroys the
only information that makes the session ratable, and — as with uplink and
downlink — the loss is not recoverable.

```rust
use mediation::{ChangeCondition, ContainerSet, Octets, RatingGroup, ServiceDataContainer,
                VolumePair};

let containers = ContainerSet::from(vec![
    ServiceDataContainer::new(RatingGroup::new(10), ChangeCondition::TariffTime)
        .with_volume(VolumePair::new(Octets::new(1_000), Octets::new(4_000))),
    ServiceDataContainer::new(RatingGroup::new(10), ChangeCondition::RecordClosure)
        .with_volume(VolumePair::new(Octets::new(500), Octets::new(2_000))),
]);

// One rating group, split by a *priced* boundary — do not merge before rating.
assert_eq!(containers.rating_groups().len(), 1);
assert!(containers.crosses_priced_boundary());

// The record's own total and the container sum are two statements of one fact.
let declared = VolumePair::new(Octets::new(1_550), Octets::new(6_000));
assert_eq!(containers.agrees_with(Some(declared)), Some(false));
assert_eq!(containers.discrepancy(Some(declared)), Some((50, 0)));
```

The crate reports the disagreement (V07) rather than picking a side: either
number may be the wrong one, and only the operator knows which node is
misbehaving. The signed discrepancy says how far out it is, which distinguishes
a rounding artefact from a dropped container.


## What the batch adds up to

Every other layer decides whether a record *may* be billed. Aggregation says
what the bill is **of** — the last place a wrong number can be caught, because
after it the quantities become money.

**A total never travels without its evidence.** `aggregate_released` returns the
sum *and* the reconciliation that produced it, because a total alone cannot be
audited: nobody can tell whether 4 TB is every record or the two-thirds that
happened to parse.

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
assert_eq!(released.reconciliation().ingested(), 3);  // all three accounted for
assert!(released.is_consistent());  // the sum matches what was released
```

**An absent quantity is counted, not just skipped.** A record that contributed
nothing is arithmetically fine and operationally suspicious — a field a decoder
dropped contributes nothing too, and the total is then short by exactly that
amount with nothing to show for it. `UsageTotal::unquantified()` makes the
silence visible in the total itself.

**Aggregation refuses to guess.** A record whose own total disagrees with its
container breakdown makes two statements of one fact — summing either picks a
side, and only the operator knows which node is misbehaving. It stops and names
the record. Overflow is treated the same way: a wrapped total is a small
plausible number, and small plausible wrong numbers are the ones that reach an
invoice.

`aggregate_by` takes a key function rather than an enum of strategies, so
grouping by local day, tenant, service or any combination is a closure.
`aggregate_rating_groups` reads the containers alone and never the record total,
so the two cannot disagree — and a record with no containers lands in `None`
rather than being priced under a tariff nothing said it belonged to.
