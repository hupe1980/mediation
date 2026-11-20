+++
title = "Roaming, interconnect and numbering"
description = "TAP batch audits, the two RAP rejection classes, and terminating operator lookup under number portability."
weight = 130
+++

## TAP batches and RAP returns

`TapBatch::audit` checks a batch's stated count, window and totals against its
events and reports every discrepancy, not the first. Money is
`ScaledAmount`, an integer with TAP's decimal places and no float accessor;
amounts add only at one scale, because rescaling either operand is a silent
rounding error.

```rust
use mediation::ScaledAmount;

let charge = ScaledAmount::new(1_234, 3);
assert_eq!(charge.to_string(), "1.234");
assert!(charge.checked_add(ScaledAmount::new(123, 2)).is_none());
assert_eq!(charge.checked_add(ScaledAmount::new(766, 3)), Some(ScaledAmount::new(2_000, 3)));
```

| RAP class (TD.32) | Rejects | Effect |
|---|---|---|
| Fatal | the whole file | its events never entered the counts; the sequence number stays open |
| Severe | individual events | returned events move from billable to rejected |

`RapReturn::effect_on` applies the right one, and returns `None` when more
events come back than were released.

## Allocation is not termination

Interconnect settles per terminating operator, and number portability breaks
the link between a number's range and that operator. `terminating_plmn` checks
portability first, then the longest matching allocation prefix; neither
resolving gives `None`, never a default.

```rust
use mediation::{Msisdn, NumberRangeSet, PlmnId, is_ported, terminating_plmn};

let mut allocation = NumberRangeSet::new();
allocation.insert("49151".parse().unwrap(), "262-01".parse::<PlmnId>().unwrap());
let number: Msisdn = "491519234567".parse().unwrap();

let none_ported = |_: Msisdn| None;
assert_eq!(terminating_plmn(number, &allocation, &none_ported).unwrap().to_string(), "262-01");

let ported = |_: Msisdn| Some("262-02".parse::<PlmnId>().unwrap());
assert_eq!(terminating_plmn(number, &allocation, &ported).unwrap().to_string(), "262-02");
assert!(is_ported(number, &allocation, &ported));
```

A range inserted twice keeps both entries, so `duplicates()` reports
conflicting reference data instead of letting load order decide.
