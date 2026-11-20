+++
title = "Where a record was produced"
description = "Cells, tracking areas, the User Location Information element, slices and charging characteristics."
weight = 50
+++

## Cells and areas

Each identity is its own type, so a 2G CGI never compares equal to a 5G one.
A cell identity cannot be split into base station and cell without the
operator's configuration; guessing attributes the cell to the wrong site.

| Identity | Bits | Base-station part |
|---|---|---|
| `Eci` | 28 | 20 (macro) or 28 (home) |
| `Nci` | 36 | 22 to 32, configured |

```rust
use mediation::{EnbKind, Eci, GnbIdLength, Nci};

let eci = Eci::new(0x00A1_B2C3).unwrap();
assert_eq!(eci.cell_id(EnbKind::Macro), Some(0xC3));
assert_eq!(eci.cell_id(EnbKind::Home), None);

let nci = Nci::new(0x0_1234_5678).unwrap();
assert_ne!(
    nci.gnb_id(GnbIdLength::new(22).unwrap()),
    nci.gnb_id(GnbIdLength::new(24).unwrap()),
);
```

A `TrackingAreaCode` carries its width (16 bits EPS, 24 bits 5GS); `widened()`
is the only conversion. The reserved values `0x0000` and *all ones less one*
mean "no valid area" and are refused by the decoders.

## The ULI element is a set

TS 29.274 §8.21 User Location Information is a flag octet followed by every
identity the flags select, commonly an ECGI and its TAI. A decoder that keeps
one picks whichever it checked first, and two sides of a reconciliation
disagree. Field widths differ (an RAI is 7 octets there, padded), and one wrong
width shifts every later field into a plausible cell in the wrong country.

```rust
use mediation::{Tai, TrackingAreaCode, Uli, UserLocation};

// Flags 0x18: TAI (five octets) then ECGI (seven).
let field = [
    0x18,
    0x62, 0xF2, 0x20, 0x00, 0x2A,
    0x62, 0xF2, 0x20, 0x00, 0xA1, 0xB2, 0xC3,
];
let uli = Uli::decode(&field).unwrap();
assert!(uli.tai.is_some() && uli.ecgi.is_some());
assert_eq!(uli.mcc().unwrap().unwrap().to_string(), "262");
assert!(matches!(uli.most_precise(), Some(UserLocation::Ecgi(_))));

// A 24-bit 5GS TAC does not fit the element's TAI field: refused, not shifted.
let plmn = "262-02".parse().unwrap();
let five_gs = Uli { tai: Some(Tai::new(plmn, TrackingAreaCode::five_gs(0x2A).unwrap())), ..Uli::new() };
assert!(five_gs.encode(&mut [0u8; 16]).is_err());
```

`Uli::mcc` returns `Ok(None)` for an empty element, `Err` when two identities
name different countries, and the country otherwise. A conflict is a fault, as
in [`usage_country`](@/docs/regulatory.md).

Locations render and parse with a tag, because `262-02-1A2B` is both a location
area and an EPS tracking area:

```rust
use mediation::UserLocation;

let cell: UserLocation = "ecgi:262-02-0A1B2C3".parse().unwrap();
assert_eq!(cell.to_string(), "ecgi:262-02-0A1B2C3");
assert!(cell.is_cell());
assert!("262-02-1A2B".parse::<UserLocation>().is_err());
```

## Slices and data networks

Every 5G PDU session has one DNN and one S-NSSAI (TS 23.501); losing either
makes a record unratable under a slice-aware tariff. An absent slice
differentiator and the reserved `0xFFFFFF` both mean "no SD"; `SD = 0` is a
real slice.

```rust
use mediation::Snssai;

let bare = Snssai::from_bytes(&[0x01]).unwrap();
let reserved = Snssai::from_bytes(&[0x01, 0xFF, 0xFF, 0xFF]).unwrap();
let zero = Snssai::from_bytes(&[0x01, 0x00, 0x00, 0x00]).unwrap();

assert_eq!(bare, reserved);
assert_ne!(zero, bare);
assert_eq!(zero.sd(), Some(0));
```

## Charging characteristics

TS 32.251 §A.1 defines the 16 behaviour bits as operator-defined, so
`ChargingCharacteristics` carries, compares and renders them and interprets
none: `0800` is prepaid at one operator and a lab profile at the next.

```rust
use mediation::ChargingCharacteristics;

let profile: ChargingCharacteristics = "0800".parse().unwrap();
assert!(profile.behaviour(11));
assert_eq!(profile.set_behaviours().collect::<Vec<_>>(), vec![11]);
assert!(ChargingCharacteristics::NONE.is_empty());
```

`RatType::NonTerrestrial` is separate from `Nr` because a satellite beam
crosses borders; `jurisdiction_is_ambiguous()` returns `true` for it and
`false` for an unrecorded access type.
