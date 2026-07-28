//! Time arithmetic against real zones and real transitions.
//!
//! A charging record carries local time plus an offset, and both halves matter.
//! This suite works through the cases where treating either half casually
//! produces a wrong, plausible-looking answer: quarter-hour zones, sessions
//! that cross a daylight-saving transition, local-day bucketing, and ordering
//! across distant offsets.
//!
//! Germany's 2026 transitions are used throughout: forward on 29 March, back on
//! 25 October.

use mediation::{
    CdrTimestamp, ChargedParty, DstAdjustment, MsTimeZone, QuarterHourOffset, RecordType, Seconds,
    SourceRef, SubscriberRef, UsageRecord,
};
use std::collections::BTreeSet;

fn session(opened: &str, closed: &str) -> UsageRecord {
    UsageRecord::new(
        RecordType::PduSession,
        ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
        opened.parse().unwrap(),
        SourceRef::new("pgw-fra-01".parse().unwrap(), 0),
    )
    .with_closed(closed.parse().unwrap())
}

// ── Quarter-hour zones ───────────────────────────────────────────────────────

/// Every non-hour zone in commercial use, through the full pipeline: parse,
/// render, convert to an instant, convert back.
#[test]
fn quarter_hour_zones_survive_the_whole_pipeline() {
    // (offset, local time, the UTC time it denotes)
    let cases = [
        ("+05:30", "2026-07-28T09:00:00", "2026-07-28 03:30:00"), // India
        ("+05:45", "2026-07-28T09:00:00", "2026-07-28 03:15:00"), // Nepal
        ("+08:45", "2026-07-28T09:00:00", "2026-07-28 00:15:00"), // Eucla
        ("+09:30", "2026-07-28T09:00:00", "2026-07-27 23:30:00"), // Adelaide
        ("+12:45", "2026-07-28T09:00:00", "2026-07-27 20:15:00"), // Chatham
        ("-03:30", "2026-07-28T09:00:00", "2026-07-28 12:30:00"), // Newfoundland
        ("-09:30", "2026-07-28T09:00:00", "2026-07-28 18:30:00"), // Marquesas
    ];

    for (offset, local, expected_utc) in cases {
        let ts: CdrTimestamp = format!("{local}{offset}").parse().unwrap();
        assert_eq!(ts.offset().to_string(), offset);
        assert_eq!(
            ts.to_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap()
                .replace('T', " ")
                .replace('Z', ""),
            expected_utc,
            "{offset} converted wrongly"
        );
        // …and the canonical string is exactly what went in.
        assert_eq!(ts.to_string(), format!("{local}{offset}"));
    }
}

/// An hour-granular offset type would lose these; the semi-octet encoding does
/// not, and neither does this crate.
#[test]
fn every_quarter_hour_step_has_a_distinct_wire_encoding() {
    let mut encodings = BTreeSet::new();
    for q in -48i8..=56 {
        let offset = QuarterHourOffset::new(q).unwrap();
        assert!(
            encodings.insert(offset.to_semi_octet()),
            "encoding collision at {q} quarter-hours"
        );
        assert_eq!(
            QuarterHourOffset::from_semi_octet(offset.to_semi_octet()),
            Some(offset)
        );
    }
    assert_eq!(encodings.len(), 105);
}

/// Hand-computed against TS 23.040 §9.2.3.11: swapped nibbles, with the sign in
/// bit 3 of the low one.
#[test]
fn semi_octet_encoding_matches_hand_computed_values() {
    let cases = [
        ("+00:00", 0x00u8), // 0 quarter-hours
        ("+02:00", 0x80),   // 8  → tens 0, units 8
        ("+05:30", 0x22),   // 22 → tens 2, units 2
        ("+05:45", 0x32),   // 23 → tens 2, units 3
        ("+14:00", 0x65),   // 56 → tens 5, units 6
        ("-05:00", 0x0A),   // 20 → tens 2 | sign, units 0
        ("-12:00", 0x8C),   // 48 → tens 4 | sign, units 8
    ];
    for (rendered, octet) in cases {
        let offset: QuarterHourOffset = rendered.parse().unwrap();
        assert_eq!(offset.to_semi_octet(), octet, "encoding {rendered}");
        assert_eq!(
            QuarterHourOffset::from_semi_octet(octet),
            Some(offset),
            "decoding 0x{octet:02X}"
        );
    }
}

// ── Daylight saving transitions ──────────────────────────────────────────────

/// The clock jumps forward, so the wall-clock difference overstates what
/// elapsed. The span is computed on instants and reports the truth.
#[test]
fn a_session_across_the_spring_transition_measures_real_elapsed_time() {
    // 29 March 2026: 02:00 CET becomes 03:00 CEST.
    let record = session("2026-03-29T01:50:00+01:00", "2026-03-29T03:20:00+02:00");

    // Naively subtracting the wall clocks would give 90 minutes.
    assert_eq!(record.wall_clock_span(), Some(Seconds::new(30 * 60)));
}

/// The clock jumps backward, so the local close time is *earlier* than the
/// local open time while a positive interval elapsed.
#[test]
fn a_session_across_the_autumn_transition_does_not_go_negative() {
    // 25 October 2026: 03:00 CEST becomes 02:00 CET.
    let record = session("2026-10-25T02:50:00+02:00", "2026-10-25T02:20:00+01:00");

    assert!(
        record.closed.unwrap().local() < record.opened.local(),
        "the wall clock ran backwards"
    );
    assert_eq!(record.wall_clock_span(), Some(Seconds::new(30 * 60)));
}

