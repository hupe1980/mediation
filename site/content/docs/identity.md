+++
title = "Identity"
description = "Subscriber and equipment identifiers, validated and check-digited, never split without a table."
weight = 40
+++

## The identifier types

| Type | Standard | Form | Check digit |
|---|---|---|---|
| `Imsi` | ITU-T E.212 | 6–15 digits | — |
| `Msisdn` | ITU-T E.164 | 1–15 digits | — |
| `Imei` | 3GPP TS 23.003 | 15 digits | Luhn |
| `Imeisv` | 3GPP TS 23.003 | 16 digits | none |
| `Iccid` | ITU-T E.118 | 19–20 digits | Luhn, when present |
| `Nai` | RFC 7542 | `user@realm`, at most 253 octets | — |
| `Supi` / `Gpsi` / `Pei` | 3GPP TS 23.003 | prefixed forms | inherits |
| `Suci` | 3GPP TS 23.003 | concealed | — |

`Supi` covers the four forms of TS 23.003 §2.2A (IMSI, network-specific NAI,
Global Line, Global Cable), so a fixed-line subscription needs no IMSI; see
[Fixed-line voice](@/docs/fixed-voice.md). Numeric identifiers store packed
digits inline and are `Copy`.

### An IMSI cannot be split without a table

An IMSI is MCC (3 digits) + MNC (2 **or** 3) + MSIN, and the digits do not say
which. A fixed split is still well-formed, so the error is silent.

```rust
use mediation::{Imsi, Mcc, MncWidth, NorthAmericanRule};

let de: Imsi = "262021234567890".parse().unwrap();
let us: Imsi = "310260123456789".parse().unwrap();

assert_eq!(de.mcc(), Mcc::GERMANY);
assert_eq!(de.plmn(&NorthAmericanRule).unwrap().to_string(), "262-02");
assert_eq!(us.plmn(&NorthAmericanRule).unwrap().to_string(), "310-260");

// Your own table is a closure. An unknown country yields no network.
let table = |mcc: Mcc| (mcc == Mcc::GERMANY).then_some(MncWidth::Two);
assert_eq!(de.plmn(&table).unwrap().to_string(), "262-02");
assert_eq!(us.plmn(&table), None);
```

### An MNC is a string, not a number

As integers, MNC `01` and `001` collapse into one value and merge two networks.

```rust
use mediation::{Mnc, PlmnId};

let telekom: PlmnId = "262-01".parse().unwrap();
assert_eq!(telekom.to_string(), "262-01");
assert_ne!(Mnc::new(1, 2).unwrap(), Mnc::new(1, 3).unwrap());
assert_eq!(Mnc::new(1, 3).unwrap().to_string(), "001");
```

### IMEI is Luhn-checked and IMEISV is not

The IMEI check-digit position is the first software-version digit in an
IMEISV, so they are separate types. The eight-octet wire IMEI carries a spare
`0` instead of the check digit; `Imei::from_tbcd` computes it (see
[Wire formats](@/docs/wire-formats.md)).

```rust
use mediation::{Imei, Imeisv, Pei};

assert!("490154203237518".parse::<Imei>().is_ok());
assert!("490154203237581".parse::<Imei>().is_err()); // transposition

let sv: Imeisv = "4901542032375120".parse().unwrap();
assert_eq!(sv.software_version().to_string(), "20");

// Both forms resolve to one device.
let a: Pei = "imei-490154203237518".parse().unwrap();
let b: Pei = "imeisv-4901542032375120".parse().unwrap();
assert_ne!(a, b);
assert_eq!(a.to_imei(), b.to_imei());
```

### A SUCI is not an identity you can count

A SUCI differs between registrations of one subscriber, so `Suci` implements
neither `Hash` nor `Ord` and cannot become a dedup or map key. A record carrying
only a SUCI is unattributable. Fields are private; read them through accessors.
The protection scheme is one hex digit, the key identifier decimal.

