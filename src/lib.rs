#![deny(unsafe_code)]
#![deny(missing_docs)]

//! Telecom usage mediation domain library.
//!
//! A **pure** library for the arithmetic and classification between a mobile or
//! fixed network and a billing system: normalising, validating, deduplicating
//! and correlating usage records. Zero I/O, no async, no clock, exact integer
//! quantities. It computes **usage**, not money: octets, seconds and event
//! counts, attributed to a charged party, a network and a service.
//!
//! <div class="warning">
//!
//! **Long-form documentation: <https://hupe1980.github.io/mediation>.** Every
//! example there is compiled by this crate's test suite.
//!
//! </div>
//!
//! # Modules
//!
//! | Module | Contents |
//! |---|---|
//! | [`error`] | `ParseError` and `DecodeError` — one per input kind, string and bytes |
//! | [`digits`] | `DigitString` — inline digits, plus the TBCD and plain-BCD codecs |
//! | [`ber`] | `BerReader`, `BerElement`, `BerTag` — the X.690 framing a CDR arrives in |
//! | [`cdrtime`] | `CdrTimestamp`, `QuarterHourOffset`, `DstAdjustment`, `MsTimeZone`, `Seconds` |
//! | [`volume`] | `Octets`, `VolumePair`, `ByteScale` — integer data volumes |
//! | [`plmn`] | `Mcc`, `Mnc`, `PlmnId`, `TadigCode` — network identity, and its three-octet wire form |
//! | [`dialplan`] | `DialContext`, `Destination`, `DestinationClass`, `NumberingPlan` — dial string to E.164 destination |
//! | [`identity`] | `Imsi`, `Msisdn`, `Imei`, `Imeisv`, `Iccid`, `Supi`, `Gpsi`, `Pei`, `Suci`, `Nai`, `AddressString` |
//! | [`jurisdiction`] | `usage_country()`, `customer_country_presumption()` — two questions, two countries |
//! | [`location`] | `UserLocation`, `Uli`, `Cgi`, `Sai`, `Rai`, `Lai`, `Ecgi`, `Ncgi`, `Tai` — where a record was produced |
//! | [`numbering`] | `NumberRange`, `NumberRangeSet`, `Porting`, `terminating_plmn` — who really terminates a number |
//! | [`party`] | `ChargedParty`, `SubscriberRef`, `TenantRef` — who the usage is charged to |
//! | [`correlation`] | `CorrelationKeys`, `ChargingId`, `Icid`, `CallId`, `correlate()`, `CorrelatedGroup`, `LegChain` |
//! | [`container`] | `ServiceDataContainer`, `ContainerSet`, `RatingGroup`, `ChangeCondition` |
//! | [`partial`] | `PartialInfo`, `PartialRun`, `Reassembly`, `RecordClosureCause` |
//! | [`pipeline`] | `process()`, `Batch`, `ProcessState`, `ProcessOutcome` — the whole layer, in one pass |
//! | [`session`] | `Dnn`, `Snssai`, `RatType`, `ChargingCharacteristics` — the network, slice, access and charging profile a session ran on |
//! | [`timeband`] | `split_session()`, `split_measured()`, `apportion()` — dividing usage at a tariff boundary |
//! | [`record`] | `UsageRecord`, `RecordType`, `ServiceDomain`, `SourceRef` |
//! | [`lateness`] | `LatenessPolicy`, `Lateness` — one concept behind every window |
//! | [`sequence`] | `SequenceSpace`, `SequenceValue`, `SequenceTracker`, `SequenceSnapshot` — wrap-aware gap detection |
//! | [`dedup`] | `RecordKey`, `DedupStore`, `DedupWindow` — natural-key deduplication |
//! | [`disposition`] | `Disposition`, `Outcome`, `BatchReconciliation`, `PeriodLedger` — the conservation law |
//! | [`validation`] | `validate_records()`, `Rule` (V01–V21), `Finding`, `Severity` |
//! | [`restatement`] | `Restatement`, `UsageDelta`, `RestatementLog` — corrections as signed events |
//! | [`quality`] | `score_batch()`, `QualityGrade`, `ReleaseBlocker` — the release gate |
//! | [`aggregation`] | `aggregate_released()`, `UsageTotal` — what the batch adds up to |
//! | [`roaming`] | `TapBatch`, `BatchTotals`, `ScaledAmount`, `RapReturn` — a partner's batch, and whether it adds up |
//! | [`fairuse`] | `assess()`, `FairUseAssessment`, `FairUseAlert`, `VolumeAllowance` — EU 2016/2286 |
//! | [`retention`] | `RetentionPolicy`, `RetentionSchedule`, `Deletion` — how long a record may be kept |
//! | [`fraud`] | `Signal`, `Indication`, `impossible_travel()` — evidence, never verdicts |
//! | [`privacy`] | `Pseudonymiser`, `Reduction`, `itemise_called()` — narrowing an identity to the question |
//!
//! # Time is local, not UTC
//!
//! A charging record carries a **local** wall-clock time and a quarter-hour
//! offset (3GPP TS 32.298). Read as UTC, an evening event lands on the wrong
//! day, and no constant corrects it because the offset varies per record.
//!
//! ```rust
//! use mediation::CdrTimestamp;
//!
//! let ts: CdrTimestamp = "2026-07-28T00:30:00+05:45".parse().unwrap();
//! assert_eq!(ts.local_date().to_string(), "2026-07-28");    // the subscriber's day
//! assert_eq!(ts.to_utc().date().to_string(), "2026-07-27"); // is not the UTC day
//! ```
//!
//! # Integer quantities
//!
//! [`Octets`] is a `u64` and [`Seconds`] a `u32`; no quantity passes through a
//! float, which stops representing consecutive integers above 2^53 octets.
//! [`VolumePair`] keeps uplink and downlink apart. Rounding to chargeable units
//! ([`Octets::units_ceil`], [`Seconds::units_ceil`]) is a rating decision, offered
//! and applied by nothing.
//!
//! # Determinism
//!
//! No function reads the clock, the filesystem, the network or other ambient
//! state. An evaluation instant is a parameter, so an archived batch replays to
//! the verdicts it originally got.
//!
//! # Every record reaches exactly one outcome
//!
//! ```text
//! carried_in + ingested == billable + suppressed + duplicate + rejected + held
//! ```
//!
//! [`BatchReconciliation::balances`] is that equation, and [`PeriodLedger`]
//! chains batches so it holds across a billing period. See [`disposition`].
//!
//! # Every input is untrusted
//!
//! Every [`FromStr`](std::str::FromStr) and wire decoder returns a `Result` for
//! arbitrary input and never panics. Anything sized by the input is bounded
//! ([`SequenceTracker::observe`], [`PartialRun::assess`], [`BerReader`] nesting
//! depth) while counts stay exact, and no field is silently skipped.

