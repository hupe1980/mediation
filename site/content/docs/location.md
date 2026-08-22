+++
title = "Where a record was produced"
description = "Cell, service area, tracking area and the User Location Information element that carries them — at every access generation, on the wire and as a string."
weight = 50
+++

## Cells, areas and the ULI element

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
location that does not exist — so the decoders refuse them.

### A ULI is a set, not a location

The User Location Information element of TS 29.274 §8.21 is a flag octet
followed by whichever identities the flags select, and a network commonly
reports **more than one** — an ECGI and the TAI it sits in, or a CGI and its
RAI. Modelling it as a single value forces a decoder to pick one, and the
natural pick is whichever field it checked first, which varies between decoders
and therefore between the two sides of a reconciliation.

```rust
use mediation::{Uli, UserLocation};

// Flags 0x18: TAI and ECGI, which is what an LTE record usually carries.
let field = [
    0x18,
    0x62, 0xF2, 0x20, 0x00, 0x2A,               // TAI first — five octets
    0x62, 0xF2, 0x20, 0x00, 0xA1, 0xB2, 0xC3,   // then ECGI — seven
];

let uli = Uli::decode(&field).unwrap();
assert!(uli.tai.is_some() && uli.ecgi.is_some());
assert_eq!(uli.mcc().unwrap().unwrap().to_string(), "262");

// A cell is more precise than the tracking area containing it.
assert!(matches!(uli.most_precise(), Some(UserLocation::Ecgi(_))));
```

The widths are the trap. TS 29.274 fixes the order — "CGI, SAI, RAI, TAI, ECGI,
LAI", with the two base-station identifiers appended after them — and each is a
different size: 7 octets for a CGI, 7 for an SAI, **7 for an RAI** (the
specification pads the one-octet routing area code with a spare octet, which no
other specification carrying an RAI does), 5 for an EPS TAI, 7 for an ECGI, 5
for an LAI. A decoder that gets one wrong does not fail: it reads the *next*
identity's PLMN octets as this one's location area and produces a plausible cell
in the wrong country.

Which is also why the **encoder refuses a 5GS tracking area**. The element's TAI
field is five octets of 16-bit EPS code; a 24-bit 5GS code does not fit it, and
writing six octets anyway would shift every field after the TAI by one and put
exactly that plausible-wrong-cell into a partner's hands. It is rejected rather
than corrupted:

```rust
use mediation::{Tai, TrackingAreaCode, Uli};
let plmn = "262-02".parse().unwrap();
let five_gs = Uli { tai: Some(Tai::new(plmn, TrackingAreaCode::five_gs(0x2A).unwrap())), ..Uli::new() };
let eps     = Uli { tai: Some(Tai::new(plmn, TrackingAreaCode::eps(0x2A).unwrap())),     ..Uli::new() };

let mut out = [0u8; 16];
assert!(five_gs.encode(&mut out).is_err());

let written = eps.encode(&mut out).unwrap();
assert_eq!(Uli::decode(&out[..written]).unwrap(), eps);
```

`Uli::mcc` has three answers, not two: `Ok(None)` when the element carried
nothing, `Err` when two identities name different countries, and the country
otherwise. A conflict is a fault, not a tie to break — the same stance
[`usage_country`](@/docs/regulatory.md)
takes — and collapsing it into the same `None` as "nothing there" would send
somebody looking for a missing field when the problem is a contradictory one.

Every location type also parses back from the string it renders, tagged, because
`262-02-1A2B` is both a location area and an EPS tracking area and nothing in
the digits says which:

```rust
use mediation::UserLocation;

let cell: UserLocation = "ecgi:262-02-0A1B2C3".parse().unwrap();
assert_eq!(cell.to_string(), "ecgi:262-02-0A1B2C3");
assert!(cell.is_cell());

let area: UserLocation = "tai:262-02-002A".parse().unwrap();
assert!(!area.is_cell(), "a tracking area is an area, not a cell");

// The tag is required, because the untagged forms collide.
assert!("262-02-1A2B".parse::<UserLocation>().is_err());
```


## Slices and data networks

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

### The charging profile the session ran under

The HSS supplies `chargingCharacteristics` with the subscription, the SGSN or
MME forwards it at IP-CAN bearer activation, and it reaches every CDR the
session produces. It is what decided whether the record exists at all, at what
time and volume limits, and which gateway it went to — so a mediation layer that
drops it has thrown away the record's own account of why it is there.

**Sixteen bits, and the crate reads none of them.** TS 32.251 §A.1 is explicit:
the parameter is "a string of 16 bits designated as Behaviours (B), *freely
defined by Operators*", each pointing at a behaviour "defined on a per operator
basis". `0800` is prepaid at one operator and a lab profile at the next, and the
specification's own tables are labelled informative examples. So the type
carries the bits, compares them and renders them — and has no `is_prepaid`,
exactly as `Mnc` has no width and `Region` has no EEA list:

```rust
use mediation::ChargingCharacteristics;

let profile: ChargingCharacteristics = "0800".parse().unwrap();
assert_eq!(profile.to_bytes(), [0x08, 0x00]);

// Behaviour 11 is set. What behaviour 11 *means* is the operator's.
assert!(profile.behaviour(11));
assert_eq!(profile.set_behaviours().collect::<Vec<_>>(), vec![11]);

// Nothing selected is not the same as no field at all — the record keeps that
// difference in an `Option`.
assert!(ChargingCharacteristics::NONE.is_empty());
```

`RatType::NonTerrestrial` is its own variant rather than folded into `Nr`,
because a satellite beam crosses borders — so `jurisdiction_is_ambiguous()` can
raise the question that the serving network alone no longer answers. An
unrecorded access type returns `false`: absence is not evidence.
