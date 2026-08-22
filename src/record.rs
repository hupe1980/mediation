//! The core usage record, and the vocabulary that classifies it.
//!
//! # Duration is not `closed − opened`
//!
//! The duration a record carries is the **chargeable** one, measured from
//! answer. A 45-second ring followed by 10 seconds of conversation is a
//! 10-second call across a 55-second wall-clock span. Deriving duration by
//! subtraction over-bills every answered call and invents duration for every
//! unanswered one, so [`UsageRecord::duration`] is a field and
//! [`UsageRecord::wall_clock_span`] is a separate, differently-named accessor.
//! Nothing falls back from one to the other.
//!
//! ```rust
//! # use mediation::{ChargedParty, RecordType, ServiceDomain, Seconds, SourceRef, UsageRecord};
//! # let party = ChargedParty::Subscriber(
//! #     mediation::SubscriberRef::new("imsi-262021234567890".parse().unwrap()));
//! # let source = SourceRef::new("msc-01".parse().unwrap(), 0);
//! let call = UsageRecord::new(
//!     RecordType::MobileOriginatedCall,
//!     party,
//!     "2026-07-28T10:00:00+02:00".parse().unwrap(),
//!     source,
//! )
//! .with_closed("2026-07-28T10:00:55+02:00".parse().unwrap())
//! .with_duration(Seconds::new(10));
//!
//! assert_eq!(call.duration, Some(Seconds::new(10)));           // billed
//! assert_eq!(call.wall_clock_span(), Some(Seconds::new(55)));  // observed
//! assert_eq!(call.service(), ServiceDomain::Voice);            // derived
//! ```
//!
//! # The service domain is derived, not stored
//!
//! [`RecordType::service_domain`] is total, so a `service` field alongside the
//! record type would be a second copy of the same fact — and two copies of one
//! fact eventually disagree. Deriving it removes the disagreement rather than
//! adding a rule to detect it.
//!
//! # Provenance is mandatory
//!
//! [`SourceRef`] is not optional and not decorative. When a billing dispute
//! reaches an auditor two years later the question is always *which file, from
//! which node, at which offset produced this line* — and a record that cannot
//! answer it is not evidence.

use std::fmt;
use std::str::FromStr;

use crate::cdrtime::{CdrTimestamp, Seconds};
use crate::container::ContainerSet;
use crate::correlation::CorrelationKeys;
use crate::error::ParseError;
use crate::identity::{Iccid, Pei};
use crate::partial::PartialInfo;
use crate::party::ChargedParty;
use crate::plmn::PlmnId;
use crate::session::SessionContext;
use crate::volume::VolumePair;

// ── ServiceDomain ────────────────────────────────────────────────────────────

/// What kind of service produced a usage record.
///
/// Exhaustive, and deliberately not `#[non_exhaustive]`: a consumer mapping
/// this vocabulary onto their own storage codes should get a compile error when
/// a new domain appears, not a silent wildcard that files the usage under the
/// wrong service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ServiceDomain {
    /// Circuit-switched or IMS telephony.
    Voice,
    /// Short messages.
    Sms,
    /// Multimedia messages.
    Mms,
    /// Packet data — PDP contexts and PDU sessions.
    Data,
    /// IMS sessions other than telephony.
    Ims,
    /// Fixed-line and broadband access.
    Fixed,
    /// Machine-type communications: NB-IoT, LTE-M and their successors.
    Iot,
    /// Network API invocations exposed to third parties.
    NetworkApi,
    /// Network slice lifecycle and performance.
    Slice,
}

/// The quantity that principally measures a service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PrimaryMeasure {
    /// Measured by elapsed chargeable time.
    Duration,
    /// Measured by transferred octets.
    Volume,
    /// Measured by a count of discrete events.
    Events,
}

