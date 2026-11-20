//! Tying a record to the usage it describes — the identifiers, and the join.
//!
//! A single subscriber action produces records at every node it traverses, and
//! the same record may arrive more than once. Recognising a retransmission and
//! joining the legs of one usage are the *same* fields read two ways, so the
//! identifiers and the algorithm that consumes them live together: this module
//! holds [`CorrelationKeys`] and the identifiers it is built from, and
//! [`correlate`] which joins a batch on them.
//!
//! # A Charging ID is not unique on its own
//!
//! 3GPP TS 32.251 is explicit: the Charging ID **together with the P-GW IP
//! address** constitutes the unique identifier for an IP-CAN bearer. Two
//! gateways will happily allocate the same 32-bit value, and a mediation layer
//! that keys on the Charging ID alone therefore discards one of two genuinely
//! different sessions — a silent under-count that only surfaces as an
//! unexplained revenue gap.
//!
//! [`CorrelationKeys::packet_session`] returns the pair or nothing; there is no
//! accessor that hands back a bare Charging ID for use as a key.
//!
//! ```rust
//! use mediation::{ChargingId, CorrelationKeys};
//! use std::net::Ipv4Addr;
//!
//! let complete = CorrelationKeys::default()
//!     .with_charging_id(ChargingId::new(1_234))
//!     .with_gateway(Ipv4Addr::new(10, 0, 0, 1).into());
//! assert!(complete.packet_session().is_some());
//!
//! // A Charging ID with no gateway does not identify a session.
//! let partial = CorrelationKeys::default().with_charging_id(ChargingId::new(1_234));
//! assert!(partial.packet_session().is_none());
//! ```
//!
//! # The recording entity is not the node the file came from
//!
//! A `localSequenceNumber` is unique **within the recording entity that
//! assigned it** — the MSC, SGSN or CSCF that created the record — and a
//! Charging Gateway Function may collect records from many of them into one
//! file. Keying on the file's node instead therefore gives the *same* record
//! two different keys when it is collected through two gateways, and every
//! retransmission passes.
//!
//! So the recording entity is a field of its own, carried on the record rather
//! than inferred from where the file arrived.
//!
//! # An ICID identifies a session, a Linked ICID identifies its predecessor
//!
//! In IMS the first network element to handle a session-initiating `INVITE`
//! generates the ICID and every subsequent message of that session carries it
//! (3GPP TS 32.260). When an element starts a new call leg it records a
//! *linked* ICID pointing at the previous one, which is what lets a multi-leg
//! call be reassembled into a call history. The two are separate fields here
//! because conflating them collapses a forwarded call into a single leg.
//!
//! # Joining: two failures, opposite in sign
//!
//! The P-GW assigns a Charging ID when it creates an IP-CAN bearer and forwards
//! it to the S-GW, so both nodes' records carry it (TS 32.251) — and an IMS
//! session's ICID travels in the `P-Charging-Vector` of every SIP message that
//! belongs to it (TS 32.260). [`correlate`] gathers them.
//!
//! **Billing both sides double-charges.** Two records describing one bearer,
//! each carrying its own volume, are not two usages. Releasing both invoices
//! the session twice, and the subscriber notices.
//!
//! **Discarding an unmatched record hides leakage.** A record whose counterpart
//! never arrived is evidence that something was lost — quite possibly the side
//! that was going to be billed. Dropping it makes the loss invisible, which is
//! how revenue leakage survives an audit.
//!
//! So [`correlate`] returns the joined groups **and** the unmatched records,
//! and never silently drops either.
//!
//! ```rust
//! use mediation::{CorrelationConfig, correlate};
//! # use mediation::{ChargedParty, ChargingId, CorrelationKeys, Octets, RecordType, SourceRef,
//! #                 SubscriberRef, UsageRecord, VolumePair};
//! # use std::net::Ipv4Addr;
//! # fn record(node: &str, volume: Option<VolumePair>) -> UsageRecord {
//! #     let mut r = UsageRecord::new(
//! #         RecordType::PduSession,
//! #         ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
//! #         "2026-07-28T10:00:00+02:00".parse().unwrap(),
//! #         SourceRef::new(node.parse().unwrap(), 0),
//! #     ).with_correlation(
//! #         CorrelationKeys::default()
//! #             .with_charging_id(ChargingId::new(42))
//! #             .with_gateway(Ipv4Addr::new(10, 0, 0, 1).into()));
//! #     r.volume = volume;
//! #     r
//! # }
//! // The same bearer, seen by two nodes, each reporting a volume.
//! let records = vec![
//!     record("sgw-01", Some(VolumePair::new(Octets::new(100), Octets::new(900)))),
//!     record("pgw-01", Some(VolumePair::new(Octets::new(100), Octets::new(900)))),
//! ];
//!
//! let result = correlate(&records, &CorrelationConfig::packet_bearer());
//! assert_eq!(result.groups().len(), 1);
//! assert!(result.groups()[0].would_double_count(), "bill one side, not both");
//! assert!(result.unmatched().is_empty());
//! ```
//!
//! # Leg chains
//!
//! When a terminating application server diverts a call it generates a **new**
//! ICID for the outgoing leg and records the incoming one as a linked ICID
//! (TS 32.260). The legs therefore form a chain, and following it reconstructs
//! the call history that a forwarded call actually took.
//!
//! A leg whose linked ICID names a session that is not present is a
//! [`BrokenLeg`] — the earlier leg was lost, and with it the evidence of who
//! originated the call. It is reported separately from the unmatched records,
//! because the leg itself joined its own session perfectly well and counting it
//! in both places would break the accounting
//! [`CorrelationResult::accounts_for`] exists to assert.

use std::collections::HashMap;
use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;

use crate::error::ParseError;
use crate::record::{NodeId, UsageRecord};

// ── ChargingId ───────────────────────────────────────────────────────────────

/// The 3GPP charging identifier for an IP-CAN bearer or PDU session.
///
/// A 32-bit value assigned by the gateway. Unique **only** in combination with
/// that gateway's address — see the module documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChargingId(u32);

impl ChargingId {
    /// An identifier from its raw value.
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// The raw value.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for ChargingId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for ChargingId {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.trim()
            .parse::<u32>()
            .map(Self)
            .map_err(|_| ParseError::format("ChargingId", s, "a 32-bit unsigned decimal integer"))
    }
}