```rust
use mediation::Suci;

let suci: Suci = "suci-0-262-02-0678-1-27-0072b7d4c1e8".parse().unwrap();
assert_eq!(suci.home_network().to_string(), "262-02");
assert_eq!(suci.routing_indicator().to_string(), "0678");
assert_eq!(suci.protection_scheme(), 1);
assert_eq!(suci.public_key_id(), 27);
assert_eq!(suci.scheme_output(), "0072b7d4c1e8");
```


## The charged party is not always a subscriber

- **Network APIs** (TS 32.254): an invocation has a paying developer and a
  subject subscriber.
- **Slice management** (TS 28.201, TS 28.202): the charged party is an
  enterprise tenant; there is no subscriber.

```rust
use mediation::{ChargedParty, SubscriberRef, TenantRef, UnattributableReason};

let api = ChargedParty::ApiConsumer {
    developer: "acme-dev".parse().unwrap(),
    subject: Some(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
};
assert_eq!(api.kind(), "API_CONSUMER");
assert!(api.subscriber().is_some());

let slice = ChargedParty::Tenant(TenantRef::new("enterprise-7".parse().unwrap()));
assert!(slice.subscriber().is_none());

// An unresolved party carries a reason and is counted.
let lost = ChargedParty::Unattributable(UnattributableReason::ConcealedIdentityOnly);
assert_eq!(lost.to_string(), "UNATTRIBUTABLE:CONCEALED_IDENTITY_ONLY");
```

`SubscriberRef` carries the tenant, so reconciliation, retention and fair use
can work per tenant. `None` means unresolved, not the host network.


## String forms: one value, one string

Each type has exactly one string form, shared by `as_str()`, `Display`,
`FromStr` and the serde tag. Two spellings of one value would make two keys.

| Type | Canonical string |
|---|---|
| `QuarterHourOffset` | `+05:45` |
| `MsTimeZone` | `+02:00/1` (offset / DST hours) |
| `CdrTimestamp` | `2026-07-28T06:00:00+05:45` |
| `PlmnId` | `262-02` |
| `Octets`, `Seconds` | decimal integer, no unit |
| `ServiceDomain`, `RecordType` | `DATA`, `PDU_SESSION` |
| `Supi`, `Gpsi`, `Pei` | `imsi-…`, `nai-…`, `gli-…`, `gci-…`, `msisdn-…`, `imei-…` |
| `PacketSessionId` | `1234@10.0.0.1` |
| `Disposition`, `RecordKey` | `SUPPRESSED:ZERO_RATED`, `PACKET_SESSION:1234@10.0.0.1/2` |

Parsing is lenient, writing is not: `+0200` and `+02` parse, `+02:00` is
written. `Z` and `-00:00` are refused because they mean "offset unknown"
(RFC 9557 §2.2); UTC is `+00:00`. `Octets` rejects unit suffixes; parse the
`ByteScale` separately. `ByteScale` refuses `KB` (ambiguous) and bit spellings.

```rust
use mediation::ByteScale;

assert_eq!("kB".parse::<ByteScale>().unwrap().factor(), 1_000);
assert_eq!("KiB".parse::<ByteScale>().unwrap().factor(), 1_024);
assert!("KB".parse::<ByteScale>().is_err());
assert!("kb".parse::<ByteScale>().is_err());
```

### Serde representation stability

With `serde`, enum tags and field names are public API under semver;
[`tests/serde_representation.rs`](https://github.com/hupe1980/mediation/blob/main/tests/serde_representation.rs)
pins them. Quantities serialize as JSON numbers, identifiers as strings.

### Enum exhaustiveness

Domain enums are not `#[non_exhaustive]`. A new `ServiceDomain` or
`ChargedParty` variant breaks consumer builds so a human maps it, rather than
falling into a wildcard arm. Adding a variant is a breaking change.
