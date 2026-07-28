//! Every runnable snippet in the README, so documentation cannot drift.
//!
//! Each test mirrors one fenced block, in the order the README presents them.
//! When a snippet changes, change it here too — and when this file fails, the
//! README is wrong rather than the test.

use mediation::{Apportionment, Eci, EnbKind, GnbIdLength, Nci, apportion, split_session};
use mediation::{
    BatchReconciliation, ByteScale, CdrTimestamp, ChangeCondition, ChargedParty, ChargingId,
    ContainerSet, CorrelationKeys, DedupDecision, DedupStore, Disposition, DstAdjustment, Imei,
    Imeisv, Imsi, Mcc, Mnc, MncWidth, MsTimeZone, NorthAmericanRule, NotKeyable, Octets,
    ParseError, PartialFormat, PartialInfo, PartialRun, Pei, PlmnId, QualityGrade,
    QuarterHourOffset, RatingGroup, Reassembly, RecordClosureCause, RecordKey, RecordType, Seconds,
    SequenceObservation, SequenceSpace, SequenceTracker, SequenceValue, ServiceDataContainer,
    ServiceDomain, SourceRef, SubscriberRef, TenantRef, UnattributableReason, UsageRecord,
    ValidationReport, VolumePair, score_batch,
};
use mediation::{
    BerReader, FairUseAssessment, Msisdn, NumberRangeSet, RejectReason, RetentionBasis,
    RetentionPolicy, ScaledAmount, Signal, TagClass, aggregate_released, impossible_travel,
    is_ported, terminating_plmn,
};
use mediation::{
    Century, ChainBreak, CorrelationConfig, DigitString, HoldReason, PeriodLedger, RestateReason,
    Restatement, RestatementLog, SliceServiceType, Snssai, UsageDelta, correlate,
};
use mediation::{Jurisdiction, customer_country_presumption, is_abroad, usage_country};
use std::net::Ipv4Addr;
use time::Duration;

/// The retransmission bound the samples assume.
fn week() -> mediation::DedupWindow {
    mediation::DedupWindow::from_bound(Duration::days(7)).expect("positive")
}

/// "Quick start"
#[test]
fn quick_start() -> Result<(), ParseError> {
    let record = UsageRecord::new(
        RecordType::PduSession,
        ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse()?)),
        "2026-07-28T10:00:00+02:00".parse()?,
        SourceRef::new("pgw-fra-01".parse()?, 0).with_file_sequence(42),
    )
    .with_closed("2026-07-28T10:30:00+02:00".parse()?)
    .with_duration(Seconds::new(1_800))
    .with_volume(VolumePair::new(Octets::new(4_096), Octets::new(65_536)))
    .with_networks("310-260".parse()?, "262-02".parse()?)
    .with_correlation(
        CorrelationKeys::default()
            .with_charging_id(ChargingId::new(1_234))
            .with_gateway(Ipv4Addr::new(10, 0, 0, 1).into()),
    );

    assert_eq!(record.service(), ServiceDomain::Data);
    assert_eq!(record.is_roaming(), Some(true));
    assert_eq!(record.local_date().to_string(), "2026-07-28");

    let mut store = DedupStore::new(week());
    let mut counts = BatchReconciliation::EMPTY;
    let at = record.opened.to_utc();

    for _ in 0..3 {
        let key = RecordKey::of(&record).expect("keyed on Charging ID + gateway");
        counts.observe(&match store.observe(key.clone(), at) {
            DedupDecision::First => Disposition::Billable,
            DedupDecision::Duplicate { .. } => Disposition::Duplicate(key),
        });
    }

    assert_eq!(counts.released(), 1);
    assert_eq!(counts.duplicate(), 2);
    assert!(counts.balances());
    Ok(())
}

/// "Deduplication: the natural key is the whole design"
#[test]
fn a_charging_id_needs_its_gateway_and_a_bare_record_has_no_key() {
    let half = CorrelationKeys::default().with_charging_id(ChargingId::new(1_234));
    assert!(half.packet_session().is_none());

    let bare = UsageRecord::new(
        RecordType::PduSession,
        ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
        "2026-07-28T10:00:00+02:00".parse().unwrap(),
        SourceRef::new("pgw-01".parse().unwrap(), 0),
    );
    assert_eq!(RecordKey::of(&bare), Err(NotKeyable::NoCorrelatorPresent));
}

