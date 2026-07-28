//! The serde representation is public API, covered by semver.
//!
//! Persisting these values means relying on a wire format, so every enum tag
//! and every struct field name is pinned here **literally**. A rename that
//! would silently orphan a consumer's stored data fails this file instead.
//!
//! Two choices are pinned deliberately:
//!
//! - **Quantities are JSON numbers, identifiers are JSON strings.** An
//!   [`Octets`] is a count and a [`PlmnId`] is not, and rendering the latter as
//!   a number would strip the leading zeros that distinguish two networks.
//! - **Sum types are internally tagged** with a readable `kind` discriminator,
//!   so adding a variant does not renumber the others.

#![cfg(feature = "serde")]

use mediation::{Ambiguity, Jurisdiction};
use mediation::{
    Apportionment, ByteScale, CdrTimestamp, ChargedParty, DstAdjustment, Ecgi, Eci, EnbKind, Gpsi,
    Iccid, Imei, Imeisv, Imsi, Indication, Lac, MsTimeZone, Msisdn, Ncgi, Nci, Octets, Pei, PlmnId,
    QuarterHourOffset, RecordType, Seconds, ServiceDomain, Signal, SourceRef, SubscriberRef, Supi,
    TacWidth, TadigCode, TenantRef, TrackingAreaCode, UnattributableReason, UsageRecord,
    UserLocation, VolumePair,
};
use serde_json::{Value, json, to_string, to_value};

/// Serialises to exactly `expected`, and reads back to the same value.
fn assert_wire<T>(value: &T, expected: &str)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let written = to_string(value).expect("serialises");
    assert_eq!(written, expected, "wire form changed");
    let read: T = serde_json::from_str(&written).expect("deserialises");
    assert_eq!(&read, value, "round trip changed the value");
}

// ── Scalars ──────────────────────────────────────────────────────────────────

#[test]
fn time_types_serialise_as_their_canonical_strings() {
    assert_wire(
        &"+05:45".parse::<QuarterHourOffset>().unwrap(),
        "\"+05:45\"",
    );
    assert_wire(&QuarterHourOffset::UTC, "\"+00:00\"");
    assert_wire(&DstAdjustment::PlusOneHour, "\"PLUS_ONE_HOUR\"");
    assert_wire(&DstAdjustment::None, "\"NONE\"");
    assert_wire(&"+02:00/1".parse::<MsTimeZone>().unwrap(), "\"+02:00/1\"");
    assert_wire(
        &"2026-07-28T06:00:00+05:45".parse::<CdrTimestamp>().unwrap(),
        "\"2026-07-28T06:00:00+05:45\"",
    );
}

/// Quantities are numbers. A duration rendered as a string would invite a
/// consumer to concatenate it.
#[test]
fn quantities_serialise_as_numbers() {
    assert_wire(&Octets::new(1_048_576), "1048576");
    assert_wire(&Seconds::new(900), "900");

    // Past the point a double can count, the integer is still exact on the wire.
    assert_wire(&Octets::new(9_007_199_254_740_993), "9007199254740993");
}

/// Identifiers are strings. A network identity rendered as a number would lose
/// the leading zero that distinguishes 262-02 from 262-2.
#[test]
fn network_identities_serialise_as_strings() {
    assert_wire(&"262-02".parse::<PlmnId>().unwrap(), "\"262-02\"");
    assert_wire(&"310-260".parse::<PlmnId>().unwrap(), "\"310-260\"");
    assert_wire(&"DEUD1".parse::<TadigCode>().unwrap(), "\"DEUD1\"");
}

#[test]
fn subscriber_identities_serialise_as_strings() {
    assert_wire(
        &"262021234567890".parse::<Imsi>().unwrap(),
        "\"262021234567890\"",
    );
    assert_wire(
        &"491701234567".parse::<Msisdn>().unwrap(),
        "\"491701234567\"",
    );
    assert_wire(
        &"490154203237518".parse::<Imei>().unwrap(),
        "\"490154203237518\"",
    );
    assert_wire(
        &"4901542032375120".parse::<Imeisv>().unwrap(),
        "\"4901542032375120\"",
    );
    assert_wire(
        &"8949012345678901234".parse::<Iccid>().unwrap(),
        "\"8949012345678901234\"",
    );
    assert_wire(
        &"imsi-262021234567890".parse::<Supi>().unwrap(),
        "\"imsi-262021234567890\"",
    );
    assert_wire(
        &"msisdn-491701234567".parse::<Gpsi>().unwrap(),
        "\"msisdn-491701234567\"",
    );
    assert_wire(
        &"imei-490154203237518".parse::<Pei>().unwrap(),
        "\"imei-490154203237518\"",
    );
}

