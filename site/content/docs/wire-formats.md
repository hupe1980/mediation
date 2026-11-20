+++
title = "Wire formats"
description = "The X.690 framing a CDR arrives in, and the BCD and TBCD codecs for its fields."
weight = 160
+++

## Reading a record as bytes

Decoding octets already in memory is a pure function: no clock, no I/O.

### One record, two BCD conventions

| Encoding | Nibble order | Filler | Used for |
|---|---|---|---|
| **TBCD** (TS 29.002) | low nibble first | `0xF` | IMSI, MSISDN, IMEI |
| **Plain BCD** | high nibble first | none | the TS 32.298 `TimeStamp` |

Reading one as the other yields a clean, wrong number.

```rust
use mediation::{DigitString, Msisdn};

// TS 29.002: "12345" is 21 43 F5.
assert_eq!(DigitString::from_tbcd(&[0x21, 0x43, 0xF5]).unwrap().to_string(), "12345");

// Read as plain BCD, the same octets are another subscriber's number.
let msisdn: Msisdn = "491701234567".parse().unwrap();
let mut wire = [0u8; 8];
let n = msisdn.to_tbcd(&mut wire).unwrap();
assert_eq!(DigitString::from_bcd(&wire[..n]).unwrap().to_string(), "947110325476");
```

### The timestamp carries no century

The nine-octet `TimeStamp` is `YYMMDDhhmmss`, an ASCII sign, and `hhmm`. The
caller supplies the century; there is no default pivot.

```rust
use mediation::{CdrTimestamp, Century};

let octets = [0x26, 0x07, 0x28, 0x10, 0x30, 0x45, b'+', 0x02, 0x00];
let ts = CdrTimestamp::from_bcd(&octets, Century::TWENTY_FIRST).unwrap();

assert_eq!(ts.to_string(), "2026-07-28T10:30:45+02:00");
assert_eq!(ts.to_bcd(Century::TWENTY_FIRST).unwrap(), octets);
```

### A `servedMSISDN` is not a bare TBCD field

In an `ISDN-AddressString` (TS 29.002 §17.7.8) the first octet holds the nature
of address and numbering plan. Read as TBCD, it prepends two digits.

```rust
use mediation::{AddressString, Msisdn, NatureOfAddress, NumberingPlanIndicator};

// 0x91 = extension bit, international, E.164; then the TBCD digits.
let field = [0x91, 0x94, 0x71, 0x10, 0x32, 0x54, 0x76];

let address = AddressString::from_bytes(&field).unwrap();
assert_eq!(address.nature(), NatureOfAddress::International);
assert_eq!(address.plan(), NumberingPlanIndicator::Isdn);
assert_eq!(address.to_msisdn().unwrap().to_string(), "491701234567");

// The wrong reading is not an error.
assert_eq!(Msisdn::from_tbcd(&field).unwrap().to_string(), "19491701234567");
```

`to_msisdn` returns `None` for a non-international number.

### The PLMN octets say how wide the MNC is

In the three-octet PLMN identity (TS 24.008 §10.5.1.13), `0xF` in the third MNC
digit means a two-digit MNC.

```rust
use mediation::PlmnId;

let telekom = PlmnId::from_bcd(&[0x62, 0xF2, 0x20]).unwrap();
assert_eq!(telekom.to_string(), "262-02");

let tmobile = PlmnId::from_bcd(&[0x13, 0x00, 0x62]).unwrap();
assert_eq!(tmobile.to_string(), "310-260");
```

### The IMEI field carries no check digit

The eight-octet TBCD `IMEI` (TS 29.002) holds 14 digits, a spare `0` and a
filler. `Imei::from_tbcd` computes the Luhn check digit; a non-zero 15th digit
is verified as a transmitted check digit.

```rust
use mediation::Imei;

let imei: Imei = "490154203237518".parse().unwrap();
let mut wire = [0u8; 8];
imei.to_tbcd(&mut wire).unwrap();
assert_eq!(Imei::from_tbcd(&wire).unwrap(), imei);
```

### A TAP timestamp is two fields

TAP3 carries local time (`YYYYMMDDhhmmss`) and the offset (`±hhmm`) separately.
There is no constructor for the local half alone.

```rust
use mediation::CdrTimestamp;

let ts = CdrTimestamp::from_tap("20260728103045", "+0545").unwrap();
assert_eq!(ts.to_string(), "2026-07-28T10:30:45+05:45");
assert_eq!(ts.to_tap(), ("20260728103045".to_owned(), "+0545".to_owned()));
```

Wire decoders validate like the string parsers; `DecodeError` carries the octet
offset.


## The framing, not the schema

Charging records are ASN.1 in BER (X.690). The crate reads the framing (tags,
lengths, nesting, bounds); the schema belongs beside a generator.

```rust
use mediation::{BerReader, TagClass};

// A constructed context-specific 0 holding two integers.
let bytes = [0xA0, 0x06, 0x02, 0x01, 0x2A, 0x02, 0x01, 0x07];
let outer = BerReader::new(&bytes).next().unwrap().unwrap();

assert_eq!(outer.tag().class(), TagClass::ContextSpecific);
assert!(outer.tag().is_constructed());

let inner: Vec<_> = outer.children().unwrap().collect::<Result<_, _>>().unwrap();
assert_eq!(inner[0].as_u64().unwrap(), 42);
assert_eq!(inner[0].offset(), 2); // offsets point into the original input
```

| Rule | Why |
|---|---|
| `BerElement::content()` borrows | Zero copy |
| Long-form lengths accepted (X.690 §8.1.3.5) | BER permits them |
| Padded tag numbers and integers refused | A second encoding of one value is a parser-differential vector |
| `BerElement::validate(max_depth)` walks iteratively | Attacker-controlled nesting cannot exhaust the stack |
| A malformed element stops the walk | Skipping it silently drops fields |
