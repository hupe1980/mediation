+++
title = "Fixed-line voice"
description = "From the dialled string to the itemised bill: dial plans, destination classes, Call-ID keys and § 11 TDDDG itemisation."
weight = 65
+++

## Dialled strings become destinations

`0151 1234567`, `0049 151 1234567` and `+49 (0)151 1234567` are one destination;
`2345678` is a Berlin number only for a caller in Berlin. The missing facts
arrive as a `DialContext`:

```rust
use mediation::{DialContext, DialError};

let berlin = DialContext::germany().with_area_code("030").unwrap();

for spelling in ["0151 1234567", "0049 151 1234567", "+49 (0)151 1234567"] {
    assert_eq!(berlin.normalise(spelling).unwrap().to_string(), "+491511234567");
}
assert_eq!(berlin.normalise("2345678").unwrap().to_string(), "+49302345678");
assert_eq!(berlin.normalise("110").unwrap().to_string(), "110");
// Only listed short codes are short codes: 19222 is a local number.
assert_eq!(berlin.normalise("19222").unwrap().to_string(), "+493019222");

// No area code: a local number is refused, not guessed.
assert_eq!(DialContext::germany().normalise("2345678"), Err(DialError::AreaCodeRequired));

assert_eq!(DialContext::italy().normalise("06 1234567").unwrap().to_string(), "+39061234567");
assert_eq!(DialContext::nanp().normalise("5551234"), Err(DialError::NationalNumberLength));
// An inconsistent context is refused at construction.
assert!(DialContext::new(49, "0").unwrap().with_trunk_prefix("00").is_err());
```

Presets are `germany()`, `italy()` (closed plan) and `nanp()`. Any other
country is built with `DialContext::new(cc, intl)`, which is fallible, and the
builders `with_trunk_prefix`, `closed()`, `with_short_codes`,
`with_national_length` and `with_area_code`. A `(0)` is dropped only directly
after the own country code. Input is bounded at 64 bytes, restricted to digits,
one leading `+` and common separators, and a refusal names the reason and
position, never the digits.

## Destination classes

A price book maps classes to usage kinds instead of maintaining prefixes.
`GermanNumberingPlan` is a dated example of the `NumberingPlan` trait; for
production, implement it over the regulator's current data.

```rust
use mediation::{Destination, DestinationClass, GermanNumberingPlan};

let class = |d: &str| d.parse::<Destination>().unwrap().class(&GermanNumberingPlan);

assert_eq!(class("+491511234567"), Some(DestinationClass::Mobile));
assert_eq!(class("+498001110111"), Some(DestinationClass::Freephone));
assert_eq!(class("116117"), Some(DestinationClass::Harmonised));
// A reserve or withdrawn range is not classified.
assert_eq!(class("+4990091234567"), None);
```

## Call records

`UsageRecord::call` takes direction, charged party, answer time, chargeable
seconds and provenance, and sets the close time to answer plus duration. A
fixed-line call is `Voice`, like a VoLTE call, and a missing PLMN is not a
finding on `RatType::Wireline`.

`RecordKey::Call` keys on the SIP Call-ID **paired with the recording
entity**: a B2BUA regenerates Call-IDs, so two softswitches can report the same
one for different calls. A Call-ID without its node is
`NotKeyable::IncompleteCallId`; an IMS ICID takes precedence. Keying on A, B and
start time is refused, because concurrent identical calls are normal on a PBX.

A fixed line has no IMSI; its SUPI is a `gli-…`, `gci-…` or network-specific
`nai-…` identity (TS 23.003 §2.2A).

## Itemisation (§ 11 TDDDG)

| Rule | Behaviour |
|---|---|
| Abs. 1: subscriber chooses | `Itemisation::Full` or `Shortened` (last three digits hidden); no default |
| Abs. 4: incoming callers | `itemise_calling` always shortens |
| Abs. 5: counselling services | destinations in `ProtectedDestinations` are omitted |

```rust
use mediation::{
    Destination, DialContext, Itemisation, ItemisedDestination, itemise_called,
    itemise_calling,
};

let berlin = DialContext::germany().with_area_code("030").unwrap();
let protected: [Destination; 1] = ["+498001110111".parse().unwrap()];

let shown = itemise_called(&berlin.normalise("030 2345678").unwrap(), Itemisation::Shortened, &protected);
assert!(matches!(shown, ItemisedDestination::Shortened(n) if n.to_string() == "+49302345xxx"));

// Every spelling of a protected number is omitted, under either choice.
let dialled = berlin.normalise("0049800-1110111").unwrap();
assert_eq!(itemise_called(&dialled, Itemisation::Full, &protected), ItemisedDestination::Omitted);

assert_eq!(itemise_calling("49301234567".parse().unwrap()).to_string(), "+49301234xxx");
```

An omitted call is still billed: itemisation is presentation, not a
disposition.

A call for an already-invoiced month is released in the batch it arrives in and
booked as a `Restatement`. Billing data retention and dispute holds are covered
in [Jurisdiction, fair use, retention and privacy](@/docs/regulatory.md).