/// "Completeness: sequence gaps are the only evidence of loss"
#[test]
fn a_wrap_is_a_successor_and_a_gap_names_its_files() {
    let last = SequenceValue::new(SequenceSpace::TAP, 99_999).unwrap();
    let next = SequenceValue::new(SequenceSpace::TAP, 1).unwrap();
    assert!(last.is_successor(next));
    assert_eq!(last.gap_to(next), Some(0));

    let mut tracker = SequenceTracker::new(SequenceSpace::TAP);
    tracker.observe_default(99_998);
    assert_eq!(
        tracker.observe_default(2),
        SequenceObservation::Gap {
            missing: vec![99_999, 1]
        },
    );
    assert!(!tracker.is_complete());
}

/// "Time is local, not UTC"
#[test]
fn local_time_is_not_the_utc_day() {
    let ts: CdrTimestamp = "2026-07-28T00:30:00+05:45".parse().unwrap();

    assert_eq!(ts.local_date().to_string(), "2026-07-28");
    assert_eq!(ts.to_utc().date().to_string(), "2026-07-27");
}

/// "The same offset is two different zones"
#[test]
fn the_same_offset_is_two_different_zones() {
    let plus_two = QuarterHourOffset::from_minutes(120).unwrap();
    let berlin_summer = MsTimeZone::new(plus_two, DstAdjustment::PlusOneHour);
    let athens_winter = MsTimeZone::new(plus_two, DstAdjustment::None);

    assert_eq!(berlin_summer.offset(), athens_winter.offset());
    assert_eq!(
        berlin_summer.standard_offset().unwrap().to_string(),
        "+01:00"
    );
    assert_eq!(
        athens_winter.standard_offset().unwrap().to_string(),
        "+02:00"
    );

    let berlin_winter = MsTimeZone::new(
        QuarterHourOffset::from_minutes(60).unwrap(),
        DstAdjustment::None,
    );
    assert!(berlin_winter.is_same_zone_across_transition(berlin_summer));
    assert!(!berlin_winter.is_same_zone_across_transition(athens_winter));
}

/// "There is no `Ord` on a timestamp"
#[test]
fn a_series_sorts_by_instant() {
    let mut series: Vec<CdrTimestamp> = vec![
        "2026-07-27T23:00:00-12:00".parse().unwrap(),
        "2026-07-28T01:00:00+14:00".parse().unwrap(),
    ];
    series.sort_by(CdrTimestamp::cmp_instant);

    assert_eq!(series[0].to_string(), "2026-07-28T01:00:00+14:00");
}

/// "Octets, not megabytes — and never a float"
#[test]
fn volumes_are_exact_integers() {
    assert_eq!(
        Octets::from_scaled(5, ByteScale::Mega).unwrap().get(),
        5_000_000
    );
    assert_eq!(
        Octets::from_scaled(5, ByteScale::Mebi).unwrap().get(),
        5_242_880
    );

    assert_eq!(Octets::new(u64::MAX).checked_add(Octets::new(1)), None);

    let v = VolumePair::new(Octets::new(100), Octets::new(900));
    assert_eq!(v.checked_total(), Some(Octets::new(1_000)));
}

/// "An IMSI cannot be split without a table"
#[test]
fn an_imsi_needs_a_table_past_the_country_code() {
    let de: Imsi = "262021234567890".parse().unwrap();
    let us: Imsi = "310260123456789".parse().unwrap();

    assert_eq!(de.mcc(), Mcc::GERMANY);

    assert_eq!(de.plmn(&NorthAmericanRule).unwrap().to_string(), "262-02");
    assert_eq!(us.plmn(&NorthAmericanRule).unwrap().to_string(), "310-260");

    let table = |mcc: Mcc| (mcc == Mcc::GERMANY).then_some(MncWidth::Two);
    assert_eq!(de.plmn(&table).unwrap().to_string(), "262-02");
    assert_eq!(us.plmn(&table), None);
}

