+++
title = "Identity"
description = "IMSI, MSISDN, IMEI, ICCID, SUPI, SUCI and the rest — validated, check-digited, and never split without the table that says where to split."
weight = 40
+++

## The identifier types

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

It has seven fields in one order, and TS 23.003 §2.2B fixes which:

```rust
use mediation::Suci;

let suci: Suci = "suci-0-262-02-0678-1-27-0072b7d4c1e8".parse().unwrap();
assert_eq!(suci.home_network.to_string(), "262-02");
assert_eq!(suci.routing_indicator.to_string(), "0678");  // leading zeros matter
assert_eq!(suci.protection_scheme, 1);                   // then the scheme …
assert_eq!(suci.public_key_id, 27);                      // … then the key
assert_eq!(suci.to_string(), suci.to_string());
```

The routing indicator is easy to leave out and the two identifiers are easy to
swap — both are small integers that look alike in a debug print — and a value
spelled either way round is rejected by every 5G core it is handed to, only at
the point of use.


## The charged party is not always a subscriber

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


## String forms: one value, one string

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

`ByteScale` also refuses the **bit** spellings. IEC 80000-13 gives `B` to the
byte and `b` to the bit, so `kb` is a kilobit — and reading it as a kilobyte is
a factor-of-eight error in exactly the direction this crate exists to prevent.
`KB` is accepted as `kB`, because that capitalisation is ubiquitous and is not a
bit spelling; the binary kilobyte is `KiB`, which differs by more than case.

```rust
use mediation::ByteScale;

assert_eq!("kB".parse::<ByteScale>().unwrap().factor(), 1_000);
assert_eq!("KB".parse::<ByteScale>().unwrap().factor(), 1_000);
assert_eq!("KiB".parse::<ByteScale>().unwrap().factor(), 1_024);
assert!("kb".parse::<ByteScale>().is_err(), "a kilobit is not a kilobyte");
```

### Serde representation stability

With `serde` enabled, the emitted representation — enum tags and struct field
names — is **part of the public API and covered by semver**. Persisting these
values means relying on a wire format, so a rename is a breaking change and will
be released as one.
[`tests/serde_representation.rs`](https://github.com/hupe1980/mediation/blob/main/tests/serde_representation.rs) pins every tag
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