impl ServiceDomain {
    /// Every variant, for exhaustive iteration in tests and mappings.
    pub const ALL: &'static [Self] = &[
        Self::Voice,
        Self::Sms,
        Self::Mms,
        Self::Data,
        Self::Ims,
        Self::Fixed,
        Self::Iot,
        Self::NetworkApi,
        Self::Slice,
    ];

    /// The canonical codes, in the order of [`ALL`](Self::ALL).
    pub const CODES: &'static [&'static str] = &[
        "VOICE",
        "SMS",
        "MMS",
        "DATA",
        "IMS",
        "FIXED",
        "IOT",
        "NETWORK_API",
        "SLICE",
    ];

    /// The quantity this service is principally measured by.
    ///
    /// A cross-check rather than a rule: a record whose populated quantity does
    /// not match its domain is worth a finding, because it usually means a
    /// field was mapped into the wrong column upstream.
    #[must_use]
    pub const fn primary_measure(self) -> PrimaryMeasure {
        match self {
            Self::Voice | Self::Ims | Self::Slice => PrimaryMeasure::Duration,
            Self::Data | Self::Fixed | Self::Iot => PrimaryMeasure::Volume,
            Self::Sms | Self::Mms | Self::NetworkApi => PrimaryMeasure::Events,
        }
    }

    /// `true` for services whose usage always belongs to a subscription.
    ///
    /// [`NetworkApi`](Self::NetworkApi) and [`Slice`](Self::Slice) are the two
    /// that need not — see [`crate::party`].
    #[must_use]
    pub const fn is_subscriber_bound(self) -> bool {
        !matches!(self, Self::NetworkApi | Self::Slice)
    }

    /// The canonical string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Voice => "VOICE",
            Self::Sms => "SMS",
            Self::Mms => "MMS",
            Self::Data => "DATA",
            Self::Ims => "IMS",
            Self::Fixed => "FIXED",
            Self::Iot => "IOT",
            Self::NetworkApi => "NETWORK_API",
            Self::Slice => "SLICE",
        }
    }
}

impl fmt::Display for ServiceDomain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ServiceDomain {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim().to_ascii_uppercase();
        Self::ALL
            .iter()
            .find(|d| d.as_str() == t)
            .copied()
            .ok_or_else(|| ParseError::one_of("ServiceDomain", s, Self::CODES))
    }
}

// ── RecordType ───────────────────────────────────────────────────────────────

/// The specific kind of record, within a [`ServiceDomain`].
///
/// Exhaustive for the same reason [`ServiceDomain`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RecordType {
    /// A call placed by the subscriber.
    MobileOriginatedCall,
    /// A call received by the subscriber.
    MobileTerminatedCall,
    /// A call forwarded on the subscriber's behalf.
    ForwardedCall,
    /// A short message sent by the subscriber.
    SmsOriginated,
    /// A short message delivered to the subscriber.
    SmsTerminated,
    /// A multimedia message sent by the subscriber.
    MmsOriginated,
    /// A multimedia message retrieved by the subscriber.
    MmsTerminated,
    /// A 5G packet data unit session.
    PduSession,
    /// A pre-5G packet data protocol context.
    PdpContext,
    /// An IMS session other than telephony.
    ImsSession,
    /// A fixed-line or broadband access session.
    FixedSession,
    /// A single invocation of an exposed network API.
    NetworkApiInvocation,
    /// A network slice lifecycle event.
    SliceLifecycle,
    /// A registration or mobility event, charged in its own right.
    Registration,
}

