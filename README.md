# 📡 mediation

[![Crates.io](https://img.shields.io/crates/v/mediation.svg)](https://crates.io/crates/mediation)
[![Docs.rs](https://docs.rs/mediation/badge.svg)](https://docs.rs/mediation)
[![CI](https://github.com/hupe1980/mediation/actions/workflows/ci.yml/badge.svg)](https://github.com/hupe1980/mediation/actions/workflows/ci.yml)
[![MSRV](https://img.shields.io/badge/MSRV-1.94-blue.svg)](https://blog.rust-lang.org/)
[![License](https://img.shields.io/crates/l/mediation.svg)](#-license)

**The arithmetic between a mobile network and a billing system**, as a domain
library: normalising, validating, deduplicating and correlating usage records —
and proving it did not lose any.

> 🧊 **Zero I/O** · ⏱️ **no async** · 🕰️ **no clock** · 🔢 **exact integer quantities** · 🔒 **no `unsafe`**

It computes **usage**, not money. There is no currency, tariff, price or rate
type anywhere in the crate. What leaves here is octets, seconds and event
counts, attributed to a charged party, a network and a service — which a rating
layer then prices.

📖 **[Full documentation →](https://hupe1980.github.io/mediation)** ·
🦀 **[API reference →](https://docs.rs/mediation)**

---

## 📦 Install

```bash
cargo add mediation
```

`serde` is the only feature flag, off by default; it adds
`Serialize`/`Deserialize` on every public type in its canonical string form.

```bash
cargo add mediation --features serde
```

**MSRV** Rust `1.94` (edition 2024), pinned in
[`rust-toolchain.toml`](rust-toolchain.toml) and verified by its own CI lane.
**API stability** pre-`1.0`, so breaking changes ship as minor releases.

---

## ⚖️ The claim

A pipeline moves records. A mediation layer can **prove** it did not lose any:

```text
carried_in + ingested == billable + suppressed + duplicate + rejected + held
```

That equation is a method — `BatchReconciliation::balances()` — and the crate is
built so it holds by construction. A batch that fails it has unknown
completeness, and billing over unknown completeness invoices an amount nobody
can prove is right.

```rust
use mediation::{BatchReconciliation, Disposition, SuppressReason};

let mut counts = BatchReconciliation::EMPTY;
counts.observe(&Disposition::Billable);
counts.observe(&Disposition::Billable);
counts.observe(&Disposition::Suppressed(SuppressReason::TestIdentity));

assert_eq!(counts.ingested(), 3);
assert_eq!(counts.released(), 2);
assert!(counts.balances());
```

Held records come back, so `PeriodLedger` chains batches and the law telescopes
across a whole billing period rather than holding only inside each one.

---

## 🚀 One pass over a batch

Every piece works on its own. `process` is the assembly, and it exists because
the **order** of the pieces is itself a decision that is easy to get wrong
quietly — deduplicating after rejecting reports one fault twice; holding a
partial run after counting it billable double-bills the session when its
successor arrives.

```rust
use mediation::{Batch, DedupWindow, ProcessConfig, ProcessState, process};
use time::{Duration, macros::datetime};
# use mediation::{ChargedParty, ChargingId, CorrelationKeys, Octets, RecordType,
#                 SourceRef, SubscriberRef, UsageRecord, VolumePair};
# use std::net::Ipv4Addr;
# fn session(id: u32) -> UsageRecord {
#     UsageRecord::new(
#         RecordType::PduSession,
#         ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
#         "2026-07-28T10:00:00+02:00".parse().unwrap(),
#         SourceRef::new("pgw-fra-01".parse().unwrap(), 0).with_file_sequence(42),
#     )
#     .with_closed("2026-07-28T10:30:00+02:00".parse().unwrap())
#     .with_volume(VolumePair::new(Octets::new(4_096), Octets::new(65_536)))
#     .with_correlation(CorrelationKeys::default()
#         .with_charging_id(ChargingId::new(id))
#         .with_gateway(Ipv4Addr::new(10, 0, 0, 1).into()))
# }
let records: Vec<UsageRecord> = (1..=5).map(session).collect();

let mut state = ProcessState::new(DedupWindow::from_bound(Duration::days(7)).unwrap());
let outcome = process(
    &Batch::new(&records),
    &ProcessConfig::default(),
    datetime!(2026-07-28 12:00:00 UTC),
    &mut state,
);

assert!(outcome.reconciliation().balances());
assert_eq!(outcome.reconciliation().released(), 5);
assert!(outcome.is_releasable());

// The files to ask a partner to resend, by name rather than by count.
for gap in outcome.sequence_gaps() {
    eprintln!("resend {:?} from {}", gap.missing, gap.node);
}
```

Deduplication and gap detection are inherently stateful, so `ProcessState` is
**caller-owned and passed in**. That keeps `process` a pure mapping from
`(batch, config, as_of, state)` to `(outcome, state′)` — which is what makes a
replay reproducible in 2026 and in 2031. The state round-trips through `serde`,
because a restart that forgot it would re-bill a window of retransmissions and
declare every source complete.

[**The order, and why it is that one →**](https://hupe1980.github.io/mediation/docs/pipeline/)

---

## 🎯 Four things it refuses to get wrong

### 🕰️ Time is local, plus a quarter-hour offset

A charging record carries a **local** wall clock and a numeric offset to UTC
(3GPP TS 32.298), and the offset counts **quarter-hours** because a great many
zones are not on an hour boundary. Reading it as UTC books every evening event
of a UTC+X network onto the wrong day — and unlike a fixed-zone mistake, no
constant fixes it downstream, because the offset varies per record.

```rust
use mediation::CdrTimestamp;

let ts: CdrTimestamp = "2026-07-28T00:30:00+05:45".parse().unwrap();

assert_eq!(ts.local_date().to_string(), "2026-07-28");   // the subscriber's day
assert_eq!(ts.to_utc().date().to_string(), "2026-07-27"); // …is not the UTC day
```

`CdrTimestamp` implements no ordering at all, because comparing two records from
different offsets by their local fields silently interleaves them.
[**More →**](https://hupe1980.github.io/mediation/docs/time/)

### 🔢 Quantities are integers

A data volume is a count, so a float is not merely imprecise but categorically
wrong: above 2^53 octets — about 9.01 PB, which a national monthly aggregate
crosses — consecutive integers are no longer representable. `Octets` is a `u64`,
`Seconds` a `u32`, and CI greps the source for a float.

```rust
use mediation::{ByteScale, Octets, VolumePair};

// Uplink and downlink never merge: asymmetric tariffs and interconnect
// settlement need the split, and it cannot be recovered once summed.
let volume = VolumePair::new(Octets::new(1_000_000), Octets::new(9_000_000));
assert_eq!(volume.checked_total(), Some(Octets::new(10_000_000)));

// "MB" is ambiguous, so it is not a storage unit here — the scale is named.
assert_eq!(Octets::new(1_048_576).units_floor(ByteScale::Mebi), 1);
assert_eq!(Octets::new(1_048_576).units_floor(ByteScale::Mega), 1);
assert_eq!(Octets::new(1_000_000).units_floor(ByteScale::Mebi), 0);
```

Rounding up to whole chargeable units is priced behaviour, so it is offered as a
named primitive and applied by nothing.
[**More →**](https://hupe1980.github.io/mediation/docs/quantities/)

### 🔎 Loss leaves exactly one trace

A lost record file leaves **no trace in the surviving data** — no short total, no
complaint, the usage simply was never billed. The only evidence is a hole in a
per-source sequence number, and the trap is the wrap at `99999 → 00001`, where
three naive checks break once per cycle.

```rust
use mediation::{SequenceSpace, SequenceValue};

let last = SequenceValue::new(SequenceSpace::TAP, 99_999).unwrap();
let next = SequenceValue::new(SequenceSpace::TAP, 1).unwrap();

// RFC 1982: across the wrap the successor is the successor, not a 99 998-file gap.
assert!(last.is_successor(next));
assert_eq!(last.gap_to(next), Some(0));
```

Holes are stored as runs, so a two-billion-file gap costs three numbers and the
count stays exact — and a source that cannot account for itself has its records
**held**, while every other source's release.
[**More →**](https://hupe1980.github.io/mediation/docs/completeness/)

### 🛡️ Every input is untrusted

A mediation layer reads records that arrived over a network from a partner's
equipment, so **no input may panic a parser or a decoder**. Every `FromStr` and
every wire decoder returns a `Result` for arbitrary input, and a property suite
hammers all of them on every build. It has already caught three real defects: two
UTF-8 boundaries and a stack overflow on nested indefinite BER lengths.

Two consequences are visible in the API. Anything sized by the input is
**bounded** — but never at the cost of the count, which stays exact. And nothing
is silently skipped: a decoder that stepped over a field it could not read would
produce a record missing exactly the fields it failed on, which is worse than no
record.
[**More →**](https://hupe1980.github.io/mediation/docs/design/)

---

## 🗂️ What is in it

| Module | Contents |
|---|---|
| `error` | `ParseError`, `DecodeError` — one per input kind, string and bytes |
| `digits` | `DigitString` — inline digits, plus the TBCD and plain-BCD codecs |
| `ber` | `BerReader`, `BerElement`, `BerTag` — the X.690 framing a CDR arrives in |
| `cdrtime` | `CdrTimestamp`, `QuarterHourOffset`, `DstAdjustment`, `MsTimeZone`, `Seconds` |
| `volume` | `Octets`, `VolumePair`, `ByteScale` — integer data volumes |
| `plmn` | `Mcc`, `Mnc`, `PlmnId`, `TadigCode` — network identity and its wire form |
| `identity` | `Imsi`, `Msisdn`, `Imei`, `Imeisv`, `Iccid`, `Supi`, `Gpsi`, `Pei`, `Suci`, `AddressString` |
| `party` | `ChargedParty`, `SubscriberRef`, `TenantRef` — who the usage is charged to |
| `jurisdiction` | `usage_country()`, `customer_country_presumption()` — two questions, two countries |
| `location` | `UserLocation`, `Uli`, `Cgi`, `Sai`, `Rai`, `Lai`, `Ecgi`, `Ncgi`, `Tai` |
| `numbering` | `NumberRange`, `NumberRangeSet`, `Porting`, `terminating_plmn` |
| `session` | `Dnn`, `Snssai`, `RatType`, `ChargingCharacteristics` |
| `correlation` | `CorrelationKeys`, `ChargingId`, `Icid`, `correlate()`, `LegChain` |
| `container` | `ServiceDataContainer`, `ContainerSet`, `RatingGroup`, `ChangeCondition` |
| `record` | `UsageRecord`, `RecordType`, `ServiceDomain`, `SourceRef` |
| `lateness` | `LatenessPolicy`, `Lateness` — one concept behind every window |
| `sequence` | `SequenceSpace`, `SequenceTracker`, `SequenceSnapshot` — wrap-aware gaps |
| `dedup` | `RecordKey`, `DedupStore`, `DedupWindow` — natural-key deduplication |
| `partial` | `PartialInfo`, `PartialRun`, `Reassembly`, `RecordClosureCause` |
| `pipeline` | `process()`, `Batch`, `ProcessState`, `ProcessOutcome` |
| `disposition` | `Disposition`, `Outcome`, `BatchReconciliation`, `PeriodLedger` |
| `validation` | `validate_records()`, `Rule` (V01–V21), `Finding`, `Severity` |
| `quality` | `score_batch()`, `QualityGrade`, `ReleaseBlocker` — the release gate |
| `aggregation` | `aggregate_released()`, `UsageTotal` — what the batch adds up to |
| `timeband` | `split_session()`, `split_measured()`, `apportion()` |
| `restatement` | `Restatement`, `UsageDelta`, `RestatementLog` — corrections as signed events |
| `roaming` | `TapBatch`, `BatchTotals`, `ScaledAmount`, `RapReturn` |
| `fairuse` | `assess()`, `FairUseAssessment`, `FairUseAlert` — EU 2016/2286 |
| `retention` | `RetentionPolicy`, `RetentionSchedule`, `Deletion` |
| `privacy` | `Pseudonymiser`, `Reduction` — narrowing an identity to the question |
| `fraud` | `Signal`, `Indication`, `impossible_travel()` — evidence, never verdicts |

---

## 🚫 What it is not

| Not this | Why | Where it belongs |
|---|---|---|
| A **collector** | GTP′, SFTP pulls, Kafka consumers and Diameter peers are I/O | your ingest service |
| A **rater** | no currency, tariff plan, bundle, discount or balance type exists here | your charging engine |
| An **online charging function** | Ro/Nchf is a live, latency-bound conversation with a CHF | your CHF / OCS |
| A **pipeline framework** | no scheduler, no retry, no queue, no state store | your orchestrator |
| A **rules GUI** | rules are compiled functions with a cited clause, not a scripting runtime | — |
| An **ASN.1 compiler** | the framing is here; the generated TS 32.298 schema is not | a sibling crate |

Parsing is **not** I/O: decoding a record from a `&[u8]` you already hold is a
pure function, so codecs are in scope and transports are not.

---

## 🧪 Testing

```bash
just ci     # fmt-check + clippy + purity + no-floats + test + doc + package
just test   # cargo test --all-features
just site   # build the documentation site (needs `zola`)
```

Three properties get `proptest` attention because they are the crate's promises:
**determinism** (same input, same output, across shuffles), **conservation**
(nothing is created or destroyed), and **totality** (nothing panics, at any
input).

Every Rust block in this README **and on the documentation site** is compiled and
run as a doctest — see `ReadmeDoctests` and `site_doctests` in
[`src/lib.rs`](src/lib.rs). Documentation that claims to be tested and is not is
worse than documentation that claims nothing, because a reader trusts it.

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

> ℹ️ This crate implements arithmetic and classification rules derived from
> published standards and regulations. It is **not legal advice** and carries no
> warranty of regulatory compliance — verify against the current text for your
> use case.
