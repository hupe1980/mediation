+++
title = "Jurisdiction, fair use, retention and privacy"
description = "Two questions and two countries, the EU roaming fair-use test, retention with no default duration, and why pseudonymised data is still personal data."
weight = 140
+++

## Two questions, two countries

Four regimes attach to a record by country: roaming regulation, retention,
lawful interception and tax. **Three of them turn on where the usage happened.
The fourth does not.**

Council Implementing Regulation (EU) 282/2011 Art. 24b(b) presumes a customer
supplied over a mobile network to be located in the country of the **mobile
country code of the IMSI on the SIM card**, and Art. 24f lists that MCC as an
item of evidence. So a German subscriber using data in Spain is governed by
Spanish telecom rules and taxed in Germany. A mediation layer that answers
"country" with one number has answered one of the two questions wrong.

There is therefore no `country()`. There are two functions, each named for its
question:

```rust
use mediation::{Jurisdiction, customer_country_presumption, is_abroad, usage_country};
use mediation::{ChargedParty, RecordType, SourceRef, SubscriberRef, UsageRecord};
let roamer = UsageRecord::new(
    RecordType::PduSession,
    ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
    "2026-07-28T10:00:00+02:00".parse().unwrap(),
    SourceRef::new("pgw-01".parse().unwrap(), 0),
).with_networks("214-07".parse().unwrap(), "262-02".parse().unwrap());

// A German SIM, used in Spain.
assert_eq!(usage_country(&roamer), Jurisdiction::Country("214".parse().unwrap()));
assert_eq!(
    customer_country_presumption(&roamer),
    Jurisdiction::Country("262".parse().unwrap()),
);
assert_eq!(is_abroad(&roamer), Some(true));
```

The two agree for domestic traffic, which is exactly why conflating them
survives testing and then bills a whole roaming population wrong.

Both questions apply the same geographic test. A test IMSI in MCC `001` gets
`NonGeographicNetwork` from the presumption, not a tax country of "001": the
presumption is that the customer is located *in* the country of the MCC, and
`001` is not a country anyone can be taxed in.

### Roaming is a network question; abroad is a country question

`UsageRecord::is_roaming` compares networks. `is_abroad` compares countries. A
subscriber national-roaming on a second network at home is **roaming and not
abroad**, and the regulations that turn on being abroad — Roam Like At Home, the
fair-use window — mean abroad. A `Regions` implementation written as
`serving == home` gets this wrong in the direction that costs the customer
money.

### Not knowing is several different things

`Jurisdiction` is not an `Option<Mcc>`, because "no country" has causes with
different remedies:

| Ambiguity | Meaning |
|---|---|
| `NoEvidence` | the record carries nothing to read a country from — fix the feed |
| `ExtraTerritorialAccess` | satellite: the serving network's country is the operator's licence, not the subscriber's position |
| `NonGeographicNetwork { mcc }` | E.212 sets aside `001`, `901`, `902`, `991` and `999` for test and trial networks, internal use and the shared codes international operators use — well-formed, and naming no territory |
| `Conflicting { serving, reported }` | two sources name two countries — a fault, reported as **V21**, never resolved by precedence |

3GPP SA1 opened a study on extra-territorial satellite coverage precisely
because the legal answer is not settled. A library that returned a country there
would be claiming more than the standards body does. A record served by MCC 901
is a different problem with a different remedy — very often a test record that
should never have entered the feed — which is why it is a separate case rather
than folded into "unknown".

```rust
use mediation::{Ambiguity, Jurisdiction, usage_country};
use mediation::{ChargedParty, RecordType, SourceRef, SubscriberRef, UsageRecord};
let satellite_operator = UsageRecord::new(
    RecordType::PduSession,
    ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
    "2026-07-28T10:00:00+02:00".parse().unwrap(),
    SourceRef::new("pgw-01".parse().unwrap(), 0),
).with_networks("901-01".parse().unwrap(), "262-02".parse().unwrap());
assert_eq!(
    usage_country(&satellite_operator),
    Jurisdiction::Unresolved(Ambiguity::NonGeographicNetwork {
        mcc: "901".parse().unwrap()
    }),
);
```