impl RecordType {
    /// Every variant, for exhaustive iteration in tests and mappings.
    pub const ALL: &'static [Self] = &[
        Self::MobileOriginatedCall,
        Self::MobileTerminatedCall,
        Self::ForwardedCall,
        Self::SmsOriginated,
        Self::SmsTerminated,
        Self::MmsOriginated,
        Self::MmsTerminated,
        Self::PduSession,
        Self::PdpContext,
        Self::ImsSession,
        Self::FixedSession,
        Self::NetworkApiInvocation,
        Self::SliceLifecycle,
        Self::Registration,
    ];

    /// The canonical codes, in the order of [`ALL`](Self::ALL).
    pub const CODES: &'static [&'static str] = &[
        "MO_CALL",
        "MT_CALL",
        "FORWARDED_CALL",
        "SMS_MO",
        "SMS_MT",
        "MMS_MO",
        "MMS_MT",
        "PDU_SESSION",
        "PDP_CONTEXT",
        "IMS_SESSION",
        "FIXED_SESSION",
        "NETWORK_API_INVOCATION",
        "SLICE_LIFECYCLE",
        "REGISTRATION",
    ];

    /// The service domain this record type belongs to.
    ///
    /// Lets a decoder derive the domain rather than carry it twice, and lets a
    /// validator catch a record whose two fields disagree.
    #[must_use]
    pub const fn service_domain(self) -> ServiceDomain {
        match self {
            Self::MobileOriginatedCall | Self::MobileTerminatedCall | Self::ForwardedCall => {
                ServiceDomain::Voice
            }
            Self::SmsOriginated | Self::SmsTerminated => ServiceDomain::Sms,
            Self::MmsOriginated | Self::MmsTerminated => ServiceDomain::Mms,
            Self::PduSession | Self::PdpContext | Self::Registration => ServiceDomain::Data,
            Self::ImsSession => ServiceDomain::Ims,
            Self::FixedSession => ServiceDomain::Fixed,
            Self::NetworkApiInvocation => ServiceDomain::NetworkApi,
            Self::SliceLifecycle => ServiceDomain::Slice,
        }
    }

    /// `true` for record types that describe a session, and can therefore be
    /// split across partial records.
    ///
    /// Event-shaped records — a short message, an API invocation — are produced
    /// whole, so an incomplete one is a different kind of problem.
    #[must_use]
    pub const fn is_session(self) -> bool {
        matches!(
            self,
            Self::MobileOriginatedCall
                | Self::MobileTerminatedCall
                | Self::ForwardedCall
                | Self::PduSession
                | Self::PdpContext
                | Self::ImsSession
                | Self::FixedSession
                | Self::SliceLifecycle
        )
    }

    /// The canonical string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MobileOriginatedCall => "MO_CALL",
            Self::MobileTerminatedCall => "MT_CALL",
            Self::ForwardedCall => "FORWARDED_CALL",
            Self::SmsOriginated => "SMS_MO",
            Self::SmsTerminated => "SMS_MT",
            Self::MmsOriginated => "MMS_MO",
            Self::MmsTerminated => "MMS_MT",
            Self::PduSession => "PDU_SESSION",
            Self::PdpContext => "PDP_CONTEXT",
            Self::ImsSession => "IMS_SESSION",
            Self::FixedSession => "FIXED_SESSION",
            Self::NetworkApiInvocation => "NETWORK_API_INVOCATION",
            Self::SliceLifecycle => "SLICE_LIFECYCLE",
            Self::Registration => "REGISTRATION",
        }
    }
}

impl fmt::Display for RecordType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RecordType {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim().to_ascii_uppercase();
        Self::ALL
            .iter()
            .find(|r| r.as_str() == t)
            .copied()
            .ok_or_else(|| ParseError::one_of("RecordType", s, Self::CODES))
    }
}

// ── Provenance ───────────────────────────────────────────────────────────────

/// The network element that produced a record file.
///
/// Printable ASCII without whitespace, 1–64 characters — the same shape a
/// 3GPP TS 32.297 file name carries as its `NodeID`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(String);

/// The shape [`NodeId`] accepts, as rendered in a [`ParseError`].
const NODE_ID_FORMAT: &str =
    "1 to 64 printable ASCII characters without whitespace, such as pgw-fra-01";

impl NodeId {
    /// The identifier as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for NodeId {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim();
        let ok = !t.is_empty()
            && t.len() <= 64
            && t.bytes()
                .all(|b| b.is_ascii_graphic() && !b.is_ascii_whitespace());
        if ok {
            Ok(Self(t.to_owned()))
        } else {
            Err(ParseError::format("NodeId", s, NODE_ID_FORMAT))
        }
    }
}

/// Where a record came from: which node, which file, which position.
///
/// Mandatory on every [`UsageRecord`]. The file sequence is what makes silent
/// loss detectable at all — a lost file leaves no trace in the surviving data,
/// and the only evidence is a hole in this number.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceRef {
    /// The network element that produced the file.
    pub node_id: NodeId,
    /// The file's name, where the transport supplied one.
    pub file_name: Option<String>,
    /// The file's sequence number within its source, where it carried one.
    ///
    /// `None` means completeness cannot be established for this record's file —
    /// which is a finding, not a neutral absence.
    pub file_sequence: Option<u32>,
    /// The record's zero-based position within its file.
    pub record_index: u32,
}

impl SourceRef {
    /// Provenance for a record at a known position in an unnamed stream.
    #[must_use]
    pub const fn new(node_id: NodeId, record_index: u32) -> Self {
        Self {
            node_id,
            file_name: None,
            file_sequence: None,
            record_index,
        }
    }

