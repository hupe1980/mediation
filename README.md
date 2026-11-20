# 📡 mediation

[![Crates.io](https://img.shields.io/crates/v/mediation.svg)](https://crates.io/crates/mediation)
[![Docs.rs](https://docs.rs/mediation/badge.svg)](https://docs.rs/mediation)
[![CI](https://github.com/hupe1980/mediation/actions/workflows/ci.yml/badge.svg)](https://github.com/hupe1980/mediation/actions/workflows/ci.yml)
[![MSRV](https://img.shields.io/badge/MSRV-1.94-blue.svg)](https://blog.rust-lang.org/)
[![License](https://img.shields.io/crates/l/mediation.svg)](#-license)

**The arithmetic between a telecom network — mobile or fixed — and a billing
system**: normalising, validating, deduplicating and correlating usage records,
and proving none was lost.

> 🧊 **Zero I/O** · ⏱️ **no async** · 🕰️ **no clock** · 🔢 **exact integer quantities** · 🔒 **no `unsafe`**

It computes **usage**, not money: octets, seconds and event counts, attributed
to a charged party, a network and a service. Pricing belongs to the rating
layer.

📖 **[Documentation](https://hupe1980.github.io/mediation)** ·
🦀 **[API reference](https://docs.rs/mediation)**

## 📦 Install

```bash
cargo add mediation                  # or: --features serde
```

`serde` (off by default) adds `Serialize`/`Deserialize` in each type's canonical
string form. MSRV is Rust 1.94 (edition 2024). Pre-1.0: breaking changes ship as
minor releases.

## 🚀 One pass over a batch

`process` deduplicates, reassembles, checks sequence gaps, validates and
disposes every record, and returns a reconciliation that must balance:

```text
carried_in + ingested == billable + suppressed + duplicate + rejected + held
```

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
```

`ProcessState` (dedup keys, sequence high-water marks, held records) is owned by
the caller and serialisable, so `process` stays a pure function of
`(batch, config, as_of, state)` and a replay gives the same verdicts.

## 📚 Topics

| Page | Failure it prevents |
|---|---|
| [Time](https://hupe1980.github.io/mediation/docs/time/) | Local CDR time read as UTC books events on the wrong day |
| [Quantities](https://hupe1980.github.io/mediation/docs/quantities/) | Float volumes lose octets; `MB` means two things |
| [Identity](https://hupe1980.github.io/mediation/docs/identity/) | Malformed IMSI/MSISDN/IMEI reaching billing |
| [Fixed-line voice](https://hupe1980.github.io/mediation/docs/fixed-voice/) | Dial strings that are not E.164 numbers |
| [Completeness](https://hupe1980.github.io/mediation/docs/completeness/) | A lost file leaves no trace except a sequence hole |
| [Deduplication](https://hupe1980.github.io/mediation/docs/deduplication/) | Retransmissions billed twice |
| [Conservation](https://hupe1980.github.io/mediation/docs/conservation/) | A record lost between ingest and release |
| [Validation](https://hupe1980.github.io/mediation/docs/validation/) | The rule catalogue V01–V21 and the release gate |
| [Regulatory](https://hupe1980.github.io/mediation/docs/regulatory/) | Jurisdiction, EU fair use, retention, itemised bills |

## 🚫 Out of scope

Collection and transport (GTP′, SFTP, Kafka, Diameter), rating and pricing,
online charging (Ro/Nchf), scheduling and storage, and generated ASN.1 schemas.
Decoding bytes already in memory is in scope; fetching them is not.

## 🧪 Development

```bash
just ci     # fmt, clippy, purity, no-floats, tests, docs, wasm, site check, package
```

Every Rust block in this README and on the documentation site runs as a doctest
(`ReadmeDoctests` and `site_doctests` in [`src/lib.rs`](src/lib.rs)). New rules
cite the clause they implement and come with a test.

## 📄 License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Unless you explicitly state
otherwise, any contribution intentionally submitted for inclusion in this crate
by you, as defined in the Apache-2.0 license, shall be dual licensed as above,
without any additional terms or conditions.

> ℹ️ Rules derived from published standards and regulations. Not legal advice;
> verify against the current text for your use case.