/// The README's code blocks, compiled as doctests.
///
/// `cfg(doctest)` is set only while rustdoc is collecting doctests, so this
/// costs nothing in a normal build and adds nothing to the rendered docs. What
/// it buys is that a snippet in the README **cannot** drift from the API: a
/// README that claims to be tested and is not is worse than one that claims
/// nothing, because a reader trusts it.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;

/// Every page of the documentation site, compiled as doctests.
///
/// The site under `site/` is the crate's long-form documentation, and the same
/// argument that applies to the README applies to it with more force: a
/// published page is read by people who will not check it against the API, and
/// a page that has quietly drifted is worse than no page at all.
///
/// So every Rust block on the site is compiled and run by `cargo test`. A page
/// that names a function the crate does not have, or asserts a value it no
/// longer returns, fails the build. Zola's TOML front matter is inert markdown
/// to rustdoc, so the files need no preprocessing to be both.
///
/// The one thing this costs: the site cannot use rustdoc's hidden-line syntax,
/// because Zola would render the marker. Examples on the site are therefore
/// whole — which is what a reader wanting to paste one into a file needs
/// anyway.
#[cfg(doctest)]
mod site_doctests {
    /// The `getting-started` documentation page, compiled as doctests.
    #[doc = include_str!("../site/content/docs/getting-started.md")]
    pub struct GettingStarted;

