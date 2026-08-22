+++
title = "Splitting a session at a tariff boundary"
description = "Splitting time is exact; splitting volume is an assumption. Both are labelled, and apportionment never creates or destroys usage."
weight = 120
+++

A session that starts before peak and ends after it is two pieces of usage
priced differently, and the rating layer cannot recover a split that was never
made. This crate makes the split; it never prices, and it holds no calendar — a
boundary is an instant a caller supplies, because when peak begins is a
commercial decision.

**Splitting time is exact. Splitting volume is not.** The wall-clock seconds on
each side follow from the two instants. *When the octets flowed does not*: a
record reports one total and never says whether the download happened before the
boundary or after. So every result is stamped with an `Apportionment`, because
an estimate that reaches a bill unlabelled is indistinguishable from a
measurement.

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

// A two-hour session with peak starting an hour in.
let split = split_session(&record, &[datetime!(2026-07-28 08:00 +2)]).unwrap();

assert_eq!(split.len(), 2);
assert_eq!(split.basis(), Apportionment::ProRata);
assert_eq!(split.slices()[0].volume.unwrap().uplink(), Octets::new(450));
assert!(split.conserves(&record));
```

`split_measured` is the alternative where the record's containers carry
tariff-time boundaries: the network closed a container at the tariff change and
reported the volume on each side, so nothing is assumed.

## Apportionment conserves

Dividing 1 000 octets across three slices by integer division gives 333 + 333 +
333 and loses one. A mediation layer that loses octets has broken the very law
it exists to uphold, so `apportion` uses the largest-remainder method: the
truncated units are distributed rather than discarded, deterministically.

```rust
use mediation::apportion;

let parts = apportion(1_000, &[1, 1, 1]);
assert_eq!(parts, vec![334, 333, 333]);
assert_eq!(parts.iter().sum::<u64>(), 1_000);
```

This is checked as a `proptest` property, not on examples: for any total and any
weights, the parts sum to exactly the whole.

`UsageSplit::conserves` checks the round trip on a real record, and the totals
it compares are **all-or-nothing**: `total_volume()` is `None` unless *every*
slice carried one. Summing the slices that happen to have a figure and calling
the result the session's total reports a number short by exactly the slices that
did not — which is the mistake a conservation check exists to catch, not to
commit. For the same reason `split_measured` refuses a record carrying an event
count: containers report volume and time and never events.
