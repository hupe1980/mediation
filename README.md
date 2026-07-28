# 📡 mediation

[![Crates.io](https://img.shields.io/crates/v/mediation.svg)](https://crates.io/crates/mediation)
[![Docs.rs](https://docs.rs/mediation/badge.svg)](https://docs.rs/mediation)
[![CI](https://github.com/hupe1980/mediation/actions/workflows/ci.yml/badge.svg)](https://github.com/hupe1980/mediation/actions/workflows/ci.yml)
[![MSRV](https://img.shields.io/badge/MSRV-1.94-blue.svg)](https://blog.rust-lang.org/)
[![License](https://img.shields.io/crates/l/mediation.svg)](#-license)

**Telecom usage mediation domain library** — charging-record time as local time
plus a quarter-hour offset, integer octet volumes, validated subscriber and
network identifiers, and charged-party attribution that does not assume every
record belongs to a subscriber.

> 🧊 **Zero I/O** · ⏱️ **no async** · 🕰️ **no clock** · 🔢 **exact integer quantities**

`mediation` is a **standalone, dependency-light domain crate**. It performs no
network or disk access, spawns no runtime, never reads the system clock, and
never represents a chargeable quantity as a floating-point number. You bring the
data; it does the arithmetic — deterministically, with the standards basis
documented at every call site.

It computes **usage**, not money: there is no currency, tariff, price or rate
type anywhere in the crate. What leaves here is octets, seconds and event
counts, attributed to a party, a network and a service — which a rating layer
then prices.

---

## 📦 Installation

```bash
cargo add mediation
```

or in `Cargo.toml`:

```toml
[dependencies]
mediation = "0.1"
```

With `serde` support for all public types:

```toml
[dependencies]
mediation = { version = "0.1", features = ["serde"] }
```

**MSRV:** Rust `1.94` (edition 2024), pinned in
[`rust-toolchain.toml`](rust-toolchain.toml) and verified by a dedicated CI lane.

**API stability:** pre-`1.0`, so breaking changes ship as minor releases.

---

## 🚀 Quick start

```rust
use mediation::{
    BatchReconciliation, ChargedParty, ChargingId, CorrelationKeys, DedupDecision, DedupStore,
    Disposition, Octets, RecordKey, RecordType, Seconds, ServiceDomain, SourceRef, SubscriberRef,
    UsageRecord, VolumePair,
};
use std::net::Ipv4Addr;

let record = UsageRecord::new(
    RecordType::PduSession,
    ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse()?)),
    "2026-07-28T10:00:00+02:00".parse()?,
    SourceRef::new("pgw-fra-01".parse()?, 0).with_file_sequence(42),
)
.with_closed("2026-07-28T10:30:00+02:00".parse()?)
.with_duration(Seconds::new(1_800))
.with_volume(VolumePair::new(Octets::new(4_096), Octets::new(65_536)))
.with_networks("310-260".parse()?, "262-02".parse()?)
.with_correlation(
    CorrelationKeys::default()
        .with_charging_id(ChargingId::new(1_234))
        .with_gateway(Ipv4Addr::new(10, 0, 0, 1).into()),
);

assert_eq!(record.service(), ServiceDomain::Data);   // derived, never stored twice
assert_eq!(record.is_roaming(), Some(true));
assert_eq!(record.local_date().to_string(), "2026-07-28");

// The store is yours: `process(input, store, as_of)` stays a pure mapping.
let mut store = DedupStore::new();
let mut counts = BatchReconciliation::EMPTY;
let at = record.opened.to_utc();

for _ in 0..3 {                                       // the same record, three times
    let key = RecordKey::of(&record).expect("keyed on Charging ID + gateway");
    counts.observe(&match store.observe(key.clone(), at) {
        DedupDecision::First => Disposition::Billable,
        DedupDecision::Duplicate { .. } => Disposition::Duplicate(key),
    });
}

assert_eq!(counts.released(), 1);                     // billed once, never twice
assert_eq!(counts.duplicate(), 2);
assert!(counts.balances());                           // nothing created, nothing lost
# Ok::<(), mediation::ParseError>(())
```

---

## 🕰️ Time is local, not UTC

The single most consequential thing this crate gets right.

The `TimeStamp` of **3GPP TS 32.298** is nine BCD octets spelling
`YYMMDDhhmmssShhmm`: a **local** date and time, a sign, and a signed offset to
UTC. There is no timezone *name* anywhere in a charging record — only the
numeric offset that was true when the record was written.

```rust
use mediation::CdrTimestamp;

let ts: CdrTimestamp = "2026-07-28T00:30:00+05:45".parse().unwrap();

// The subscriber's billing day …
assert_eq!(ts.local_date().to_string(), "2026-07-28");
// … is not the UTC day.
assert_eq!(ts.to_utc().date().to_string(), "2026-07-27");
```

Three failure modes this removes from every downstream consumer:

- **Reading a local timestamp as UTC** books every evening event of a UTC+X
  network onto the wrong calendar day. Unlike a fixed-zone mistake it cannot be
  corrected downstream by subtracting a constant, because the offset varies
  from record to record.
- **Assuming whole-hour offsets.** The offset comes from the MS Time Zone
  element (TS 29.060 §7.7.52, encoded per TS 24.008, sharing the semi-octet form
  of TS 23.040 §9.2.3.11) and counts **quarter-hours**. India (+05:30), Nepal
  (+05:45), Eucla (+08:45) and the Chatham Islands (+12:45) all land on the
  wrong hour, and therefore the wrong tariff window, under an `i8` of hours.
- **Dropping the daylight-saving bits.** Without them an offset cannot be mapped
  back to a zone at all.

### Two ranges, deliberately

The semi-octet encoding can express ±79 quarter-hours — almost ±20 hours — while
only UTC−12:00 … UTC+14:00 are assigned. `QuarterHourOffset::new` accepts the
assigned range; `from_encoded` accepts anything the wire can hold, so a decoder
can carry an out-of-range offset forward as a finding rather than panicking or
clamping it. Clamping is the worst of the three: it turns corrupt input into a
plausible timestamp that no later stage can identify as wrong.

### The same offset is two different zones

UTC+02:00 is Berlin in July and Athens in January. Only the daylight-saving
field tells them apart, so it is kept rather than folded into the offset:

```rust
use mediation::{DstAdjustment, MsTimeZone, QuarterHourOffset};

let plus_two = QuarterHourOffset::from_minutes(120).unwrap();
let berlin_summer = MsTimeZone::new(plus_two, DstAdjustment::PlusOneHour);
let athens_winter = MsTimeZone::new(plus_two, DstAdjustment::None);

assert_eq!(berlin_summer.offset(), athens_winter.offset());
assert_eq!(berlin_summer.standard_offset().unwrap().to_string(), "+01:00");
assert_eq!(athens_winter.standard_offset().unwrap().to_string(), "+02:00");

// Which is also how a DST transition is told apart from a journey.
let berlin_winter = MsTimeZone::new(QuarterHourOffset::from_minutes(60).unwrap(), DstAdjustment::None);
assert!(berlin_winter.is_same_zone_across_transition(berlin_summer));
assert!(!berlin_winter.is_same_zone_across_transition(athens_winter));
```

### There is no `Ord` on a timestamp

Records from different offsets compare correctly only as *instants*. A derived
ordering would compare the local fields and silently interleave a UTC+14:00
record ahead of a UTC−12:00 one that happened a day earlier — so the trait is
simply absent, and the instant comparator is the only way:

```rust
use mediation::CdrTimestamp;

let mut series: Vec<CdrTimestamp> = vec![
    "2026-07-27T23:00:00-12:00".parse().unwrap(),
    "2026-07-28T01:00:00+14:00".parse().unwrap(),
];
series.sort_by(CdrTimestamp::cmp_instant);

// The +14:00 record is the earlier instant, despite the later local date.
assert_eq!(series[0].to_string(), "2026-07-28T01:00:00+14:00");
```

### Duration is not `closed − opened`

The duration a record carries is the **chargeable** one, measured from answer. A
45-second ring followed by 10 seconds of conversation is a 10-second call across
a 55-second wall-clock span. Deriving duration by subtraction over-bills every
answered call and invents duration for every unanswered one, so `duration` is a
field and `wall_clock_span()` is a separate, differently-named accessor. Nothing
falls back from one to the other.

---

## 🔢 Octets, not megabytes — and never a float

A data volume is inherently an integer count of octets, so a float is not merely
imprecise but categorically wrong: above 2^53 octets (≈ 9.01 PB) consecutive
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

---

## 🪪 Identity

| Type | Standard | Digits | Check digit |
|---|---|---|---|
| `Imsi` | ITU-T E.212 | 6–15 | — |
| `Msisdn` | ITU-T E.164 | 1–15 | — |
| `Imei` | 3GPP TS 23.003 | 15 | **Luhn** |
| `Imeisv` | 3GPP TS 23.003 | 16 | **none** |
| `Iccid` | ITU-T E.118 | 19–20 | Luhn, when present |
| `Supi` / `Gpsi` / `Pei` | 3GPP TS 23.003 | — | inherits |
| `Suci` | 3GPP TS 23.003 | — | — |

Every numeric identifier stores its digits packed two per byte, inline — a
mediation layer holds millions in flight at once, and a heap allocation per
subscriber identity is the difference between fitting in cache and not. All of
them are `Copy`.

### An IMSI cannot be split without a table

An IMSI is MCC (3 digits) + MNC (**2 or 3**) + MSIN, and **nothing in the digits
says which**. North American MCCs use three-digit MNCs; most of the world uses
two. Splitting at a fixed offset misattributes an entire region to networks that
do not exist — silently, because the result is still well-formed.

```rust
use mediation::{Imsi, Mcc, MncWidth, NorthAmericanRule};

let de: Imsi = "262021234567890".parse().unwrap();
let us: Imsi = "310260123456789".parse().unwrap();

// The country code needs no table at all.
assert_eq!(de.mcc(), Mcc::GERMANY);

// Everything past it does.
assert_eq!(de.plmn(&NorthAmericanRule).unwrap().to_string(), "262-02");
assert_eq!(us.plmn(&NorthAmericanRule).unwrap().to_string(), "310-260");

// Your own table is a closure — no newtype needed.
let table = |mcc: Mcc| (mcc == Mcc::GERMANY).then_some(MncWidth::Two);
assert_eq!(de.plmn(&table).unwrap().to_string(), "262-02");
// An unknown country yields no network, rather than a default that would
// misfile the subscriber under a network that did not serve them.
assert_eq!(us.plmn(&table), None);
```

The crate ships the structure and the validation, not a table that would be
stale the week it was published.

### An MNC is a string, not a number

German MNCs under MCC 262 are `01` (Telekom), `02` (Vodafone), `03`/`07`
(Telefónica) and `23` (1&1). Held as an integer, `01` and `1` collapse into one
value — merging two operators, or splitting one, depending on which way the
coercion falls.

```rust
use mediation::{Mnc, PlmnId};

let telekom: PlmnId = "262-01".parse().unwrap();
assert_eq!(telekom.to_string(), "262-01");

// Same number, different declared width, different network.
assert_ne!(Mnc::new(1, 2).unwrap(), Mnc::new(1, 3).unwrap());
assert_eq!(Mnc::new(1, 3).unwrap().to_string(), "001");
```

### IMEI is Luhn-checked and IMEISV is not

The fifteenth position that carries the check digit in an IMEI is the first
software-version digit in an IMEISV. Running the check over an IMEISV rejects
valid equipment; not running it over an IMEI accepts corrupt records. They are
separate types so the compiler enforces the difference.

```rust
use mediation::{Imei, Imeisv, Pei};

// A transposition breaks the checksum, which is the point of having one.
assert!("490154203237518".parse::<Imei>().is_ok());
assert!("490154203237581".parse::<Imei>().is_err());

// The same equipment with a software version — no checksum to break.
let sv: Imeisv = "4901542032375120".parse().unwrap();
assert_eq!(sv.software_version().to_string(), "20");

// Both forms collapse onto one device, so a mixed record set does not
// double-count handsets.
let a: Pei = "imei-490154203237518".parse().unwrap();
let b: Pei = "imeisv-4901542032375120".parse().unwrap();
assert_ne!(a, b);
assert_eq!(a.to_imei(), b.to_imei());
```

### A SUCI is not an identity you can count

`Suci` implements neither `Hash` nor `Ord`, deliberately, so it cannot reach a
deduplication key, a map key or a sort. It is designed to be non-linkable and
differs between registrations of the same subscriber; treating it as an identity
splits one subscriber into many and makes both billing and fraud detection
quietly wrong. A record that only ever carried a SUCI is unattributable.

---

## 👤 The charged party is not always a subscriber

Two kinds of usage already in production break a mandatory subscriber field:

- **Network APIs.** Under GSMA Open Gateway, operators sell capability directly,
  and 3GPP specifies the charging in **TS 32.254**. The chargeable unit is an
  API invocation with **two** counterparties — the subscriber whose number was
  verified, and the developer who paid to verify it.
- **Network slice management.** **TS 28.202** charges slice instance creation,
  modification and termination, and **TS 28.201** charges slice performance and
  analytics, via the Charging Enablement Function. **There is no subscriber at
  all**; the charged party is an enterprise tenant.

```rust
use mediation::{ChargedParty, SubscriberRef, TenantRef, UnattributableReason};

// An API invocation keeps payer and subject apart — the developer is not a
// subscriber and must not be counted as one.
let api = ChargedParty::ApiConsumer {
    developer: "acme-dev".parse().unwrap(),
    subject: Some(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
};
assert_eq!(api.kind(), "API_CONSUMER");
assert!(api.subscriber().is_some());

// Slice management has a tenant and no subscriber at all.
let slice = ChargedParty::Tenant(TenantRef::new("enterprise-7".parse().unwrap()));
assert!(slice.subscriber().is_none());

// And an unresolved party is a counted outcome carrying a reason, never a null.
let lost = ChargedParty::Unattributable(UnattributableReason::ConcealedIdentityOnly);
assert_eq!(lost.to_string(), "UNATTRIBUTABLE:CONCEALED_IDENTITY_ONLY");
```

`Unattributable` is a variant rather than an `Option` because a record whose
party could not be resolved must carry **why**, and must reconcile as a counted
outcome instead of vanishing into a null.

Tenancy is on `SubscriberRef` rather than bolted on afterwards: reconciliation
must balance **per tenant** — a host-network batch that balances overall can
still have lost every record of one MVNO — retention policy differs per tenant,
and fair-use assessment is per retail brand. `None` means unresolved, not "the
host network".

---

## ⚖️ Every record reaches exactly one outcome

The crate's central invariant, and the difference between a mediation layer and
a data pipeline: a pipeline moves records, while a mediation layer can **prove**
it did not lose any.

```text
carried_in + ingested == billable + suppressed + duplicate + rejected + held
```

`BatchReconciliation::balances()` is that equation. A batch that fails it has
unknown completeness, and billing over unknown completeness invoices an amount
nobody can prove is right.

Four outcomes are final; `Held` is the one that may return — a partial record
whose successor has not arrived, a leg whose counterpart is still in flight, an
identity awaiting reference data.

### The law spans batches, not just one

A held record comes back. If the batch that re-presents it counts it as newly
ingested, the two batches together report it twice; if neither counts it, it
vanishes. **Both are the law failing at a seam, which is exactly where nobody
looks** — each batch balances perfectly on its own.

So a re-presented record is counted through `observe_carried` rather than
`observe`, and `PeriodLedger` checks that each batch carries in exactly what its
predecessor left held:

```rust
use mediation::{BatchReconciliation, ChainBreak, Disposition, HoldReason, PeriodLedger};

let mut monday = BatchReconciliation::EMPTY;
monday.observe(&Disposition::Held(HoldReason::AwaitingPartial));

// Tuesday re-presents it — but counts it as a new arrival.
let mut tuesday = BatchReconciliation::EMPTY;
tuesday.observe(&Disposition::Billable);
assert!(tuesday.balances(), "the batch is internally consistent");

let mut period = PeriodLedger::new();
period.append(monday).unwrap();
assert_eq!(
    period.append(tuesday),
    Err(ChainBreak::CarryMismatch { batch: 1, expected: 1, found: 0 }),
);
assert!(!period.is_closable());
```

Chained that way the law telescopes: across a whole period, everything ingested
reached a final outcome or is still explicitly outstanding and named as such. A
past break is never forgiven — `is_closable()` stays false even if the caller
continued.

Both laws are checked as `proptest`s over arbitrary batches and arbitrary
periods — see [`tests/conservation.rs`](tests/conservation.rs) and
[`tests/period_and_correlation.rs`](tests/period_and_correlation.rs).

---

## 🧬 Deduplication: the natural key is the whole design

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
# use mediation::{ChargedParty, RecordType, SourceRef, SubscriberRef, UsageRecord};
use std::net::Ipv4Addr;

# let party = ChargedParty::Subscriber(
#     SubscriberRef::new("imsi-262021234567890".parse().unwrap()));
# let source = SourceRef::new("pgw-01".parse().unwrap(), 0);
# let opened = "2026-07-28T10:00:00+02:00".parse().unwrap();
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

---

## 🔎 Completeness: sequence gaps are the only evidence of loss

A lost record file leaves **no trace in the surviving data**. Nothing that
arrives says the missing file existed, no total is visibly short, and no
subscriber complains — the usage simply was never billed.

The only evidence is a hole in a per-source sequence number, and the trap is the
wrap. A GSMA TAP file sequence runs `99999` then `00001`, at which point three
naive checks break, once per cycle: `next == previous + 1` reports a 99 998-file
gap, `next > previous` reports the entire next cycle as out of order, and
`next - previous` underflows.

`SequenceSpace` therefore carries the modulus and comparison follows the
serial-number arithmetic of **RFC 1982**:

```rust
use mediation::{SequenceObservation, SequenceSpace, SequenceTracker, SequenceValue};

let last = SequenceValue::new(SequenceSpace::TAP, 99_999).unwrap();
let next = SequenceValue::new(SequenceSpace::TAP, 1).unwrap();
assert!(last.is_successor(next));
assert_eq!(last.gap_to(next), Some(0));

// Gaps are identities, not a count: the operational response is a resend
// request for exactly these files.
let mut tracker = SequenceTracker::new(SequenceSpace::TAP);
tracker.observe_default(99_998);
assert_eq!(
    tracker.observe_default(2),
    SequenceObservation::Gap { missing: vec![99_999, 1] },
);
assert!(!tracker.is_complete());
```

One tracker per **scope**, and the scope is part of the standard: a TAP sequence
is per sender/recipient TADIG pair, a CDR file counter is per CGF node, a
`localSequenceNumber` is per recording entity. Sharing a tracker across scopes
manufactures gaps that do not exist.

---

## ⏳ Late data is the whole problem

Almost every window is the same window wearing a different hat: an event
happened at T, it is being considered at `as_of`, and the elapsed time changes
what may be done with it. `LatenessPolicy` is that one concept.

| Window | Bound |
|---|---|
| TAP inclusion | **30 days** (GSMA TD.57) |
| NRTRDE delivery | **4 hours** (GSMA TD.35) |
| Deduplication | the partner's resend bound |

Two distinctions the type refuses to blur:

- **Late and unbillable are different.** `Lateness::Late` is still billable —
  the holding structures may have moved on, which is an operational finding.
  `Lateness::BeyondDeadline` can never become billable. Collapsing them either
  bills unbillable usage or discards billable usage.
- **A future event is not an on-time event.** `Lateness::Future` exists because
  a record timestamped ahead of the evaluation instant is clock skew or a
  corrupt field, and folding it into `OnTime` hides the only signal that would
  catch it.

`as_of` is always a parameter, which is what lets a 2019 archive replayed in
2026 reproduce the *2019* verdicts. A policy that read the clock would declare
every historic record beyond every deadline — the whole archive rejected.

---

## ➕ What the batch adds up to

Every other layer decides whether a record *may* be billed. Aggregation says
what the bill is **of** — the last place a wrong number can be caught, because
after it the quantities become money.

**A total never travels without its evidence.** `aggregate_released` returns the
sum *and* the reconciliation that produced it, because a total alone cannot be
audited: nobody can tell whether 4 TB is every record or the two-thirds that
happened to parse.

```rust
use mediation::{Disposition, Octets, RejectReason, aggregate_released};
# use mediation::{ChargedParty, RecordType, SourceRef, SubscriberRef, UsageRecord, VolumePair};
# fn record(up: u64) -> UsageRecord {
#     UsageRecord::new(
#         RecordType::PduSession,
#         ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
#         "2026-07-28T10:00:00+02:00".parse().unwrap(),
#         SourceRef::new("pgw-01".parse().unwrap(), 0),
#     ).with_volume(VolumePair::new(Octets::new(up), Octets::ZERO))
# }
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

---

## 🌍 A partner's batch, and whether it adds up

A GSMA TAP batch is self-describing: it states how many events it holds, the
window they fall in, and what they total. Those claims are checkable against the
events, and `TapBatch::audit` reports **every** discrepancy rather than the
first — a batch with three problems should be fixed once.

**Money is an integer with a scale.** TAP carries monetary values as integers
scaled by `tapDecimalPlaces`; a charge of `1234` with 3 places is `1.234`.
Rendering that through a float reintroduces exactly the error the integer
encoding exists to avoid, so `ScaledAmount` has no float accessor:

```rust
use mediation::ScaledAmount;

let charge = ScaledAmount::new(1_234, 3);
assert_eq!(charge.to_string(), "1.234");

// Amounts add only at one scale. Rescaling the coarser operand invents
// precision; rescaling the finer one discards it. Either is a rounding error
// entering a settlement silently.
assert!(charge.checked_add(ScaledAmount::new(123, 2)).is_none());
assert_eq!(
    charge.checked_add(ScaledAmount::new(766, 3)),
    Some(ScaledAmount::new(2_000, 3)),
);
```

The crate still prices nothing — it carries an amount a partner computed, so a
reconciliation can check it.

**A returned batch and a returned record reconcile differently.** GSMA TD.32
distinguishes two rejection classes, and conflating them breaks the conservation
law in opposite directions:

| Class | Rejects | Consequence |
|---|---|---|
| **Fatal** | the whole file | none of its events ever entered the counts; the sequence number stays open |
| **Severe** | individual events | the file counted; the returned events move from billable to rejected |

`RapReturn::effect_on` applies each correctly rather than leaving a caller to
remember which is which — and returns `None` when more events come back than
were released, because that is a disagreement with the partner rather than
arithmetic to absorb.

---

## 🚦 The release gate

Everything in this crate exists to answer one question: **may this batch be
invoiced?** `score_batch` answers it from three inputs, and reports every reason
it said no rather than only the first — fixing one blocker does not imply the
batch is releasable.

| Blocker | Meaning |
|---|---|
| `Unbalanced` | the conservation law does not hold |
| `IncompleteSources` | a source has an unfilled sequence hole, so completeness is **unknown** |
| `BlockingFindings` | at least one rule fired at blocking severity |

```rust
use mediation::{BatchReconciliation, Disposition, QualityGrade, ValidationReport, score_batch};

let counts: BatchReconciliation = vec![Disposition::Billable; 1_000_000].into_iter().collect();

// A million clean records still cannot outvote an unknown hole in a source.
let report = score_batch(&counts, &ValidationReport::new(), false);
assert_eq!(report.grade, QualityGrade::F);
assert!(report.blocks_release());
```

Unknown completeness is a **hard gate**, not a score. Nothing in such a batch is
wrong; what is wrong is that the batch cannot say what it is missing, and
releasing it invoices an amount nobody can prove is right.

The grade is derived rather than configurable, and its thresholds are
proportions — a batch of a hundred and a batch of a hundred million grade the
same way for the same defect rate. A per-deployment threshold would make two
teams' "grade B" mean different things.

---

## 🔍 The rule catalogue (V01–V21)

A stable, numbered rule set a downstream system can cite in a ticket: "V07 on
the Frankfurt gateway" means the same thing to everyone who reads it, this year
and in five years. Numbers are never reused; a retired rule keeps its number
reserved.

| Rule | Detects | Blocks |
|---|---|---|
| V01 `UnassignedOffset` | UTC offset outside UTC−12:00 … UTC+14:00 | ✅ |
| V02 `FutureTimestamp` | record opens after the evaluation instant | ✅ |
| V03 `ClosedBeforeOpened` | record closes before it opens | ✅ |
| V04 `DurationExceedsSpan` | chargeable duration longer than the wall-clock span | ✅ |
| V05 `ImplausibleVolume` | volume the elapsed time could not have carried | — |
| V06 `ZeroUsage` | no quantity at all | — |
| V07 `ContainerSumMismatch` | container sum disagrees with the record total | ✅ |
| V08 `UnattributableParty` | no charged party could be determined | ✅ |
| V09 `NotKeyable` | no natural key, so the record cannot be deduplicated | ✅ |
| V10 `BeyondDeadline` | past the inclusion deadline | ✅ |
| V11 `LateArrival` | late, still billable | — |
| V12 `ServingNetworkAbsent` | no serving network on subscriber-bound usage | — |
| V13 `QuantityDoesNotMatchService` | quantity absent for what the service measures | — |
| V14 `CheckDigitFailure` | identifier check digit does not verify | — |
| V15 `VolumeOverflow` | the record's own directions do not sum | ✅ |
| V16 `UnorderedSeries` | input not in ascending instant order | — |
| V17 `DuplicateRecord` | retransmission of a record already seen | — |
| V18 `SequenceGap` | source sequence has an unfilled hole | ✅ |
| V19 `OrphanPartial` | run of partials cannot be reassembled at all | ✅ |
| V20 `IncompleteReassembly` | run of partials is missing members | ✅ |
| V21 `JurisdictionConflict` | serving network and reported location disagree on the country | ✅ |

Three properties carried over deliberately:

- **`validate_records` is pure.** The three inherently stateful rules — V17, V18
  and V19/V20 — live in `dedup`, `sequence` and `partial`, and their outcomes
  join the same catalogue through `Finding::duplicate`, `Finding::sequence_gap`
  and `Finding::reassembly`. A report speaks one vocabulary rather than four.
- **Order independence.** V16 evaluates against a sorted view and is reported
  **once** for the slice, so a shuffled input yields one finding rather than a
  cascade that buries the rules that matter.
- **Findings point into your slice.** `Finding::record_index` refers to the
  input you passed, not to an internal ordering.

Severity can be tightened but never loosened: `ValidationConfig::escalating`
raises a warning to blocking, and there is no call that does the reverse. The
defaults encode "billing this invoices an amount that cannot be defended", which
is not a local preference.

---

## 🧩 Partial records: reconstruct, never invent

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

---

## 📦 Containers: the list is the data

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

---

## 🔗 Correlation: two failures, opposite in sign

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
# use mediation::{ChargedParty, ChargingId, CorrelationKeys, Octets, RecordType, SourceRef,
#                 SubscriberRef, UsageRecord, VolumePair};
# use std::net::Ipv4Addr;
# fn record(node: &str) -> UsageRecord {
#     let mut r = UsageRecord::new(
#         RecordType::PduSession,
#         ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
#         "2026-07-28T10:00:00+02:00".parse().unwrap(),
#         SourceRef::new(node.parse().unwrap(), 0),
#     ).with_correlation(CorrelationKeys::default()
#         .with_charging_id(ChargingId::new(42))
#         .with_gateway(Ipv4Addr::new(10, 0, 0, 1).into()));
#     r.volume = Some(VolumePair::new(Octets::new(100), Octets::new(900)));
#     r
# }
let records = vec![record("sgw-01"), record("pgw-01")];
let result = correlate(&records, &CorrelationConfig::packet_bearer());

assert_eq!(result.groups().len(), 1);
assert!(result.groups()[0].would_double_count(), "bill one side, not both");
assert!(result.accounts_for(records.len()));
```

**Leg chains.** When a terminating application server diverts a call it
generates a *new* ICID and records the incoming one as a linked ICID
(TS 32.260), so the legs form a chain. Following it reconstructs the history a
forwarded call actually took — and a leg naming a predecessor that never
arrived is `PredecessorAbsent` rather than an origin, because the difference is
the evidence of who placed the call.

---

## 🧾 Corrections are new records, never edits

The past changes constantly: a late partial completes a run already released, a
partner returns records they had already booked, a reclassification arrives
after month-end. Going back and fixing the record **cannot balance** — once a
batch's counts have been reported, editing one of its records invalidates the
report with no way left to reconcile what was released against what is stored.
It also destroys the evidence, and the question at audit is *what did you bill,
and on what basis*.

So `UsageRecord` is immutable, `Disposition` is terminal, and a correction is an
explicit event carrying a **signed delta** — what changes, not what the value
becomes:

```rust
use mediation::{RestateReason, Restatement, RestatementLog, UsageDelta};
# use mediation::RecordKey;
# let key = RecordKey::RecordingEntity {
#     entity: "msc-01".parse().unwrap(), local_sequence: 1 };
let correction = Restatement::new(
    key,
    RestateReason::LatePartial,
    UsageDelta::default().with_duration(300),
    "2026-08-02T09:00:00+02:00".parse().unwrap(),
);

// Withdrawing one is reversing it — and both entries survive.
let log: RestatementLog =
    vec![correction.clone(), correction.reverse()].into_iter().collect();
assert_eq!(log.net(), Some(UsageDelta::ZERO));
assert_eq!(log.len(), 2);
```

`UsageDelta` holds `i128` for octets. A `VolumePair` is unsigned because a
volume cannot be negative — but a *correction* to one can be, and as large as
the original. Narrowing here would silently cap the one number whose whole
purpose is to say how wrong the last one was.

`RestateReason` is a closed set rather than free text, because GSMA TD.57
permits a roaming event to be resubmitted in exactly one case: where it was
returned through the RAP process. A correction nobody can name the basis for is
the one nobody should accept.

`Restatement` refers to its target by `RecordKey` — stable across
retransmission, which is exactly the property a correction needs to find its
record months later.

---

## 🔡 Reading a record as bytes

Decoding octets already in memory is a pure function — it reads no clock,
touches no socket, and given the same bytes returns the same value — so it
belongs here. Fetching those octets does not.

### One record, two BCD conventions

A 3GPP charging record encodes digits **two different ways**, and using either
where the other belongs produces a plausible, wrong value:

| Encoding | Nibble order | Filler | Used for |
|---|---|---|---|
| **TBCD** (TS 29.002) | **swapped** — low nibble first | `0xF` | IMSI, MSISDN, IMEI |
| **Plain BCD** | natural — high nibble first | none | the `TimeStamp` of TS 32.298 |

```rust
use mediation::{DigitString, Msisdn};

// The published TS 29.002 example: "12345" is 21 43 F5.
assert_eq!(DigitString::from_tbcd(&[0x21, 0x43, 0xF5]).unwrap().to_string(), "12345");

// An even-length identity read the wrong way round decodes *cleanly* into a
// different subscriber's number. Nothing anywhere reports an error.
let msisdn: Msisdn = "491701234567".parse().unwrap();
let mut wire = [0u8; 8];
let n = msisdn.to_tbcd(&mut wire).unwrap();
assert_eq!(DigitString::from_bcd(&wire[..n]).unwrap().to_string(), "947110325476");
```

They are two separate calls rather than one with a flag, precisely because a
flag is a thing you can pass wrong.

### The timestamp carries no century

The nine-octet `TimeStamp` is `YYMMDDhhmmssShhmm`: six plain-BCD octets, an
**ASCII** sign octet, then two more BCD octets for the offset. It carries `YY`,
so a decoder must supply the century — there is no correct default, and picking
a pivot silently relocates one record in a hundred years:

```rust
use mediation::{CdrTimestamp, Century};

let octets = [0x26, 0x07, 0x28, 0x10, 0x30, 0x45, b'+', 0x02, 0x00];
let ts = CdrTimestamp::from_bcd(&octets, Century::TWENTY_FIRST).unwrap();

assert_eq!(ts.to_string(), "2026-07-28T10:30:45+02:00");
assert_eq!(ts.to_bcd().unwrap(), octets);
```

Wire decoders apply **the same rules as the string parsers** — an IMEI decoded
from TBCD is Luhn-checked exactly as one parsed from a string, because a
checksum only some code paths apply is a checksum nobody can rely on.
`DecodeError` carries the octet offset rather than a rejected string, since a
caller holding a binary record cannot search it for one.

---

## 🧬 The framing, not the schema

A 3GPP charging record is ASN.1 (X.680) transferred in BER (X.690). This crate
reads the **framing** — tags, lengths, nesting, bounds — and stops there. The
schema is thousands of generated lines that belong beside a generator.

Framing alone is enough to walk a file, find a field by its tag, check that a
record's declared length matches what arrived, and reject a truncated one. The
fields it finds are then decoded by the codecs above:

```rust
use mediation::{BerReader, TagClass};

// A constructed context-specific 0 holding two integers.
let bytes = [0xA0, 0x06, 0x02, 0x01, 0x2A, 0x02, 0x01, 0x07];
let outer = BerReader::new(&bytes).next().unwrap().unwrap();

assert_eq!(outer.tag().class(), TagClass::ContextSpecific);
assert!(outer.tag().is_constructed());

let inner: Vec<_> = outer.children().unwrap().collect::<Result<_, _>>().unwrap();
assert_eq!(inner.len(), 2);
assert_eq!(inner[0].as_u64().unwrap(), 42);
// Offsets point into the original file at every depth, so a finding can be
// taken back to the bytes that produced it.
assert_eq!(inner[0].offset(), 2);
```

**Zero copy.** `BerElement::content()` borrows from the input, so walking a
200 MB record file copies nothing.

**Non-minimal encodings are rejected.** X.690 permits exactly one encoding of
each tag number, length and integer. A long-form length that would fit the short
form, or a padded integer, is a *second* encoding of one value — which gives one
record two hashes and two deduplication keys. It is also the classic
parser-differential vector: two decoders disagreeing about what a record says is
how a validation step gets bypassed.

**Depth is bounded.** Nesting is attacker-controlled in a record that arrived
over a network, so `BerElement::validate(max_depth)` walks iteratively and
refuses to descend past a caller-supplied bound rather than exhausting the
stack.

**A malformed element stops the walk.** `BerReader` yields `Result`, and a
decoder that skips what it could not read produces a record missing exactly the
fields it failed on — which is worse than no record.

---

## 🍰 Slices and data networks

Every 5G PDU session is associated with one DNN and one S-NSSAI (3GPP
TS 23.501), and both reach the record. Losing either makes a data record
unratable under a slice-aware tariff.

**An absent slice differentiator is not a zero one.** The SD reserves
`0xFFFFFF` for *"no SD associated with this SST"*, and some protocols omit the
field instead. Both mean the same thing, and neither means `SD = 0` — which is a
differentiator an operator may genuinely have assigned:

```rust
use mediation::{SliceServiceType, Snssai};

let bare = Snssai::from_bytes(&[0x01]).unwrap();
let reserved = Snssai::from_bytes(&[0x01, 0xFF, 0xFF, 0xFF]).unwrap();
let zero = Snssai::from_bytes(&[0x01, 0x00, 0x00, 0x00]).unwrap();

assert_eq!(bare, reserved);       // the reserved value means "no SD"
assert_ne!(zero, bare);           // …and zero is a real slice
assert_eq!(zero.sd(), Some(0));
```

One meaning, one encoding: a slice with no differentiator is always written
short, never with the reserved value.

`RatType::NonTerrestrial` is its own variant rather than folded into `Nr`,
because a satellite beam crosses borders — so `jurisdiction_is_ambiguous()` can
raise the question that the serving network alone no longer answers. An
unrecorded access type returns `false`: absence is not evidence.

---

## ⚖️ Fair use, and the reading that costs a customer money

Under Roam Like At Home a provider may apply a fair use policy, and Implementing
Regulation (EU) 2016/2286 says how. Three parts of it are easy to get slightly
wrong, all in the same direction.

**The safe harbour is an OR.** *Either* prevailing domestic consumption *or*
prevailing domestic presence is proof of non-abusive use. Reading "presence and
consumption are observed cumulatively" as an AND surcharges lawful customers, so
the two are separate predicates:

```rust
use mediation::{FairUseAssessment, Octets};

// Abroad most of the window, but consuming mostly at home.
let assessment = FairUseAssessment::from_counts(
    /* domestic presence */ 40, /* roaming presence */ 80,
    /* domestic use */ Octets::new(9_000), /* roaming use */ Octets::new(1_000),
    /* window days */ 120,
);

assert!(!assessment.prevailing_domestic_presence());
assert!(assessment.prevailing_domestic_consumption());
assert!(assessment.prevailing_domestic(), "either alone is the safe harbour");
assert!(!assessment.may_warn());
```

**The sequence is observe, warn, surcharge.** `may_surcharge` takes whether a
warning was actually issued rather than inferring it, so surcharging without
having warned is not expressible.

**Presence is not consumption.** A subscriber can be abroad for six weeks and
generate nothing chargeable. Presence comes from registration events (3GPP
TS 32.256), so `assess` takes a presence series alongside the usage history
rather than deriving one from the other.

**Roaming is not the complement of domestic.** RLAH applies within the EEA;
`Region` has three cases and rest-of-world usage is excluded from *both* sides
of the ratio rather than inflating the roaming one.

Ties resolve in the customer's favour, because an equal split does not establish
the abuse a provider would have to show. And `VolumeAllowance` carries the 80 %
alert Regulation (EU) 2022/612 requires — computed by multiplying before
dividing, since `limit / 100 * 80` would alert a 199-octet allowance at 40 %.

---

## 🗃️ Retention: no default, on purpose

The German rule has changed three times in three years:

| When | What |
|---|---|
| TKG 2021 | §§ 175/176 required ten weeks of traffic data, four of location data |
| 14 Aug 2023 | the Bundesverwaltungsgericht held it incompatible with Art. 15(1) of Directive 2002/58/EC, and inapplicable |
| Apr 2026 | after a further CJEU ruling, a new draft § 176 TKG requires access providers to store **IP addresses and timestamps for three months**, explicitly excluding content, destinations, location and browsing behaviour |

A library that had hard-coded any one of those would have been wrong for most of
the period, and wrong in the direction of keeping personal data longer than the
law allowed. So the duration is caller-supplied and the crate contributes the
arithmetic, the anchor and the citation.

**The anchor is not the event date.** § 10 TDDDG permits billing traffic data to
be kept up to six months **after the invoice is dispatched** — not after the
call. Anchoring on the event deletes a record from the start of a billing period
weeks early, while a dispute over that invoice may still be open:

```rust
use mediation::{RetentionBasis, RetentionPolicy};
use time::{Duration, macros::datetime};

let policy = RetentionPolicy::new(
    RetentionBasis::BillingTdddg10 {
        invoice_dispatched_at: datetime!(2026-08-05 00:00 UTC),
    },
    Duration::days(183),
)
.unwrap();

let call = datetime!(2026-07-01 12:00 UTC);
assert!(policy.deadline() > call + Duration::days(183));
```

A record commonly sits under several bases at once, and in a `RetentionSchedule`
the **longest governs** — deleting while any basis still requires the data is
the error that cannot be undone. A record under *no* basis is deletable
immediately: no ground to keep it is not a licence to keep it forever.

---

## 🕶️ Pseudonymised data is still personal data

EDPB Guidelines 01/2025 are explicit, and it is the most consequential
misunderstanding in this area: a pseudonym is reversible by whoever holds the
key, so the output stays within the GDPR's scope. `Pseudonym` is therefore not
named "anonymous" and carries no method suggesting it is.

**The key is yours.** The crate holds no key material and ships no hash — a
pseudonym must be *keyed*, since an unkeyed digest of a 15-digit IMSI is
reversed by enumerating all of them. `Pseudonymiser` takes the primitive you
already use, exactly as `MncWidths` takes the assignment table.

What the crate contributes is the part that is easy to get wrong: which fields
carry identity, and which reductions keep the analytical value without it.

```rust
use mediation::{Imei, Imsi, Reduction};

let imsi: Imsi = "262021234567890".parse().unwrap();
let imei: Imei = "490154203237518".parse().unwrap();

// A country-traffic report needs the country, not the subscriber.
assert_eq!(Reduction::CountryOnly.apply_to_imsi(imsi).to_string(), "262");

// A device-mix report needs the model, not the handset.
assert_eq!(Reduction::ModelOnly.apply_to_imei(imei).to_string(), "49015420");
```

A `Reduction` is **irreversible** — the discarded digits are gone for everyone,
key holder included — which is what makes a reduced dataset genuinely narrower
rather than merely obscured. `NumberPrefix::of` refuses to "truncate" to the
whole number, because returning identifying data from a method named for
truncation is how it reaches a report that believed it was reduced.

---

## ☎️ A number's range says who allocated it, not who terminates it

Number portability broke that inference in every market that introduced it, and
**interconnect settles per terminating operator**. Rating a call on its
allocation range therefore pays the wrong partner — in both directions, by
amounts that surface only when a partner's invoice disagrees with yours.

`terminating_plmn` consults portability first and the allocation range second:

```rust
use mediation::{Msisdn, NumberRangeSet, PlmnId, is_ported, terminating_plmn};

let mut allocation = NumberRangeSet::new();
allocation.insert("49151".parse().unwrap(), "262-01".parse::<PlmnId>().unwrap());

let number: Msisdn = "491519234567".parse().unwrap();

// Nothing ported: the range answers.
let none_ported = |_: Msisdn| None;
assert_eq!(terminating_plmn(number, &allocation, &none_ported).unwrap().to_string(), "262-01");
assert!(!is_ported(number, &allocation, &none_ported));

// Once ported, the range is simply wrong.
let ported = |_: Msisdn| Some("262-02".parse::<PlmnId>().unwrap());
assert_eq!(terminating_plmn(number, &allocation, &ported).unwrap().to_string(), "262-02");
assert!(is_ported(number, &allocation, &ported));
```

**Longest prefix wins**, because number plans nest: `49` is a country, `49151`
is a mobile range inside it, `4915112` may be a block carved out of that. A
lookup returning the first or shortest match answers with the country when the
caller asked about the block.

Neither source resolving gives `None`, never a default — settling a guess is a
two-sided error. And a range inserted twice keeps **both** entries, so
`duplicates()` can report reference data that answers one question twice rather
than letting load order decide.

---

## 🕵️ Fraud signals: evidence, never verdicts

Mediation is where fraud is visible — NRTRDE exists because a roaming extract
reaching the home network within four hours is the difference between catching a
fraud and paying for it.

What the detectors return is an `Indication`: the signal, the records behind it,
and what was measured. **No score**, because the weight of an indicator depends
on an operator's traffic mix, partners and tariffs. **No verdict**, because
suspending a subscriber has legal weight. **No shipped lists**, because
high-risk destination ranges are commercial data that changes weekly.

```rust
use mediation::{Signal, impossible_travel};
# use mediation::{ChargedParty, RecordType, SourceRef, SubscriberRef, UsageRecord};
use time::Duration;
# fn at(opened: &str, serving: &str) -> UsageRecord {
#     UsageRecord::new(
#         RecordType::MobileOriginatedCall,
#         ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
#         opened.parse().unwrap(),
#         SourceRef::new("msc-01".parse().unwrap(), 0),
#     ).with_networks(serving.parse().unwrap(), "262-02".parse().unwrap())
# }

// Germany, then the United States, twenty minutes apart.
let records = vec![
    at("2026-07-28T10:00:00+02:00", "262-02"),
    at("2026-07-28T10:20:00+02:00", "310-260"),
];

let found = impossible_travel(&records, Duration::hours(4));
assert_eq!(found.len(), 1);
assert_eq!(found[0].signal, Signal::ImpossibleTravel);
assert!(found[0].signal.is_arithmetic());
```

`impossible_travel` compares **countries, not cells** — a country change is a
lower bound on distance that needs no geography and no coordinate table — and it
compares **per charged party**, so a whole batch can be passed without one
subscriber's movement being read against another's.

| Signal | Shape |
|---|---|
| `ImpossibleTravel` | Two countries closer together in time than any journey allows |
| `PremiumDestination` | Traffic to a caller-flagged range — the IRSF shape |
| `WangiriPattern` | Many short inbound calls, of the kind meant to provoke a callback |
| `BypassSignature` | Mostly outbound calls, one network — the naive SIM-box shape |
| `AitSignature` | Message traffic concentrated on few destination ranges |

Only `ImpossibleTravel` is `is_arithmetic()`: it is impossible rather than
unusual, and it is the only one a single record justifies acting on.

### What CDR-based detection cannot catch

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

---

## ⏱️ Splitting a session at a tariff boundary

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
# use mediation::{ChargedParty, RecordType, SourceRef, SubscriberRef, UsageRecord};
use time::macros::datetime;
# let record = UsageRecord::new(
#     RecordType::PduSession,
#     ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
#     "2026-07-28T07:00:00+02:00".parse().unwrap(),
#     SourceRef::new("pgw-01".parse().unwrap(), 0),
# )
# .with_closed("2026-07-28T09:00:00+02:00".parse().unwrap())
# .with_volume(VolumePair::new(Octets::new(900), Octets::new(300)));

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

### Apportionment conserves

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

---

## 📍 Where a record was produced

Every charging record carries some form of User Location Information, and the
forms are not interchangeable. They are separate types, so a 2G Cell Global
Identity can never compare equal to a 5G one by accident.

**A cell identity cannot be decomposed without the operator's configuration** —
the same trap as the two-versus-three-digit MNC, in a new place:

| Identity | Total | Base station | Notes |
|---|---|---|---|
| `Eci` | 28 bits | 20 (macro) or 28 (home) | a home eNB consumes the whole value |
| `Nci` | 36 bits | 22 to 32, configured | the remainder is the cell |

```rust
use mediation::{EnbKind, Eci, GnbIdLength, Nci};

let eci = Eci::new(0x00A1_B2C3).unwrap();
assert_eq!(eci.cell_id(EnbKind::Macro), Some(0xC3));
assert_eq!(eci.cell_id(EnbKind::Home), None, "a home eNB has no cell part");

// Eleven configured gNB-ID lengths, eleven different base stations.
let nci = Nci::new(0x0_1234_5678).unwrap();
assert_ne!(
    nci.gnb_id(GnbIdLength::new(22).unwrap()),
    nci.gnb_id(GnbIdLength::new(24).unwrap()),
);
```

Guessing the split would attribute a cell to the wrong base station silently,
which is worse than not answering because the report built on it looks fine.

**A tracking area code carries its width.** It is 16 bits in EPS and 24 in 5GS,
and an EPS code appears in 5GS zero-padded. Storing the number alone would make
the same area compare equal or unequal depending on which way the comparison was
written, so `TrackingAreaCode` carries its width the way `Mnc` carries its digit
count, and `widened()` is the only way across.

`0x0000` and *all ones less one* are reserved at both widths to mean *no valid
area*. They reach a record when an element copies its internal "unknown" marker
into the field, and reading them as ordinary areas files real usage in a
location that does not exist.

---

## 🌐 Which country's rules apply — and which question you asked

Four regimes attach to a record by country: roaming regulation, retention,
lawful interception and tax. **Three of them turn on where the usage happened.
The fourth does not.**

Council Implementing Regulation (EU) 282/2011 Art. 24b(b) presumes a customer
supplied over a mobile network to be located in the country of the **mobile
country code of the IMSI on the SIM card**, and Art. 24f lists that MCC as an
item of evidence. So a German subscriber using data in Spain is governed by
Spanish telecom rules and taxed in Germany. A mediation layer that answers
"country" with one number has answered one of the two questions wrong.

There is therefore no `country()`. There are two functions, each named for its
question:

```rust
use mediation::{Jurisdiction, customer_country_presumption, is_abroad, usage_country};
# use mediation::{ChargedParty, RecordType, SourceRef, SubscriberRef, UsageRecord};
# let roamer = UsageRecord::new(
#     RecordType::PduSession,
#     ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
#     "2026-07-28T10:00:00+02:00".parse().unwrap(),
#     SourceRef::new("pgw-01".parse().unwrap(), 0),
# ).with_networks("214-07".parse().unwrap(), "262-02".parse().unwrap());

// A German SIM, used in Spain.
assert_eq!(usage_country(&roamer), Jurisdiction::Country("214".parse().unwrap()));
assert_eq!(
    customer_country_presumption(&roamer),
    Jurisdiction::Country("262".parse().unwrap()),
);
assert_eq!(is_abroad(&roamer), Some(true));
```

The two agree for domestic traffic, which is exactly why conflating them
survives testing and then bills a whole roaming population wrong.

### Roaming is a network question; abroad is a country question

`UsageRecord::is_roaming` compares networks. `is_abroad` compares countries. A
subscriber national-roaming on a second network at home is **roaming and not
abroad**, and the regulations that turn on being abroad — Roam Like At Home, the
fair-use window — mean abroad. A `Regions` implementation written as
`serving == home` gets this wrong in the direction that costs the customer
money.

### Not knowing is several different things

`Jurisdiction` is not an `Option<Mcc>`, because "no country" has causes with
different remedies:

| Ambiguity | Meaning |
|---|---|
| `NoEvidence` | the record carries nothing to read a country from — fix the feed |
| `ExtraTerritorialAccess` | satellite: the serving network's country is the operator's licence, not the subscriber's position |
| `Conflicting { serving, reported }` | two sources name two countries — a fault, reported as **V21**, never resolved by precedence |

3GPP SA1 opened a study on extra-territorial satellite coverage precisely
because the legal answer is not settled. A library that returned a country there
would be claiming more than the standards body does.

---

## 🗂️ Modules at a glance

| Module | Key types | Purpose |
|---|---|---|
| `error` | `ParseError`, `DecodeError` | One error type per input kind — strings and bytes |
| `digits` | `DigitString` | Inline digits, plus the TBCD and plain-BCD codecs |
| `ber` | `BerReader`, `BerElement`, `BerTag`, `TagClass` | The X.690 framing a charging record arrives in |
| `cdrtime` | `CdrTimestamp`, `QuarterHourOffset`, `DstAdjustment`, `MsTimeZone`, `Seconds` | Local-time-plus-offset arithmetic, DST-boundary detection |
| `volume` | `Octets`, `VolumePair`, `ByteScale` | Integer octets, explicit conversion, explicit rounding primitives |
| `plmn` | `Mcc`, `Mnc`, `PlmnId`, `TadigCode`, `MncWidths` | Network identity; the 2-vs-3-digit MNC problem, handled explicitly |
| `identity` | `Imsi`, `Msisdn`, `Imei`, `Imeisv`, `Iccid`, `Supi`, `Gpsi`, `Pei`, `Suci` | Validated identifiers with check digits, stored inline |
| `location` | `UserLocation`, `Cgi`, `Sai`, `Ecgi`, `Ncgi`, `Tai`, `GnbIdLength` | Where a record was produced, at every access generation |
| `jurisdiction` | `usage_country()`, `customer_country_presumption()`, `is_abroad()`, `Ambiguity` | Which country's rules apply, and which question was asked |
| `numbering` | `NumberRange`, `NumberRangeSet`, `Porting`, `terminating_plmn` | Longest-prefix ranges, and who really terminates a number |
| `party` | `ChargedParty`, `SubscriberRef`, `TenantRef`, `UnattributableReason` | Who the usage is charged to |
| `correlation` | `CorrelationKeys`, `ChargingId`, `Icid`, `PacketSessionId` | The identifiers that tie a record to its session |
| `record` | `UsageRecord`, `RecordType`, `ServiceDomain`, `SourceRef` | The canonical usage type and its provenance |
| `lateness` | `LatenessPolicy`, `Lateness` | One concept behind every window |
| `sequence` | `SequenceSpace`, `SequenceValue`, `SequenceTracker` | Wrap-aware gap detection, RFC 1982 |
| `dedup` | `RecordKey`, `DedupStore`, `DedupWindow`, `NotKeyable` | Natural-key deduplication |
| `disposition` | `Disposition`, `BatchReconciliation` | The conservation law |
| `container` | `ServiceDataContainer`, `ContainerSet`, `RatingGroup`, `ChangeCondition` | Per-rating-group usage inside one record |
| `partial` | `PartialInfo`, `PartialRun`, `Reassembly`, `RecordClosureCause` | Partial-record reassembly |
| `validation` | `validate_records()`, `Rule` (V01–V21), `Finding`, `Severity` | The numbered rule catalogue |
| `session` | `Dnn`, `Snssai`, `SliceServiceType`, `RatType` | The network, slice, access and location a session ran on |
| `timeband` | `split_session()`, `split_measured()`, `apportion()`, `Apportionment` | Dividing usage at a tariff boundary, conservatively |
| `correlate` | `correlate()`, `CorrelatedGroup`, `LegChain`, `Unmatched` | Joining one usage's records across nodes |
| `restatement` | `Restatement`, `UsageDelta`, `RestatementLog` | Corrections as signed, append-only events |
| `aggregation` | `aggregate_released()`, `UsageTotal`, `ReleasedTotals` | What the batch adds up to, with its evidence |
| `roaming` | `TapBatch`, `ScaledAmount`, `RapReturn`, `BatchDiscrepancy` | A partner's batch, and whether it adds up |
| `quality` | `score_batch()`, `QualityGrade`, `ReleaseBlocker` | The release gate |
| `fairuse` | `assess()`, `FairUseAssessment`, `VolumeAllowance`, `Region` | EU 2016/2286 permanent-roaming assessment |
| `fraud` | `Signal`, `Indication`, `TrafficShape`, `CalledParty` | Fraud evidence — never a score, never a verdict |
| `retention` | `RetentionPolicy`, `RetentionSchedule`, `RetentionBasis` | How long a record may be kept, and from when |
| `privacy` | `Pseudonymiser`, `Pseudonym`, `Reduction`, `NumberPrefix` | Narrowing an identity to the question |

---

## 🧱 Design constraints

| Constraint | Detail |
|---|---|
| 🧊 **No I/O** | All inputs are arguments. CI-enforced. |
| ⏱️ **No async** | Synchronous throughout. |
| 🕰️ **No clock** | No clock read anywhere; evaluation instants are parameters. CI-enforced. |
| 🔢 **Exact quantities** | Octets are `u64`, durations `u32`. No float reaches a quantity. CI-enforced. |
| 🎯 **Deterministic** | Same inputs always produce the same output — replayable and cacheable. |
| ♻️ **Idempotent** | Feeding the same record twice yields one billable outcome and one recorded duplicate — never two, never zero. |
| 🧱 **Immutable records** | A `UsageRecord` is never mutated; a correction is a new record. |
| 🔒 **No `unsafe`** | `#![deny(unsafe_code)]`, also checked by the purity job. |
| 🪶 **Small dependency set** | `time`, `thiserror` — `serde` optional. |

### Why no clock

A clock read is ambient state in the same family as I/O: it makes construction
non-deterministic, so two values built from identical inputs are never equal and
no storage layer can write a round-trip test. It also breaks replay — an
archived batch reprocessed years later must yield the verdicts it originally
did, not the verdicts of today. Callers that *have* a clock pass the instant in;
callers replaying an archive pass the archived instant and get the archived
answer back.

---

## 🎛️ Feature flags

| Flag | Default | Effect |
|---|---|---|
| `serde` | ❌ | Derive `Serialize`/`Deserialize` on all public types |

---

## 🔤 String forms — one value, one string

Every type with a string form has **exactly one** of them, shared by `as_str()`,
`Display`, `FromStr` and the `serde` tag. The rule is worth stating because the
alternative fails silently: a value with two spellings produces two database
keys, two map entries and two "distinct" rows that mean the same thing, and
nothing anywhere reports an error.

| Type | Canonical string |
|---|---|
| `QuarterHourOffset` | `+05:45` — always signed, always `±HH:MM` |
| `MsTimeZone` | `+02:00/1` — offset and DST hours |
| `CdrTimestamp` | `2026-07-28T06:00:00+05:45` — RFC 3339, whole seconds |
| `PlmnId` | `262-02` — MNC leading zeros preserved |
| `Octets`, `Seconds` | a decimal integer, no unit suffix |
| `ServiceDomain`, `RecordType` | the `as_str()` code — `DATA`, `PDU_SESSION` |
| `Supi`, `Gpsi`, `Pei` | the 3GPP prefixed form — `imsi-…`, `msisdn-…`, `imei-…` |
| `PacketSessionId` | `1234@10.0.0.1` — the whole identity, never half of it |
| `Disposition`, `RecordKey` | `KIND:detail` — `SUPPRESSED:ZERO_RATED`, `PACKET_SESSION:1234@10.0.0.1/2` |

**Parsing is lenient; writing is not.** `+0200`, `+02` and `Z` all parse;
`+02:00` and `+00:00` are what come out. `+491701234567` and `49-170-1234567`
both parse; `491701234567` is what comes out.

`Octets` deliberately rejects a unit suffix — `"5MB"` would have to guess
between 5 000 000 and 5 242 880. Parse the number and the `ByteScale`
separately.

### Serde representation stability

With `serde` enabled, the emitted representation — enum tags and struct field
names — is **part of the public API and covered by semver**. Persisting these
values means relying on a wire format, so a rename is a breaking change and will
be released as one.
[`tests/serde_representation.rs`](tests/serde_representation.rs) pins every tag
literally, so the commitment is mechanical rather than a promise.

Quantities are JSON numbers and identifiers are JSON strings: an `Octets` is a
count, while a `PlmnId` rendered as a number would lose the leading zero that
distinguishes two networks.

### Enum exhaustiveness

**Domain enums are exhaustive; that is deliberate, and the opposite of the usual
library default.** `#[non_exhaustive]` buys the library freedom to add variants
without a major version and charges the consumer a wildcard arm — which is where
the cost lands. When a new `ServiceDomain` or `ChargedParty` appears, a consumer
mapping this vocabulary onto their own storage codes wants their build to break
so a human decides what it means. With a wildcard they get a silent fallback
instead: usage filed under the wrong service, or attributed to the wrong
counterparty. Adding a variant here is a breaking change and will be released as
one.

---

## 🧪 Testing

```bash
cargo test --all-features
```

| Suite | Covers |
|---|---|
| [`offset_arithmetic.rs`](tests/offset_arithmetic.rs) | Every quarter-hour zone, both 2026 German DST transitions, local-day vs UTC-day bucketing, instant ordering, hand-computed semi-octet vectors |
| [`identity_checkdigits.rs`](tests/identity_checkdigits.rs) | Every single-digit error and adjacent transposition on a reference IMEI — including the known Luhn `09`/`90` blind spot — and IMEISV explicitly *not* checksummed |
| [`string_canonicalisation.rs`](tests/string_canonicalisation.rs) | Stability, idempotence, totality and injectivity under `proptest`, for both the string and the `serde` form |
| [`pipeline.rs`](tests/pipeline.rs) | The whole layer on one batch: sequence tracking, dedup, validation, disposition and the gate, plus determinism and archive replay |
| [`period_and_correlation.rs`](tests/period_and_correlation.rs) | The two seams where usage disappears — between batches and between nodes — plus corrections after a period closed |
| [`wire_codecs.rs`](tests/wire_codecs.rs) | TBCD and plain BCD against the published examples, the nine-octet timestamp over every quarter-hour zone, and slice identities on the wire |
| [`ber_framing.rs`](tests/ber_framing.rs) | Walking a record shaped like a real CDR, against an independently written encoder — then truncation, pathological nesting and non-minimal encodings |
| [`roaming_batch.rs`](tests/roaming_batch.rs) | A partner's batch through every layer at once, and both RAP return classes against the conservation law |
| [`jurisdiction.rs`](tests/jurisdiction.rs) | The regulator and the tax authority in different countries; a national roamer that is roaming and not abroad; each reason a country cannot be determined |
| [`timeband_and_location.rs`](tests/timeband_and_location.rs) | `proptest`: apportionment never creates or destroys usage, for any total and any weights; a session through midnight; measured versus estimated; the cell-identity split that cannot be guessed |
| [`numbering_and_fraud.rs`](tests/numbering_and_fraud.rs) | Settlement against the wrong partner for every ported number, an unresolvable number reported as such, and each fraud signal against the shape it does *not* catch |
| [`regulatory.rs`](tests/regulatory.rs) | The observe-warn-surcharge sequence, the safe harbour under `proptest`, retention anchors and holds, and the reduction/pseudonym distinction |
| [`conservation.rs`](tests/conservation.rs) | The conservation law under `proptest`: every batch balances, in any order, split and recombined |
| [`sequence_and_dedup.rs`](tests/sequence_and_dedup.rs) | Wrap-safe sequence arithmetic across the whole TAP space, and dedup idempotence over any number of copies |
| [`serde_representation.rs`](tests/serde_representation.rs) | Every enum tag and struct field name, pinned literally |
| [`readme_samples.rs`](tests/readme_samples.rs) | Every snippet in this file, so the README cannot drift from the code |

### 🛠️ Dev tasks

A [`justfile`](justfile) wraps the common commands ([just](https://just.systems)):

```bash
just            # list all recipes
just ci         # fmt-check + lint + purity + test + doc + package
just test       # cargo test --all-features
just lint       # clippy with -D warnings, default + all features
just purity     # assert no clock read, no I/O, no unsafe in src/
just no-floats  # assert no float type reaches a quantity
just msrv       # compile on the pinned MSRV
just doc-open   # build the docs and open them
```

---

## 📚 Standards basis

- **3GPP TS 32.298** — CDR parameter description; the nine-octet BCD `TimeStamp`
- **3GPP TS 29.060 §7.7.52 / TS 24.008 / TS 23.040 §9.2.3.11** — MS Time Zone:
  semi-octet quarter-hour offset, DST-adjusted, separate DST field
- **3GPP TS 23.003** — numbering, addressing and identification (IMSI, MSISDN,
  IMEI/IMEISV, SUPI, SUCI, GPSI, PEI); S-NSSAI and DNN structure; CGI, SAI, ECGI
  (28-bit ECI), NCGI (36-bit NCI, 22–32-bit gNB ID) and the TAI, whose code is 16
  bits in EPS and 24 in 5GS
- **3GPP TS 24.008** — the two location-area codes reserved to mean *no valid
  area*, reused at tracking-area width
- **3GPP TS 23.501** — slice/service types, and the 5QI a container runs under
- **3GPP TS 29.002** — TBCD-STRING: swapped nibbles, `0xF` filler
- **3GPP TS 32.251** — packet-switched charging; the Charging ID is unique only together with the P-GW address
- **3GPP TS 32.240** — the first partial CDR of a run is fully qualified; reduced partials carry only what changed
- **3GPP TS 32.260** — IMS charging; the ICID and the linked ICID
- **3GPP TS 32.298** — `CauseForRecClosing`, `ChangeCondition` and `ListOfServiceData`; a container closed on `tariffTimeChange` is the measured alternative to pro-rata apportionment
- **3GPP TS 23.501** — 5QI, the quality class a container's traffic ran under
- **3GPP TS 32.297** — CDR file format and transfer; the per-node file counter
- **3GPP TS 32.254** — Exposure Function Northbound API charging
- **3GPP TS 28.201 / 28.202** — network slice performance and management charging
- **ITU-T E.212** — IMSI, MCC and MNC assignment
- **ITU-T E.164** — international public telecommunication numbering
- **ITU-T E.118** — the SIM card identifier
- **ISO/IEC 7812-1 Annex B** — the Luhn check digit
- **ITU-T X.680 / X.690** — ASN.1 and the Basic Encoding Rules a record is framed in
- **IEC 80000-13** — binary and decimal multiples of the octet
- **GSMA TD.57** — TAP file sequence numbers `00001`…`99999`, the 30-day inclusion deadline, and monetary values as integers scaled by `tapDecimalPlaces`
- **GSMA TD.32** — RAP: a fatal return rejects the file, a severe return rejects individual events
- **Implementing Regulation (EU) 2016/2286** — fair use: a ≥ 4-month observation window, and prevailing domestic presence *or* consumption as the safe harbour
- **Regulation (EU) 2022/612** — Roam Like At Home; the 80 % fair-use alert; in force to 2032
- **Council Implementing Regulation (EU) 282/2011 Art. 24b(b) and 24f** — the customer of a mobile supply is presumed located in the country of the SIM's IMSI MCC, which is not where the usage happened
- **3GPP SA1 study on extra-territorial satellite coverage** — the jurisdiction question a beam crossing borders raises, still open at the standards level
- **§ 10 TDDDG** — billing traffic data, up to six months after the invoice is dispatched
- **§ 176 TKG** — retention; rewritten three times, which is why this crate ships no default
- **GDPR Art. 4(5) and EDPB Guidelines 01/2025** — pseudonymisation, and that its output remains personal data
- **GSMA TD.35** — NRTRDE, the four-hour exchange window; the roaming extract that makes fraud visible before it is paid for
- **ITU-T E.164 / national number plans** — allocation ranges nest, so a lookup resolves by longest prefix
- **GSMA FF.21** — fraud loss survey: IRSF and Wangiri as the dominant revenue-share vectors
- **RFC 1982** — serial-number arithmetic, which is how a sequence wrap is not a gap
- **GSMA TD.57** — resubmission is permitted only where the event was returned through RAP

> ℹ️ This crate implements arithmetic and classification rules derived from the
> sources above. It is **not legal advice** and carries no warranty of
> regulatory compliance — verify against the current text for your use case.

---

## 🤝 Contributing

Issues and pull requests are welcome at
[github.com/hupe1980/mediation](https://github.com/hupe1980/mediation).

Please make sure `just ci` passes before opening a PR. New rules should cite the
clause they implement in a doc comment and come with a test derived from a
published example wherever one exists.

---

## 📄 License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this crate by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