## Fair use, and the reading that costs a customer money

Under Roam Like At Home a provider may apply a fair use policy, and Implementing
Regulation (EU) 2016/2286 says how. Four parts of it are easy to get slightly
wrong, all in the same direction.

**The safe harbour is an OR.** *Either* prevailing domestic consumption *or*
prevailing domestic presence is proof of non-abusive use. Reading "presence and
consumption are observed cumulatively" as an AND surcharges lawful customers, so
the two are separate predicates:

```rust
use mediation::{FairUseAssessment, Octets};

// Abroad most of the window, but consuming mostly at home.
let assessment = FairUseAssessment::from_counts(
    /* domestic presence */ 40, /* roaming presence */ 80,
    /* domestic use */ Octets::new(9_000), /* roaming use */ Octets::new(1_000),
    /* window days */ 120,
);

assert!(!assessment.prevailing_domestic_presence());
assert!(assessment.prevailing_domestic_consumption());
assert!(assessment.prevailing_domestic(), "either alone is the safe harbour");
assert!(!assessment.may_warn());
```

**The sequence is observe, alert, *wait*, surcharge — and the wait is two
weeks.** Article 5(3) requires the customer to be alerted before any surcharge,
and Article 5(4) requires the alert to give them "a period which cannot be
shorter than 2 weeks" to change their usage pattern. A boolean "was warned"
cannot express that: it is satisfied on the day the alert goes out, when a
surcharge would still breach the regulation. So `may_surcharge` takes the alert
itself, with the date it was issued, and the evaluation date:

```rust
use mediation::{FairUseAlert, FairUseAssessment, Octets};
use time::macros::date;

// Roaming prevails on both indicators over a full window.
let assessment = FairUseAssessment::from_counts(
    20, 100, Octets::new(1_000), Octets::new(9_000), 120,
);
assert!(assessment.may_warn());

let alert = FairUseAlert::new(date!(2026 - 07 - 01));
assert_eq!(alert.expires().to_string(), "2026-07-15");

assert!(!assessment.may_surcharge(None, date!(2027 - 01 - 01)), "no alert, ever");
assert!(!assessment.may_surcharge(Some(alert), date!(2026 - 07 - 14)));
assert!(assessment.may_surcharge(Some(alert), date!(2026 - 07 - 15)));
```

A provider may give longer than the floor — `FairUseAlert::with_period` — and is
then held to what it promised. A shorter period is not constructible, because it
is not a stricter policy but a non-compliant one.

**Presence is not consumption.** A subscriber can be abroad for six weeks and
generate nothing chargeable. Presence comes from registration events (3GPP
TS 32.256), so `assess` takes a presence series alongside the usage history
rather than deriving one from the other.

**Roaming is not the complement of domestic.** RLAH applies within the EEA;
`Region` has three cases and rest-of-world usage is excluded from *both* sides
of the ratio rather than inflating the roaming one.

Ties resolve in the customer's favour, because an equal split does not establish
the abuse a provider would have to show. And `VolumeAllowance` carries the 80 %
alert Regulation (EU) 2022/612 requires — computed by multiplying before
dividing, since `limit / 100 * 80` would alert a 199-octet allowance at 40 %.


## Retention: no default duration, on purpose

The German rule has changed three times in three years:

| When | What |
|---|---|
| TKG 2021 | §§ 175/176 required ten weeks of traffic data, four of location data |
| 14 Aug 2023 | the Bundesverwaltungsgericht held it incompatible with Art. 15(1) of Directive 2002/58/EC, and inapplicable |
| Apr 2026 | after a further CJEU ruling, a new draft § 176 TKG requires access providers to store **IP addresses and timestamps for three months**, explicitly excluding content, destinations, location and browsing behaviour |

A library that had hard-coded any one of those would have been wrong for most of
the period, and wrong in the direction of keeping personal data longer than the
law allowed. So the duration is caller-supplied and the crate contributes the
arithmetic, the anchor and the citation.