/// "An MNC is a string, not a number"
#[test]
fn an_mnc_keeps_its_leading_zeros() {
    let telekom: PlmnId = "262-01".parse().unwrap();
    assert_eq!(telekom.to_string(), "262-01");

    assert_ne!(Mnc::new(1, 2).unwrap(), Mnc::new(1, 3).unwrap());
    assert_eq!(Mnc::new(1, 3).unwrap().to_string(), "001");
}

/// "IMEI is Luhn-checked and IMEISV is not"
#[test]
fn equipment_identities_behave_differently_by_design() {
    assert!("490154203237518".parse::<Imei>().is_ok());
    assert!("490154203237581".parse::<Imei>().is_err());

    let sv: Imeisv = "4901542032375120".parse().unwrap();
    assert_eq!(sv.software_version().to_string(), "20");

    let a: Pei = "imei-490154203237518".parse().unwrap();
    let b: Pei = "imeisv-4901542032375120".parse().unwrap();
    assert_ne!(a, b);
    assert_eq!(a.to_imei(), b.to_imei());
}

/// "The charged party is not always a subscriber"
#[test]
fn the_charged_party_is_a_sum_type() {
    let api = ChargedParty::ApiConsumer {
        developer: "acme-dev".parse().unwrap(),
        subject: Some(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
    };
    assert_eq!(api.kind(), "API_CONSUMER");
    assert!(api.subscriber().is_some());

    let slice = ChargedParty::Tenant(TenantRef::new("enterprise-7".parse().unwrap()));
    assert!(slice.subscriber().is_none());

    let lost = ChargedParty::Unattributable(UnattributableReason::ConcealedIdentityOnly);
    assert_eq!(lost.to_string(), "UNATTRIBUTABLE:CONCEALED_IDENTITY_ONLY");
}

/// "String forms — one value, one string": the lenient-in, canonical-out rule.
#[test]
fn parsing_is_lenient_and_writing_is_not() {
    for raw in ["+0200", "+02", "+02:00"] {
        assert_eq!(
            raw.parse::<QuarterHourOffset>().unwrap().to_string(),
            "+02:00"
        );
    }
    assert_eq!(
        "Z".parse::<QuarterHourOffset>().unwrap().to_string(),
        "+00:00"
    );

    for raw in ["+491701234567", "49-170-1234567", "491701234567"] {
        assert_eq!(
            raw.parse::<mediation::Msisdn>().unwrap().to_string(),
            "491701234567"
        );
    }

    // A unit suffix would have to guess a scale, so it is rejected.
    assert!("5MB".parse::<Octets>().is_err());
}

/// "The release gate"
#[test]
fn a_million_clean_records_cannot_outvote_an_unknown_hole() {
    let counts: BatchReconciliation = vec![Disposition::Billable; 1_000_000].into_iter().collect();

    let report = score_batch(&counts, &ValidationReport::new(), false);
    assert_eq!(report.grade, QualityGrade::F);
    assert!(report.blocks_release());
}

/// "Partial records: reconstruct, never invent"
#[test]
fn an_orphaned_run_cannot_be_fixed_by_waiting_but_an_open_one_can() {
    let orphan = PartialRun::from_parts(vec![PartialInfo::new(
        1,
        PartialFormat::Reduced,
        RecordClosureCause::VolumeLimit,
    )]);
    assert!(matches!(orphan.assess(), Reassembly::Orphan { .. }));
    assert!(!orphan.assess().may_still_complete());

    let open = PartialRun::from_parts(vec![
        PartialInfo::new(
            1,
            PartialFormat::FullyQualified,
            RecordClosureCause::VolumeLimit,
        ),
        PartialInfo::new(2, PartialFormat::Reduced, RecordClosureCause::VolumeLimit),
    ]);
    assert!(open.assess().may_still_complete());
}

/// "Containers: the list is the data"
#[test]
fn containers_keep_a_priced_boundary_and_report_a_discrepancy() {
    let containers = ContainerSet::from(vec![
        ServiceDataContainer::new(RatingGroup::new(10), ChangeCondition::TariffTime)
            .with_volume(VolumePair::new(Octets::new(1_000), Octets::new(4_000))),
        ServiceDataContainer::new(RatingGroup::new(10), ChangeCondition::RecordClosure)
            .with_volume(VolumePair::new(Octets::new(500), Octets::new(2_000))),
    ]);

    assert_eq!(containers.rating_groups().len(), 1);
    assert!(containers.crosses_priced_boundary());

    let declared = VolumePair::new(Octets::new(1_550), Octets::new(6_000));
    assert_eq!(containers.agrees_with(Some(declared)), Some(false));
    assert_eq!(containers.discrepancy(Some(declared)), Some((50, 0)));
}

/// "The law spans batches, not just one"
#[test]
fn a_carried_record_counted_as_new_breaks_the_period() {
    let mut monday = BatchReconciliation::EMPTY;
    monday.observe(&Disposition::Held(HoldReason::AwaitingPartial));

    let mut tuesday = BatchReconciliation::EMPTY;
    tuesday.observe(&Disposition::Billable);
    assert!(tuesday.balances(), "the batch is internally consistent");

    let mut period = PeriodLedger::new();
    period.append(monday).unwrap();
    assert_eq!(
        period.append(tuesday),
        Err(ChainBreak::CarryMismatch {
            batch: 1,
            expected: 1,
            found: 0,
        }),
    );
    assert!(!period.is_closable());
}

/// "Correlation: two failures, opposite in sign"
#[test]
fn one_bearer_seen_by_two_nodes_is_joined_and_nothing_is_dropped() {
    fn record(node: &str) -> UsageRecord {
        let mut r = UsageRecord::new(
            RecordType::PduSession,
            ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
            "2026-07-28T10:00:00+02:00".parse().unwrap(),
            SourceRef::new(node.parse().unwrap(), 0),
        )
        .with_correlation(
            CorrelationKeys::default()
                .with_charging_id(ChargingId::new(42))
                .with_gateway(Ipv4Addr::new(10, 0, 0, 1).into()),
        );
        r.volume = Some(VolumePair::new(Octets::new(100), Octets::new(900)));
        r
    }

    let records = vec![record("sgw-01"), record("pgw-01")];
    let result = correlate(&records, &CorrelationConfig::packet_bearer());

    assert_eq!(result.groups().len(), 1);
    assert!(result.groups()[0].would_double_count());
    assert!(result.accounts_for(records.len()));
}

/// "Corrections are new records, never edits"
#[test]
fn a_withdrawn_correction_nets_to_nothing_and_both_entries_survive() {
    let key = RecordKey::RecordingEntity {
        entity: "msc-01".parse().unwrap(),
        local_sequence: 1,
    };
    let correction = Restatement::new(
        key,
        RestateReason::LatePartial,
        UsageDelta::default().with_duration(300),
        "2026-08-02T09:00:00+02:00".parse().unwrap(),
    );

    let log: RestatementLog = vec![correction.clone(), correction.reverse()]
        .into_iter()
        .collect();
    assert_eq!(log.net(), Some(UsageDelta::ZERO));
    assert_eq!(log.len(), 2);
}

/// "One record, two BCD conventions"
#[test]
fn tbcd_matches_the_published_example_and_a_misread_is_silent() {
    assert_eq!(
        DigitString::from_tbcd(&[0x21, 0x43, 0xF5])
            .unwrap()
            .to_string(),
        "12345"
    );

    let msisdn: mediation::Msisdn = "491701234567".parse().unwrap();
    let mut wire = [0u8; 8];
    let n = msisdn.to_tbcd(&mut wire).unwrap();
    assert_eq!(
        DigitString::from_bcd(&wire[..n]).unwrap().to_string(),
        "947110325476"
    );
}

/// "The timestamp carries no century"
#[test]
fn the_nine_octet_timestamp_round_trips() {
    let octets = [0x26, 0x07, 0x28, 0x10, 0x30, 0x45, b'+', 0x02, 0x00];
    let ts = CdrTimestamp::from_bcd(&octets, Century::TWENTY_FIRST).unwrap();

    assert_eq!(ts.to_string(), "2026-07-28T10:30:45+02:00");
    assert_eq!(ts.to_bcd().unwrap(), octets);
}

/// "Slices and data networks"
#[test]
fn an_absent_slice_differentiator_is_not_a_zero_one() {
    let bare = Snssai::from_bytes(&[0x01]).unwrap();
    let reserved = Snssai::from_bytes(&[0x01, 0xFF, 0xFF, 0xFF]).unwrap();
    let zero = Snssai::from_bytes(&[0x01, 0x00, 0x00, 0x00]).unwrap();

    assert_eq!(bare, reserved);
    assert_ne!(zero, bare);
    assert_eq!(zero.sd(), Some(0));
    assert_eq!(bare.sst(), SliceServiceType::EMBB);
}

/// "The framing, not the schema"
#[test]
fn a_ber_element_yields_its_children_with_input_offsets() {
    let bytes = [0xA0, 0x06, 0x02, 0x01, 0x2A, 0x02, 0x01, 0x07];
    let outer = BerReader::new(&bytes).next().unwrap().unwrap();

    assert_eq!(outer.tag().class(), TagClass::ContextSpecific);
    assert!(outer.tag().is_constructed());

    let inner: Vec<_> = outer
        .children()
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(inner.len(), 2);
    assert_eq!(inner[0].as_u64().unwrap(), 42);
    assert_eq!(inner[0].offset(), 2);
}

/// "What the batch adds up to"
#[test]
fn a_total_travels_with_the_reconciliation_that_justifies_it() {
    fn record(up: u64) -> UsageRecord {
        UsageRecord::new(
            RecordType::PduSession,
            ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
            "2026-07-28T10:00:00+02:00".parse().unwrap(),
            SourceRef::new("pgw-01".parse().unwrap(), 0),
        )
        .with_volume(VolumePair::new(Octets::new(up), Octets::ZERO))
    }

    let records = vec![record(100), record(200), record(400)];
    let dispositions = vec![
        Disposition::Billable,
        Disposition::Billable,
        Disposition::Rejected(RejectReason::Malformed),
    ];

    let released = aggregate_released(&records, &dispositions).unwrap();

    assert_eq!(released.total().volume().uplink(), Octets::new(300));
    assert_eq!(released.reconciliation().ingested(), 3);
    assert!(released.is_consistent());
}

/// "A partner's batch, and whether it adds up"
#[test]
fn amounts_are_integers_with_a_scale_and_add_only_at_one() {
    let charge = ScaledAmount::new(1_234, 3);
    assert_eq!(charge.to_string(), "1.234");

    assert!(charge.checked_add(ScaledAmount::new(123, 2)).is_none());
    assert_eq!(
        charge.checked_add(ScaledAmount::new(766, 3)),
        Some(ScaledAmount::new(2_000, 3)),
    );
}

/// "Fair use, and the reading that costs a customer money"
#[test]
fn either_indicator_alone_is_the_safe_harbour() {
    let assessment =
        FairUseAssessment::from_counts(40, 80, Octets::new(9_000), Octets::new(1_000), 120);

    assert!(!assessment.prevailing_domestic_presence());
    assert!(assessment.prevailing_domestic_consumption());
    assert!(assessment.prevailing_domestic());
    assert!(!assessment.may_warn());
}

/// "Retention: no default, on purpose"
#[test]
fn a_billing_period_runs_from_the_invoice_not_the_call() {
    let policy = RetentionPolicy::new(
        RetentionBasis::BillingTdddg10 {
            invoice_dispatched_at: time::macros::datetime!(2026-08-05 00:00 UTC),
        },
        time::Duration::days(183),
    )
    .unwrap();

    let call = time::macros::datetime!(2026-07-01 12:00 UTC);
    assert!(policy.deadline() > call + time::Duration::days(183));
}

/// "Pseudonymised data is still personal data"
#[test]
fn a_reduction_keeps_the_dimension_and_loses_the_identity() {
    let imsi: Imsi = "262021234567890".parse().unwrap();
    let imei: Imei = "490154203237518".parse().unwrap();

    assert_eq!(
        mediation::Reduction::CountryOnly
            .apply_to_imsi(imsi)
            .to_string(),
        "262"
    );
    assert_eq!(
        mediation::Reduction::ModelOnly
            .apply_to_imei(imei)
            .to_string(),
        "49015420"
    );
}

/// "A number's range says who allocated it, not who terminates it"
#[test]
fn portability_answers_before_the_allocation_range_does() {
    let mut allocation = NumberRangeSet::new();
    allocation.insert(
        "49151".parse().unwrap(),
        "262-01".parse::<PlmnId>().unwrap(),
    );

    let number: Msisdn = "491519234567".parse().unwrap();

    let none_ported = |_: Msisdn| None;
    assert_eq!(
        terminating_plmn(number, &allocation, &none_ported)
            .unwrap()
            .to_string(),
        "262-01"
    );
    assert!(!is_ported(number, &allocation, &none_ported));

    let ported = |_: Msisdn| Some("262-02".parse::<PlmnId>().unwrap());
    assert_eq!(
        terminating_plmn(number, &allocation, &ported)
            .unwrap()
            .to_string(),
        "262-02"
    );
    assert!(is_ported(number, &allocation, &ported));
}

/// "Fraud signals: evidence, never verdicts"
#[test]
fn two_countries_twenty_minutes_apart_is_impossible_not_unusual() {
    fn at(opened: &str, serving: &str) -> UsageRecord {
        UsageRecord::new(
            RecordType::MobileOriginatedCall,
            ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
            opened.parse().unwrap(),
            SourceRef::new("msc-01".parse().unwrap(), 0),
        )
        .with_networks(serving.parse().unwrap(), "262-02".parse().unwrap())
    }

    let records = vec![
        at("2026-07-28T10:00:00+02:00", "262-02"),
        at("2026-07-28T10:20:00+02:00", "310-260"),
    ];

    let found = impossible_travel(&records, time::Duration::hours(4));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].signal, Signal::ImpossibleTravel);
    assert!(found[0].signal.is_arithmetic());
}