// ── Icid ─────────────────────────────────────────────────────────────────────

/// An IMS Charging Identifier, as carried in the `P-Charging-Vector` header.
///
/// Opaque: generated by the first IMS element to handle the session, with a
/// format the generating network chooses. Treated here as a token, validated
/// for shape only.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Icid(String);

/// The shape [`Icid`] accepts, as rendered in a [`ParseError`].
const ICID_FORMAT: &str = "1 to 255 printable ASCII characters without whitespace";

impl Icid {
    /// The identifier as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Icid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Icid {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim();
        let ok = !t.is_empty() && t.len() <= 255 && t.bytes().all(|b| b.is_ascii_graphic());
        if ok {
            Ok(Self(t.to_owned()))
        } else {
            Err(ParseError::format("Icid", s, ICID_FORMAT))
        }
    }
}

// ── CallId ───────────────────────────────────────────────────────────────────

/// A SIP Call-ID, as a voice platform's CDR carries it.
///
/// TS 32.260 puts the Call-ID of RFC 3261 §8.1.1.4 into the mandatory *Session ID*
/// field of every IMS CDR, and a fixed-line softswitch exports the same value. It
/// is "globally unique over space and time" in theory — but a back-to-back user
/// agent regenerates it per leg, so it identifies a call only **within the node
/// that recorded it**. [`RecordKey::Call`](crate::RecordKey::Call) therefore pairs
/// it with the recording entity, and there is no key on a Call-ID alone.
///
/// Accepts the RFC 3261 `callid` grammar — `word [ "@" word ]`, where `word` is
/// alphanumerics and `-.!%*_+`'~()<>:\"/[]?{}` — up to [`MAX_LEN`](Self::MAX_LEN)
/// characters. RFC 3261 sets no length limit; this bound is the crate's, so
/// hostile input costs a constant.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CallId(String);

/// The shape [`CallId`] accepts, as rendered in a [`ParseError`].
const CALL_ID_FORMAT: &str = "1 to 256 RFC 3261 word characters with at most one '@', such as a84b4c76e66710@pc33.example.com";

impl CallId {
    /// The longest Call-ID accepted, in bytes.
    pub const MAX_LEN: usize = 256;

    /// The identifier as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// RFC 3261 §25.1 `word` characters.
const fn is_word_char(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'-' | b'.'
                | b'!'
                | b'%'
                | b'*'
                | b'_'
                | b'+'
                | b'`'
                | b'\''
                | b'~'
                | b'('
                | b')'
                | b'<'
                | b'>'
                | b':'
                | b'\\'
                | b'"'
                | b'/'
                | b'['
                | b']'
                | b'?'
                | b'{'
                | b'}'
        )
}

impl fmt::Display for CallId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for CallId {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim();
        let mut parts = t.split('@');
        let ok = !t.is_empty()
            && t.len() <= Self::MAX_LEN
            && parts.clone().count() <= 2
            && parts.all(|w| !w.is_empty() && w.bytes().all(is_word_char));
        if ok {
            Ok(Self(t.to_owned()))
        } else {
            // Withheld: Call-IDs often embed the caller's number (`+4915…@host`).
            Err(ParseError::withheld("CallId", CALL_ID_FORMAT))
        }
    }
}

// ── PacketSessionId ──────────────────────────────────────────────────────────

/// The pair that actually identifies a packet session: Charging ID **and** the
/// gateway that allocated it.
///
/// Constructible only from both halves, so a caller cannot accidentally key on
/// the Charging ID alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PacketSessionId {
    charging_id: ChargingId,
    gateway: IpAddr,
}

impl PacketSessionId {
    /// The identifier from its two halves.
    #[must_use]
    pub const fn new(charging_id: ChargingId, gateway: IpAddr) -> Self {
        Self {
            charging_id,
            gateway,
        }
    }

    /// The gateway-assigned charging identifier.
    #[must_use]
    pub const fn charging_id(self) -> ChargingId {
        self.charging_id
    }

    /// The gateway that assigned it.
    #[must_use]
    pub const fn gateway(self) -> IpAddr {
        self.gateway
    }
}

impl fmt::Display for PacketSessionId {
    /// Writes `charging_id@gateway`, which is the whole identity and never half
    /// of it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.charging_id, self.gateway)
    }
}

/// The shape [`PacketSessionId`] accepts, as rendered in a [`ParseError`].
const PACKET_SESSION_FORMAT: &str =
    "a charging identifier and gateway address, such as 1234@10.0.0.1";

impl FromStr for PacketSessionId {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseError::format("PacketSessionId", s, PACKET_SESSION_FORMAT);
        // Split from the right: an IPv6 literal contains colons but no `@`.
        let (id, gateway) = s.trim().rsplit_once('@').ok_or_else(err)?;
        Ok(Self::new(
            id.parse().map_err(|_| err())?,
            gateway.parse().map_err(|_| err())?,
        ))
    }
}

// ── CorrelationKeys ──────────────────────────────────────────────────────────

/// Every correlator a record might carry.
///
/// All fields are optional because real records are: a circuit-switched CDR has
/// no Charging ID, an IMS record has no gateway address, and a partial record
/// may arrive before the identifiers that would join it to its siblings. What
/// matters is that the *combinations* which actually identify something are
/// exposed as accessors, and the halves that do not are not.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CorrelationKeys {
    /// The gateway-assigned charging identifier, where the record carried one.
    pub charging_id: Option<ChargingId>,
    /// The address of the gateway that assigned the charging identifier.
    pub gateway: Option<IpAddr>,
    /// The IMS session identifier.
    pub icid: Option<Icid>,
    /// The IMS identifier of the preceding call leg, for multi-leg histories.
    pub linked_icid: Option<Icid>,
    /// The record's position within a run of partial records for one session.
    ///
    /// Present only on partial and last-partial records (3GPP TS 32.298).
    pub record_sequence_number: Option<u32>,
    /// A serial number unique within the recording entity that produced it.
    ///
    /// The circuit-switched domain's identifier for a record, and the only one
    /// available for many voice CDRs. Meaningless without
    /// [`recording_entity`](Self::recording_entity), which scopes it.
    pub local_sequence_number: Option<u32>,
    /// The network element that created the record.
    ///
    /// **Not** the node whose file it arrived in: a gateway may collect records
    /// from many elements, so the two differ routinely and only this one scopes
    /// [`local_sequence_number`](Self::local_sequence_number).
    pub recording_entity: Option<NodeId>,
    /// The SIP Call-ID — the CDR's *Session ID* (TS 32.260) — scoped by
    /// [`recording_entity`](Self::recording_entity).
    pub call_id: Option<CallId>,
}

