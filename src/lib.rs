#![deny(unsafe_code)]
#![deny(missing_docs)]

//! Telecom usage mediation domain library.
//!
//! A **standalone**, **pure** library for the arithmetic and classification that
//! sits between a mobile or fixed network and a billing system: normalising,
//! validating, deduplicating and correlating usage records. Zero I/O, no async,
//! no clock, exact integer quantities.
//!
//! It computes **usage**, not money: there is no currency, tariff, price or rate
//! type here. What leaves this crate is octets, seconds and event counts,
//! attributed to a charged party, a network and a service, which a rating layer
//! then prices.
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
//! | [`plmn`] | `Mcc`, `Mnc`, `PlmnId`, `TadigCode` — network identity |
//! | [`identity`] | `Imsi`, `Msisdn`, `Imei`, `Imeisv`, `Iccid`, `Supi`, `Gpsi`, `Pei`, `Suci` |
//! | [`jurisdiction`] | `usage_country()`, `customer_country_presumption()` — two questions, two countries |
//! | [`location`] | `UserLocation`, `Cgi`, `Ecgi`, `Ncgi`, `Tai` — where a record was produced |
//! | [`numbering`] | `NumberRange`, `NumberRangeSet`, `Porting`, `terminating_plmn` — who really terminates a number |
//! | [`party`] | `ChargedParty`, `SubscriberRef`, `TenantRef` — who the usage is charged to |
//! | [`correlation`] | `CorrelationKeys`, `ChargingId`, `Icid`, `PacketSessionId` |
//! | [`container`] | `ServiceDataContainer`, `ContainerSet`, `RatingGroup`, `ChangeCondition` |
//! | [`correlate`](mod@correlate) | `correlate()`, `CorrelatedGroup`, `LegChain` — joining one usage's records |
//! | [`partial`] | `PartialInfo`, `PartialRun`, `Reassembly`, `RecordClosureCause` |
//! | [`session`] | `Dnn`, `Snssai`, `RatType` — the network, slice and access a session ran on |
//! | [`timeband`] | `split_session()`, `apportion()`, `Apportionment` — dividing usage at a tariff boundary |
//! | [`record`] | `UsageRecord`, `RecordType`, `ServiceDomain`, `SourceRef` |
//! | [`lateness`] | `LatenessPolicy`, `Lateness` — one concept behind every window |
//! | [`sequence`] | `SequenceSpace`, `SequenceValue`, `SequenceTracker` — wrap-aware gap detection |
//! | [`dedup`] | `RecordKey`, `DedupStore`, `DedupWindow` — natural-key deduplication |
//! | [`disposition`] | `Disposition`, `BatchReconciliation` — the conservation law |
//! | [`validation`] | `validate_records()`, `Rule` (V01–V21), `Finding`, `Severity` |
//! | [`restatement`] | `Restatement`, `UsageDelta`, `RestatementLog` — corrections as signed events |
//! | [`quality`] | `score_batch()`, `QualityGrade`, `ReleaseBlocker` — the release gate |
//! | [`aggregation`] | `aggregate_released()`, `UsageTotal` — what the batch adds up to |
//! | [`roaming`] | `TapBatch`, `ScaledAmount`, `RapReturn` — a partner's batch, and whether it adds up |
//! | [`fairuse`] | `assess()`, `FairUseAssessment`, `VolumeAllowance` — EU 2016/2286 |
//! | [`retention`] | `RetentionPolicy`, `RetentionSchedule` — how long a record may be kept |
//! | [`fraud`] | `Signal`, `Indication`, `impossible_travel()` — evidence, never verdicts |
//! | [`privacy`] | `Pseudonymiser`, `Reduction` — narrowing an identity to the question |
//!
//! # Time is local, not UTC
//!
//! A charging record carries a **local** wall-clock time and a numeric offset
//! (3GPP TS 32.298), and the offset counts **quarter-hours** because a great
//! many zones are not on an hour boundary. Reading such a value as UTC books
//! every evening event of a UTC+X network onto the wrong day, and unlike a
//! fixed-zone mistake it cannot be corrected downstream by a constant, because
//! the offset varies per record.
//!
//! ```rust
//! use mediation::CdrTimestamp;
//!
//! let ts: CdrTimestamp = "2026-07-28T00:30:00+05:45".parse().unwrap();
//!
//! // The subscriber's day …
//! assert_eq!(ts.local_date().to_string(), "2026-07-28");
//! // … is not the UTC day.
//! assert_eq!(ts.to_utc().date().to_string(), "2026-07-27");
//! ```
//!
//! [`cdrtime`] documents the rest, including why [`CdrTimestamp`] implements no
//! ordering at all.
//!
//! # Numeric types
//!
//! Every chargeable quantity is an **integer**: [`Octets`] is a `u64` and
//! [`Seconds`] a `u32`. A data volume is inherently a count, so a float is not
//! merely imprecise but categorically wrong — above 2^53 octets (about 9.01 PB)
//! consecutive integers are no longer representable, and a national monthly
//! aggregate crosses that. No value here is ever routed through a float.
//!
//! Uplink and downlink are kept apart in [`VolumePair`], with no constructor
//! that takes only a total: asymmetric tariffs and interconnect settlement need
//! the split, and it cannot be recovered once summed.
//!
//! # Determinism
//!
//! **No function in this crate reads the system clock, the filesystem, the
//! network or any other ambient state.** Every input is an argument, so equal
//! inputs always produce equal outputs and any result can be replayed or
//! cached. Where an evaluation instant is needed it is a parameter, which is
//! what allows an archived batch to be reprocessed years later and yield the
//! verdicts it originally did rather than the verdicts of today.
//!
//! # Every record reaches exactly one outcome
//!
//! The crate's central invariant, and the difference between a mediation layer
//! and a data pipeline: a pipeline moves records, while a mediation layer can
//! **prove** it did not lose any.
//!
//! ```text
//! carried_in + ingested == billable + suppressed + duplicate + rejected + held
//! ```
//!
//! [`BatchReconciliation::balances`] is that equation. A batch that fails it has
//! unknown completeness, and billing over unknown completeness invoices an
//! amount nobody can prove is right. And because a held record comes back,
//! [`PeriodLedger`] chains batches so the law telescopes across a whole period
//! rather than holding only inside each one. See [`disposition`].
//!
//! # Rounding is a rating decision
//!
//! Rounding up to whole chargeable units — per minute, per kilobyte — is priced
//! behaviour. It is offered here as a named primitive
//! ([`Octets::units_ceil`], [`Seconds::units_ceil`]) and applied by nothing.
//! What leaves this crate is the measured quantity.

