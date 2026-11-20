//! The regulatory layer, on the cases that decide money and lawfulness.
//!
//! Three rules where a plausible reading is the wrong one:
//!
//! - the fair use **safe harbour is an OR** — reading it as an AND surcharges
//!   lawful customers;
//! - a billing retention period runs from the **invoice**, not the call —
//!   anchoring on the event deletes evidence while a dispute is open;
//! - a pseudonym is **still personal data** — treating it as anonymous puts a
//!   dataset outside a policy it was always inside.

use mediation::{
    AllowanceStatus, ChargedParty, Deletion, FairUseAlert, FairUseAssessment, Imsi,
    NorthAmericanRule, Octets, PlmnId, PresenceDay, Pseudonymiser, RecordType, ReducedSubscriber,
    Region, Regions, RetentionBasis, RetentionPolicy, RetentionSchedule, SafeHarbour, SourceRef,
    SubscriberRef, Supi, UsageRecord, VolumeAllowance, VolumePair, fairuse, reduce_to_country,
};
use proptest::prelude::*;
use time::{Duration, OffsetDateTime, macros::date, macros::datetime};

fn home() -> PlmnId {
    "262-02".parse().unwrap()
}

/// One EEA partner, everything else outside — supplied, never guessed.
fn regions() -> impl Regions {
    |serving: PlmnId, home: PlmnId| {
        // Country, not network: a national roamer is at home.
        if serving.is_same_country(home) {
            Region::Domestic
        } else if serving.mcc().get() == 234 {
            Region::Eea
        } else {
            Region::RestOfWorld
        }
    }
}

/// How many days a window ending on `ending` covers.
fn window_days(window: fairuse::ObservationWindow, ending: time::Date) -> u32 {
    let start = window.start(ending).unwrap();
    u32::try_from((ending - start).whole_days() + 1).unwrap()
}

fn record(day: &str, serving: &str, octets: u64) -> UsageRecord {
    UsageRecord::new(
        RecordType::PduSession,
        ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
        format!("{day}T10:00:00+02:00").parse().unwrap(),
        SourceRef::new("pgw-01".parse().unwrap(), 0),
    )
    .with_networks(serving.parse().unwrap(), home())
    .with_volume(VolumePair::new(Octets::new(octets), Octets::ZERO))
}

// ── Fair use ─────────────────────────────────────────────────────────────────

/// The full sequence the regulation prescribes: observe for four months, then
/// warn, and only then surcharge.
#[test]
fn the_regulation_sequence_is_observe_warn_surcharge() {
    let window = fairuse::ObservationWindow::minimum();
    let ending = date!(2026 - 07 - 28);

    // A subscriber abroad the whole window, consuming almost entirely there.
    let mut presence = Vec::new();
    let mut day = window.start(ending).unwrap();
    for _ in 0..window_days(window, ending) {
        presence.push(PresenceDay::new(day, Region::Eea));
        day += Duration::days(1);
    }
    let usage = vec![
        record("2026-07-20", "234-15", 9_500),
        record("2026-07-21", "262-02", 500),
    ];

    let assessment = fairuse::assess(&usage, &presence, home(), window, ending, &regions());

    assert!(assessment.window_sufficient());
    assert_eq!(assessment.safe_harbour(), None);
    assert!(assessment.may_warn());
    assert!(!assessment.may_surcharge(None, ending), "alert first");

    // Article 5(4): the alert must give the customer at least two weeks to
    // change their pattern, so the alert date decides, not a boolean.
    let alert = FairUseAlert::new(ending);
    assert!(!assessment.may_surcharge(Some(alert), ending + Duration::days(13)));
    assert!(assessment.may_surcharge(Some(alert), ending + Duration::days(14)));
}

/// The error the API shape prevents: either indicator alone is enough, so a
/// subscriber abroad but consuming at home cannot be surcharged.
#[test]
fn a_subscriber_abroad_but_consuming_at_home_keeps_the_safe_harbour() {
    let window = fairuse::ObservationWindow::minimum();
    let ending = date!(2026 - 07 - 28);

    let mut presence = Vec::new();
    let mut day = window.start(ending).unwrap();
    for _ in 0..window_days(window, ending) {
        presence.push(PresenceDay::new(day, Region::Eea));
        day += Duration::days(1);
    }
    let usage = vec![
        record("2026-07-20", "262-02", 9_500),
        record("2026-07-21", "234-15", 500),
    ];

    let assessment = fairuse::assess(&usage, &presence, home(), window, ending, &regions());

    assert!(!assessment.prevailing_domestic_presence());
    assert!(assessment.prevailing_domestic_consumption());
    assert_eq!(assessment.safe_harbour(), Some(SafeHarbour::Consumption));
    assert!(!assessment.may_warn());
    assert!(
        !assessment.may_surcharge(Some(FairUseAlert::new(ending)), ending + Duration::days(30))
    );
}

