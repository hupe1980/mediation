+++
title = "Jurisdiction, fair use, retention and privacy"
description = "Usage country versus customer country, the EU roaming fair-use test, retention without default durations, and pseudonymisation."
weight = 140
+++

## Two questions, two countries

Roaming rules, retention and lawful interception follow where the usage
happened. VAT follows the customer: Regulation (EU) 282/2011 Art. 24b(b)
presumes a mobile customer is in the country of the SIM's IMSI MCC. A German
SIM used in Spain is under Spanish telecom rules and German VAT, so there is no
`country()`, only one function per question:

```rust
use mediation::{Jurisdiction, customer_country_presumption, is_abroad, usage_country};
use mediation::{ChargedParty, RecordType, SourceRef, SubscriberRef, UsageRecord};

let roamer = UsageRecord::new(
    RecordType::PduSession,
    ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
    "2026-07-28T10:00:00+02:00".parse().unwrap(),
    SourceRef::new("pgw-01".parse().unwrap(), 0),
)
.with_networks("214-07".parse().unwrap(), "262-02".parse().unwrap());

assert_eq!(usage_country(&roamer), Jurisdiction::Country("214".parse().unwrap()));
assert_eq!(customer_country_presumption(&roamer), Jurisdiction::Country("262".parse().unwrap()));
assert_eq!(is_abroad(&roamer), Some(true));
```

- **Countries are not MCCs.** Both `usage_country` and `is_abroad` compare with
  `Mcc::same_country` (ITU-T E.212 multi-MCC areas): a `310` SIM on a `311`
  network is at home.
- **The VAT presumption applies only to a `ChargedParty::Subscriber` on 3GPP
  radio access.** A `Wireline` or `Wlan` record, or a subscriber without an
  IMSI, yields `NoEvidence`.
- **Roaming is not abroad.** `UsageRecord::is_roaming` compares networks;
  `is_abroad` compares countries. National roaming is roaming and not abroad,
  and Roam Like At Home and fair use turn on abroad.

`Jurisdiction::Unresolved` says why no country was found:

| Ambiguity | Meaning |
|---|---|
| `NoEvidence` | nothing to read a country from |
| `ExtraTerritorialAccess` | satellite access; the serving network's country is not the subscriber's position |
| `NonGeographicNetwork { mcc }` | E.212 non-geographic codes (`001`, `901`, `902`, `991`, `999`), often test traffic |
| `Conflicting { serving, reported }` | two sources name two countries; reported as **V21**, never resolved by precedence |

## Fair use (Regulation (EU) 2016/2286)

| Rule | Behaviour |
|---|---|
| Art. 4(1): presence **or** consumption | either prevailing domestically is the safe harbour; ties favour the customer |
| Art. 4(4): at least 4 months | `ObservationWindow::months(n)` counts calendar months and refuses n < 4 |
| Art. 4(4): any domestic log-on | a day seen at home and abroad counts as domestic only |
| Art. 5(3)–(4): alert, then 2 weeks | `may_surcharge` takes the dated `FairUseAlert`; an alert that predates the window is refused |
| scope | `Region` excludes rest-of-world usage from both sides of the ratio |

Presence comes from registration events, not usage, so `assess` takes a
presence series alongside the usage history. `VolumeAllowance` carries the 80 %
alert of Regulation (EU) 2022/612, computed multiply-first.

```rust
use mediation::{FairUseAlert, FairUseAssessment, Octets};
use time::macros::date;

// Roaming prevails on both indicators over a full window.
let assessment = FairUseAssessment::from_counts(
    20, 100, Octets::new(1_000), Octets::new(9_000), 120, date!(2026 - 03 - 01),
);
assert!(assessment.may_warn());

let alert = FairUseAlert::new(date!(2026 - 07 - 01));
assert!(!assessment.may_surcharge(None, date!(2027 - 01 - 01)));
assert!(!assessment.may_surcharge(Some(alert), date!(2026 - 07 - 14)));
assert!(assessment.may_surcharge(Some(alert), date!(2026 - 07 - 15)));

let stale = FairUseAlert::new(date!(2025 - 01 - 01));
assert!(!assessment.may_surcharge(Some(stale), date!(2026 - 07 - 15)));

// Consuming mostly at home is the safe harbour, whatever the presence.
let home_user = FairUseAssessment::from_counts(
    40, 80, Octets::new(9_000), Octets::new(1_000), 120, date!(2026 - 03 - 29),
);
assert!(home_user.prevailing_domestic() && !home_user.may_warn());
```

## Retention

Durations are caller-supplied, except where the law fixes the anchor and the
ceiling. `RetentionPolicy::billing_tdddg10(dispatched)` runs six calendar months
from invoice dispatch (§ 10 TDDDG), not from the call; a longer § 10 period is
refused, not trimmed. In a `RetentionSchedule` the longest basis governs, and a
record under no basis is deletable immediately.

A dispute hold ends when the dispute is resolved, not on a date:
`RetentionPolicy::until_released` builds it, and `RetentionPolicy::new` refuses
a dated hold. Only holds may be open-ended.

```rust
use mediation::{Deletion, RetentionBasis, RetentionPolicy, RetentionSchedule};
use time::{Duration, macros::datetime};

let billing = RetentionPolicy::billing_tdddg10(datetime!(2026-08-31 00:00 UTC)).unwrap();
assert_eq!(billing.deadline(), Some(datetime!(2027-02-28 00:00 UTC)));

let basis = RetentionBasis::BillingTdddg10 { invoice_dispatched_at: datetime!(2026-08-31 00:00 UTC) };
assert!(RetentionPolicy::new(basis, Duration::days(182)).is_none());

let hold = RetentionBasis::DisputeHold { raised_at: datetime!(2026-08-10 00:00 UTC) };
assert!(RetentionPolicy::new(hold, Duration::days(30)).is_none());
let dispute = RetentionPolicy::until_released(hold).unwrap();

let schedule: RetentionSchedule = vec![billing, dispute].into_iter().collect();
assert!(schedule.deletion().is_blocked());
assert!(!schedule.is_deletable(datetime!(2099-01-01 00:00 UTC)));
assert_eq!(RetentionSchedule::new().deletion(), Deletion::Immediate);
```

Releasing a hold means rebuilding the schedule without it, which leaves a
record that the hold applied.

## Pseudonymisation

A pseudonym is reversible by the key holder, so it remains personal data (EDPB
Guidelines 01/2025). The crate holds no key and ships no hash: `Pseudonymiser`
takes your keyed primitive, since an unkeyed digest of an IMSI is reversed by
enumeration. A `Reduction` discards digits for everyone and is irreversible.

```rust
use mediation::{Imei, Imsi, reduce_equipment, reduce_to_country};

let imsi: Imsi = "262021234567890".parse().unwrap();
let imei: Imei = "490154203237518".parse().unwrap();

assert_eq!(reduce_to_country(imsi).to_string(), "262");
assert_eq!(reduce_equipment(imei.into()).to_string(), "49015420");
```

`NumberPrefix::of` refuses to "truncate" to the whole number.