#[test]
fn vocabulary_codes_are_pinned() {
    assert_wire(&ServiceDomain::Data, "\"DATA\"");
    assert_wire(&ServiceDomain::NetworkApi, "\"NETWORK_API\"");
    assert_wire(&RecordType::PduSession, "\"PDU_SESSION\"");
    assert_wire(&RecordType::MobileOriginatedCall, "\"MO_CALL\"");
    assert_wire(&ByteScale::Mebi, "\"MiB\"");
    assert_wire(&ByteScale::Mega, "\"MB\"");
    assert_wire(
        &UnattributableReason::ConcealedIdentityOnly,
        "\"CONCEALED_IDENTITY_ONLY\"",
    );
    assert_wire(&Signal::ImpossibleTravel, "\"IMPOSSIBLE_TRAVEL\"");
    assert_wire(&Apportionment::ProRata, "\"PRO_RATA\"");
    assert_wire(&EnbKind::Macro, "\"MACRO\"");
    assert_wire(&TacWidth::FiveGs, "\"5GS\"");
    assert_wire(&Signal::AitSignature, "\"AIT_SIGNATURE\"");
}

/// Every variant of every vocabulary has a pinned tag, so a new variant cannot
/// ship with a half-wired representation.
#[test]
fn every_vocabulary_variant_has_a_stable_tag() {
    for (values, codes) in [
        (
            ServiceDomain::ALL.iter().map(to_value).collect::<Vec<_>>(),
            ServiceDomain::CODES,
        ),
        (
            RecordType::ALL.iter().map(to_value).collect::<Vec<_>>(),
            RecordType::CODES,
        ),
        (
            DstAdjustment::ALL.iter().map(to_value).collect::<Vec<_>>(),
            DstAdjustment::CODES,
        ),
        (
            UnattributableReason::ALL
                .iter()
                .map(to_value)
                .collect::<Vec<_>>(),
            UnattributableReason::CODES,
        ),
        (
            ByteScale::ALL.iter().map(to_value).collect::<Vec<_>>(),
            ByteScale::CODES,
        ),
        (
            Signal::ALL.iter().map(to_value).collect::<Vec<_>>(),
            Signal::CODES,
        ),
        (
            Apportionment::ALL.iter().map(to_value).collect::<Vec<_>>(),
            Apportionment::CODES,
        ),
        (
            EnbKind::ALL.iter().map(to_value).collect::<Vec<_>>(),
            EnbKind::CODES,
        ),
        (
            TacWidth::ALL.iter().map(to_value).collect::<Vec<_>>(),
            TacWidth::CODES,
        ),
    ] {
        assert_eq!(values.len(), codes.len());
        for (value, code) in values.into_iter().zip(codes) {
            assert_eq!(value.unwrap(), Value::String((*code).to_owned()));
        }
    }
}

// ── Composites ───────────────────────────────────────────────────────────────

/// Both directions are named, and neither is optional — a total alone would be
/// unrecoverable.
#[test]
fn a_volume_pair_names_both_directions() {
    assert_wire(
        &VolumePair::new(Octets::new(100), Octets::new(900)),
        r#"{"uplink_octets":100,"downlink_octets":900}"#,
    );
}