/// Usage outside the EEA is not roaming under this regulation, and counting it
/// as such would tip a test it is not part of.
#[test]
fn rest_of_world_usage_cannot_trigger_a_surcharge() {
    let window = fairuse::ObservationWindow::minimum();
    let ending = date!(2026 - 07 - 28);

    let usage = vec![
        record("2026-07-20", "262-02", 1_000),
        record("2026-07-21", "234-15", 100),
        // A long trip well outside the EEA.
        record("2026-07-22", "310-260", 500_000),
    ];

    let assessment = fairuse::assess(&usage, &[], home(), window, ending, &regions());
    assert_eq!(assessment.roaming_consumption(), Octets::new(100));
    assert!(assessment.prevailing_domestic());
    assert!(!assessment.may_warn());
}

/// Regulation (EU) 2022/612 requires an alert at 80 % of a fair use limit.
#[test]
fn the_customer_is_alerted_before_the_limit_is_reached() {
    let allowance =
        VolumeAllowance::new(Octets::from_scaled(10, mediation::ByteScale::Giga).unwrap()).unwrap();

    let eight_gb = Octets::from_scaled(8, mediation::ByteScale::Giga).unwrap();
    assert_eq!(allowance.alert_at(), eight_gb);

    assert_eq!(
        allowance.status(Octets::from_scaled(7, mediation::ByteScale::Giga).unwrap()),
        AllowanceStatus::Within
    );
    assert_eq!(allowance.status(eight_gb), AllowanceStatus::AlertDue);
    assert!(allowance.status(eight_gb).requires_alert());
    assert_eq!(
        allowance.status(Octets::from_scaled(11, mediation::ByteScale::Giga).unwrap()),
        AllowanceStatus::Exceeded
    );
}

proptest! {
    /// However the counts fall, a customer inside either safe harbour is never
    /// warnable — the property the whole module exists to guarantee.
    #[test]
    fn the_safe_harbour_is_never_overridden(
        domestic_days in 0u32..200,
        roaming_days in 0u32..200,
        domestic_use in 0u64..1_000_000,
        roaming_use in 0u64..1_000_000,
    ) {
        let assessment = FairUseAssessment::from_counts(
            domestic_days,
            roaming_days,
            Octets::new(domestic_use),
            Octets::new(roaming_use),
            120,
            date!(2026 - 01 - 01),
        );

        if assessment.prevailing_domestic_presence()
            || assessment.prevailing_domestic_consumption()
        {
            prop_assert!(!assessment.may_warn());
            prop_assert!(!assessment.may_surcharge(
                Some(FairUseAlert::new(date!(2026 - 01 - 01))),
                date!(2026 - 12 - 31)
            ));
            prop_assert!(assessment.safe_harbour().is_some());
        } else {
            prop_assert!(assessment.may_warn());
            prop_assert_eq!(assessment.safe_harbour(), None);
        }
    }

    /// A surcharge is impossible without a warning, whatever the counts.
    #[test]
    fn no_surcharge_without_a_warning(
        domestic_days in 0u32..200,
        roaming_days in 0u32..200,
    ) {
        let assessment = FairUseAssessment::from_counts(
            domestic_days,
            roaming_days,
            Octets::ZERO,
            Octets::new(1),
            120,
            date!(2026 - 01 - 01),
        );
        prop_assert!(!assessment.may_surcharge(None, date!(2026 - 12 - 31)));
    }

    /// The alert always falls due strictly before the limit, so a customer is
    /// never first told at the moment they are cut off.
    #[test]
    fn the_alert_always_precedes_the_limit(limit in 100u64..u64::MAX / 2) {
        let allowance = VolumeAllowance::new(Octets::new(limit)).unwrap();
        prop_assert!(allowance.alert_at().get() < limit);
        // Exact to within the rounding: never above 80 %, never more than one
        // octet below it.
        let exact = (u128::from(limit) * 80) / 100;
        prop_assert_eq!(u128::from(allowance.alert_at().get()), exact);
        prop_assert_eq!(
            allowance.status(allowance.alert_at()),
            AllowanceStatus::AlertDue
        );
    }
}