    /// The same provenance with the file's name recorded.
    #[must_use]
    pub fn with_file_name(mut self, name: impl Into<String>) -> Self {
        self.file_name = Some(name.into());
        self
    }

    /// The same provenance with the file's sequence number recorded.
    #[must_use]
    pub const fn with_file_sequence(mut self, sequence: u32) -> Self {
        self.file_sequence = Some(sequence);
        self
    }

    /// `true` when this record's file can take part in gap detection.
    #[must_use]
    pub const fn supports_completeness_check(&self) -> bool {
        self.file_sequence.is_some()
    }
}

impl fmt::Display for SourceRef {
    /// Writes `node[/file][#sequence]:index`, compact enough for a log line.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.node_id)?;
        if let Some(name) = &self.file_name {
            write!(f, "/{name}")?;
        }
        if let Some(sequence) = self.file_sequence {
            write!(f, "#{sequence}")?;
        }
        write!(f, ":{}", self.record_index)
    }
}

// ── UsageRecord ──────────────────────────────────────────────────────────────

/// One unit of usage, normalised out of whatever format carried it.
///
/// The canonical internal form. Decoders for 3GPP CDRs, TAP batches, RADIUS
/// accounting and the 5G charging service all target this type, so that no
/// wire format's idiosyncrasies reach the rules that operate on it.
///
/// Immutable by convention: a correction is a new record that supersedes an old
/// one, never an edit. Once a batch's counts have been reported, editing one of
/// its records invalidates the report with no way left to reconcile what was
/// released against what is now stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageRecord {
    /// The specific kind of record, from which the service domain follows.
    pub record_type: RecordType,
    /// Who the usage is charged to.
    pub party: ChargedParty,
    /// The equipment involved, where the record identified it.
    pub equipment: Option<Pei>,
    /// The SIM the subscription was served on, where the record carried it.
    ///
    /// Distinct from the subscription itself: a subscription outlives the cards
    /// it has been issued on, so counting cards and counting subscriptions give
    /// different answers and both are sometimes wanted.
    pub iccid: Option<Iccid>,
    /// The network where the usage happened.
    ///
    /// `None` for management-plane usage that no access network served.
    pub serving_plmn: Option<PlmnId>,
    /// The network that owns the subscription.
    pub home_plmn: Option<PlmnId>,
    /// When the usage began, in local time with its offset.
    pub opened: CdrTimestamp,
    /// When the usage ended, where it has ended.
    pub closed: Option<CdrTimestamp>,
    /// The **chargeable** duration the record carried.
    ///
    /// Never derived from [`opened`](Self::opened) and [`closed`](Self::closed)
    /// — see [`wall_clock_span`](Self::wall_clock_span).
    pub duration: Option<Seconds>,
    /// Transferred volume, uplink and downlink kept apart.
    pub volume: Option<VolumePair>,
    /// A count of discrete events, for event-shaped services.
    pub events: Option<u32>,
    /// Per-rating-group usage, where the record broke its total down.
    ///
    /// Kept alongside the record-level total rather than replacing it: the two
    /// are separate statements of one fact and can disagree, which is worth
    /// reporting rather than resolving by fiat.
    pub containers: ContainerSet,
    /// This record's place in a run of partials, when it is one.
    pub partial: Option<PartialInfo>,
    /// The data network, slice and access technology the session ran on.
    pub session: SessionContext,
    /// The identifiers that tie this record to its session and its siblings.
    pub correlation: CorrelationKeys,
    /// Which file, from which node, at which position produced this record.
    pub provenance: SourceRef,
}

impl UsageRecord {
    /// A record with only the fields every record must have.
    ///
    /// Quantities are attached with the `with_*` methods, so a record can never
    /// be constructed with a quantity and no provenance, or with a close time
    /// and no open time.
    #[must_use]
    pub const fn new(
        record_type: RecordType,
        party: ChargedParty,
        opened: CdrTimestamp,
        provenance: SourceRef,
    ) -> Self {
        Self {
            record_type,
            party,
            equipment: None,
            iccid: None,
            serving_plmn: None,
            home_plmn: None,
            opened,
            closed: None,
            duration: None,
            volume: None,
            events: None,
            containers: ContainerSet::empty(),
            partial: None,
            session: SessionContext::none(),
            correlation: CorrelationKeys::none(),
            provenance,
        }
    }

