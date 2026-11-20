+++
title = "Time is local, not UTC"
description = "A charging record carries a local wall clock and a quarter-hour offset to UTC, and reading it as an instant books evening events onto the wrong day."
weight = 20
+++

## A CDR timestamp is local time plus an offset

The 3GPP TS 32.298 `TimeStamp` is a **local** date and time plus a signed
offset to UTC. `CdrTimestamp` keeps both. `CdrTimestamp::new` and `from_utc`
return `Option`: the instant must fall in UTC years 1–9998.

```rust
use mediation::CdrTimestamp;

let ts: CdrTimestamp = "2026-07-28T00:30:00+05:45".parse().unwrap();

// The billing day is not the UTC day.
assert_eq!(ts.local_date().to_string(), "2026-07-28");
assert_eq!(ts.to_utc().date().to_string(), "2026-07-27");

// `Z` and `-00:00` mean "offset unknown" (RFC 9557 §2.2) and are refused.
assert!("2026-07-28T00:30:00Z".parse::<CdrTimestamp>().is_err());
assert!("2026-07-28T00:30:00-00:00".parse::<CdrTimestamp>().is_err());
let utc: CdrTimestamp = "2026-07-28T00:30:00+00:00".parse().unwrap();
assert_eq!(utc.to_string(), "2026-07-28T00:30:00+00:00");
```

| Mistake | Consequence |
|---|---|
| Local time read as UTC | Evening events land on the wrong day |
| Whole-hour offsets | +05:30, +05:45 land in the wrong tariff window |
| DST bits dropped | The offset no longer maps to a zone |

`QuarterHourOffset::new` accepts UTC−12:00 to UTC+14:00; `from_encoded` accepts
the full wire range so a decoder reports an out-of-range offset, never clamps it.

### The same offset is two different zones

UTC+02:00 is Berlin in July and Athens in January; the DST field tells them
apart.

```rust
use mediation::{DstAdjustment, MsTimeZone, QuarterHourOffset};

let plus_two = QuarterHourOffset::from_minutes(120).unwrap();
let berlin_summer = MsTimeZone::new(plus_two, DstAdjustment::PlusOneHour);
let athens_winter = MsTimeZone::new(plus_two, DstAdjustment::None);

assert_eq!(berlin_summer.offset(), athens_winter.offset());
assert_eq!(berlin_summer.standard_offset().unwrap().to_string(), "+01:00");
assert_eq!(athens_winter.standard_offset().unwrap().to_string(), "+02:00");

let berlin_winter =
    MsTimeZone::new(QuarterHourOffset::from_minutes(60).unwrap(), DstAdjustment::None);
assert!(berlin_winter.is_same_zone_across_transition(berlin_summer));
assert!(!berlin_winter.is_same_zone_across_transition(athens_winter));
```

### There is no `Ord` on a timestamp

A derived ordering would compare local fields and put a UTC+14:00 record ahead
of an earlier UTC−12:00 one. Sort by instant instead:

```rust
use mediation::CdrTimestamp;

let mut series: Vec<CdrTimestamp> = vec![
    "2026-07-27T23:00:00-12:00".parse().unwrap(),
    "2026-07-28T01:00:00+14:00".parse().unwrap(),
];
series.sort_by(CdrTimestamp::cmp_instant);
assert_eq!(series[0].to_string(), "2026-07-28T01:00:00+14:00");
```

### Duration is not `closed − opened`

`duration` is chargeable time from answer: a 45-second ring and 10 seconds of
talk is a 10-second call. `wall_clock_span()` is separate; neither falls back to
the other.


## Late data is the whole problem

`LatenessPolicy` judges an event at T against an evaluation instant `as_of`.

| Window | Bound |
|---|---|
| TAP inclusion | 30 days (GSMA TD.57) |
| NRTRDE delivery | 4 hours (GSMA TD.35) |
| Deduplication | the partner's resend bound |

- `Lateness::Late` is still billable; `Lateness::BeyondDeadline` never is.
- `Lateness::Future` flags clock skew or a corrupt field instead of passing it
  as on time.

`as_of` is a parameter, never the clock, so replaying an archive reproduces the
verdicts of the day it was processed.
