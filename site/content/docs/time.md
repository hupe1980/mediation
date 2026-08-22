+++
title = "Time is local, not UTC"
description = "A charging record carries a local wall clock and a quarter-hour offset to UTC. Reading it as an instant books every evening event onto the wrong day."
weight = 20
+++

## A CDR timestamp is local time plus an offset

The single most consequential thing this crate gets right.

The `TimeStamp` of **3GPP TS 32.298** is nine BCD octets spelling
`YYMMDDhhmmssShhmm`: a **local** date and time, a sign, and a signed offset to
UTC. There is no timezone *name* anywhere in a charging record — only the
numeric offset that was true when the record was written.

```rust
use mediation::CdrTimestamp;

let ts: CdrTimestamp = "2026-07-28T00:30:00+05:45".parse().unwrap();

// The subscriber's billing day …
assert_eq!(ts.local_date().to_string(), "2026-07-28");
// … is not the UTC day.
assert_eq!(ts.to_utc().date().to_string(), "2026-07-27");
```

Three failure modes this removes from every downstream consumer:

- **Reading a local timestamp as UTC** books every evening event of a UTC+X
  network onto the wrong calendar day. Unlike a fixed-zone mistake it cannot be
  corrected downstream by subtracting a constant, because the offset varies
  from record to record.
- **Assuming whole-hour offsets.** The offset comes from the MS Time Zone
  element (TS 29.060 §7.7.52, encoded per TS 24.008, sharing the semi-octet form
  of TS 23.040 §9.2.3.11) and counts **quarter-hours**. India (+05:30), Nepal
  (+05:45), Eucla (+08:45) and the Chatham Islands (+12:45) all land on the
  wrong hour, and therefore the wrong tariff window, under an `i8` of hours.
- **Dropping the daylight-saving bits.** Without them an offset cannot be mapped
  back to a zone at all.

### Two ranges, deliberately

The semi-octet encoding can express ±79 quarter-hours — almost ±20 hours — while
only UTC−12:00 … UTC+14:00 are assigned. `QuarterHourOffset::new` accepts the
assigned range; `from_encoded` accepts anything the wire can hold, so a decoder
can carry an out-of-range offset forward as a finding rather than panicking or
clamping it. Clamping is the worst of the three: it turns corrupt input into a
plausible timestamp that no later stage can identify as wrong.

### The same offset is two different zones

UTC+02:00 is Berlin in July and Athens in January. Only the daylight-saving
field tells them apart, so it is kept rather than folded into the offset:

```rust
use mediation::{DstAdjustment, MsTimeZone, QuarterHourOffset};

let plus_two = QuarterHourOffset::from_minutes(120).unwrap();
let berlin_summer = MsTimeZone::new(plus_two, DstAdjustment::PlusOneHour);
let athens_winter = MsTimeZone::new(plus_two, DstAdjustment::None);

assert_eq!(berlin_summer.offset(), athens_winter.offset());
assert_eq!(berlin_summer.standard_offset().unwrap().to_string(), "+01:00");
assert_eq!(athens_winter.standard_offset().unwrap().to_string(), "+02:00");

// Which is also how a DST transition is told apart from a journey.
let berlin_winter = MsTimeZone::new(QuarterHourOffset::from_minutes(60).unwrap(), DstAdjustment::None);
assert!(berlin_winter.is_same_zone_across_transition(berlin_summer));
assert!(!berlin_winter.is_same_zone_across_transition(athens_winter));
```

### There is no `Ord` on a timestamp

Records from different offsets compare correctly only as *instants*. A derived
ordering would compare the local fields and silently interleave a UTC+14:00
record ahead of a UTC−12:00 one that happened a day earlier — so the trait is
simply absent, and the instant comparator is the only way:

```rust
use mediation::CdrTimestamp;

let mut series: Vec<CdrTimestamp> = vec![
    "2026-07-27T23:00:00-12:00".parse().unwrap(),
    "2026-07-28T01:00:00+14:00".parse().unwrap(),
];
series.sort_by(CdrTimestamp::cmp_instant);

// The +14:00 record is the earlier instant, despite the later local date.
assert_eq!(series[0].to_string(), "2026-07-28T01:00:00+14:00");
```

### Duration is not `closed − opened`

The duration a record carries is the **chargeable** one, measured from answer. A
45-second ring followed by 10 seconds of conversation is a 10-second call across
a 55-second wall-clock span. Deriving duration by subtraction over-bills every
answered call and invents duration for every unanswered one, so `duration` is a
field and `wall_clock_span()` is a separate, differently-named accessor. Nothing
falls back from one to the other.


## Late data is the whole problem

Almost every window is the same window wearing a different hat: an event
happened at T, it is being considered at `as_of`, and the elapsed time changes
what may be done with it. `LatenessPolicy` is that one concept.

| Window | Bound |
|---|---|
| TAP inclusion | **30 days** (GSMA TD.57) |
| NRTRDE delivery | **4 hours** (GSMA TD.35) |
| Deduplication | the partner's resend bound |

Two distinctions the type refuses to blur:

- **Late and unbillable are different.** `Lateness::Late` is still billable —
  the holding structures may have moved on, which is an operational finding.
  `Lateness::BeyondDeadline` can never become billable. Collapsing them either
  bills unbillable usage or discards billable usage.
- **A future event is not an on-time event.** `Lateness::Future` exists because
  a record timestamped ahead of the evaluation instant is clock skew or a
  corrupt field, and folding it into `OnTime` hides the only signal that would
  catch it.

`as_of` is always a parameter, which is what lets a 2019 archive replayed in
2026 reproduce the *2019* verdicts. A policy that read the clock would declare
every historic record beyond every deadline — the whole archive rejected.