    /// The service domain, derived from the record type.
    ///
    /// Not a stored field: a second copy of the same fact eventually disagrees
    /// with the first, and there is no correct way to resolve the disagreement
    /// after the fact.
    #[must_use]
    pub const fn service(&self) -> ServiceDomain {
        self.record_type.service_domain()
    }

    /// The same record with its session correlators.
    #[must_use]
    pub fn with_correlation(mut self, correlation: CorrelationKeys) -> Self {
        self.correlation = correlation;
        self
    }

    /// The same record with its per-rating-group breakdown.
    #[must_use]
    pub fn with_containers(mut self, containers: ContainerSet) -> Self {
        self.containers = containers;
        self
    }

    /// The same record with the network, slice and access it ran on.
    #[must_use]
    pub fn with_session(mut self, session: SessionContext) -> Self {
        self.session = session;
        self
    }

    /// The same record marked as one partial of a run.
    #[must_use]
    pub const fn with_partial(mut self, partial: PartialInfo) -> Self {
        self.partial = Some(partial);
        self
    }

    /// `true` when this record is one of a run rather than a whole session.
    #[must_use]
    pub const fn is_partial(&self) -> bool {
        self.partial.is_some()
    }

    /// The volume to bill: the record's own total, or the container sum when
    /// it has none.
    ///
    /// A record that carries only a breakdown still described real usage, and
    /// dropping it for want of a record-level field would lose it. Where both
    /// are present the record's own total wins — and
    /// [`containers_agree`](Self::containers_agree) is how a caller learns they
    /// disagree before trusting either.
    #[must_use]
    pub fn billable_volume(&self) -> Option<VolumePair> {
        self.volume.or_else(|| {
            if self.containers.is_empty() {
                None
            } else {
                self.containers.checked_total()
            }
        })
    }

    /// Whether the record-level volume agrees with its container breakdown.
    ///
    /// `None` when there is nothing to compare — no containers, no total, or a
    /// sum that overflows. See [`ContainerSet::discrepancy`] for how far out a
    /// disagreement is.
    #[must_use]
    pub fn containers_agree(&self) -> Option<bool> {
        if self.containers.is_empty() {
            return None;
        }
        self.containers.agrees_with(self.volume)
    }

    /// The same record with a close time.
    #[must_use]
    pub const fn with_closed(mut self, closed: CdrTimestamp) -> Self {
        self.closed = Some(closed);
        self
    }

    /// The same record with a chargeable duration.
    #[must_use]
    pub const fn with_duration(mut self, duration: Seconds) -> Self {
        self.duration = Some(duration);
        self
    }

    /// The same record with a transferred volume.
    #[must_use]
    pub const fn with_volume(mut self, volume: VolumePair) -> Self {
        self.volume = Some(volume);
        self
    }

    /// The same record with an event count.
    #[must_use]
    pub const fn with_events(mut self, events: u32) -> Self {
        self.events = Some(events);
        self
    }

    /// The same record with the equipment identified.
    #[must_use]
    pub const fn with_equipment(mut self, equipment: Pei) -> Self {
        self.equipment = Some(equipment);
        self
    }

    /// The same record with the SIM identified.
    #[must_use]
    pub const fn with_iccid(mut self, iccid: Iccid) -> Self {
        self.iccid = Some(iccid);
        self
    }

    /// The same record with its serving and home networks.
    #[must_use]
    pub const fn with_networks(mut self, serving: PlmnId, home: PlmnId) -> Self {
        self.serving_plmn = Some(serving);
        self.home_plmn = Some(home);
        self
    }

    /// The elapsed time between the open and close timestamps.
    ///
    /// **Not** the chargeable duration, and not a substitute for it. Useful for
    /// plausibility checks — a chargeable duration exceeding the wall-clock span
    /// is impossible — and for nothing that reaches an invoice.
    ///
    /// `None` when the record has not closed, or when it closed before it
    /// opened, which is a broken record rather than a negative duration.
    #[must_use]
    pub fn wall_clock_span(&self) -> Option<Seconds> {
        let closed = self.closed?;
        let elapsed = (closed.to_utc() - self.opened.to_utc()).whole_seconds();
        u32::try_from(elapsed).ok().map(Seconds::new)
    }