// ── Retention ────────────────────────────────────────────────────────────────

const CALL: OffsetDateTime = datetime!(2026-07-01 12:00 UTC);
const INVOICED: OffsetDateTime = datetime!(2026-08-05 00:00 UTC);

/// § 10 TDDDG runs from the invoice. Anchoring on the call deletes the record
/// five weeks early — while a dispute over that invoice may still be open.
#[test]
fn a_billing_period_outlives_the_call_it_covers() {
    let six_months = Duration::days(183);

    let correct = RetentionPolicy::new(
        RetentionBasis::BillingTdddg10 {
            invoice_dispatched_at: INVOICED,
        },
        six_months,
    )
    .unwrap();
    let wrong_anchor =
        RetentionPolicy::new(RetentionBasis::OperatorPolicy { from: CALL }, six_months).unwrap();

    assert!(correct.deadline() > wrong_anchor.deadline());
    assert_eq!(
        correct.deadline().unwrap() - wrong_anchor.deadline().unwrap(),
        INVOICED - CALL
    );

    // On a day between the two deadlines, only the wrong anchor has expired.
    let between = wrong_anchor.deadline().unwrap() + Duration::days(1);
    assert!(wrong_anchor.is_expired(between));
    assert!(!correct.is_expired(between));
}

/// A record commonly sits under several bases, and the longest governs:
/// deleting while any of them still requires the data cannot be undone.
#[test]
fn a_longer_basis_outlives_the_billing_period() {
    let billing = RetentionPolicy::billing_tdddg10(INVOICED).unwrap();
    let dispute = RetentionPolicy::new(
        RetentionBasis::LawfulAccess {
            recorded_at: datetime!(2026-12-01 00:00 UTC),
        },
        Duration::days(365),
    )
    .unwrap();

    let schedule: RetentionSchedule = vec![billing, dispute].into_iter().collect();
    assert!(!schedule.is_held());
    assert_eq!(schedule.governing_policy(), Some(dispute));

    // The billing period lapsing does not make the record deletable.
    let after_billing = billing.deadline().unwrap() + Duration::days(1);
    assert!(billing.is_expired(after_billing));
    assert!(!schedule.is_deletable(after_billing));
    assert!(schedule.is_deletable(dispute.deadline().unwrap() + Duration::seconds(1)));
}

/// No basis to keep the data is not a licence to keep it indefinitely.
#[test]
fn a_record_with_no_basis_is_deletable_at_once() {
    let schedule = RetentionSchedule::new();
    assert!(schedule.is_deletable(CALL));
    assert_eq!(schedule.governing_deadline(), None);
}

proptest! {
    /// Whatever policies apply, a record is deletable exactly when every one of
    /// them has expired — never earlier.
    #[test]
    fn deletion_waits_for_the_last_policy(offsets in proptest::collection::vec(1i64..500, 1..8)) {
        let policies: Vec<RetentionPolicy> = offsets
            .iter()
            .map(|days| {
                RetentionPolicy::new(
                    RetentionBasis::OperatorPolicy { from: CALL },
                    Duration::days(*days),
                )
                .unwrap()
            })
            .collect();
        let schedule: RetentionSchedule = policies.iter().copied().collect();

        let longest = policies.iter().filter_map(|p| p.deadline()).max().unwrap();
        prop_assert_eq!(schedule.governing_deadline(), Some(longest));
        prop_assert_eq!(schedule.deletion(), Deletion::After(longest));

        prop_assert!(!schedule.is_deletable(longest));
        prop_assert!(schedule.is_deletable(longest + Duration::seconds(1)));

        // While any policy is unexpired, the record stays.
        for policy in &policies {
            if !policy.is_expired(longest) {
                prop_assert!(!schedule.is_deletable(longest));
            }
        }
    }
}

