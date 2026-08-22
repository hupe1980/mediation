+++
title = "Deduplication"
description = "Hashing a record fails, because a retransmission is rarely byte-identical. The natural key comes from the standard, and the window is a promise about the outside world."
weight = 90
+++

Duplicates are the normal consequence of a retransmission after a lost
acknowledgement, a re-sent file after a partner's failed load, and a node's own
recovery replay.

**Hashing the record fails.** A retransmitted record is frequently not
byte-identical: node identifiers, padding and extension fields differ, so a
whole-record hash lets retransmissions straight through.

**Keying on subscriber and start time also fails.** Concurrent sessions on one
subscription are normal — a 5G handset holds several simultaneous PDU sessions
across different data networks, and two can be established in the same second.

So the key comes from the standard. **3GPP TS 32.251 is explicit that a Charging
ID is unique only together with the P-GW address**, so `CorrelationKeys` yields
the pair or nothing:

```rust
use mediation::{ChargingId, CorrelationKeys, NotKeyable, RecordKey};
use mediation::{ChargedParty, RecordType, SourceRef, SubscriberRef, UsageRecord};
use std::net::Ipv4Addr;

let party = ChargedParty::Subscriber(
    SubscriberRef::new("imsi-262021234567890".parse().unwrap()));
let source = SourceRef::new("pgw-01".parse().unwrap(), 0);
let opened = "2026-07-28T10:00:00+02:00".parse().unwrap();
// A Charging ID with no gateway does not identify a session.
let half = CorrelationKeys::default().with_charging_id(ChargingId::new(1_234));
assert!(half.packet_session().is_none());

// …and a record carrying no correlator at all cannot be deduplicated, which is
// said rather than papered over: a provenance-derived key differs for every
// copy, so every retransmission would pass.
let bare = UsageRecord::new(RecordType::PduSession, party, opened, source);
assert_eq!(RecordKey::of(&bare), Err(NotKeyable::NoCorrelatorPresent));
```

**The recording entity is not the collecting node.** A `localSequenceNumber` is
unique within the element that *created* the record, and a gateway routinely
collects records from several elements into one file. Keying on the file's node
would give one record two different keys when it is collected through two
gateways — so every retransmission would pass, which is the failure the whole
mechanism exists to prevent. `CorrelationKeys::recording_entity` carries the
creating element, and a serial number without it is `NotKeyable` rather than
silently scoped to the wrong thing.

The `DedupWindow` is mandatory and named for what it encodes: "our partners
never resend later than this". If that is false, duplicates pass and revenue is
billed twice. An unbounded store is not a design, it is a memory leak.