pub mod aggregation;
pub mod ber;
pub mod cdrtime;
pub mod container;
pub mod correlate;
pub mod correlation;
pub mod dedup;
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
    aggregate_released,
};
pub use ber::{BerElement, BerLength, BerReader, BerTag, TagClass};
pub use cdrtime::{CdrTimestamp, Century, DstAdjustment, MsTimeZone, QuarterHourOffset, Seconds};
pub use container::{ChangeCondition, ContainerSet, Qos5qi, RatingGroup, ServiceDataContainer};
pub use correlate::{
    CorrelatedGroup, CorrelationConfig, CorrelationKey, CorrelationResult, LegChain, Unmatched,
    UnmatchedReason, correlate,
};
pub use correlation::{ChargingId, CorrelationKeys, Icid, PacketSessionId};
pub use dedup::{DedupDecision, DedupStore, DedupWindow, NotKeyable, RecordKey};
pub use digits::DigitString;
pub use disposition::{
    BatchReconciliation, ChainBreak, Disposition, HoldReason, PeriodLedger, RejectReason,
    SuppressReason,
};
pub use error::{DecodeError, ParseError};
pub use fairuse::{
    AllowanceStatus, FairUseAssessment, ObservationWindow, PresenceDay, Region, Regions,
    SafeHarbour, VolumeAllowance,
};
pub use fraud::{
    CalledParty, Indication, Signal, TrafficShape, ait_signature, bypass_signature,
    impossible_travel, premium_destination, wangiri_pattern,
};
pub use identity::{Gpsi, Iccid, Imei, Imeisv, Imsi, Msin, Msisdn, Nai, Pei, Suci, Supi, Tac};
pub use jurisdiction::{
    Ambiguity, Jurisdiction, customer_country_presumption, is_abroad, usage_country,
};
pub use lateness::{Lateness, LatenessPolicy};
pub use location::{
    CellId, Cgi, Ecgi, Eci, EnbKind, GnbIdLength, Lac, Ncgi, Nci, Sai, ServiceAreaCode, TacWidth,
    Tai, TrackingAreaCode, UserLocation,
};
pub use numbering::{NumberRange, NumberRangeSet, Porting, is_ported, terminating_plmn};
pub use partial::{
    OrphanReason, PartialFormat, PartialInfo, PartialRun, Reassembly, RecordClosureCause,
};
pub use party::{
    ChargedParty, NfInstanceId, PartyId, SubscriberRef, TenantRef, UnattributableReason,
};
pub use plmn::{Mcc, Mnc, MncWidth, MncWidths, NorthAmericanRule, PlmnId, TadigCode};
pub use privacy::{
    NumberPrefix, Pseudonym, Pseudonymiser, ReducedSubscriber, Reduction, reduce_equipment,
    software_version,
};
pub use quality::{QualityGrade, QualityReport, ReleaseBlocker, score_batch};
pub use record::{NodeId, PrimaryMeasure, RecordType, ServiceDomain, SourceRef, UsageRecord};
pub use restatement::{RestateReason, Restatement, RestatementLog, UsageDelta};
pub use retention::{RetentionBasis, RetentionPolicy, RetentionSchedule};
pub use roaming::{BatchDiscrepancy, RapReturn, ScaledAmount, TapBatch};
pub use sequence::{SequenceObservation, SequenceSpace, SequenceTracker, SequenceValue};
pub use session::{Dnn, RatType, SessionContext, SliceServiceType, Snssai};
pub use timeband::{
    Apportionment, SplitError, TimeSlice, UsageSplit, apportion, split_measured, split_session,
};
pub use validation::{
    Finding, Rule, Severity, ValidationConfig, ValidationReport, validate_records,
};
pub use volume::{ByteScale, Octets, VolumePair};