/// A hold has no deadline, so no instant makes the record deletable. The old
/// shape — a hold with a nominal duration — silently permitted deletion the
/// moment that nominal period ran out, while the dispute was still open.
#[test]
fn an_open_hold_is_never_deletable() {
    let billing = RetentionPolicy::new(
        RetentionBasis::BillingTdddg10 {
            invoice_dispatched_at: INVOICED,
        },
        Duration::days(183),
    )
    .unwrap();
    let dispute = RetentionPolicy::until_released(RetentionBasis::DisputeHold {
        raised_at: INVOICED + Duration::days(5),
    })
    .unwrap();

    let schedule: RetentionSchedule = vec![billing, dispute].into_iter().collect();
    assert!(schedule.is_held());
    assert!(schedule.deletion().is_blocked());
    assert_eq!(schedule.deletion().deadline(), None);
    assert!(!schedule.is_deletable(INVOICED + Duration::days(10_000)));

    // Releasing the hold is rebuilding the schedule without it.
    let released: RetentionSchedule = vec![billing].into_iter().collect();
    assert_eq!(
        released.deletion(),
        Deletion::After(billing.deadline().unwrap())
    );
    assert!(released.is_deletable(INVOICED + Duration::days(10_000)));

    // And only a hold may run without a duration.
    assert!(
        RetentionPolicy::until_released(RetentionBasis::OperatorPolicy { from: INVOICED })
            .is_none()
    );
}

/// No basis to keep the data is an instruction to delete, not a licence to
/// keep it forever.
#[test]
fn an_empty_schedule_says_delete_now() {
    let schedule = RetentionSchedule::new();
    assert_eq!(schedule.deletion(), Deletion::Immediate);
    assert!(schedule.is_deletable(INVOICED));
}

// ── Privacy ──────────────────────────────────────────────────────────────────

/// A deliberately trivial keyed digest, for tests only.
struct TestKey(u8);

impl Pseudonymiser for TestKey {
    fn digest(&self, input: &[u8]) -> [u8; 32] {
        let mut out = [self.0; 32];
        for (index, byte) in input.iter().enumerate() {
            out[index % 32] ^= byte.rotate_left((index % 8) as u32);
        }
        out
    }
}

/// A reduction is irreversible; a pseudonym is not. That is the distinction the
/// two types exist to keep visible.
#[test]
fn a_reduction_loses_information_and_a_pseudonym_does_not() {
    let a: Imsi = "262021234567890".parse().unwrap();
    let b: Imsi = "262029999999999".parse().unwrap();

    // Reduced: both become the same country, and nothing recovers which.
    assert_eq!(reduce_to_country(a), reduce_to_country(b));

    // Pseudonymised: distinct, because the key holder can still tell them apart.
    let key = TestKey(1);
    assert_ne!(key.pseudonymise(&a), key.pseudonymise(&b));
}

/// A pseudonymisation domain is a boundary: the same subscriber under two keys
/// is two unrelated values.
#[test]
fn two_keys_are_two_domains() {
    let supi: Supi = "imsi-262021234567890".parse().unwrap();
    assert_ne!(
        TestKey(1).pseudonymise(&supi),
        TestKey(2).pseudonymise(&supi)
    );
    assert_eq!(
        TestKey(1).pseudonymise(&supi),
        TestKey(1).pseudonymise(&supi)
    );
}

/// The warehouse shape: identity gone, reporting dimensions intact.
#[test]
fn a_reduced_subscriber_keeps_what_a_report_needs() {
    let supi: Supi = "imsi-262021234567890".parse().unwrap();
    let reduced = ReducedSubscriber::of(&supi, &TestKey(1), &NorthAmericanRule);

    assert_eq!(reduced.network.unwrap().to_string(), "262-02");
    assert_eq!(reduced.country.unwrap().to_string(), "262");

    // The pseudonym is stable, so a per-subscriber count still works …
    let again = ReducedSubscriber::of(&supi, &TestKey(1), &NorthAmericanRule);
    assert_eq!(reduced.supi, again.supi);
    // … and it is not the identity.
    assert_ne!(reduced.supi.to_string(), supi.to_string());
}

