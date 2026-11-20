+++
title = "Deduplication"
description = "A natural key from the standard, held for a mandatory window."
weight = 90
+++

Retransmissions are rarely byte-identical, so a record hash misses them; a
subscriber-and-start-time key drops concurrent sessions. `RecordKey` uses the
standard's identifiers:

| Domain | Key |
|---|---|
| Packet data | Charging ID **and** gateway, partial sequence |
| IMS | ICID, partial sequence |
| SIP call | recording entity, record type, Call-ID, partial sequence |
| Other | recording entity, `localSequenceNumber` |

The partial sequence comes from `PartialInfo` when present. A record with half
an identity, or none, is `NotKeyable` and is rejected rather than risk double
billing.

```rust
use mediation::{NotKeyable, RecordKey};
use mediation::{ChargedParty, RecordType, SourceRef, SubscriberRef, UsageRecord};

let bare = UsageRecord::new(
    RecordType::PduSession,
    ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
    "2026-07-28T10:00:00+02:00".parse().unwrap(),
    SourceRef::new("pgw-01".parse().unwrap(), 0),
);
assert_eq!(RecordKey::of(&bare), Err(NotKeyable::NoCorrelatorPresent));
```

## The window

`DedupWindow::from_bound` is mandatory: "partners never resend later than
this". In `process`, keys are stamped with the batch's `as_of`. A carried-in
held record refreshes its key instead of being checked. A rejected record
leaves no key, so a corrected resend is judged on its merits.