    /// The `time` documentation page, compiled as doctests.
    #[doc = include_str!("../site/content/docs/time.md")]
    pub struct Time;

    /// The `quantities` documentation page, compiled as doctests.
    #[doc = include_str!("../site/content/docs/quantities.md")]
    pub struct Quantities;

    /// The `identity` documentation page, compiled as doctests.
    #[doc = include_str!("../site/content/docs/identity.md")]
    pub struct Identity;

    /// The `location` documentation page, compiled as doctests.
    #[doc = include_str!("../site/content/docs/location.md")]
    pub struct Location;

    /// The `pipeline` documentation page, compiled as doctests.
    #[doc = include_str!("../site/content/docs/pipeline.md")]
    pub struct Pipeline;

    /// The `fixed-voice` documentation page, compiled as doctests.
    #[doc = include_str!("../site/content/docs/fixed-voice.md")]
    pub struct FixedVoice;

    /// The `conservation` documentation page, compiled as doctests.
    #[doc = include_str!("../site/content/docs/conservation.md")]
    pub struct Conservation;

    /// The `completeness` documentation page, compiled as doctests.
    #[doc = include_str!("../site/content/docs/completeness.md")]
    pub struct Completeness;

    /// The `deduplication` documentation page, compiled as doctests.
    #[doc = include_str!("../site/content/docs/deduplication.md")]
    pub struct Deduplication;

    /// The `reassembly` documentation page, compiled as doctests.
    #[doc = include_str!("../site/content/docs/reassembly.md")]
    pub struct Reassembly;

    /// The `validation` documentation page, compiled as doctests.
    #[doc = include_str!("../site/content/docs/validation.md")]
    pub struct Validation;

    /// The `tariff-boundaries` documentation page, compiled as doctests.
    #[doc = include_str!("../site/content/docs/tariff-boundaries.md")]
    pub struct TariffBoundaries;

    /// The `roaming` documentation page, compiled as doctests.
    #[doc = include_str!("../site/content/docs/roaming.md")]
    pub struct Roaming;

    /// The `regulatory` documentation page, compiled as doctests.
    #[doc = include_str!("../site/content/docs/regulatory.md")]
    pub struct Regulatory;

    /// The `fraud` documentation page, compiled as doctests.
    #[doc = include_str!("../site/content/docs/fraud.md")]
    pub struct Fraud;

    /// The `wire-formats` documentation page, compiled as doctests.
    #[doc = include_str!("../site/content/docs/wire-formats.md")]
    pub struct WireFormats;

    /// The `design` documentation page, compiled as doctests.
    #[doc = include_str!("../site/content/docs/design.md")]
    pub struct Design;
}

pub mod aggregation;
pub mod ber;
pub mod cdrtime;
pub mod container;
pub mod correlation;
pub mod dedup;
pub mod dialplan;
pub mod digits;
pub mod disposition;
pub mod error;
pub mod fairuse;
pub mod fraud;
pub mod identity;
pub mod jurisdiction;
pub mod lateness;
pub mod location;
pub mod numbering;
pub mod partial;
pub mod party;
pub mod pipeline;
pub mod plmn;
pub mod privacy;
pub mod quality;
pub mod record;
pub mod restatement;
pub mod retention;
pub mod roaming;
pub mod sequence;
pub mod session;
pub mod timeband;
pub mod validation;
pub mod volume;