**The anchor is not the event date.** § 10 TDDDG permits billing traffic data to
be kept up to six months **after the invoice is dispatched** — not after the
call. Anchoring on the event deletes a record from the start of a billing period
weeks early, while a dispute over that invoice may still be open:

```rust
use mediation::{RetentionBasis, RetentionPolicy};
use time::{Duration, macros::datetime};

let policy = RetentionPolicy::new(
    RetentionBasis::BillingTdddg10 {
        invoice_dispatched_at: datetime!(2026-08-05 00:00 UTC),
    },
    Duration::days(183),
)
.unwrap();

let call = datetime!(2026-07-01 12:00 UTC);
assert!(policy.deadline().unwrap() > call + Duration::days(183));
```

A record commonly sits under several bases at once, and in a `RetentionSchedule`
the **longest governs** — deleting while any basis still requires the data is
the error that cannot be undone. A record under *no* basis is deletable
immediately: no ground to keep it is not a licence to keep it forever.

### A hold has no deadline, and that is not the same as no policy

A statutory period has a length. A dispute does not — it is over when it is
resolved, and no arithmetic produces that instant. Giving a hold a nominal
duration anyway makes deletion permitted the moment that nominal period runs
out, while the dispute is still open.

So `RetentionPolicy::until_released` builds a policy with no deadline, and
`RetentionSchedule::deletion` has **three** answers rather than an `Option`,
because "no deadline" is ambiguous in exactly the direction that costs somebody
their evidence:

```rust
use mediation::{Deletion, RetentionBasis, RetentionPolicy, RetentionSchedule};
use time::macros::datetime;

let dispute = RetentionPolicy::until_released(RetentionBasis::DisputeHold {
    raised_at: datetime!(2026-08-10 00:00 UTC),
}).unwrap();

let schedule: RetentionSchedule = vec![dispute].into_iter().collect();
assert!(schedule.deletion().is_blocked());
assert!(!schedule.is_deletable(datetime!(2099-01-01 00:00 UTC)));

// Nothing at all is the opposite answer, not the same one.
assert_eq!(RetentionSchedule::new().deletion(), Deletion::Immediate);

// And only a hold may run without a duration — an open-ended statutory period
// is not a retention policy but an intention to keep data forever.
let from = datetime!(2026-08-05 00:00 UTC);
assert!(RetentionPolicy::until_released(RetentionBasis::OperatorPolicy { from }).is_none());
```

Releasing a hold means rebuilding the schedule without it, because a hold that
could be lifted in place would leave no evidence that it had ever applied.


## Pseudonymised data is still personal data

EDPB Guidelines 01/2025 are explicit, and it is the most consequential
misunderstanding in this area: a pseudonym is reversible by whoever holds the
key, so the output stays within the GDPR's scope. `Pseudonym` is therefore not
named "anonymous" and carries no method suggesting it is.

**The key is yours.** The crate holds no key material and ships no hash — a
pseudonym must be *keyed*, since an unkeyed digest of a 15-digit IMSI is
reversed by enumerating all of them. `Pseudonymiser` takes the primitive you
already use, exactly as `MncWidths` takes the assignment table.

What the crate contributes is the part that is easy to get wrong: which fields
carry identity, and which reductions keep the analytical value without it.

```rust
use mediation::{Imei, Imsi, reduce_equipment, reduce_to_country};

let imsi: Imsi = "262021234567890".parse().unwrap();
let imei: Imei = "490154203237518".parse().unwrap();

// A country-traffic report needs the country, not the subscriber.
assert_eq!(reduce_to_country(imsi).to_string(), "262");

// A device-mix report needs the model, not the handset.
assert_eq!(reduce_equipment(imei.into()).to_string(), "49015420");
```

A `Reduction` is **irreversible** — the discarded digits are gone for everyone,
key holder included — which is what makes a reduced dataset genuinely narrower
rather than merely obscured. `NumberPrefix::of` refuses to "truncate" to the
whole number, because returning identifying data from a method named for
truncation is how it reaches a report that believed it was reduced.
