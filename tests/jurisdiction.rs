//! Which country's rules apply, and which question is being asked.
//!
//! Every regime that attaches to a record by country — roaming regulation,
//! retention, lawful interception, tax — needs a country, and they do not all
//! need the *same* country. These tests pin the difference.

use mediation::{
    Ambiguity, ChargedParty, Jurisdiction, Mcc, Ncgi, Nci, PlmnId, RatType, RecordType, Region,
    Regions, Rule, SessionContext, Severity, SourceRef, SubscriberRef, Supi, UsageRecord,
    UserLocation, ValidationConfig, customer_country_presumption, is_abroad, usage_country,
    validate_records,
};
use time::macros::datetime;

fn record(imsi: &str, serving: &str, home: &str) -> UsageRecord {
    UsageRecord::new(
        RecordType::PduSession,
        ChargedParty::Subscriber(SubscriberRef::new(imsi.parse().unwrap())),
        "2026-07-28T10:00:00+02:00".parse().unwrap(),
        SourceRef::new("pgw-01".parse().unwrap(), 0),
    )
    .with_networks(serving.parse().unwrap(), home.parse().unwrap())
}

fn cell(plmn: &str) -> UserLocation {
    UserLocation::Ncgi(Ncgi::new(
        plmn.parse().unwrap(),
        Nci::new(0x0_1234_5678).unwrap(),
    ))
}

fn mcc(code: &str) -> Mcc {
    code.parse().expect("a valid country code")
}

// ── The two questions ────────────────────────────────────────────────────────

/// A German SIM used in Spain is governed by Spanish telecom rules and taxed in
/// Germany. One number cannot say both, which is why there are two functions.
#[test]
fn the_regulator_and_the_tax_authority_are_in_different_countries() {
    let roamer = record("imsi-262021234567890", "214-07", "262-02");

    assert_eq!(usage_country(&roamer), Jurisdiction::Country(mcc("214")));
    assert_eq!(
        customer_country_presumption(&roamer),
        Jurisdiction::Country(mcc("262"))
    );
    assert_ne!(
        usage_country(&roamer),
        customer_country_presumption(&roamer)
    );
    assert_eq!(is_abroad(&roamer), Some(true));
}

/// The two agree for domestic traffic, which is exactly why conflating them
/// survives testing and then bills a whole roaming population wrong.
#[test]
fn domestic_traffic_hides_the_distinction() {
    let domestic = record("imsi-262021234567890", "262-02", "262-02");

    assert_eq!(
        usage_country(&domestic),
        customer_country_presumption(&domestic)
    );
    assert_eq!(is_abroad(&domestic), Some(false));
}

// ── Roaming is a network question; abroad is a country question ──────────────

/// The distinction that decides whether a customer may be surcharged: a
/// subscriber on a second network at home is roaming and is not abroad.
#[test]
fn a_national_roamer_is_roaming_and_is_not_abroad() {
    let national = record("imsi-262021234567890", "262-01", "262-02");

    assert_eq!(national.is_roaming(), Some(true));
    assert_eq!(is_abroad(&national), Some(false));

    // A fair-use resolver that compared networks would file this under
    // regulated roaming and inflate the ratio against the customer.
    let by_country: &dyn Fn(PlmnId, PlmnId) -> Region = &|serving: PlmnId, home: PlmnId| {
        if serving.is_same_country(home) {
            Region::Domestic
        } else {
            Region::Eea
        }
    };
    let by_network: &dyn Fn(PlmnId, PlmnId) -> Region = &|serving: PlmnId, home: PlmnId| {
        if serving.is_same_network(home) {
            Region::Domestic
        } else {
            Region::Eea
        }
    };

    let serving = national.serving_plmn.unwrap();
    let home = national.home_plmn.unwrap();
    assert_eq!(by_country.region_of(serving, home), Region::Domestic);
    assert_eq!(
        by_network.region_of(serving, home),
        Region::Eea,
        "the network comparison is the wrong rule, and wrong against the customer"
    );
}

// ── The VAT presumption reads the SIM ────────────────────────────────────────

/// Art. 24b(b) names the MCC of the IMSI on the SIM. The home PLMN is a routing
/// fact that an MVNO arrangement can point somewhere else entirely.
#[test]
fn the_presumption_follows_the_sim_not_the_host_network() {
    let mvno = record("imsi-262021234567890", "262-02", "232-01");

    assert_eq!(
        customer_country_presumption(&mvno),
        Jurisdiction::Country(mcc("262"))
    );
    assert_eq!(usage_country(&mvno), Jurisdiction::Country(mcc("262")));
}