impl CorrelationKeys {
    /// No correlators at all.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            charging_id: None,
            gateway: None,
            icid: None,
            linked_icid: None,
            record_sequence_number: None,
            local_sequence_number: None,
            recording_entity: None,
            call_id: None,
        }
    }

    /// The packet session this record belongs to, when **both** halves are
    /// present.
    ///
    /// `None` when either is missing. A Charging ID without its gateway does
    /// not identify a session and must never be used as though it did.
    #[must_use]
    pub const fn packet_session(&self) -> Option<PacketSessionId> {
        match (self.charging_id, self.gateway) {
            (Some(id), Some(gateway)) => Some(PacketSessionId::new(id, gateway)),
            _ => None,
        }
    }

    /// `true` when this record names a predecessor leg.
    #[must_use]
    pub const fn is_linked_leg(&self) -> bool {
        self.linked_icid.is_some()
    }

    /// `true` when the record carried no correlator at all.
    ///
    /// Such a record can be neither joined to its siblings nor recognised as a
    /// retransmission, which is a finding rather than a neutral absence.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.charging_id.is_none()
            && self.gateway.is_none()
            && self.icid.is_none()
            && self.linked_icid.is_none()
            && self.record_sequence_number.is_none()
            && self.local_sequence_number.is_none()
            && self.recording_entity.is_none()
            && self.call_id.is_none()
    }

    /// The record's own serial number, scoped by the entity that assigned it.
    ///
    /// `None` unless **both** are present. A serial number without its entity
    /// is not an identity: two elements will happily emit the same number.
    #[must_use]
    pub const fn entity_serial(&self) -> Option<(&NodeId, u32)> {
        match (&self.recording_entity, self.local_sequence_number) {
            (Some(entity), Some(serial)) => Some((entity, serial)),
            _ => None,
        }
    }

    /// The record's Call-ID, scoped by the entity that recorded it.
    ///
    /// `None` unless **both** are present: a back-to-back user agent regenerates
    /// the Call-ID per leg, so two nodes can report the same one for different
    /// calls.
    #[must_use]
    pub const fn entity_call(&self) -> Option<(&NodeId, &CallId)> {
        match (&self.recording_entity, &self.call_id) {
            (Some(entity), Some(call_id)) => Some((entity, call_id)),
            _ => None,
        }
    }

    /// The same keys with a SIP Call-ID.
    #[must_use]
    pub fn with_call_id(mut self, call_id: CallId) -> Self {
        self.call_id = Some(call_id);
        self
    }

    /// The same keys with a charging identifier.
    #[must_use]
    pub fn with_charging_id(mut self, id: ChargingId) -> Self {
        self.charging_id = Some(id);
        self
    }

    /// The same keys with the assigning gateway's address.
    #[must_use]
    pub fn with_gateway(mut self, gateway: IpAddr) -> Self {
        self.gateway = Some(gateway);
        self
    }

    /// The same keys with an IMS session identifier.
    #[must_use]
    pub fn with_icid(mut self, icid: Icid) -> Self {
        self.icid = Some(icid);
        self
    }

    /// The same keys with the preceding leg's IMS session identifier.
    #[must_use]
    pub fn with_linked_icid(mut self, icid: Icid) -> Self {
        self.linked_icid = Some(icid);
        self
    }

    /// The same keys with a partial-record sequence number.
    #[must_use]
    pub const fn with_record_sequence(mut self, sequence: u32) -> Self {
        self.record_sequence_number = Some(sequence);
        self
    }

    /// The same keys with a recording-entity-local serial number.
    #[must_use]
    pub const fn with_local_sequence(mut self, sequence: u32) -> Self {
        self.local_sequence_number = Some(sequence);
        self
    }

    /// The same keys with the element that created the record.
    #[must_use]
    pub fn with_recording_entity(mut self, entity: NodeId) -> Self {
        self.recording_entity = Some(entity);
        self
    }
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::{CallId, ChargingId, CorrelationKeys, Icid, PacketSessionId};
    use serde::de::{self, Visitor};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::fmt;
    use std::net::IpAddr;

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

    string_serde!(Icid, "an IMS charging identifier");
    string_serde!(
        CallId,
        "a SIP Call-ID such as \"a84b4c76e66710@pc33.example.com\""
    );
    string_serde!(
        PacketSessionId,
        "a charging identifier and gateway, such as \"1234@10.0.0.1\""
    );

    impl Serialize for ChargingId {
        /// A number: the charging identifier is a 32-bit counter.
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_u32(self.get())
        }
    }

    impl<'de> Deserialize<'de> for ChargingId {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            u32::deserialize(deserializer).map(ChargingId::new)
        }
    }

    /// Field names are part of the public representation and covered by semver.
    #[derive(Serialize, Deserialize)]
    #[serde(rename = "CorrelationKeys")]
    struct CorrelationKeysRepr {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        charging_id: Option<ChargingId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gateway: Option<IpAddr>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        icid: Option<Icid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        linked_icid: Option<Icid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        record_sequence_number: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        local_sequence_number: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recording_entity: Option<crate::record::NodeId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call_id: Option<CallId>,
    }

    impl Serialize for CorrelationKeys {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            CorrelationKeysRepr {
                charging_id: self.charging_id,
                gateway: self.gateway,
                icid: self.icid.clone(),
                linked_icid: self.linked_icid.clone(),
                record_sequence_number: self.record_sequence_number,
                local_sequence_number: self.local_sequence_number,
                recording_entity: self.recording_entity.clone(),
                call_id: self.call_id.clone(),
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for CorrelationKeys {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = CorrelationKeysRepr::deserialize(deserializer)?;
            Ok(CorrelationKeys {
                charging_id: r.charging_id,
                gateway: r.gateway,
                icid: r.icid,
                linked_icid: r.linked_icid,
                record_sequence_number: r.record_sequence_number,
                local_sequence_number: r.local_sequence_number,
                recording_entity: r.recording_entity,
                call_id: r.call_id,
            })
        }
    }
}