    /// Whether the usage happened outside the subscription's home network.
    ///
    /// `None` when either network is unknown: a record that cannot establish
    /// this is not a domestic record, and defaulting it to `false` understates
    /// roaming in exactly the direction that costs money.
    #[must_use]
    pub fn is_roaming(&self) -> Option<bool> {
        let (serving, home) = (self.serving_plmn?, self.home_plmn?);
        Some(!serving.is_same_network(home))
    }

    /// The local calendar date the usage began on — the day a subscriber would
    /// recognise on an itemised bill.
    #[must_use]
    pub fn local_date(&self) -> time::Date {
        self.opened.local_date()
    }

    /// `true` when the record carries no quantity at all.
    ///
    /// Zero volume, zero duration and zero events together mean the record
    /// describes nothing; whether that is chargeable is a policy question, but
    /// it is always worth surfacing.
    ///
    /// The volume is [`billable_volume`](Self::billable_volume), not the
    /// record-level field: a packet-data record commonly carries its volume
    /// **only** in its containers, and reading the absent record total as zero
    /// would call a five-gigabyte session empty — which a suppression policy
    /// then deliberately does not bill.
    #[must_use]
    pub fn is_zero_usage(&self) -> bool {
        self.duration.is_none_or(Seconds::is_zero)
            && self.billable_volume().is_none_or(VolumePair::is_zero)
            && self.events.is_none_or(|e| e == 0)
    }
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::{NodeId, RecordType, ServiceDomain, SourceRef, UsageRecord};
    use serde::de::{self, Visitor};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::fmt;

