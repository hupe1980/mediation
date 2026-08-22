//! The User Location Information element, on the wire and as a string.
//!
//! Every identity in a ULI begins with the same three PLMN octets and then
//! differs in width. A decoder that gets one width wrong does not fail: it
//! reads the *next* identity's network octets as this one's location area and
//! produces a plausible cell in the wrong country. So these tests check the
//! widths, the field order, and the round trip — not just that something
//! decoded.

use mediation::*;

fn plmn(s: &str) -> PlmnId {
    s.parse().unwrap()
}

// ── PLMN identity ────────────────────────────────────────────────────────────

/// The filler nibble is the only thing on the wire that says how wide the MNC
/// is — which is why this decoder needs no assignment table where
/// [`Imsi::plmn`] does.
#[test]
fn the_wire_carries_the_mnc_width_that_a_digit_string_loses() {
    let two_digit = PlmnId::from_bcd(&[0x62, 0xF2, 0x20]).unwrap();
    assert_eq!(two_digit.to_string(), "262-02");
    assert_eq!(two_digit.mnc().width(), 2);

    let three_digit = PlmnId::from_bcd(&[0x13, 0x00, 0x62]).unwrap();
    assert_eq!(three_digit.to_string(), "310-260");
    assert_eq!(three_digit.mnc().width(), 3);
}

// ── The identities ───────────────────────────────────────────────────────────

#[test]
fn every_identity_round_trips_through_its_octets() {
    let cgi = Cgi::new(
        plmn("262-02"),
        Lac::new(0x1A2B).unwrap(),
        CellId::new(0x00FF),
    );
    assert_eq!(Cgi::from_bytes(&cgi.to_bytes()).unwrap(), cgi);
    assert_eq!(cgi.to_bytes().len(), Cgi::OCTETS);

    let sai = Sai::new(
        plmn("262-02"),
        Lac::new(0x1A2B).unwrap(),
        ServiceAreaCode::new(7),
    );
    assert_eq!(Sai::from_bytes(&sai.to_bytes()).unwrap(), sai);

    let rai = Rai::new(
        plmn("262-02"),
        Lac::new(0x1A2B).unwrap(),
        RoutingAreaCode::new(5),
    );
    assert_eq!(Rai::from_bytes(&rai.to_bytes()).unwrap(), rai);

    let lai = Lai::new(plmn("262-02"), Lac::new(0x1A2B).unwrap());
    assert_eq!(Lai::from_bytes(&lai.to_bytes()).unwrap(), lai);

    let ecgi = Ecgi::new(plmn("262-02"), Eci::new(0x0A1_B2C3).unwrap());
    assert_eq!(Ecgi::from_bytes(&ecgi.to_bytes()).unwrap(), ecgi);

    let ncgi = Ncgi::new(plmn("262-02"), Nci::new(0x0_1234_5678).unwrap());
    assert_eq!(Ncgi::from_bytes(&ncgi.to_bytes()).unwrap(), ncgi);
}

/// TS 29.274 pads the one-octet routing area code with a spare octet, which
/// every other specification carrying an RAI does not. A six-octet reader walks
/// off the end of the field.
#[test]
fn a_routing_area_identity_is_seven_octets_inside_a_uli() {
    let rai = Rai::new(
        plmn("262-02"),
        Lac::new(0x1A2B).unwrap(),
        RoutingAreaCode::new(5),
    );
    let octets = rai.to_bytes();
    assert_eq!(octets.len(), 7);
    assert_eq!(octets[6], Rai::SPARE);
    assert!(
        Rai::from_bytes(&octets[..6]).is_err(),
        "six octets is not an RAI"
    );
}

/// The width a tracking area code was assigned at is taken from the field
/// length and kept, so an EPS code never silently becomes a 5GS one.
#[test]
fn a_tracking_area_keeps_the_width_it_arrived_at() {
    let eps = Tai::new(plmn("262-02"), TrackingAreaCode::eps(0x002A).unwrap());
    let mut buf = [0u8; 8];
    let written = eps.to_bytes(&mut buf).unwrap();
    assert_eq!(written, Tai::EPS_OCTETS);
    assert_eq!(Tai::from_bytes(&buf[..written]).unwrap(), eps);

    let five_gs = Tai::new(
        plmn("262-02"),
        TrackingAreaCode::five_gs(0x00_002A).unwrap(),
    );
    let written = five_gs.to_bytes(&mut buf).unwrap();
    assert_eq!(written, Tai::FIVE_GS_OCTETS);
    assert_eq!(Tai::from_bytes(&buf[..written]).unwrap(), five_gs);

    // Same number, different widths, and the crate refuses to call them equal.
    assert_ne!(eps, five_gs);
    assert!(eps.tac().is_same_area(five_gs.tac()));
}