pub use aggregation::{
    AggregationError, ReleasedTotals, UsageTotal, aggregate, aggregate_by, aggregate_rating_groups,
    aggregate_released, aggregate_released_with_carry,
};
pub use ber::{BerElement, BerLength, BerReader, BerTag, TagClass};
pub use cdrtime::{CdrTimestamp, Century, DstAdjustment, MsTimeZone, QuarterHourOffset, Seconds};
pub use container::{ChangeCondition, ContainerSet, Qos5qi, RatingGroup, ServiceDataContainer};
pub use correlation::{
    BrokenLeg, CallId, ChargingId, CorrelatedGroup, CorrelationConfig, CorrelationKey,
    CorrelationKeys, CorrelationResult, Icid, LegChain, PacketSessionId, Unmatched,
    UnmatchedReason, correlate,
};
pub use dedup::{DedupDecision, DedupStore, DedupWindow, NotKeyable, RecordKey};
pub use dialplan::{
    AreaCode, Destination, DestinationClass, DialContext, DialContextError, DialError,
    GermanNumberingPlan, NumberingPlan, ShortCode, ShortCodePattern,
};
pub use digits::DigitString;
pub use disposition::{
    BatchReconciliation, ChainBreak, Disposition, HoldReason, Outcome, PeriodLedger, PeriodTotals,
    RejectReason, SuppressReason,
};
pub use error::{DecodeError, ParseError};
pub use fairuse::{
    AllowanceStatus, FairUseAlert, FairUseAssessment, ObservationWindow, PresenceDay, Region,
    Regions, SafeHarbour, VolumeAllowance,
};
pub use fraud::{
    Indication, Signal, TrafficShape, TransitTimes, UniformTransit, ait_signature,
    bypass_signature, impossible_travel, premium_destination, wangiri_pattern,
};
pub use identity::{
    AddressString, Gpsi, Iccid, Imei, Imeisv, Imsi, Msin, Msisdn, Nai, NatureOfAddress,
    NumberingPlanIndicator, Pei, RoutingIndicator, Suci, Supi, Tac,
};
pub use jurisdiction::{
    Ambiguity, Jurisdiction, customer_country_presumption, is_abroad, usage_country,
};
pub use lateness::{Lateness, LatenessPolicy};
pub use location::{
    CellId, Cgi, Ecgi, Eci, EnbKind, ExtendedEnbKind, ExtendedMacroEnbId, GnbIdLength, Lac, Lai,
    MacroEnbId, Ncgi, Nci, Rai, RoutingAreaCode, Sai, ServiceAreaCode, TacWidth, Tai,
    TrackingAreaCode, Uli, UserLocation,
};
pub use numbering::{NumberRange, NumberRangeSet, Porting, is_ported, terminating_plmn};
pub use partial::{
    OrphanReason, PartialFormat, PartialInfo, PartialRun, Reassembly, RecordClosureCause,
};
pub use party::{
    ChargedParty, NfInstanceId, PartyId, SubscriberRef, TenantRef, UnattributableReason,
};
pub use pipeline::{
    Batch, NoSuppression, ProcessConfig, ProcessOutcome, ProcessState, SequenceGap, Suppression,
    process, process_with,
};
pub use plmn::{Mcc, Mnc, MncWidth, MncWidths, NorthAmericanRule, PlmnId, TadigCode};
pub use privacy::{
    Itemisation, ItemisedDestination, NumberPrefix, ProtectedDestinations, Pseudonym,
    Pseudonymiser, ReducedSubscriber, Reduction, ShortenedNumber, itemise_called, itemise_calling,
    reduce_equipment, reduce_to_country, reduce_to_network, software_version,
};
pub use quality::{QualityGrade, QualityReport, ReleaseBlocker, score_batch, score_dispositions};
pub use record::{
    CallDirection, NodeId, PrimaryMeasure, RecordType, ServiceDomain, SourceRef, UsageRecord,
};
pub use restatement::{RestateReason, Restatement, RestatementLog, UsageDelta};
pub use retention::{Deletion, RetentionBasis, RetentionPolicy, RetentionSchedule};
pub use roaming::{BatchDiscrepancy, BatchTotals, RapReturn, ScaledAmount, TapBatch, TotalKind};
pub use sequence::{SequenceObservation, SequenceSpace, SequenceTracker, SequenceValue};
pub use session::{
    ChargingCharacteristics, Dnn, RatType, SessionContext, SliceServiceType, Snssai,
};
pub use timeband::{
    Apportionment, SplitError, TimeSlice, UsageSplit, apportion, split_measured, split_session,
};
pub use validation::{
    Finding, Rule, Severity, ValidationConfig, ValidationReport, validate_records,
};
pub use volume::{ByteScale, Octets, VolumePair};
