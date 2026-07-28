//! Dividing usage at a boundary, and saying where it happened.
//!
//! Apportionment gets `proptest` attention because it is the conservation law
//! applied inside one record: splitting must never create or destroy usage, for
//! any total and any set of weights.

use mediation::{
    Apportionment, CellId, ChangeCondition, ChargedParty, ContainerSet, Ecgi, Eci, EnbKind,
    GnbIdLength, Lac, Ncgi, Nci, Octets, RatType, RatingGroup, RecordType, Seconds,
    ServiceDataContainer, SessionContext, SourceRef, SplitError, SubscriberRef, TacWidth, Tai,
    TrackingAreaCode, UsageRecord, UserLocation, VolumePair, apportion, split_measured,
    split_session,
};
use proptest::prelude::*;
use time::macros::datetime;

fn session(opened: &str, closed: &str) -> UsageRecord {
    UsageRecord::new(
        RecordType::PduSession,
        ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
        opened.parse().unwrap(),
        SourceRef::new("pgw-01".parse().unwrap(), 0),
    )
    .with_closed(closed.parse().unwrap())
}

// ── Apportionment conserves, for any input ───────────────────────────────────

proptest! {
    /// The property the whole module rests on: the parts sum to exactly the
    /// whole. Naive integer division fails this for almost every uneven split.
    #[test]
    fn apportionment_never_creates_or_destroys(
        total in any::<u64>(),
        weights in prop::collection::vec(0u64..1_000_000, 1..12),
    ) {
        let parts = apportion(total, &weights);
        prop_assert_eq!(parts.len(), weights.len());

        let sum: u128 = parts.iter().map(|p| u128::from(*p)).sum();
        if weights.iter().all(|w| *w == 0) {
            prop_assert_eq!(sum, 0, "no proportion to divide by");
        } else {
            prop_assert_eq!(sum, u128::from(total));
        }
    }

    /// Every part is bounded by its own share, so no slice can be handed more
    /// than the weights justify.
    #[test]
    fn no_part_exceeds_its_share_by_more_than_one(
        total in 0u64..u64::MAX / 2,
        weights in prop::collection::vec(1u64..10_000, 1..8),
    ) {
        let sum: u128 = weights.iter().map(|w| u128::from(*w)).sum();
        for (part, weight) in apportion(total, &weights).iter().zip(&weights) {
            let exact = u128::from(total) * u128::from(*weight) / sum;
            prop_assert!(u128::from(*part) >= exact);
            prop_assert!(u128::from(*part) <= exact + 1, "at most one extra unit");
        }
    }

    /// Splitting is deterministic: the same record and the same boundaries give
    /// the same slices, which is what makes a reprocessed archive reproduce its
    /// original bill.
    #[test]
    fn splitting_is_deterministic(uplink in any::<u32>(), downlink in any::<u32>()) {
        let record = session("2026-07-28T07:00:00+02:00", "2026-07-28T10:00:00+02:00")
            .with_volume(VolumePair::new(
                Octets::new(u64::from(uplink)),
                Octets::new(u64::from(downlink)),
            ));
        let cuts = [
            datetime!(2026-07-28 08:17:03 +2),
            datetime!(2026-07-28 09:00:00 +2),
        ];

        let first = split_session(&record, &cuts).unwrap();
        let second = split_session(&record, &cuts).unwrap();
        prop_assert_eq!(&first, &second);
        prop_assert!(first.conserves(&record));
    }
}

// ── A day boundary is the commonest split of all ─────────────────────────────

/// A session running through midnight belongs to two calendar days, and a
/// mediation layer that reports it under one has misstated both.
#[test]
fn a_session_through_midnight_belongs_to_two_days() {
    let record = session("2026-07-28T23:00:00+02:00", "2026-07-29T01:00:00+02:00")
        .with_volume(VolumePair::new(Octets::new(3_000), Octets::new(1_000)))
        .with_duration(Seconds::new(7_200));

    let split = split_session(&record, &[datetime!(2026-07-29 00:00 +2)]).unwrap();

    assert_eq!(split.len(), 2);
    assert_eq!(
        split.slices()[0]
            .end
            .to_offset(time::macros::offset!(+2))
            .date()
            .to_string(),
        "2026-07-29"
    );
    assert_eq!(split.slices()[0].wall_clock, Seconds::new(3_600));
    assert_eq!(split.slices()[1].wall_clock, Seconds::new(3_600));
    assert!(split.conserves(&record));
    assert_eq!(split.basis(), Apportionment::ProRata);
}