/// The reserved codes mean *no valid area*. They reach a record when an element
/// copies its own "unknown" marker into the field, and reading one as an area
/// puts real usage somewhere that does not exist.
#[test]
fn a_reserved_area_code_is_refused_rather_than_decoded() {
    for lac in [0x0000u16, 0xFFFE] {
        let bytes = [0x62, 0xF2, 0x20, (lac >> 8) as u8, lac as u8, 0x00, 0x01];
        assert!(Cgi::from_bytes(&bytes).is_err(), "LAC {lac:04X}");
    }
    // 0xFFFF is not reserved and decodes normally.
    assert!(Cgi::from_bytes(&[0x62, 0xF2, 0x20, 0xFF, 0xFF, 0x00, 0x01]).is_ok());
}

/// The cell identity is 28 bits, so the top nibble of its first octet is spare.
/// Elements do set it, and folding it into the identity produces a cell that
/// does not exist.
#[test]
fn the_spare_nibble_of_an_ecgi_is_masked_not_absorbed() {
    let clean = Ecgi::from_bytes(&[0x62, 0xF2, 0x20, 0x0A, 0x1B, 0x2C, 0x3D]).unwrap();
    let dirty = Ecgi::from_bytes(&[0x62, 0xF2, 0x20, 0xFA, 0x1B, 0x2C, 0x3D]).unwrap();
    assert_eq!(clean, dirty);
    assert_eq!(clean.eci().to_string(), "A1B2C3D");
}

// ── The element ──────────────────────────────────────────────────────────────

/// A ULI commonly reports more than one identity, and the fields appear in a
/// fixed order that is *not* the flag order.
#[test]
fn an_element_carrying_two_identities_decodes_both() {
    let field = [
        0x18, // TAI and ECGI
        0x62, 0xF2, 0x20, 0x00, 0x2A, // TAI first, five octets
        0x62, 0xF2, 0x20, 0x00, 0xA1, 0xB2, 0xC3, // then ECGI, seven
    ];

    let uli = Uli::decode(&field).unwrap();
    assert_eq!(uli.tai.unwrap().tac().get(), 0x2A);
    assert_eq!(uli.ecgi.unwrap().eci().get(), 0x00A1_B2C3);
    assert_eq!(uli.mcc().unwrap().unwrap().to_string(), "262");

    // A cell is more precise than the tracking area containing it.
    assert!(matches!(uli.most_precise(), Some(UserLocation::Ecgi(_))));

    let mut out = [0u8; 32];
    let written = uli.encode(&mut out).unwrap();
    assert_eq!(&out[..written], &field);
}

#[test]
fn every_flag_combination_round_trips() {
    let full = Uli {
        cgi: Some(Cgi::new(
            plmn("262-02"),
            Lac::new(0x1A2B).unwrap(),
            CellId::new(1),
        )),
        sai: Some(Sai::new(
            plmn("262-02"),
            Lac::new(0x1A2B).unwrap(),
            ServiceAreaCode::new(2),
        )),
        rai: Some(Rai::new(
            plmn("262-02"),
            Lac::new(0x1A2B).unwrap(),
            RoutingAreaCode::new(3),
        )),
        tai: Some(Tai::new(
            plmn("262-02"),
            TrackingAreaCode::eps(0x002A).unwrap(),
        )),
        ecgi: Some(Ecgi::new(plmn("262-02"), Eci::new(0x0A1_B2C3).unwrap())),
        lai: Some(Lai::new(plmn("262-02"), Lac::new(0x1A2B).unwrap())),
        macro_enb: Some(MacroEnbId::new(plmn("262-02"), 0x0_ABCD).unwrap()),
        extended_macro_enb: Some(
            ExtendedMacroEnbId::new(plmn("262-02"), ExtendedEnbKind::Short, 0x0_1234).unwrap(),
        ),
    };

    let mut out = [0u8; 64];
    let written = full.encode(&mut out).unwrap();
    assert_eq!(written, full.encoded_len());
    assert_eq!(out[0], 0xFF, "every flag set");
    assert_eq!(Uli::decode(&out[..written]).unwrap(), full);
}

/// An element whose flags promise a field the octets do not hold is truncated,
/// and a decoder must say so rather than read whatever follows.
#[test]
fn a_truncated_element_is_reported() {
    assert!(
        Uli::decode(&[]).is_err(),
        "an element is at least a flag octet"
    );
    assert!(
        Uli::decode(&[0x01, 0x62, 0xF2]).is_err(),
        "CGI promised, three octets given"
    );
    assert!(Uli::decode(&[0x00]).unwrap().is_empty());
    assert_eq!(Uli::decode(&[0x00]).unwrap().most_precise(), None);
}

/// The element is declared extensible, so a field a later release adds must not
/// make today's records undecodable.
#[test]
fn trailing_octets_are_ignored_rather_than_rejected() {
    let mut field = vec![0x08, 0x62, 0xF2, 0x20, 0x00, 0x2A];
    field.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
    assert!(Uli::decode(&field).unwrap().tai.is_some());
}