/// A non-3GPP subscription has no IMSI, so there is no presumption to make —
/// and falling back to the serving network would answer the other question.
#[test]
fn no_imsi_means_no_presumption_rather_than_the_wrong_one() {
    let nai = UsageRecord::new(
        RecordType::PduSession,
        ChargedParty::Subscriber(SubscriberRef::new(Supi::NetworkSpecific(
            "user@example.net".parse().unwrap(),
        ))),
        "2026-07-28T10:00:00+02:00".parse().unwrap(),
        SourceRef::new("pgw-01".parse().unwrap(), 0),
    )
    .with_networks("214-07".parse().unwrap(), "262-02".parse().unwrap());

    assert_eq!(
        customer_country_presumption(&nai),
        Jurisdiction::Unresolved(Ambiguity::NoEvidence)
    );
    assert_eq!(usage_country(&nai), Jurisdiction::Country(mcc("214")));
    assert_eq!(is_abroad(&nai), None, "one country is not two");
}

// ── Ambiguity is not one thing ───────────────────────────────────────────────

/// A satellite beam crosses borders, so the serving network's country describes
/// the operator's licence rather than the subscriber's position — and 3GPP has
/// an open study on exactly this.
#[test]
fn extra_territorial_access_refuses_to_name_a_country() {
    let orbital = record("imsi-262021234567890", "214-07", "262-02")
        .with_session(SessionContext::none().with_rat(RatType::NonTerrestrial));

    assert_eq!(
        usage_country(&orbital),
        Jurisdiction::Unresolved(Ambiguity::ExtraTerritorialAccess)
    );
    assert!(!usage_country(&orbital).is_resolved());
    assert!(!Ambiguity::ExtraTerritorialAccess.is_contradiction());

    // The SIM did not move, so the customer question still answers.
    assert_eq!(
        customer_country_presumption(&orbital),
        Jurisdiction::Country(mcc("262"))
    );
    assert_eq!(is_abroad(&orbital), None);
}

/// Silence and contradiction are different faults with different remedies, so
/// they are different variants rather than one `None`.
#[test]
fn the_reasons_for_not_knowing_are_distinguished() {
    let mut silent = record("imsi-262021234567890", "214-07", "262-02");
    silent.serving_plmn = None;
    assert_eq!(
        usage_country(&silent),
        Jurisdiction::Unresolved(Ambiguity::NoEvidence)
    );

    let contradictory = record("imsi-262021234567890", "214-07", "262-02")
        .with_session(SessionContext::none().with_location(cell("232-01")));
    assert_eq!(
        usage_country(&contradictory),
        Jurisdiction::Unresolved(Ambiguity::Conflicting {
            serving: mcc("214"),
            reported: mcc("232"),
        })
    );

    assert!(!Ambiguity::NoEvidence.is_contradiction());
    assert!(
        usage_country(&contradictory)
            .ambiguity()
            .unwrap()
            .is_contradiction()
    );
}

/// A contradiction is a data fault, so it reaches the rule catalogue and stops
/// the record being billed — the country decides which regulator, tax authority
/// and retention rule applies.
#[test]
fn a_country_disagreement_blocks_the_record() {
    let contradictory = record("imsi-262021234567890", "214-07", "262-02")
        .with_session(SessionContext::none().with_location(cell("232-01")));

    let report = validate_records(
        std::slice::from_ref(&contradictory),
        &ValidationConfig::default(),
        datetime!(2026-07-28 12:00 UTC),
    );

    assert_eq!(
        report.severity_of(Rule::JurisdictionConflict),
        Some(Severity::Blocking)
    );
    assert_eq!(Rule::JurisdictionConflict.code(), "V21");

    // Two sources that agree produce no finding at all.
    let agreeing = record("imsi-262021234567890", "214-07", "262-02")
        .with_session(SessionContext::none().with_location(cell("214-07")));
    let clean = validate_records(
        std::slice::from_ref(&agreeing),
        &ValidationConfig::default(),
        datetime!(2026-07-28 12:00 UTC),
    );
    assert_eq!(clean.severity_of(Rule::JurisdictionConflict), None);
}

/// Determinism: the country a record yields depends on the record alone, so an
/// archive replayed years later reaches the verdict it originally did.
#[test]
fn jurisdiction_is_replayable() {
    let roamer = record("imsi-262021234567890", "214-07", "262-02")
        .with_session(SessionContext::none().with_location(cell("214-07")));

    for _ in 0..3 {
        assert_eq!(usage_country(&roamer), Jurisdiction::Country(mcc("214")));
        assert_eq!(
            customer_country_presumption(&roamer),
            Jurisdiction::Country(mcc("262"))
        );
    }
}