// ── CorrelationKey ───────────────────────────────────────────────────────────

/// What a group of records was joined on.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CorrelationKey {
    /// One IP-CAN bearer or PDU session, seen by every node that served it.
    PacketBearer(PacketSessionId),
    /// One IMS session, seen by every element that handled it.
    ImsSession(Icid),
}

impl CorrelationKey {
    /// A short code naming the join, for grouping and reporting.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::PacketBearer(_) => "PACKET_BEARER",
            Self::ImsSession(_) => "IMS_SESSION",
        }
    }
}

impl fmt::Display for CorrelationKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PacketBearer(session) => write!(f, "PACKET_BEARER:{session}"),
            Self::ImsSession(icid) => write!(f, "IMS_SESSION:{icid}"),
        }
    }
}

// ── Configuration ────────────────────────────────────────────────────────────

/// Which correlator to join on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorrelationConfig {
    join_packet_bearers: bool,
    join_ims_sessions: bool,
    follow_leg_chains: bool,
}

impl CorrelationConfig {
    /// Join packet-data records on Charging ID **and** gateway.
    #[must_use]
    pub const fn packet_bearer() -> Self {
        Self {
            join_packet_bearers: true,
            join_ims_sessions: false,
            follow_leg_chains: false,
        }
    }

    /// Join IMS records on their session identifier, and follow leg chains.
    #[must_use]
    pub const fn ims() -> Self {
        Self {
            join_packet_bearers: false,
            join_ims_sessions: true,
            follow_leg_chains: true,
        }
    }

    /// Join on every correlator this crate understands.
    #[must_use]
    pub const fn all() -> Self {
        Self {
            join_packet_bearers: true,
            join_ims_sessions: true,
            follow_leg_chains: true,
        }
    }

    /// The same configuration without leg-chain reconstruction.
    #[must_use]
    pub const fn without_leg_chains(mut self) -> Self {
        self.follow_leg_chains = false;
        self
    }
}

// ── Group ────────────────────────────────────────────────────────────────────

/// The records that describe one usage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorrelatedGroup {
    /// What the members were joined on.
    pub key: CorrelationKey,
    /// Indices into the **caller's** slice, ascending.
    pub members: Vec<usize>,
    /// How many members carry a quantity that could be billed.
    ///
    /// The number that matters: two nodes both reporting the volume of one
    /// bearer is normal, and billing both is not.
    pub quantity_bearing: usize,
}

impl CorrelatedGroup {
    /// `true` when more than one member carries a billable quantity.
    ///
    /// Not itself a fault — an S-GW and a P-GW reporting the same bearer is the
    /// expected shape. It is a fault only if a caller releases both, which is
    /// exactly what this flag exists to prevent.
    #[must_use]
    pub const fn would_double_count(&self) -> bool {
        self.quantity_bearing > 1
    }

    /// `true` when only one record described this usage.
    ///
    /// Whether that is normal or a missing counterpart depends on the
    /// deployment, which is why it is reported rather than judged: many
    /// networks emit a P-GW record and nothing else.
    #[must_use]
    pub fn is_solitary(&self) -> bool {
        self.members.len() == 1
    }

    /// How many records were joined.
    #[must_use]
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Always `false` — a group is created only when it has a member.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }
}

impl fmt::Display for CorrelatedGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} — {} record(s), {} bearing a quantity",
            self.key,
            self.members.len(),
            self.quantity_bearing
        )
    }
}

// ── Unmatched ────────────────────────────────────────────────────────────────

/// Why a record could not be joined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UnmatchedReason {
    /// The record carries no correlator at all.
    ///
    /// Not necessarily a fault: a short message has no session to belong to.
    /// It is a fault when the record type *should* have one, which is a
    /// question about the feed rather than about the join.
    NoCorrelator,
    /// The record carries a correlator the configuration does not join on.
    ///
    /// Distinguished from [`NoCorrelator`](Self::NoCorrelator) because the
    /// remedy is different and is entirely local: an IMS record handed to
    /// [`CorrelationConfig::packet_bearer`] is not evidence of anything missing
    /// from the feed, it is evidence of a configuration that was not asked to
    /// join it.
    CorrelatorNotJoined,
}

impl UnmatchedReason {
    /// Every variant, for exhaustive iteration in tests and mappings.
    pub const ALL: &'static [Self] = &[Self::NoCorrelator, Self::CorrelatorNotJoined];

    /// The canonical string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoCorrelator => "NO_CORRELATOR",
            Self::CorrelatorNotJoined => "CORRELATOR_NOT_JOINED",
        }
    }
}

impl fmt::Display for UnmatchedReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A record that no group claimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unmatched {
    /// Index into the **caller's** slice.
    pub record_index: usize,
    /// Why it could not be joined.
    pub reason: UnmatchedReason,
}

/// A leg naming a predecessor that is not present.
///
/// **Not an unmatched record.** The leg itself joined its own session
/// perfectly well; what is missing is the leg *before* it, and with it the
/// evidence of who originated the call. Reporting it as unmatched would count
/// the record twice — once in its group and once outside — and break the very
/// accounting [`CorrelationResult::accounts_for`] exists to assert.
///
/// It carries the identifier that is missing, because the operational response
/// is to go and look for that session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrokenLeg {
    /// Index into the **caller's** slice — the leg that named the predecessor.
    pub record_index: usize,
    /// The session it named, which no record in the batch carries.
    pub missing: Icid,
}

// ── Leg chains ───────────────────────────────────────────────────────────────

/// A call's legs, in the order they were created.
///
/// Reconstructed by following linked ICIDs backwards to the leg that names no
/// predecessor, then forwards. A forwarded call that a caller bills leg by leg
/// without this looks like several unrelated calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegChain {
    /// The session identifiers, origin first.
    pub legs: Vec<Icid>,
}

impl LegChain {
    /// How many legs the call took.
    #[must_use]
    pub fn len(&self) -> usize {
        self.legs.len()
    }

    /// Always `false` — a chain is created only when it has a leg.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }

    /// `true` when the call was diverted at least once.
    #[must_use]
    pub fn is_forwarded(&self) -> bool {
        self.legs.len() > 1
    }

    /// The leg the call originated on.
    #[must_use]
    pub fn origin(&self) -> &Icid {
        // A chain is never constructed empty.
        &self.legs[0]
    }
}