#[test]
fn provenance_omits_absent_optional_fields() {
    let bare = SourceRef::new("pgw-fra-01".parse().unwrap(), 0);
    assert_wire(&bare, r#"{"node_id":"pgw-fra-01","record_index":0}"#);

    let full = SourceRef::new("pgw-fra-01".parse().unwrap(), 7)
        .with_file_name("CDR-20260728-0001")
        .with_file_sequence(42);
    assert_wire(
        &full,
        r#"{"node_id":"pgw-fra-01","file_name":"CDR-20260728-0001","file_sequence":42,"record_index":7}"#,
    );
}

/// The charged party is internally tagged with a readable discriminator, so a
/// new variant does not renumber the existing ones.
#[test]
fn the_charged_party_is_internally_tagged() {
    let subscriber = SubscriberRef::new("imsi-262021234567890".parse().unwrap());

    assert_wire(
        &ChargedParty::Subscriber(subscriber.clone()),
        r#"{"kind":"SUBSCRIBER","subscriber":{"supi":"imsi-262021234567890"}}"#,
    );
    assert_wire(
        &ChargedParty::Tenant(TenantRef::new("mvno-alpha".parse().unwrap())),
        r#"{"kind":"TENANT","tenant":"mvno-alpha"}"#,
    );
    assert_wire(
        &ChargedParty::ApiConsumer {
            developer: "acme-dev".parse().unwrap(),
            subject: None,
        },
        r#"{"kind":"API_CONSUMER","developer":"acme-dev"}"#,
    );
    assert_wire(
        &ChargedParty::NetworkFunction("smf-01".parse().unwrap()),
        r#"{"kind":"NETWORK_FUNCTION","nf_instance":"smf-01"}"#,
    );
    assert_wire(
        &ChargedParty::Unattributable(UnattributableReason::IdentityAbsent),
        r#"{"kind":"UNATTRIBUTABLE","reason":"IDENTITY_ABSENT"}"#,
    );
}

/// The whole record, pinned field by field.
#[test]
fn a_usage_record_has_a_stable_shape() {
    let record = UsageRecord::new(
        RecordType::PduSession,
        ChargedParty::Subscriber(
            SubscriberRef::new("imsi-262021234567890".parse().unwrap())
                .with_gpsi("msisdn-491701234567".parse().unwrap())
                .with_tenant(TenantRef::new("mvno-alpha".parse().unwrap())),
        ),
        "2026-07-28T10:00:00+02:00".parse().unwrap(),
        SourceRef::new("pgw-fra-01".parse().unwrap(), 3).with_file_sequence(9),
    )
    .with_closed("2026-07-28T10:30:00+02:00".parse().unwrap())
    .with_duration(Seconds::new(1_800))
    .with_volume(VolumePair::new(Octets::new(4_096), Octets::new(65_536)))
    .with_equipment("imei-490154203237518".parse().unwrap())
    .with_networks("310-260".parse().unwrap(), "262-02".parse().unwrap());

    let expected = json!({
        "record_type": "PDU_SESSION",
        "party": {
            "kind": "SUBSCRIBER",
            "subscriber": {
                "supi": "imsi-262021234567890",
                "gpsi": "msisdn-491701234567",
                "tenant": "mvno-alpha"
            }
        },
        "equipment": "imei-490154203237518",
        "serving_plmn": "310-260",
        "home_plmn": "262-02",
        "opened": "2026-07-28T10:00:00+02:00",
        "closed": "2026-07-28T10:30:00+02:00",
        "duration_s": 1800,
        "volume": { "uplink_octets": 4096, "downlink_octets": 65536 },
        "events": null,
        "provenance": {
            "node_id": "pgw-fra-01",
            "file_sequence": 9,
            "record_index": 3
        }
    });

    let mut expected = expected;
    // Absent optionals are omitted rather than written as null.
    expected.as_object_mut().unwrap().remove("events");

    assert_eq!(to_value(&record).unwrap(), expected);

    let round_tripped: UsageRecord = serde_json::from_value(to_value(&record).unwrap()).unwrap();
    assert_eq!(round_tripped, record);
}

/// A record read back from storage keeps the offset it was written with, not
/// the reader's — the timestamp is local time, and normalising it to UTC on the
/// way through would discard the wall-clock reading an operator quotes back.
#[test]
fn a_stored_timestamp_keeps_its_offset() {
    let stored = "\"2026-07-28T00:30:00+05:45\"";
    let read: CdrTimestamp = serde_json::from_str(stored).unwrap();
    assert_eq!(read.offset().to_string(), "+05:45");
    assert_eq!(read.local_date().to_string(), "2026-07-28");
    assert_eq!(read.to_utc().date().to_string(), "2026-07-27");
    assert_eq!(to_string(&read).unwrap(), stored);
}

/// An indication is evidence, and the fields that carry it are pinned: a
/// consumer storing why a subscriber was queued must be able to read it back.
#[test]
fn an_indication_carries_its_evidence() {
    let indication = Indication::new(Signal::ImpossibleTravel, vec![0, 4], "262 to 310 in 20m");

    assert_eq!(
        to_value(&indication).unwrap(),
        json!({
            "signal": "IMPOSSIBLE_TRAVEL",
            "records": [0, 4],
            "detail": "262 to 310 in 20m"
        })
    );

    let read: Indication = serde_json::from_value(to_value(&indication).unwrap()).unwrap();
    assert_eq!(read, indication);
}

/// A location is tagged on `kind`, so a 2G cell and a 5G one never read alike
/// and adding an access generation does not renumber the others.
#[test]
fn a_user_location_is_internally_tagged() {
    let cell = UserLocation::Ncgi(Ncgi::new(
        "262-02".parse().unwrap(),
        Nci::new(0x0_1234_5678).unwrap(),
    ));

    assert_eq!(
        to_value(cell).unwrap(),
        json!({ "kind": "NCGI", "plmn": "262-02", "nci": 0x0_1234_5678u64 })
    );

    let read: UserLocation = serde_json::from_value(to_value(cell).unwrap()).unwrap();
    assert_eq!(read, cell);

    let area = UserLocation::Tai(mediation::Tai::new(
        "262-02".parse().unwrap(),
        TrackingAreaCode::eps(0x00AB).unwrap(),
    ));
    assert_eq!(
        to_value(area).unwrap(),
        json!({ "kind": "TAI", "plmn": "262-02", "tac": { "value": 0x00AB, "width": "EPS" } })
    );
    assert_eq!(
        serde_json::from_value::<UserLocation>(to_value(area).unwrap()).unwrap(),
        area
    );
}

/// A cell identity is a number, and one that does not fit its width is
/// rejected on the way in rather than truncated into a different cell.
#[test]
fn out_of_range_identities_are_refused_not_truncated() {
    assert!(serde_json::from_value::<Eci>(json!(1u64 << 28)).is_err());
    assert!(serde_json::from_value::<Eci>(json!((1u64 << 28) - 1)).is_ok());
    assert!(serde_json::from_value::<Nci>(json!(1u64 << 36)).is_err());

    // And the two reserved area codes never come back as areas.
    assert!(serde_json::from_value::<Lac>(json!(0xFFFE)).is_err());
    assert!(serde_json::from_value::<Lac>(json!(0x0000)).is_err());
    assert!(serde_json::from_value::<Lac>(json!(0x1234)).is_ok());

    // A stored width that the value cannot hold is a refusal, not a silent
    // widening.
    assert!(
        serde_json::from_value::<TrackingAreaCode>(json!({ "value": 0x01_0000, "width": "EPS" }))
            .is_err()
    );

    // An E-UTRAN cell global identity round-trips through its parts.
    let ecgi = Ecgi::new("262-02".parse().unwrap(), Eci::new(0x0012_3456).unwrap());
    assert_eq!(
        to_value(ecgi).unwrap(),
        json!({ "plmn": "262-02", "eci": 0x0012_3456 })
    );
    assert_eq!(
        serde_json::from_value::<Ecgi>(to_value(ecgi).unwrap()).unwrap(),
        ecgi
    );
}

/// A country and the reason there is not one must never read alike: they are
/// tagged, and the contradicting case carries the two countries that disagreed.
#[test]
fn a_jurisdiction_distinguishes_a_country_from_a_reason() {
    let resolved = Jurisdiction::Country("262".parse().unwrap());
    assert_eq!(
        to_value(resolved).unwrap(),
        json!({ "kind": "COUNTRY", "mcc": "262" })
    );

    let contradictory = Jurisdiction::Unresolved(Ambiguity::Conflicting {
        serving: "214".parse().unwrap(),
        reported: "232".parse().unwrap(),
    });
    assert_eq!(
        to_value(contradictory).unwrap(),
        json!({
            "kind": "UNRESOLVED",
            "ambiguity": { "reason": "CONFLICTING", "serving": "214", "reported": "232" }
        })
    );

    for value in [
        resolved,
        contradictory,
        Jurisdiction::Unresolved(Ambiguity::NoEvidence),
        Jurisdiction::Unresolved(Ambiguity::ExtraTerritorialAccess),
    ] {
        assert_eq!(
            serde_json::from_value::<Jurisdiction>(to_value(value).unwrap()).unwrap(),
            value
        );
    }
}