/// Two subscribers in different zones cross midnight at different instants, and
/// a boundary is an instant — which is why this crate never holds a calendar.
#[test]
fn midnight_is_not_one_instant() {
    let berlin = session("2026-07-28T23:30:00+02:00", "2026-07-29T00:30:00+02:00")
        .with_volume(VolumePair::new(Octets::new(600), Octets::new(0)));
    let kathmandu = session("2026-07-28T23:30:00+05:45", "2026-07-29T00:30:00+05:45")
        .with_volume(VolumePair::new(Octets::new(600), Octets::new(0)));

    // Each subscriber's own midnight divides their own session.
    assert_eq!(
        split_session(&berlin, &[datetime!(2026-07-29 00:00 +2)])
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        split_session(&kathmandu, &[datetime!(2026-07-29 00:00 +2)])
            .unwrap()
            .len(),
        1,
        "Berlin's midnight falls outside the Kathmandu session entirely"
    );
}

// ── Measured beats estimated ─────────────────────────────────────────────────

/// The reason the basis is labelled: pro rata and the containers disagree, and
/// only one of them is what the network counted.
#[test]
fn an_estimate_and_a_measurement_differ_and_say_so() {
    let containers = ContainerSet::from(vec![
        ServiceDataContainer::new(RatingGroup::new(10), ChangeCondition::TariffTime)
            .with_volume(VolumePair::new(Octets::new(9_000), Octets::new(0)))
            .with_closed("2026-07-28T08:00:00+02:00".parse().unwrap()),
        ServiceDataContainer::new(RatingGroup::new(10), ChangeCondition::RecordClosure)
            .with_volume(VolumePair::new(Octets::new(1_000), Octets::new(0)))
            .with_closed("2026-07-28T09:00:00+02:00".parse().unwrap()),
    ]);
    let record = session("2026-07-28T07:00:00+02:00", "2026-07-28T09:00:00+02:00")
        .with_volume(VolumePair::new(Octets::new(10_000), Octets::new(0)))
        .with_containers(containers);

    let estimated = split_session(&record, &[datetime!(2026-07-28 08:00 +2)]).unwrap();
    let measured = split_measured(&record).unwrap();

    // Equal halves, so pro rata splits the volume down the middle.
    assert_eq!(estimated.basis(), Apportionment::ProRata);
    assert_eq!(
        estimated.slices()[0].volume.unwrap().uplink(),
        Octets::new(5_000)
    );

    // The network says the burst was all in the first band.
    assert_eq!(measured.basis(), Apportionment::Measured);
    assert_eq!(
        measured.slices()[0].volume.unwrap().uplink(),
        Octets::new(9_000)
    );

    assert_ne!(
        estimated.slices()[0].volume,
        measured.slices()[0].volume,
        "an estimate that reached a bill unlabelled would be indistinguishable"
    );
    // Both still account for the whole session.
    assert_eq!(measured.total_volume(), record.volume);
    assert!(estimated.conserves(&record));
}

/// Refusing is better than inferring a boundary that was never recorded.
#[test]
fn a_measured_split_refuses_untimed_containers() {
    let record = session("2026-07-28T07:00:00+02:00", "2026-07-28T09:00:00+02:00").with_containers(
        ContainerSet::from(vec![
            ServiceDataContainer::new(RatingGroup::new(10), ChangeCondition::TariffTime)
                .with_volume(VolumePair::new(Octets::new(100), Octets::new(0))),
        ]),
    );

    assert_eq!(split_measured(&record), Err(SplitError::ContainerNotTimed));
}

// ── Location ─────────────────────────────────────────────────────────────────

/// The trap the module exists for: the same 36 bits name two different base
/// stations under two deployments, and only the operator knows which.
#[test]
fn a_cell_identity_means_nothing_without_the_deployment() {
    let nci = Nci::new(0xA_BCDE_F012).unwrap();

    let answers: Vec<u64> = (GnbIdLength::MIN..=GnbIdLength::MAX)
        .map(|bits| nci.gnb_id(GnbIdLength::new(bits).unwrap()))
        .collect();

    // Eleven configurations, eleven different base stations.
    assert_eq!(answers.len(), 11);
    let mut distinct = answers.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(distinct.len(), 11, "no two lengths agree");

    // And the two halves always reconstruct the whole, whatever the split.
    for bits in GnbIdLength::MIN..=GnbIdLength::MAX {
        let length = GnbIdLength::new(bits).unwrap();
        assert_eq!(
            (nci.gnb_id(length) << length.cell_bits()) | nci.cell_id(length),
            nci.get()
        );
    }
}