/// The same reported offset belongs to different zones, and only the
/// daylight-saving field distinguishes them.
#[test]
fn a_transition_is_distinguishable_from_a_journey() {
    let plus_one = QuarterHourOffset::from_minutes(60).unwrap();
    let plus_two = QuarterHourOffset::from_minutes(120).unwrap();

    let berlin_winter = MsTimeZone::new(plus_one, DstAdjustment::None);
    let berlin_summer = MsTimeZone::new(plus_two, DstAdjustment::PlusOneHour);
    let athens_winter = MsTimeZone::new(plus_two, DstAdjustment::None);

    // A handset that stayed in Berlin across 29 March.
    assert!(berlin_winter.is_same_zone_across_transition(berlin_summer));
    assert_eq!(
        berlin_winter.standard_offset(),
        berlin_summer.standard_offset()
    );

    // A handset that flew from Berlin to Athens in January.
    assert!(!berlin_winter.is_same_zone_across_transition(athens_winter));

    // Berlin in July and Athens in January report the same offset …
    assert_eq!(berlin_summer.offset(), athens_winter.offset());
    // … and are still told apart.
    assert_ne!(
        berlin_summer.standard_offset(),
        athens_winter.standard_offset()
    );
}

// ── Local-day bucketing ──────────────────────────────────────────────────────

/// The day a subscriber sees on an itemised bill is the local one. Grouping on
/// the UTC date moves the early and late edges of every day into a neighbour.
#[test]
fn local_day_bucketing_differs_from_utc_day_bucketing() {
    let records = [
        // Just after local midnight in Nepal: still the 28th locally, the 27th
        // in UTC.
        session("2026-07-28T00:30:00+05:45", "2026-07-28T00:31:00+05:45"),
        // Late on the 28th in Newfoundland: still the 28th locally, the 29th in
        // UTC.
        session("2026-07-28T23:30:00-03:30", "2026-07-28T23:31:00-03:30"),
    ];

    let local_days: BTreeSet<_> = records.iter().map(UsageRecord::local_date).collect();
    let utc_days: BTreeSet<_> = records.iter().map(|r| r.opened.to_utc().date()).collect();

    // One billing day locally …
    assert_eq!(local_days.len(), 1);
    // … spread across two UTC days.
    assert_eq!(utc_days.len(), 2);
}

// ── Ordering ─────────────────────────────────────────────────────────────────

/// Records from distant offsets order correctly only as instants. There is no
/// `Ord` on the timestamp, so this is the supported way and there is no wrong
/// way sitting next to it.
#[test]
fn a_series_orders_by_instant_not_by_wall_clock() {
    let mut series: Vec<CdrTimestamp> = [
        "2026-07-28T01:00:00+14:00", // earliest instant, latest-looking date
        "2026-07-27T23:00:00-12:00", // latest instant, earliest-looking date
        "2026-07-28T00:00:00+00:00",
    ]
    .iter()
    .map(|s| s.parse().unwrap())
    .collect();

    series.sort_by(CdrTimestamp::cmp_instant);

    let ordered: Vec<String> = series.iter().map(ToString::to_string).collect();
    assert_eq!(
        ordered,
        vec![
            "2026-07-28T01:00:00+14:00",
            "2026-07-28T00:00:00+00:00",
            "2026-07-27T23:00:00-12:00",
        ]
    );

    // Sorting on the local field would have produced the opposite order.
    let mut by_wall_clock = series.clone();
    by_wall_clock.sort_by_key(|ts| ts.local());
    assert_ne!(by_wall_clock, series);
}

/// A day is 24 hours apart in offset terms from UTC−12:00 to UTC+12:00, and the
/// assigned range spans 26 — so two records with the same local reading can be
/// 26 hours apart.
#[test]
fn the_assigned_offset_range_spans_twenty_six_hours() {
    let east: CdrTimestamp = "2026-07-28T12:00:00+14:00".parse().unwrap();
    let west: CdrTimestamp = "2026-07-28T12:00:00-12:00".parse().unwrap();

    let apart = (west.to_utc() - east.to_utc()).whole_hours();
    assert_eq!(apart, 26);
}

// ── Chargeable duration ──────────────────────────────────────────────────────

/// Chargeable duration is carried, never derived — and it may legitimately be
/// far shorter than the span, or absent while a span exists.
#[test]
fn chargeable_duration_is_independent_of_the_span() {
    let answered = session("2026-07-28T10:00:00+02:00", "2026-07-28T10:00:55+02:00")
        .with_duration(Seconds::new(10));
    assert_eq!(answered.duration, Some(Seconds::new(10)));
    assert_eq!(answered.wall_clock_span(), Some(Seconds::new(55)));

    let unanswered = session("2026-07-28T10:00:00+02:00", "2026-07-28T10:00:45+02:00");
    assert_eq!(unanswered.duration, None);
    assert_eq!(unanswered.wall_clock_span(), Some(Seconds::new(45)));

    // A chargeable duration exceeding the span is impossible, and detectable
    // precisely because the two are separate numbers.
    let impossible = session("2026-07-28T10:00:00+02:00", "2026-07-28T10:00:10+02:00")
        .with_duration(Seconds::new(55));
    assert!(impossible.duration.unwrap() > impossible.wall_clock_span().unwrap());
}