impl fmt::Display for LegChain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (position, leg) in self.legs.iter().enumerate() {
            if position > 0 {
                f.write_str(" → ")?;
            }
            write!(f, "{leg}")?;
        }
        Ok(())
    }
}

// ── Result ───────────────────────────────────────────────────────────────────

/// Everything the join produced.
///
/// Groups and unmatched records together account for **every** input record
/// exactly once — asserted by [`accounts_for`](Self::accounts_for). Dropping an
/// unmatched record would make lost usage invisible, so the type makes that
/// impossible to do accidentally.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CorrelationResult {
    groups: Vec<CorrelatedGroup>,
    unmatched: Vec<Unmatched>,
    chains: Vec<LegChain>,
    broken_legs: Vec<BrokenLeg>,
}

impl CorrelationResult {
    /// The joined groups, ordered by their first member.
    #[must_use]
    pub fn groups(&self) -> &[CorrelatedGroup] {
        &self.groups
    }

    /// The records no group claimed, ascending by index.
    #[must_use]
    pub fn unmatched(&self) -> &[Unmatched] {
        &self.unmatched
    }

    /// The call histories reconstructed from linked session identifiers.
    #[must_use]
    pub fn chains(&self) -> &[LegChain] {
        &self.chains
    }

    /// Legs whose named predecessor is not in the batch, ascending by index.
    ///
    /// Each is a chain that cannot be reconstructed, and with it an origin that
    /// cannot be established. The records themselves are in their groups — see
    /// [`BrokenLeg`].
    #[must_use]
    pub fn broken_legs(&self) -> &[BrokenLeg] {
        &self.broken_legs
    }

    /// Groups where more than one member carries a billable quantity.
    ///
    /// Release one member of each, not all — see
    /// [`CorrelatedGroup::would_double_count`].
    #[must_use]
    pub fn double_counting_groups(&self) -> Vec<&CorrelatedGroup> {
        self.groups
            .iter()
            .filter(|g| g.would_double_count())
            .collect()
    }

    /// `true` when every one of `total` input records was accounted for exactly
    /// once.
    ///
    /// The correlation counterpart of the conservation law: a join that loses a
    /// record loses the usage it described.
    #[must_use]
    pub fn accounts_for(&self, total: usize) -> bool {
        let grouped: usize = self.groups.iter().map(CorrelatedGroup::len).sum();
        grouped + self.unmatched.len() == total
    }
}

impl fmt::Display for CorrelationResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} group(s), {} unmatched, {} chain(s), {} broken leg(s), {} at risk of double counting",
            self.groups.len(),
            self.unmatched.len(),
            self.chains.len(),
            self.broken_legs.len(),
            self.double_counting_groups().len()
        )
    }
}

// ── The engine ───────────────────────────────────────────────────────────────

/// Join the records that describe one usage.
///
/// Deterministic and order-independent in outcome: groups are returned ordered
/// by their first member's index and their members ascending, so a shuffled
/// input produces the same groups with the same contents.
///
/// Indices refer to `records` as passed.
#[must_use]
pub fn correlate(records: &[UsageRecord], config: &CorrelationConfig) -> CorrelationResult {
    let mut buckets: HashMap<CorrelationKey, Vec<usize>> = HashMap::new();
    let mut unmatched = Vec::new();

    for (index, record) in records.iter().enumerate() {
        let key = if config.join_packet_bearers
            && let Some(session) = record.correlation.packet_session()
        {
            Some(CorrelationKey::PacketBearer(session))
        } else if config.join_ims_sessions
            && let Some(icid) = &record.correlation.icid
        {
            Some(CorrelationKey::ImsSession(icid.clone()))
        } else {
            None
        };

        match key {
            Some(key) => buckets.entry(key).or_default().push(index),
            // A record the configuration *could* have joined but was not asked
            // to is a different problem from one that carries nothing.
            None => {
                let carries_one = record.correlation.packet_session().is_some()
                    || record.correlation.icid.is_some();
                unmatched.push(Unmatched {
                    record_index: index,
                    reason: if carries_one {
                        UnmatchedReason::CorrelatorNotJoined
                    } else {
                        UnmatchedReason::NoCorrelator
                    },
                });
            }
        }
    }

    let mut groups: Vec<CorrelatedGroup> = buckets
        .into_iter()
        .map(|(key, mut members)| {
            members.sort_unstable();
            let quantity_bearing = members
                .iter()
                .filter(|index| !records[**index].is_zero_usage())
                .count();
            CorrelatedGroup {
                key,
                members,
                quantity_bearing,
            }
        })
        .collect();
    // Deterministic order: by first member, which is unique across groups.
    groups.sort_by_key(|group| group.members[0]);

    let mut broken_legs = Vec::new();
    let chains = if config.follow_leg_chains {
        build_leg_chains(records, &mut broken_legs)
    } else {
        Vec::new()
    };
    unmatched.sort_by_key(|u| u.record_index);
    broken_legs.sort_by_key(|b| b.record_index);

    CorrelationResult {
        groups,
        unmatched,
        chains,
        broken_legs,
    }
}