/// A home eNodeB fills all 28 bits, so there is no cell beneath it — and
/// returning zero would be a plausible value for a cell that does not exist.
#[test]
fn a_home_enodeb_has_no_cell_part() {
    let eci = Eci::new(0x0FED_CBA9).unwrap();

    assert_eq!(eci.cell_id(EnbKind::Home), None);
    assert_eq!(eci.cell_id(EnbKind::Macro), Some(0xA9));
    assert_eq!(eci.enb_id(EnbKind::Home), eci.get());
    assert!(eci.enb_id(EnbKind::Macro) < eci.get());
}

/// An EPS tracking area appears in 5GS zero-padded, so the same area written at
/// two widths must be neither silently equal nor impossible to compare.
#[test]
fn a_tracking_area_written_at_two_widths_is_one_area() {
    let eps = TrackingAreaCode::eps(0x00AB).unwrap();
    let five_gs = TrackingAreaCode::five_gs(0x0000_00AB).unwrap();

    assert_ne!(eps, five_gs);
    assert!(eps.is_same_area(five_gs));
    assert_eq!(eps.widened(), five_gs);
    assert_eq!(eps.width(), TacWidth::Eps);
    assert_eq!(eps.to_string(), "00AB");
    assert_eq!(five_gs.to_string(), "0000AB");
}

/// The two values that mean *no valid area* must not become an area.
#[test]
fn the_reserved_codes_never_become_a_location() {
    assert!(Lac::new(0x0000).is_none());
    assert!(Lac::new(0xFFFE).is_none());
    assert!(Lac::is_reserved(0xFFFE));
    assert!(TrackingAreaCode::eps(0x0000).is_none());
    assert!(TrackingAreaCode::eps(0xFFFE).is_none());
    assert!(TrackingAreaCode::five_gs(0x00FF_FFFE).is_none());

    // A 5GS code must still fit 24 bits.
    assert!(TrackingAreaCode::five_gs(0x0100_0000).is_none());
    assert!(TrackingAreaCode::five_gs(0x00FF_FFFF).is_some());
}

/// A location answers one question in this crate — the country — and every
/// form answers it without any cell database at all.
#[test]
fn every_location_form_answers_the_country_question() {
    let plmn = "262-02".parse().unwrap();
    let forms = [
        UserLocation::Ecgi(Ecgi::new(plmn, Eci::new(0x0012_3456).unwrap())),
        UserLocation::Ncgi(Ncgi::new(plmn, Nci::new(0x0_1234_5678).unwrap())),
        UserLocation::Tai(Tai::new(
            plmn,
            TrackingAreaCode::five_gs(0x0000_00AB).unwrap(),
        )),
    ];

    for form in forms {
        assert_eq!(form.mcc().to_string(), "262");
    }
    assert!(forms[0].is_cell());
    assert!(
        !forms[2].is_cell(),
        "a tracking area is an area, not a cell"
    );

    let context = SessionContext::none()
        .with_rat(RatType::Nr)
        .with_location(forms[1]);
    assert_eq!(context.serving_country(), Some("262".parse().unwrap()));
    assert_eq!(
        SessionContext::none().serving_country(),
        None,
        "no location is not a country"
    );
}

/// A 2G cell and a 3G service area carrying the same numbers must never be
/// confused, in either direction.
#[test]
fn the_access_generations_stay_apart() {
    let plmn = "262-02".parse().unwrap();
    let lac = Lac::new(0x1234).unwrap();

    let cgi = UserLocation::Cgi(mediation::Cgi::new(plmn, lac, CellId::new(0xABCD)));
    let sai = UserLocation::Sai(mediation::Sai::new(
        plmn,
        lac,
        mediation::ServiceAreaCode::new(0xABCD),
    ));

    assert_ne!(cgi, sai);
    assert_ne!(cgi.to_string(), sai.to_string());
    assert_eq!(cgi.kind(), "CGI");
    assert_eq!(sai.kind(), "SAI");
}
