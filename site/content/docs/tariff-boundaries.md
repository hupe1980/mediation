+++
title = "Splitting a session at a tariff boundary"
description = "Exact time splits, labelled volume splits, and apportionment that never creates or destroys usage."
weight = 120
+++

A session crossing a tariff boundary is two pieces of usage, and rating cannot
recover a split that was never made. Boundaries are instants the caller
supplies; the crate holds no calendar and prices nothing.

Time splits exactly. Volume does not: a record reports one total, not when the
octets flowed. Every result is labelled with an `Apportionment`, so an estimate
never reaches a bill looking like a measurement.

```rust
use mediation::{Apportionment, Octets, VolumePair, split_session};
use mediation::{ChargedParty, RecordType, SourceRef, SubscriberRef, UsageRecord};
use time::macros::datetime;

let record = UsageRecord::new(
    RecordType::PduSession,
    ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
    "2026-07-28T07:00:00+02:00".parse().unwrap(),
    SourceRef::new("pgw-01".parse().unwrap(), 0),
)
.with_closed("2026-07-28T09:00:00+02:00".parse().unwrap())
.with_volume(VolumePair::new(Octets::new(900), Octets::new(300)));

let split = split_session(&record, &[datetime!(2026-07-28 08:00 +2)]).unwrap();
assert_eq!(split.basis(), Apportionment::ProRata);
assert_eq!(split.slices()[0].volume.unwrap().uplink(), Octets::new(450));
assert!(split.conserves(&record));
```

`split_measured` uses containers the network closed at the tariff change, so
nothing is assumed. It groups containers by close instant across rating groups
and in any order, and refuses a container closing after the record or a record
carrying an event count.

## Apportionment conserves

Integer division loses units (1 000 / 3 = 333 × 3). `apportion` distributes the
remainder by largest remainder, deterministically, and a property test checks
the parts always sum to the whole.

```rust
use mediation::apportion;

let parts = apportion(1_000, &[1, 1, 1]);
assert_eq!(parts, vec![334, 333, 333]);
```

`UsageSplit::conserves` compares all-or-nothing totals: `total_volume()` is
`None` unless every slice carried a volume.
