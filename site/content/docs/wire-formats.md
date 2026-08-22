+++
title = "Wire formats"
description = "The X.690 framing a CDR arrives in, the two BCD conventions that share a record, and why the schema belongs beside a generator."
weight = 160
+++

## Reading a record as bytes

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
assert_eq!(ts.to_bcd(Century::TWENTY_FIRST).unwrap(), octets);
```

### A `servedMSISDN` is not a bare TBCD field

The number fields of a charging record are `ISDN-AddressString`s (TS 29.002
§17.7.8), and the first octet is **not** a digit pair — it carries an extension
bit, a nature-of-address indicator and a numbering-plan indicator.

Reading those octets as raw TBCD prepends two digits to the number. The result
is a well-formed MSISDN belonging to somebody else: the field parses, the record
validates, and the usage is billed to the wrong subscriber. It is the most
common decoding error in this area, and invisible without a reference decode to
compare against.

```rust
use mediation::{AddressString, Msisdn, NatureOfAddress, NumberingPlan};

// 0x91 = extension bit set, international, E.164 — then the TBCD digits.
let field = [0x91, 0x94, 0x71, 0x10, 0x32, 0x54, 0x76];

let address = AddressString::from_bytes(&field).unwrap();
assert_eq!(address.nature(), NatureOfAddress::International);
assert_eq!(address.plan(), NumberingPlan::Isdn);
assert_eq!(address.to_msisdn().unwrap().to_string(), "491701234567");

// The wrong reading is not an error. It is a different subscriber.
assert_eq!(Msisdn::from_tbcd(&field).unwrap().to_string(), "19491701234567");
```

`to_msisdn` returns `None` for anything that is not an *international* number: a
national one needs the operator's dialling plan to become E.164, and prefixing a
country code by assumption settles the call against the wrong partner.

### The wire says how wide an MNC is, and the digits do not

Splitting an IMSI needs an E.212 table because nothing in the digit string
reveals where the MNC ends. The three-octet PLMN identity of TS 24.008
§10.5.1.13 does say: `0xF` in the MNC-digit-3 position means a two-digit code.

```rust
use mediation::PlmnId;

let telekom = PlmnId::from_bcd(&[0x62, 0xF2, 0x20]).unwrap();
assert_eq!(telekom.to_string(), "262-02");
assert_eq!(telekom.mnc().width(), 2);

let tmobile = PlmnId::from_bcd(&[0x13, 0x00, 0x62]).unwrap();
assert_eq!(tmobile.to_string(), "310-260");   // three digits, no filler
```

### A TAP timestamp is two fields, and forgetting one is silent

TAP3 carries local time and its offset **separately** — fourteen digits of
`YYYYMMDDhhmmss`, and five characters of `±hhmm` — usually with the offset
behind a code table declared once per file. A decoder that reads the local
timestamp and forgets to resolve the code produces a value that looks complete,
parses cleanly, and is wrong by up to fourteen hours for every event in the
file. So there is no constructor that takes the local half alone:

```rust
use mediation::CdrTimestamp;

let ts = CdrTimestamp::from_tap("20260728103045", "+0545").unwrap();
assert_eq!(ts.to_string(), "2026-07-28T10:30:45+05:45");
assert_eq!(ts.to_tap(), ("20260728103045".to_owned(), "+0545".to_owned()));

// Unlike the 3GPP BCD form, TAP carries the century — no pivot, no
// hundred-year relocation.
assert_eq!(
    CdrTimestamp::from_tap("19990101000000", "+0000").unwrap().local_date().year(),
    1999,
);
```

Wire decoders apply **the same rules as the string parsers** — an IMEI decoded
from TBCD is Luhn-checked exactly as one parsed from a string, because a
checksum only some code paths apply is a checksum nobody can rely on.
`DecodeError` carries the octet offset rather than a rejected string, since a
caller holding a binary record cannot search it for one.


## The framing, not the schema

A 3GPP charging record is ASN.1 (X.680) transferred in BER (X.690). This crate
reads the **framing** — tags, lengths, nesting, bounds — and stops there. The
schema is thousands of generated lines that belong beside a generator.

Framing alone is enough to walk a file, find a field by its tag, check that a
record's declared length matches what arrived, and reject a truncated one. The
fields it finds are then handed to the codecs described earlier on this page:

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