proptest! {
    /// Every subscriber in one country reduces to the same value, so the
    /// reduction is genuinely many-to-one and nothing distinguishes them
    /// afterwards.
    #[test]
    fn a_country_reduction_is_many_to_one(a in "[0-9]{12}", b in "[0-9]{12}") {
        let first: Imsi = format!("262{a}").parse().unwrap();
        let second: Imsi = format!("262{b}").parse().unwrap();

        prop_assert_eq!(
            reduce_to_country(first),
            reduce_to_country(second)
        );
        prop_assert_eq!(
            reduce_to_country(first).to_string(),
            "262"
        );
    }

    /// A pseudonym is a total function of the identity: same in, same out,
    /// which is what makes it usable as a key.
    #[test]
    fn pseudonyms_are_stable(rest in "[0-9]{12}") {
        let supi: Supi = format!("imsi-262{rest}").parse().unwrap();
        let key = TestKey(9);
        prop_assert_eq!(key.pseudonymise(&supi), key.pseudonymise(&supi));
    }
}

/// The gate that stops a provider concluding from thin data has to measure the
/// data, not the configuration. `ObservationWindow::months` already refuses
/// anything shorter than four months, so filling `observed_days` from the
/// window's length made `window_sufficient` a restatement of the caller's own
/// setting — always true, and blocking nothing.
#[test]
fn the_sufficiency_gate_measures_the_evidence_not_the_configuration() {
    let window = fairuse::ObservationWindow::minimum();
    let ending = date!(2026 - 07 - 28);

    // Three weeks of registration data inside a four-month window.
    let mut presence = Vec::new();
    let mut day = window.start(ending).unwrap();
    for _ in 0..21 {
        presence.push(PresenceDay::new(day, Region::Eea));
        day += Duration::days(1);
    }
    let usage = vec![record("2026-07-20", "234-15", 9_500)];

    let thin = fairuse::assess(&usage, &presence, home(), window, ending, &regions());

    assert_eq!(thin.observed_days(), 21);
    assert!(!thin.window_sufficient());
    assert!(
        !thin.may_warn(),
        "insufficient evidence favours the customer"
    );
    assert!(!thin.may_surcharge(
        Some(FairUseAlert::new(date!(2026 - 01 - 01))),
        date!(2026 - 12 - 31)
    ));
    assert_eq!(
        thin.safe_harbour(),
        Some(fairuse::SafeHarbour::WindowTooShort)
    );
}

/// A feed that reports a subscriber twice on one day has still observed one
/// day. Counting entries rather than days tilts a test with legal consequences
/// by however often the feed repeats itself.
#[test]
fn presence_counts_days_not_entries() {
    let window = fairuse::ObservationWindow::minimum();
    let ending = date!(2026 - 07 - 28);

    let mut presence = Vec::new();
    let mut day = window.start(ending).unwrap();
    for index in 0..window_days(window, ending) {
        // Every day abroad, but the first thirty are reported three times over.
        let region = if index < 30 {
            Region::Eea
        } else {
            Region::Domestic
        };
        presence.push(PresenceDay::new(day, region));
        if index < 30 {
            presence.push(PresenceDay::new(day, region));
            presence.push(PresenceDay::new(day, region));
        }
        day += Duration::days(1);
    }

    let assessment = fairuse::assess(&[], &presence, home(), window, ending, &regions());

    assert_eq!(assessment.observed_days(), window_days(window, ending));
    assert_eq!(assessment.roaming_presence_days(), 30);
    assert_eq!(
        assessment.domestic_presence_days(),
        window_days(window, ending) - 30
    );
    assert!(assessment.prevailing_domestic_presence());
}

/// A `Reduction` names a policy; it does not perform one. The three reductions
/// take different inputs and return different types, so a single `apply` would
/// have to ignore the variant it was called on — which is what it used to do,
/// returning a country from a model reduction.
#[test]
fn a_reduction_names_a_policy_and_says_what_it_needs() {
    use mediation::Reduction;

    assert!(Reduction::CountryOnly.applies_to_subscription());
    assert!(Reduction::NetworkOnly.applies_to_subscription());
    assert!(!Reduction::ModelOnly.applies_to_subscription());

    // Only the network reduction needs the assignment table, which is what a
    // configuration check wants to know before it runs.
    assert!(!Reduction::CountryOnly.needs_assignment_table());
    assert!(Reduction::NetworkOnly.needs_assignment_table());

    for reduction in Reduction::ALL {
        assert_eq!(reduction.as_str().parse::<Reduction>().unwrap(), *reduction);
    }
    assert_eq!(Reduction::ALL.len(), Reduction::CODES.len());
}