/// Two identities in one element naming different countries is a fault, not a
/// tie to break — exactly as in `jurisdiction`.
#[test]
fn a_self_contradicting_element_fixes_no_country() {
    let uli = Uli {
        tai: Some(Tai::new(
            plmn("262-02"),
            TrackingAreaCode::eps(0x002A).unwrap(),
        )),
        ecgi: Some(Ecgi::new(plmn("214-07"), Eci::new(1).unwrap())),
        ..Uli::new()
    };
    // Not `Ok(None)`: the element said something, and what it said cannot
    // both be true. A caller who cannot tell those apart files the usage
    // under a country no evidence supports.
    let conflict = uli.mcc().expect_err("two countries in one element");
    assert_eq!(conflict.first.to_string(), "262");
    assert_eq!(conflict.second.to_string(), "214");
    assert_eq!(Uli::new().mcc(), Ok(None), "silence is not a contradiction");
}

/// The ULI element's TAI field is five octets of EPS tracking area. A 24-bit
/// 5GS code does not fit it, and writing one anyway would shift every field
/// after the TAI by an octet — so it is refused rather than corrupted.
#[test]
fn a_five_gs_tracking_area_is_refused_rather_than_written_wrong() {
    let uli = Uli {
        tai: Some(Tai::new(
            plmn("262-02"),
            TrackingAreaCode::five_gs(0x00_002A).unwrap(),
        )),
        ..Uli::new()
    };

    let mut out = [0u8; 32];
    assert!(uli.encode(&mut out).is_err());

    // The EPS code with the same number is fine, and round-trips.
    let eps = Uli {
        tai: Some(Tai::new(
            plmn("262-02"),
            TrackingAreaCode::eps(0x002A).unwrap(),
        )),
        ..Uli::new()
    };
    let written = eps.encode(&mut out).unwrap();
    assert_eq!(written, eps.encoded_len());
    assert_eq!(Uli::decode(&out[..written]).unwrap(), eps);
}

// ── The string forms ─────────────────────────────────────────────────────────

/// Everything that renders must parse back, or a stored location cannot be read
/// again — which is the whole reason the crate insists on one spelling.
#[test]
fn every_location_round_trips_through_its_string() {
    let locations = [
        UserLocation::Cgi(Cgi::new(
            plmn("262-02"),
            Lac::new(0x1A2B).unwrap(),
            CellId::new(0x00FF),
        )),
        UserLocation::Sai(Sai::new(
            plmn("262-02"),
            Lac::new(0x1A2B).unwrap(),
            ServiceAreaCode::new(7),
        )),
        UserLocation::Ecgi(Ecgi::new(plmn("262-02"), Eci::new(0x0A1_B2C3).unwrap())),
        UserLocation::Ncgi(Ncgi::new(plmn("262-02"), Nci::new(0x0_1234_5678).unwrap())),
        UserLocation::Tai(Tai::new(
            plmn("262-02"),
            TrackingAreaCode::eps(0x002A).unwrap(),
        )),
        UserLocation::Tai(Tai::new(
            plmn("262-02"),
            TrackingAreaCode::five_gs(0x00_002A).unwrap(),
        )),
        UserLocation::Rai(Rai::new(
            plmn("262-02"),
            Lac::new(0x1A2B).unwrap(),
            RoutingAreaCode::new(5),
        )),
        UserLocation::Lai(Lai::new(plmn("262-02"), Lac::new(0x1A2B).unwrap())),
    ];

    for location in locations {
        let rendered = location.to_string();
        assert_eq!(
            rendered.parse::<UserLocation>().unwrap(),
            location,
            "{rendered}"
        );
    }
}

/// The tag is required. A location area and an EPS tracking area write the same
/// digits, and nothing in them says which.
#[test]
fn an_untagged_location_does_not_parse() {
    assert!("262-02-1A2B".parse::<UserLocation>().is_err());
    assert_eq!(
        "lai:262-02-1A2B".parse::<UserLocation>().unwrap().kind(),
        "LAI"
    );
    assert_eq!(
        "tai:262-02-1A2B".parse::<UserLocation>().unwrap().kind(),
        "TAI"
    );
}

/// The digit count is the width, so a tracking area cannot be widened by
/// spelling it differently.
#[test]
fn the_digit_count_of_a_tracking_area_is_its_width() {
    let eps: Tai = "262-02-002A".parse().unwrap();
    let five_gs: Tai = "262-02-00002A".parse().unwrap();
    assert_eq!(eps.tac().width(), TacWidth::Eps);
    assert_eq!(five_gs.tac().width(), TacWidth::FiveGs);
    assert_ne!(eps, five_gs);

    // Any other count is not a tracking area at all.
    for s in ["262-02-2A", "262-02-0002A", "262-02-0000002A"] {
        assert!(s.parse::<Tai>().is_err(), "{s}");
    }
}

/// A tracking area is an area, not a cell — an assessment that treated one as a
/// cell would claim a precision the record does not carry.
#[test]
fn only_the_cell_forms_report_a_cell() {
    let cell = UserLocation::Ecgi(Ecgi::new(plmn("262-02"), Eci::new(1).unwrap()));
    let area = UserLocation::Rai(Rai::new(
        plmn("262-02"),
        Lac::new(1).unwrap(),
        RoutingAreaCode::new(1),
    ));
    assert!(cell.is_cell());
    assert!(!area.is_cell());
}