/// "Splitting a session at a tariff boundary"
#[test]
fn a_session_across_a_boundary_splits_and_conserves() {
    let record = UsageRecord::new(
        RecordType::PduSession,
        ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
        "2026-07-28T07:00:00+02:00".parse().unwrap(),
        SourceRef::new("pgw-01".parse().unwrap(), 0),
    )
    .with_closed("2026-07-28T09:00:00+02:00".parse().unwrap())
    .with_volume(VolumePair::new(Octets::new(900), Octets::new(300)));

    let split = split_session(&record, &[time::macros::datetime!(2026-07-28 08:00 +2)]).unwrap();

    assert_eq!(split.len(), 2);
    assert_eq!(split.basis(), Apportionment::ProRata);
    assert_eq!(split.slices()[0].volume.unwrap().uplink(), Octets::new(450));
    assert!(split.conserves(&record));
}

/// "Apportionment conserves"
#[test]
fn the_truncated_units_are_distributed_not_discarded() {
    let parts = apportion(1_000, &[1, 1, 1]);
    assert_eq!(parts, vec![334, 333, 333]);
    assert_eq!(parts.iter().sum::<u64>(), 1_000);
}

/// "Where a record was produced"
#[test]
fn a_cell_identity_splits_only_where_the_deployment_says() {
    let eci = Eci::new(0x00A1_B2C3).unwrap();
    assert_eq!(eci.cell_id(EnbKind::Macro), Some(0xC3));
    assert_eq!(eci.cell_id(EnbKind::Home), None);

    let nci = Nci::new(0x0_1234_5678).unwrap();
    assert_ne!(
        nci.gnb_id(GnbIdLength::new(22).unwrap()),
        nci.gnb_id(GnbIdLength::new(24).unwrap()),
    );
}

/// "Which country's rules apply — and which question you asked"
#[test]
fn a_roamer_is_governed_here_and_taxed_there() {
    let roamer = UsageRecord::new(
        RecordType::PduSession,
        ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
        "2026-07-28T10:00:00+02:00".parse().unwrap(),
        SourceRef::new("pgw-01".parse().unwrap(), 0),
    )
    .with_networks("214-07".parse().unwrap(), "262-02".parse().unwrap());

    assert_eq!(
        usage_country(&roamer),
        Jurisdiction::Country("214".parse().unwrap())
    );
    assert_eq!(
        customer_country_presumption(&roamer),
        Jurisdiction::Country("262".parse().unwrap())
    );
    assert_eq!(is_abroad(&roamer), Some(true));
}
