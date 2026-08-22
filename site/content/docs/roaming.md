+++
title = "Roaming, interconnect and numbering"
description = "A partner's batch checked against its own audit block, the two RAP rejection classes, and why a number's range is not the operator that terminates it."
weight = 130
+++

## A partner's batch, and whether it adds up

A GSMA TAP batch is self-describing: it states how many events it holds, the
window they fall in, and what they total. Those claims are checkable against the
events, and `TapBatch::audit` reports **every** discrepancy rather than the
first — a batch with three problems should be fixed once.

**Money is an integer with a scale.** TAP carries monetary values as integers
scaled by `tapDecimalPlaces`; a charge of `1234` with 3 places is `1.234`.
Rendering that through a float reintroduces exactly the error the integer
encoding exists to avoid, so `ScaledAmount` has no float accessor:

```rust
use mediation::ScaledAmount;

let charge = ScaledAmount::new(1_234, 3);
assert_eq!(charge.to_string(), "1.234");

// Amounts add only at one scale. Rescaling the coarser operand invents
// precision; rescaling the finer one discards it. Either is a rounding error
// entering a settlement silently.
assert!(charge.checked_add(ScaledAmount::new(123, 2)).is_none());
assert_eq!(
    charge.checked_add(ScaledAmount::new(766, 3)),
    Some(ScaledAmount::new(2_000, 3)),
);
```

The crate still prices nothing — it carries an amount a partner computed, so a
reconciliation can check it.

**A returned batch and a returned record reconcile differently.** GSMA TD.32
distinguishes two rejection classes, and conflating them breaks the conservation
law in opposite directions:

| Class | Rejects | Consequence |
|---|---|---|
| **Fatal** | the whole file | none of its events ever entered the counts; the sequence number stays open |
| **Severe** | individual events | the file counted; the returned events move from billable to rejected |

`RapReturn::effect_on` applies each correctly rather than leaving a caller to
remember which is which — and returns `None` when more events come back than
were released, because that is a disagreement with the partner rather than
arithmetic to absorb.


## Allocation is not termination

Number portability broke that inference in every market that introduced it, and
**interconnect settles per terminating operator**. Rating a call on its
allocation range therefore pays the wrong partner — in both directions, by
amounts that surface only when a partner's invoice disagrees with yours.

`terminating_plmn` consults portability first and the allocation range second:

```rust
use mediation::{Msisdn, NumberRangeSet, PlmnId, is_ported, terminating_plmn};

let mut allocation = NumberRangeSet::new();
allocation.insert("49151".parse().unwrap(), "262-01".parse::<PlmnId>().unwrap());

let number: Msisdn = "491519234567".parse().unwrap();

// Nothing ported: the range answers.
let none_ported = |_: Msisdn| None;
assert_eq!(terminating_plmn(number, &allocation, &none_ported).unwrap().to_string(), "262-01");
assert!(!is_ported(number, &allocation, &none_ported));

// Once ported, the range is simply wrong.
let ported = |_: Msisdn| Some("262-02".parse::<PlmnId>().unwrap());
assert_eq!(terminating_plmn(number, &allocation, &ported).unwrap().to_string(), "262-02");
assert!(is_ported(number, &allocation, &ported));
```

**Longest prefix wins**, because number plans nest: `49` is a country, `49151`
is a mobile range inside it, `4915112` may be a block carved out of that. A
lookup returning the first or shortest match answers with the country when the
caller asked about the block.

Neither source resolving gives `None`, never a default — settling a guess is a
two-sided error. And a range inserted twice keeps **both** entries, so
`duplicates()` can report reference data that answers one question twice rather
than letting load order decide.