/// Reconstruct call histories from linked session identifiers.
///
/// A leg naming a predecessor that is not present is recorded as a
/// [`BrokenLeg`] rather than treated as an origin, because the difference is
/// exactly the evidence of who placed the call. It is **not** recorded as
/// unmatched: the record joined its own session, and counting it outside its
/// group as well would break [`CorrelationResult::accounts_for`].
fn build_leg_chains(records: &[UsageRecord], broken: &mut Vec<BrokenLeg>) -> Vec<LegChain> {
    // Every session present, and what each names as its predecessor.
    let mut present: HashMap<&Icid, usize> = HashMap::new();
    let mut successor: HashMap<&Icid, &Icid> = HashMap::new();
    let mut linked_from: Vec<(usize, &Icid, &Icid)> = Vec::new();

    for (index, record) in records.iter().enumerate() {
        if let Some(icid) = &record.correlation.icid {
            present.entry(icid).or_insert(index);
            if let Some(previous) = &record.correlation.linked_icid {
                linked_from.push((index, icid, previous));
            }
        }
    }
    for (index, icid, previous) in &linked_from {
        if present.contains_key(previous) {
            successor.insert(previous, icid);
        } else {
            broken.push(BrokenLeg {
                record_index: *index,
                missing: (*previous).clone(),
            });
        }
    }

    // An origin is a session nothing resolvable points forward from.
    let has_predecessor: std::collections::HashSet<&Icid> = linked_from
        .iter()
        .filter(|(_, _, previous)| present.contains_key(previous))
        .map(|(_, icid, _)| *icid)
        .collect();

    let mut origins: Vec<(&Icid, usize)> = present
        .iter()
        .filter(|(icid, _)| !has_predecessor.contains(*icid))
        .map(|(icid, index)| (*icid, *index))
        .collect();
    origins.sort_by_key(|(_, index)| *index);

    origins
        .into_iter()
        .map(|(origin, _)| {
            let mut legs = vec![origin.clone()];
            let mut cursor = origin;
            // Bounded by the number of distinct sessions, so a cycle in
            // malformed input cannot loop forever.
            while legs.len() <= present.len()
                && let Some(next) = successor.get(cursor)
            {
                legs.push((*next).clone());
                cursor = next;
            }
            LegChain { legs }
        })
        .collect()
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod correlate_serde {
    use super::{BrokenLeg, CorrelationKey, LegChain, Unmatched, UnmatchedReason};
    use serde::de::{self, Visitor};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::fmt;

    /// Field names are part of the public representation and covered by
    /// semver. The missing identifier is carried, because the operational
    /// response is to go and look for that session.
    #[derive(Serialize, Deserialize)]
    #[serde(rename = "BrokenLeg")]
    struct BrokenLegRepr {
        record_index: usize,
        missing: crate::correlation::Icid,
    }

    impl Serialize for BrokenLeg {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            BrokenLegRepr {
                record_index: self.record_index,
                missing: self.missing.clone(),
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for BrokenLeg {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = BrokenLegRepr::deserialize(deserializer)?;
            Ok(BrokenLeg {
                record_index: r.record_index,
                missing: r.missing,
            })
        }
    }

    impl Serialize for UnmatchedReason {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_str(self.as_str())
        }
    }

    impl<'de> Deserialize<'de> for UnmatchedReason {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct V;
            impl Visitor<'_> for V {
                type Value = UnmatchedReason;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("an unmatched reason code")
                }
                fn visit_str<E: de::Error>(self, v: &str) -> Result<UnmatchedReason, E> {
                    UnmatchedReason::ALL
                        .iter()
                        .find(|r| r.as_str() == v)
                        .copied()
                        .ok_or_else(|| de::Error::custom(format!("unknown reason {v:?}")))
                }
            }
            deserializer.deserialize_str(V)
        }
    }

    /// An internally tagged representation, as elsewhere in the crate.
    #[derive(Serialize, Deserialize)]
    #[serde(tag = "kind", rename = "CorrelationKey")]
    enum KeyRepr {
        #[serde(rename = "PACKET_BEARER")]
        PacketBearer {
            session: crate::correlation::PacketSessionId,
        },
        #[serde(rename = "IMS_SESSION")]
        ImsSession { icid: crate::correlation::Icid },
    }

    impl Serialize for CorrelationKey {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            match self {
                Self::PacketBearer(session) => KeyRepr::PacketBearer { session: *session },
                Self::ImsSession(icid) => KeyRepr::ImsSession { icid: icid.clone() },
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for CorrelationKey {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            Ok(match KeyRepr::deserialize(deserializer)? {
                KeyRepr::PacketBearer { session } => Self::PacketBearer(session),
                KeyRepr::ImsSession { icid } => Self::ImsSession(icid),
            })
        }
    }

    /// Field names are part of the public representation and covered by semver.
    #[derive(Serialize, Deserialize)]
    #[serde(rename = "Unmatched")]
    struct UnmatchedRepr {
        record_index: usize,
        reason: UnmatchedReason,
    }

    impl Serialize for Unmatched {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            UnmatchedRepr {
                record_index: self.record_index,
                reason: self.reason,
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for Unmatched {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = UnmatchedRepr::deserialize(deserializer)?;
            Ok(Unmatched {
                record_index: r.record_index,
                reason: r.reason,
            })
        }
    }

    impl Serialize for LegChain {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            self.legs.serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for LegChain {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let legs = Vec::<crate::correlation::Icid>::deserialize(deserializer)?;
            if legs.is_empty() {
                return Err(de::Error::custom("a leg chain has at least one leg"));
            }
            Ok(LegChain { legs })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    /// The bug this API shape exists to prevent: keying a packet session on the
    /// Charging ID alone merges sessions from different gateways.
    #[test]
    fn a_charging_id_alone_does_not_identify_a_session() {
        let bare = CorrelationKeys::default().with_charging_id(ChargingId::new(1_234));
        assert!(bare.packet_session().is_none());

        let complete = bare.clone().with_gateway(Ipv4Addr::new(10, 0, 0, 1).into());
        assert!(complete.packet_session().is_some());

        // Two gateways, the same counter value, two different sessions.
        let other = bare.with_gateway(Ipv4Addr::new(10, 0, 0, 2).into());
        assert_ne!(complete.packet_session(), other.packet_session());
    }

    #[test]
    fn a_gateway_without_a_charging_id_is_equally_useless() {
        let keys = CorrelationKeys::default().with_gateway(Ipv4Addr::LOCALHOST.into());
        assert!(keys.packet_session().is_none());
    }

    #[test]
    fn packet_session_identifiers_round_trip_over_both_address_families() {
        for raw in [
            "1234@10.0.0.1",
            "0@127.0.0.1",
            "4294967295@2001:db8::1",
            "7@::1",
        ] {
            let id: PacketSessionId = raw.parse().unwrap();
            assert_eq!(id.to_string(), raw);
        }
    }

    /// An IPv6 literal is full of colons, so the split must come from the right
    /// and must be on `@`.
    #[test]
    fn an_ipv6_gateway_does_not_confuse_the_parser() {
        let id: PacketSessionId = "99@2001:db8::dead:beef".parse().unwrap();
        assert_eq!(id.charging_id(), ChargingId::new(99));
        assert_eq!(
            id.gateway(),
            IpAddr::V6("2001:db8::dead:beef".parse::<Ipv6Addr>().unwrap())
        );
    }

    #[test]
    fn malformed_session_identifiers_are_rejected() {
        for s in [
            "1234",
            "@10.0.0.1",
            "1234@",
            "abc@10.0.0.1",
            "1234@not-an-ip",
            "",
        ] {
            assert!(
                s.parse::<PacketSessionId>().is_err(),
                "{s:?} must not parse"
            );
        }
    }

    /// A linked ICID is the predecessor leg, not the session — conflating them
    /// collapses a forwarded call into one leg.
    #[test]
    fn a_linked_icid_is_a_separate_field() {
        let keys = CorrelationKeys::default()
            .with_icid("leg-2".parse().unwrap())
            .with_linked_icid("leg-1".parse().unwrap());

        assert!(keys.is_linked_leg());
        assert_ne!(keys.icid, keys.linked_icid);
        assert_eq!(keys.icid.unwrap().as_str(), "leg-2");
    }

    /// The bug this field exists to prevent: a serial number scoped to the
    /// wrong thing gives one record two keys.
    #[test]
    fn a_serial_number_is_nothing_without_its_recording_entity() {
        let serial_only = CorrelationKeys::default().with_local_sequence(42);
        assert_eq!(serial_only.entity_serial(), None);

        let entity_only =
            CorrelationKeys::default().with_recording_entity("msc-01".parse().unwrap());
        assert_eq!(entity_only.entity_serial(), None);

        let complete = serial_only.with_recording_entity("msc-01".parse().unwrap());
        let (entity, serial) = complete.entity_serial().unwrap();
        assert_eq!(entity.as_str(), "msc-01");
        assert_eq!(serial, 42);
    }

    #[test]
    fn icids_reject_whitespace_and_overlong_input() {
        assert!("abc123".parse::<Icid>().is_ok());
        for s in ["", "has space", &"x".repeat(256)] {
            assert!(s.parse::<Icid>().is_err(), "{s:?} must not parse");
        }
    }

    #[test]
    fn charging_identifiers_round_trip() {
        for raw in ["0", "1234", "4294967295"] {
            assert_eq!(raw.parse::<ChargingId>().unwrap().to_string(), raw);
        }
        assert!("4294967296".parse::<ChargingId>().is_err());
        assert!("-1".parse::<ChargingId>().is_err());
    }

    // ── The join ─────────────────────────────────────────────────────────────
    use crate::party::{ChargedParty, SubscriberRef};
    use crate::record::{RecordType, SourceRef};
    use crate::volume::{Octets, VolumePair};

    fn record(node: &str, keys: CorrelationKeys, volume: Option<VolumePair>) -> UsageRecord {
        let mut record = UsageRecord::new(
            RecordType::PduSession,
            ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
            "2026-07-28T10:00:00+02:00".parse().unwrap(),
            SourceRef::new(node.parse().unwrap(), 0),
        )
        .with_correlation(keys);
        record.volume = volume;
        record
    }

    fn bearer(charging_id: u32) -> CorrelationKeys {
        CorrelationKeys::default()
            .with_charging_id(ChargingId::new(charging_id))
            .with_gateway(Ipv4Addr::new(10, 0, 0, 1).into())
    }

    fn some_volume() -> Option<VolumePair> {
        Some(VolumePair::new(Octets::new(100), Octets::new(900)))
    }

    /// The failure that costs a subscriber money: two nodes describing one
    /// bearer, both carrying its volume.
    #[test]
    fn two_nodes_on_one_bearer_are_flagged_before_they_are_billed_twice() {
        let records = vec![
            record("sgw-01", bearer(42), some_volume()),
            record("pgw-01", bearer(42), some_volume()),
        ];
        let result = correlate(&records, &CorrelationConfig::packet_bearer());

        assert_eq!(result.groups().len(), 1);
        let group = &result.groups()[0];
        assert_eq!(group.members, vec![0, 1]);
        assert_eq!(group.quantity_bearing, 2);
        assert!(group.would_double_count());
        assert_eq!(result.double_counting_groups().len(), 1);
    }

    /// A node reporting the bearer without a quantity is not double counting —
    /// only one side can be billed, and only one side carries anything.
    #[test]
    fn a_quantityless_counterpart_is_not_double_counting() {
        let records = vec![
            record("sgw-01", bearer(42), None),
            record("pgw-01", bearer(42), some_volume()),
        ];
        let result = correlate(&records, &CorrelationConfig::packet_bearer());
        let group = &result.groups()[0];

        assert_eq!(group.len(), 2);
        assert_eq!(group.quantity_bearing, 1);
        assert!(!group.would_double_count());
    }

    /// Different gateways using the same counter value are different bearers,
    /// per TS 32.251 — joining them would merge two subscribers' sessions.
    #[test]
    fn the_gateway_is_part_of_the_join() {
        let other_gateway = CorrelationKeys::default()
            .with_charging_id(ChargingId::new(42))
            .with_gateway(Ipv4Addr::new(10, 0, 0, 2).into());
        let records = vec![
            record("pgw-01", bearer(42), some_volume()),
            record("pgw-02", other_gateway, some_volume()),
        ];
        let result = correlate(&records, &CorrelationConfig::packet_bearer());

        assert_eq!(result.groups().len(), 2);
        assert!(result.groups().iter().all(CorrelatedGroup::is_solitary));
    }

    /// Nothing is dropped: every input record is in a group or in the unmatched
    /// list, and the result says so.
    #[test]
    fn every_record_is_accounted_for() {
        let records = vec![
            record("pgw-01", bearer(1), some_volume()),
            record("sgw-01", bearer(1), some_volume()),
            record("smsc-01", CorrelationKeys::none(), some_volume()),
            record("pgw-01", bearer(2), some_volume()),
        ];
        let result = correlate(&records, &CorrelationConfig::packet_bearer());

        assert!(result.accounts_for(records.len()));
        assert_eq!(result.unmatched().len(), 1);
        assert_eq!(result.unmatched()[0].record_index, 2);
        assert_eq!(result.unmatched()[0].reason, UnmatchedReason::NoCorrelator);
    }

    /// A shuffled input produces the same groups with the same contents.
    #[test]
    fn the_join_is_order_independent() {
        let make = |order: [usize; 4]| {
            let all = [
                record("pgw-01", bearer(1), some_volume()),
                record("sgw-01", bearer(1), some_volume()),
                record("pgw-01", bearer(2), some_volume()),
                record("sgw-01", bearer(2), some_volume()),
            ];
            order.map(|i| all[i].clone()).to_vec()
        };

        let forward = correlate(&make([0, 1, 2, 3]), &CorrelationConfig::packet_bearer());
        let shuffled = correlate(&make([3, 1, 0, 2]), &CorrelationConfig::packet_bearer());

        // The same two bearers, each with two members, whatever the order.
        assert_eq!(forward.groups().len(), shuffled.groups().len());
        for result in [&forward, &shuffled] {
            assert!(result.groups().iter().all(|g| g.len() == 2));
            assert!(result.accounts_for(4));
        }
    }

    // ── Leg chains ───────────────────────────────────────────────────────────

    fn leg(icid: &str, linked: Option<&str>) -> UsageRecord {
        let mut keys = CorrelationKeys::default().with_icid(icid.parse().unwrap());
        if let Some(previous) = linked {
            keys = keys.with_linked_icid(previous.parse().unwrap());
        }
        record("scscf-01", keys, some_volume())
    }

    /// A diverted call: each new leg names the one it came from, so following
    /// the links reconstructs the history.
    #[test]
    fn a_forwarded_call_reassembles_into_one_history() {
        let records = vec![
            leg("leg-a", None),
            leg("leg-b", Some("leg-a")),
            leg("leg-c", Some("leg-b")),
        ];
        let result = correlate(&records, &CorrelationConfig::ims());

        assert_eq!(result.chains().len(), 1);
        let chain = &result.chains()[0];
        assert_eq!(chain.len(), 3);
        assert!(chain.is_forwarded());
        assert_eq!(chain.origin().as_str(), "leg-a");
        assert_eq!(chain.to_string(), "leg-a → leg-b → leg-c");
    }

    /// A leg naming a predecessor that never arrived is reported, not assumed
    /// to be the origin: the difference is the evidence of who placed the call.
    ///
    /// It is reported as a **broken leg**, not as an unmatched record. The leg
    /// joined its own session perfectly well, and counting it outside its group
    /// as well would break the accounting `accounts_for` exists to assert.
    #[test]
    fn a_lost_predecessor_is_reported_rather_than_assumed_to_be_the_origin() {
        let records = vec![leg("leg-b", Some("leg-a"))];
        let result = correlate(&records, &CorrelationConfig::ims());

        assert_eq!(result.broken_legs().len(), 1);
        assert_eq!(result.broken_legs()[0].record_index, 0);
        assert_eq!(result.broken_legs()[0].missing.as_str(), "leg-a");

        assert!(result.unmatched().is_empty());
        assert!(
            result.accounts_for(records.len()),
            "every record exactly once: {result}"
        );

        // The leg still forms a chain of its own — the usage is not discarded.
        assert_eq!(result.chains().len(), 1);
        assert!(!result.chains()[0].is_forwarded());
    }

    /// The accounting law, over every shape of input the join can meet.
    #[test]
    fn every_record_is_accounted_for_exactly_once() {
        let batches: Vec<Vec<UsageRecord>> = vec![
            vec![],
            vec![leg("leg-a", None)],
            vec![leg("leg-b", Some("leg-a"))],
            vec![leg("leg-a", None), leg("leg-b", Some("leg-a"))],
            vec![leg("leg-a", Some("leg-b")), leg("leg-b", Some("leg-a"))],
            vec![
                record("sgw-01", bearer(42), some_volume()),
                record("pgw-01", bearer(42), some_volume()),
                leg("leg-a", None),
            ],
            vec![record("msc-01", CorrelationKeys::default(), None)],
        ];
        for records in batches {
            for config in [
                CorrelationConfig::all(),
                CorrelationConfig::ims(),
                CorrelationConfig::packet_bearer(),
            ] {
                let result = correlate(&records, &config);
                assert!(
                    result.accounts_for(records.len()),
                    "{} record(s): {result}",
                    records.len()
                );
            }
        }
    }

    /// A record the configuration was not asked to join is not evidence of
    /// anything missing from the feed.
    #[test]
    fn a_correlator_the_configuration_ignores_says_so() {
        let records = vec![leg("leg-a", None)];
        let result = correlate(&records, &CorrelationConfig::packet_bearer());

        assert_eq!(result.unmatched().len(), 1);
        assert_eq!(
            result.unmatched()[0].reason,
            UnmatchedReason::CorrelatorNotJoined
        );
    }

    /// A single un-diverted call is a chain of one, not a forwarded call.
    #[test]
    fn a_direct_call_is_a_chain_of_one() {
        let result = correlate(&[leg("leg-a", None)], &CorrelationConfig::ims());
        assert_eq!(result.chains().len(), 1);
        assert!(!result.chains()[0].is_forwarded());
        assert!(result.unmatched().is_empty());
    }

    /// Malformed input must not hang the join.
    #[test]
    fn a_cycle_in_linked_identifiers_terminates() {
        let records = vec![leg("leg-a", Some("leg-b")), leg("leg-b", Some("leg-a"))];
        let result = correlate(&records, &CorrelationConfig::ims());

        // Every leg has a predecessor, so no origin exists and no chain forms —
        // but the call returns.
        assert!(result.chains().iter().all(|c| c.len() <= 2));
        assert!(result.accounts_for(records.len()));
    }

    #[test]
    fn leg_chains_can_be_switched_off() {
        let records = vec![leg("leg-a", None), leg("leg-b", Some("leg-a"))];
        let result = correlate(&records, &CorrelationConfig::ims().without_leg_chains());
        assert!(result.chains().is_empty());
        // The sessions still join individually.
        assert_eq!(result.groups().len(), 2);
    }

    #[test]
    fn packet_and_ims_records_can_be_joined_together() {
        let records = vec![
            record("pgw-01", bearer(1), some_volume()),
            leg("leg-a", None),
        ];
        let result = correlate(&records, &CorrelationConfig::all());

        assert_eq!(result.groups().len(), 2);
        assert_eq!(result.groups()[0].key.kind(), "PACKET_BEARER");
        assert_eq!(result.groups()[1].key.kind(), "IMS_SESSION");
        assert!(result.accounts_for(2));
    }

    #[test]
    fn an_empty_input_produces_an_empty_result() {
        let result = correlate(&[], &CorrelationConfig::all());
        assert!(result.groups().is_empty());
        assert!(result.unmatched().is_empty());
        assert!(result.accounts_for(0));
        assert_eq!(result.double_counting_groups().len(), 0);
    }
}
