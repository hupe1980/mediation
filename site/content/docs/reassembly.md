+++
title = "Partial records and correlation"
description = "Putting a run of partials back together without inventing anything, and joining the records several nodes wrote about one usage without billing it twice."
weight = 100
+++

## Partial records: reconstruct, never invent

A long session produces a run of partial records, closed and reopened at each
volume limit, time limit or tariff boundary. Billing the session means putting
the run back together — and **only** putting it back together.

Some metering domains are authorised to substitute a value for a lost reading.
Telecoms is not: usage that cannot be evidenced is not billable. So this crate
reassembles what arrived and reports what did not. There is no API here that
would let it synthesise a quantity.

**The first partial of a run must be fully qualified** (3GPP TS 32.240). A
Reduced Partial CDR carries only mandatory fields plus what *changed*, so
reading an absent field as zero is a correctness bug — the field did not become
zero, it did not change:

```rust
use mediation::{PartialFormat, PartialInfo, PartialRun, Reassembly, RecordClosureCause};

// A run whose first record is reduced has no basis to complete itself from.
let orphan = PartialRun::from_parts(vec![
    PartialInfo::new(1, PartialFormat::Reduced, RecordClosureCause::VolumeLimit),
]);
assert!(matches!(orphan.assess(), Reassembly::Orphan { .. }));
assert!(!orphan.assess().may_still_complete(), "waiting cannot fix it");

// A run with no holes and no terminal record is open, not broken.
let open = PartialRun::from_parts(vec![
    PartialInfo::new(1, PartialFormat::FullyQualified, RecordClosureCause::VolumeLimit),
    PartialInfo::new(2, PartialFormat::Reduced, RecordClosureCause::VolumeLimit),
]);
assert!(open.assess().may_still_complete());
```

**Why a partial exists tells you whether it is suspicious.** A run ending on an
abnormal release or a management intervention has an explained ending — the node
could not be expected to emit a successor. The same shape after a volume limit
is unexplained loss. `Reassembly::is_explained_loss` is that distinction, and it
is only available because the closure cause is modelled.


## Correlation: two failures, opposite in sign

One subscriber action produces records at every node it traverses. The P-GW
assigns a Charging ID and forwards it to the S-GW (3GPP TS 32.251), so both
nodes' records carry it — and that creates two failures pointing opposite ways:

- **Billing both sides double-charges.** Two records describing one bearer, each
  carrying its own volume, are not two usages.
- **Discarding an unmatched record hides leakage.** A record whose counterpart
  never arrived is evidence that something was lost — quite possibly the side
  that was going to be billed.

So `correlate` returns the joined groups **and** the unmatched records, and
`accounts_for` asserts that together they cover every input exactly once:

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

**Leg chains.** When a terminating application server diverts a call it
generates a *new* ICID and records the incoming one as a linked ICID
(TS 32.260), so the legs form a chain. Following it reconstructs the history a
forwarded call actually took — and a leg naming a predecessor that never arrived
is reported as a `BrokenLeg` rather than assumed to be an origin, because the
difference is the evidence of who placed the call.

A broken leg is **not** an unmatched record: the leg joined its own session
perfectly well, and what is missing is the leg before it. Counting it in both
places would break `accounts_for`, which is the correlation counterpart of the
conservation law — groups and unmatched records together account for every input
record exactly once, whatever the batch contains.

```rust
use mediation::{CorrelationConfig, CorrelationKeys, correlate};
use mediation::{ChargedParty, RecordType, SourceRef, SubscriberRef, UsageRecord};
fn leg(icid: &str, linked: Option<&str>) -> UsageRecord {
    let mut keys = CorrelationKeys::default().with_icid(icid.parse().unwrap());
    if let Some(previous) = linked {
        keys = keys.with_linked_icid(previous.parse().unwrap());
    }
    UsageRecord::new(
        RecordType::ImsSession,
        ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
        "2026-07-28T10:00:00+02:00".parse().unwrap(),
        SourceRef::new("scscf-01".parse().unwrap(), 0),
    ).with_correlation(keys)
}
// A diverted call whose first leg never arrived.
let records = vec![leg("leg-b", Some("leg-a"))];
let result = correlate(&records, &CorrelationConfig::ims());

assert_eq!(result.broken_legs().len(), 1);
assert_eq!(result.broken_legs()[0].missing.as_str(), "leg-a");
assert!(result.unmatched().is_empty());
assert!(result.accounts_for(records.len()));
```