    macro_rules! string_serde {
        ($ty:ty, $expecting:literal) => {
            impl Serialize for $ty {
                fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                    serializer.collect_str(self)
                }
            }

            impl<'de> Deserialize<'de> for $ty {
                fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                    struct V;
                    impl Visitor<'_> for V {
                        type Value = $ty;
                        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                            f.write_str($expecting)
                        }
                        fn visit_str<E: de::Error>(self, v: &str) -> Result<$ty, E> {
                            v.parse().map_err(de::Error::custom)
                        }
                    }
                    deserializer.deserialize_str(V)
                }
            }
        };
    }

    string_serde!(ServiceDomain, "a service domain code such as \"DATA\"");
    string_serde!(RecordType, "a record type code such as \"PDU_SESSION\"");
    string_serde!(NodeId, "a node identifier such as \"pgw-fra-01\"");

    /// Field names are part of the public representation and covered by semver.
    #[derive(Serialize, Deserialize)]
    #[serde(rename = "SourceRef")]
    struct SourceRefRepr {
        node_id: NodeId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_sequence: Option<u32>,
        record_index: u32,
    }

    impl Serialize for SourceRef {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            SourceRefRepr {
                node_id: self.node_id.clone(),
                file_name: self.file_name.clone(),
                file_sequence: self.file_sequence,
                record_index: self.record_index,
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for SourceRef {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = SourceRefRepr::deserialize(deserializer)?;
            Ok(SourceRef {
                node_id: r.node_id,
                file_name: r.file_name,
                file_sequence: r.file_sequence,
                record_index: r.record_index,
            })
        }
    }

    #[derive(Serialize, Deserialize)]
    #[serde(rename = "UsageRecord")]
    struct UsageRecordRepr {
        record_type: RecordType,
        party: crate::party::ChargedParty,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        equipment: Option<crate::identity::Pei>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        iccid: Option<crate::identity::Iccid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        serving_plmn: Option<crate::plmn::PlmnId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        home_plmn: Option<crate::plmn::PlmnId>,
        opened: crate::cdrtime::CdrTimestamp,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        closed: Option<crate::cdrtime::CdrTimestamp>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_s: Option<crate::cdrtime::Seconds>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        volume: Option<crate::volume::VolumePair>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        events: Option<u32>,
        #[serde(
            default,
            skip_serializing_if = "crate::container::ContainerSet::is_empty"
        )]
        containers: crate::container::ContainerSet,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        partial: Option<crate::partial::PartialInfo>,
        #[serde(
            default,
            skip_serializing_if = "crate::session::SessionContext::is_empty"
        )]
        session: crate::session::SessionContext,
        #[serde(
            default,
            skip_serializing_if = "crate::correlation::CorrelationKeys::is_empty"
        )]
        correlation: crate::correlation::CorrelationKeys,
        provenance: SourceRef,
    }

    impl Serialize for UsageRecord {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            UsageRecordRepr {
                record_type: self.record_type,
                party: self.party.clone(),
                equipment: self.equipment,
                iccid: self.iccid,
                serving_plmn: self.serving_plmn,
                home_plmn: self.home_plmn,
                opened: self.opened,
                closed: self.closed,
                duration_s: self.duration,
                volume: self.volume,
                events: self.events,
                containers: self.containers.clone(),
                partial: self.partial,
                session: self.session.clone(),
                correlation: self.correlation.clone(),
                provenance: self.provenance.clone(),
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for UsageRecord {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = UsageRecordRepr::deserialize(deserializer)?;
            Ok(UsageRecord {
                record_type: r.record_type,
                party: r.party,
                equipment: r.equipment,
                iccid: r.iccid,
                serving_plmn: r.serving_plmn,
                home_plmn: r.home_plmn,
                opened: r.opened,
                closed: r.closed,
                duration: r.duration_s,
                volume: r.volume,
                events: r.events,
                containers: r.containers,
                partial: r.partial,
                session: r.session,
                correlation: r.correlation,
                provenance: r.provenance,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::party::SubscriberRef;
    use crate::volume::Octets;

    fn party() -> ChargedParty {
        ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap()))
    }

    fn source() -> SourceRef {
        SourceRef::new("pgw-fra-01".parse().unwrap(), 0)
    }

    fn call() -> UsageRecord {
        UsageRecord::new(
            RecordType::MobileOriginatedCall,
            party(),
            "2026-07-28T10:00:00+02:00".parse().unwrap(),
            source(),
        )
    }

    /// The bug this API shape exists to prevent: chargeable duration is not the
    /// wall-clock span, and no accessor may quietly substitute one for the other.
    #[test]
    fn chargeable_duration_and_wall_clock_span_are_different_numbers() {
        let record = call()
            .with_closed("2026-07-28T10:00:55+02:00".parse().unwrap())
            .with_duration(Seconds::new(10));

        assert_eq!(record.duration, Some(Seconds::new(10)));
        assert_eq!(record.wall_clock_span(), Some(Seconds::new(55)));

        // An unanswered call has a span but no chargeable duration, and nothing
        // fills the gap.
        let unanswered = call().with_closed("2026-07-28T10:00:45+02:00".parse().unwrap());
        assert_eq!(unanswered.duration, None);
        assert_eq!(unanswered.wall_clock_span(), Some(Seconds::new(45)));
    }

    #[test]
    fn a_record_that_closed_before_it_opened_has_no_span() {
        let broken = call().with_closed("2026-07-28T09:59:00+02:00".parse().unwrap());
        assert_eq!(broken.wall_clock_span(), None);
        // An open record has no span either — for a different reason.
        assert_eq!(call().wall_clock_span(), None);
    }

    /// The span is computed on instants, so a session that crossed a DST
    /// transition measures correctly even though the wall clock jumped.
    #[test]
    fn a_span_across_a_dst_transition_measures_the_real_elapsed_time() {
        // 02:00+01:00 to 03:30+02:00 — the clock advanced 90 minutes on the
        // wall but only 30 minutes actually elapsed.
        let record = UsageRecord::new(
            RecordType::PduSession,
            party(),
            "2026-03-29T01:50:00+01:00".parse().unwrap(),
            source(),
        )
        .with_closed("2026-03-29T03:20:00+02:00".parse().unwrap());

        assert_eq!(record.wall_clock_span(), Some(Seconds::new(30 * 60)));
    }

    #[test]
    fn roaming_is_unknown_rather_than_false_when_a_network_is_missing() {
        let home: PlmnId = "262-02".parse().unwrap();
        let visited: PlmnId = "310-260".parse().unwrap();

        assert_eq!(call().is_roaming(), None);
        assert_eq!(call().with_networks(home, home).is_roaming(), Some(false));
        assert_eq!(call().with_networks(visited, home).is_roaming(), Some(true));
    }

    #[test]
    fn zero_usage_is_detected_across_all_three_quantities() {
        assert!(call().is_zero_usage());
        assert!(call().with_duration(Seconds::ZERO).is_zero_usage());
        assert!(!call().with_duration(Seconds::new(1)).is_zero_usage());
        assert!(
            !call()
                .with_volume(VolumePair::new(Octets::ZERO, Octets::new(1)))
                .is_zero_usage()
        );
        assert!(!call().with_events(1).is_zero_usage());
    }

    /// The service domain is derived, so a record cannot disagree with itself
    /// about which service produced it.
    #[test]
    fn the_service_domain_follows_from_the_record_type() {
        assert_eq!(call().service(), ServiceDomain::Voice);

        let session = UsageRecord::new(
            RecordType::PduSession,
            party(),
            "2026-07-28T10:00:00+02:00".parse().unwrap(),
            source(),
        );
        assert_eq!(session.service(), ServiceDomain::Data);
        assert_eq!(session.service(), session.record_type.service_domain());
    }

    /// Correlators default to absent, and attaching them does not disturb the
    /// quantities.
    #[test]
    fn correlators_are_optional_and_additive() {
        use crate::correlation::{ChargingId, CorrelationKeys};
        use std::net::Ipv4Addr;

        assert!(call().correlation.packet_session().is_none());

        let keyed = call().with_correlation(
            CorrelationKeys::default()
                .with_charging_id(ChargingId::new(7))
                .with_gateway(Ipv4Addr::new(10, 0, 0, 1).into()),
        );
        assert!(keyed.correlation.packet_session().is_some());
        assert_eq!(keyed.duration, call().duration);
    }

    #[test]
    fn every_record_type_maps_to_a_domain_and_every_code_round_trips() {
        assert_eq!(RecordType::ALL.len(), RecordType::CODES.len());
        for (r, code) in RecordType::ALL.iter().zip(RecordType::CODES) {
            assert_eq!(r.as_str(), *code);
            assert_eq!(code.parse::<RecordType>().unwrap(), *r);
            assert!(ServiceDomain::ALL.contains(&r.service_domain()));
        }

        assert_eq!(ServiceDomain::ALL.len(), ServiceDomain::CODES.len());
        for (d, code) in ServiceDomain::ALL.iter().zip(ServiceDomain::CODES) {
            assert_eq!(d.as_str(), *code);
            assert_eq!(code.parse::<ServiceDomain>().unwrap(), *d);
        }
    }

    /// The two domains that need not belong to a subscription are exactly the
    /// two that made the charged party a sum type.
    #[test]
    fn only_network_api_and_slice_are_subscriber_free() {
        for d in ServiceDomain::ALL {
            let expected = !matches!(d, ServiceDomain::NetworkApi | ServiceDomain::Slice);
            assert_eq!(d.is_subscriber_bound(), expected, "{d}");
        }
    }

    /// Completeness cannot be established without a sequence number, and the
    /// record says so rather than implying it is fine.
    #[test]
    fn provenance_reports_whether_gaps_are_detectable() {
        let bare = source();
        assert!(!bare.supports_completeness_check());
        assert_eq!(bare.to_string(), "pgw-fra-01:0");

        let full = source()
            .with_file_name("CDR-20260728-0001")
            .with_file_sequence(1);
        assert!(full.supports_completeness_check());
        assert_eq!(full.to_string(), "pgw-fra-01/CDR-20260728-0001#1:0");
    }

    #[test]
    fn node_identifiers_reject_whitespace_and_overlong_input() {
        assert!("pgw-fra-01".parse::<NodeId>().is_ok());
        for s in ["", "has space", &"x".repeat(65)] {
            assert!(s.parse::<NodeId>().is_err(), "{s:?} must not parse");
        }
    }

    #[test]
    fn sessions_and_events_are_distinguished() {
        assert!(RecordType::PduSession.is_session());
        assert!(RecordType::MobileOriginatedCall.is_session());
        assert!(!RecordType::SmsOriginated.is_session());
        assert!(!RecordType::NetworkApiInvocation.is_session());
    }

    #[test]
    fn the_primary_measure_matches_the_domain() {
        assert_eq!(
            ServiceDomain::Voice.primary_measure(),
            PrimaryMeasure::Duration
        );
        assert_eq!(
            ServiceDomain::Data.primary_measure(),
            PrimaryMeasure::Volume
        );
        assert_eq!(ServiceDomain::Sms.primary_measure(), PrimaryMeasure::Events);
        assert_eq!(
            ServiceDomain::NetworkApi.primary_measure(),
            PrimaryMeasure::Events
        );
    }
}
