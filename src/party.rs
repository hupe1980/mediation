//! Who a usage record is attributed to.
//!
//! # Why not just a subscriber
//!
//! A record model with a mandatory subscriber field cannot express two kinds of
//! usage that are already in production:
//!
//! - **Network APIs.** Under GSMA Open Gateway, an operator sells capability —
//!   number verification, device location, quality on demand — and 3GPP
//!   specifies the charging in TS 32.254. The chargeable unit is an API
//!   invocation with **two** counterparties: the subscriber whose number was
//!   verified, and the developer who paid to verify it. Settlement needs both.
//! - **Network slice management.** TS 28.202 charges slice instance creation,
//!   modification and termination, and TS 28.201 charges slice performance and
//!   analytics. **There is no subscriber at all** — the charged party is an
//!   enterprise tenant and the quantity is a slice lifetime.
//!
//! So the charged party is a sum type. [`ChargedParty::Unattributable`] is one
//! of its variants rather than an `Option`, because a record whose party could
//! not be resolved must carry **why**, and must reconcile as a counted outcome
//! instead of vanishing into a null.
//!
//! # Tenancy is not an afterthought
//!
//! A host operator's mediation layer serves its own brand plus every MVNO
//! riding on it, interleaved in the same files from the same nodes. Three
//! things depend on knowing which is which, and all three are wrong by default
//! when it is missing: reconciliation must balance **per tenant** — a batch that
//! balances overall can still have lost every record of one MVNO — retention
//! policy differs per tenant, and fair-use assessment is per retail brand.
//! Hence [`TenantRef`] on [`SubscriberRef`], resolved from a caller-supplied
//! identity range map.

use std::fmt;
use std::str::FromStr;

use crate::error::ParseError;
use crate::identity::{Gpsi, Supi};

/// A short, opaque identifier for a party outside the subscriber base.
///
/// Used for tenants, API consumers and network functions — anything this crate
/// must be able to attribute usage to without modelling its internal structure.
/// Printable ASCII without whitespace, 1–64 characters, so it survives a file
/// name, a log line and a database key unchanged.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PartyId(String);

/// The shape [`PartyId`] accepts, as rendered in a [`ParseError`].
const PARTY_ID_FORMAT: &str =
    "1 to 64 printable ASCII characters without whitespace, such as mvno-alpha";

impl PartyId {
    /// The identifier as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PartyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for PartyId {
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
            Err(ParseError::format("PartyId", s, PARTY_ID_FORMAT))
        }
    }
}

/// The retail brand a subscription belongs to — the host network's own, or an
/// MVNO's.
///
/// Attribution is by identity range, which the caller resolves; this type
/// carries the answer rather than deriving it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TenantRef(PartyId);

impl TenantRef {
    /// A tenant from its identifier.
    #[must_use]
    pub const fn new(id: PartyId) -> Self {
        Self(id)
    }

    /// The tenant's identifier.
    #[must_use]
    pub const fn id(&self) -> &PartyId {
        &self.0
    }
}

impl fmt::Display for TenantRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for TenantRef {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(Self)
    }
}

/// A 5G network function instance, for management-plane charging.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NfInstanceId(PartyId);

impl NfInstanceId {
    /// An instance identity from its identifier.
    #[must_use]
    pub const fn new(id: PartyId) -> Self {
        Self(id)
    }

    /// The instance's identifier.
    #[must_use]
    pub const fn id(&self) -> &PartyId {
        &self.0
    }
}

impl fmt::Display for NfInstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for NfInstanceId {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(Self)
    }
}

/// A subscription, as a record identifies it.
///
/// [`Supi`] is the stable identity and is mandatory. [`Gpsi`] is the public one
/// and is frequently absent from network-side records; it is not a substitute,
/// because two subscriptions can share an MSISDN over time while their SUPIs
/// stay distinct.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SubscriberRef {
    /// The permanent subscription identity.
    pub supi: Supi,
    /// The public identity, where the record carried one.
    pub gpsi: Option<Gpsi>,
    /// The retail brand, where it has been resolved.
    ///
    /// `None` means unresolved, not "the host network". A batch cannot be
    /// reconciled per tenant while this is missing, which is a finding rather
    /// than a default.
    pub tenant: Option<TenantRef>,
}

impl SubscriberRef {
    /// A subscription known only by its permanent identity.
    #[must_use]
    pub const fn new(supi: Supi) -> Self {
        Self {
            supi,
            gpsi: None,
            tenant: None,
        }
    }

    /// The same subscription with a public identity attached.
    #[must_use]
    pub fn with_gpsi(mut self, gpsi: Gpsi) -> Self {
        self.gpsi = Some(gpsi);
        self
    }

    /// The same subscription attributed to a retail brand.
    #[must_use]
    pub fn with_tenant(mut self, tenant: TenantRef) -> Self {
        self.tenant = Some(tenant);
        self
    }
}

impl fmt::Display for SubscriberRef {
    /// Writes the SUPI, which is the identity that keys everything downstream.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.supi.fmt(f)
    }
}

/// Why a record could not be attributed to a party.
///
/// A counted outcome, never a silent null. Each variant answers a different
/// operational question, and collapsing them loses the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UnattributableReason {
    /// The record carried only a concealed identity, which is not linkable.
    ///
    /// Resolution to a [`Supi`] is the network's job. The record is evidence of
    /// usage but not of whose.
    ConcealedIdentityOnly,
    /// The record carried no subscriber identity at all.
    IdentityAbsent,
    /// An identity was present but did not resolve to a known network.
    ///
    /// Typically an unassigned MCC, or a country the caller's assignment table
    /// does not cover.
    NetworkUnresolved,
    /// An identity resolved, but not to a known tenant.
    TenantUnresolved,
    /// The identity was present and well-formed but failed validation.
    IdentityMalformed,
}

impl UnattributableReason {
    /// Every variant, for exhaustive iteration in tests and mappings.
    pub const ALL: &'static [Self] = &[
        Self::ConcealedIdentityOnly,
        Self::IdentityAbsent,
        Self::NetworkUnresolved,
        Self::TenantUnresolved,
        Self::IdentityMalformed,
    ];

    /// The canonical codes, in the order of [`ALL`](Self::ALL).
    pub const CODES: &'static [&'static str] = &[
        "CONCEALED_IDENTITY_ONLY",
        "IDENTITY_ABSENT",
        "NETWORK_UNRESOLVED",
        "TENANT_UNRESOLVED",
        "IDENTITY_MALFORMED",
    ];

    /// `true` when supplying better reference data could resolve the record.
    ///
    /// The two malformed-input cases cannot be fixed by a table; the others can,
    /// which decides whether a held record is worth retrying after a reference
    /// data load.
    #[must_use]
    pub const fn is_resolvable_with_reference_data(self) -> bool {
        matches!(self, Self::NetworkUnresolved | Self::TenantUnresolved)
    }

    /// The canonical string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ConcealedIdentityOnly => "CONCEALED_IDENTITY_ONLY",
            Self::IdentityAbsent => "IDENTITY_ABSENT",
            Self::NetworkUnresolved => "NETWORK_UNRESOLVED",
            Self::TenantUnresolved => "TENANT_UNRESOLVED",
            Self::IdentityMalformed => "IDENTITY_MALFORMED",
        }
    }
}

impl fmt::Display for UnattributableReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for UnattributableReason {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim().to_ascii_uppercase();
        Self::ALL
            .iter()
            .find(|r| r.as_str() == t)
            .copied()
            .ok_or_else(|| ParseError::one_of("UnattributableReason", s, Self::CODES))
    }
}

/// The party a usage record is charged to.
///
/// Exhaustive, and deliberately not `#[non_exhaustive]`: a consumer mapping
/// this vocabulary onto their own storage codes should get a compile error when
/// a new kind of charged party appears, not a silent wildcard that files the
/// usage under the wrong counterparty.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ChargedParty {
    /// Retail usage by a subscription.
    Subscriber(SubscriberRef),
    /// Usage charged to an enterprise tenant or an MVNO wholesale account,
    /// with no subscriber — slice management, enterprise connectivity.
    Tenant(TenantRef),
    /// A network API invocation, which has two counterparties.
    ApiConsumer {
        /// The developer or aggregator that invoked the API and pays for it.
        developer: PartyId,
        /// The subscription the invocation concerned, where there was one.
        ///
        /// Absent for invocations that touch no subscriber, and present but
        /// distinct from `developer` otherwise — the settlement needs both.
        subject: Option<SubscriberRef>,
    },
    /// Management-plane usage produced by a network function.
    NetworkFunction(NfInstanceId),
    /// The party could not be determined, with the reason recorded.
    Unattributable(UnattributableReason),
}

impl ChargedParty {
    /// The subscription this usage concerns, if any.
    ///
    /// For an API invocation this is the *subject*, not the payer — the
    /// developer is not a subscriber and must not be counted as one.
    #[must_use]
    pub const fn subscriber(&self) -> Option<&SubscriberRef> {
        match self {
            Self::Subscriber(s) => Some(s),
            Self::ApiConsumer { subject, .. } => subject.as_ref(),
            Self::Tenant(_) | Self::NetworkFunction(_) | Self::Unattributable(_) => None,
        }
    }

    /// The tenant this usage belongs to, if it is known.
    #[must_use]
    pub const fn tenant(&self) -> Option<&TenantRef> {
        match self {
            Self::Tenant(t) => Some(t),
            Self::Subscriber(s) => s.tenant.as_ref(),
            Self::ApiConsumer { subject, .. } => match subject {
                Some(s) => s.tenant.as_ref(),
                None => None,
            },
            Self::NetworkFunction(_) | Self::Unattributable(_) => None,
        }
    }

    /// `true` when the usage could not be attributed.
    #[must_use]
    pub const fn is_unattributable(&self) -> bool {
        matches!(self, Self::Unattributable(_))
    }

    /// Why the usage could not be attributed, if it could not be.
    #[must_use]
    pub const fn unattributable_reason(&self) -> Option<UnattributableReason> {
        match self {
            Self::Unattributable(r) => Some(*r),
            _ => None,
        }
    }

    /// A short code naming the kind of party, for grouping and reporting.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Subscriber(_) => "SUBSCRIBER",
            Self::Tenant(_) => "TENANT",
            Self::ApiConsumer { .. } => "API_CONSUMER",
            Self::NetworkFunction(_) => "NETWORK_FUNCTION",
            Self::Unattributable(_) => "UNATTRIBUTABLE",
        }
    }
}

impl fmt::Display for ChargedParty {
    /// Writes `KIND:identity`, so a party is legible in a log without a
    /// separate type column.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Subscriber(s) => write!(f, "SUBSCRIBER:{s}"),
            Self::Tenant(t) => write!(f, "TENANT:{t}"),
            Self::ApiConsumer { developer, .. } => write!(f, "API_CONSUMER:{developer}"),
            Self::NetworkFunction(n) => write!(f, "NETWORK_FUNCTION:{n}"),
            Self::Unattributable(r) => write!(f, "UNATTRIBUTABLE:{r}"),
        }
    }
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::{
        ChargedParty, NfInstanceId, PartyId, SubscriberRef, TenantRef, UnattributableReason,
    };
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

    string_serde!(PartyId, "a party identifier such as \"mvno-alpha\"");
    string_serde!(TenantRef, "a tenant identifier such as \"mvno-alpha\"");
    string_serde!(NfInstanceId, "a network function instance identifier");
    string_serde!(
        UnattributableReason,
        "an unattributable reason code such as \"IDENTITY_ABSENT\""
    );

    /// Field names are part of the public representation and covered by semver.
    #[derive(Serialize, Deserialize)]
    #[serde(rename = "SubscriberRef")]
    struct SubscriberRefRepr {
        supi: crate::identity::Supi,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gpsi: Option<crate::identity::Gpsi>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tenant: Option<TenantRef>,
    }

    impl Serialize for SubscriberRef {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            SubscriberRefRepr {
                supi: self.supi.clone(),
                gpsi: self.gpsi.clone(),
                tenant: self.tenant.clone(),
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for SubscriberRef {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let r = SubscriberRefRepr::deserialize(deserializer)?;
            Ok(SubscriberRef {
                supi: r.supi,
                gpsi: r.gpsi,
                tenant: r.tenant,
            })
        }
    }

    /// An internally tagged representation: the `kind` discriminator is stable
    /// and readable, and adding a variant does not renumber the others.
    #[derive(Serialize, Deserialize)]
    #[serde(tag = "kind", rename = "ChargedParty")]
    enum ChargedPartyRepr {
        #[serde(rename = "SUBSCRIBER")]
        Subscriber { subscriber: SubscriberRef },
        #[serde(rename = "TENANT")]
        Tenant { tenant: TenantRef },
        #[serde(rename = "API_CONSUMER")]
        ApiConsumer {
            developer: PartyId,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            subject: Option<SubscriberRef>,
        },
        #[serde(rename = "NETWORK_FUNCTION")]
        NetworkFunction { nf_instance: NfInstanceId },
        #[serde(rename = "UNATTRIBUTABLE")]
        Unattributable { reason: UnattributableReason },
    }

    impl Serialize for ChargedParty {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let repr = match self {
                Self::Subscriber(s) => ChargedPartyRepr::Subscriber {
                    subscriber: s.clone(),
                },
                Self::Tenant(t) => ChargedPartyRepr::Tenant { tenant: t.clone() },
                Self::ApiConsumer { developer, subject } => ChargedPartyRepr::ApiConsumer {
                    developer: developer.clone(),
                    subject: subject.clone(),
                },
                Self::NetworkFunction(n) => ChargedPartyRepr::NetworkFunction {
                    nf_instance: n.clone(),
                },
                Self::Unattributable(r) => ChargedPartyRepr::Unattributable { reason: *r },
            };
            repr.serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for ChargedParty {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            Ok(match ChargedPartyRepr::deserialize(deserializer)? {
                ChargedPartyRepr::Subscriber { subscriber } => Self::Subscriber(subscriber),
                ChargedPartyRepr::Tenant { tenant } => Self::Tenant(tenant),
                ChargedPartyRepr::ApiConsumer { developer, subject } => {
                    Self::ApiConsumer { developer, subject }
                }
                ChargedPartyRepr::NetworkFunction { nf_instance } => {
                    Self::NetworkFunction(nf_instance)
                }
                ChargedPartyRepr::Unattributable { reason } => Self::Unattributable(reason),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subscriber() -> SubscriberRef {
        SubscriberRef::new("imsi-262021234567890".parse().unwrap())
    }

    /// The reason the party is a sum type: slice management charging has no
    /// subscriber, and a mandatory subscriber field cannot express it.
    #[test]
    fn a_charged_party_need_not_be_a_subscriber() {
        let tenant = ChargedParty::Tenant(TenantRef::new("enterprise-7".parse().unwrap()));
        assert!(tenant.subscriber().is_none());
        assert_eq!(tenant.tenant().unwrap().to_string(), "enterprise-7");
        assert_eq!(tenant.kind(), "TENANT");
    }

    /// A network API invocation has two counterparties, and the payer is not a
    /// subscriber — counting them as one would double the subscriber base.
    #[test]
    fn an_api_invocation_keeps_payer_and_subject_apart() {
        let party = ChargedParty::ApiConsumer {
            developer: "acme-dev".parse().unwrap(),
            subject: Some(subscriber()),
        };
        assert_eq!(party.kind(), "API_CONSUMER");
        assert_eq!(
            party.subscriber().unwrap().supi.to_string(),
            "imsi-262021234567890"
        );
        assert_eq!(party.to_string(), "API_CONSUMER:acme-dev");

        // An invocation touching no subscriber still has a payer.
        let anonymous = ChargedParty::ApiConsumer {
            developer: "acme-dev".parse().unwrap(),
            subject: None,
        };
        assert!(anonymous.subscriber().is_none());
    }

    /// Unattributable is a counted outcome carrying a reason, not a null.
    #[test]
    fn an_unattributable_record_says_why() {
        let party = ChargedParty::Unattributable(UnattributableReason::ConcealedIdentityOnly);
        assert!(party.is_unattributable());
        assert_eq!(
            party.unattributable_reason(),
            Some(UnattributableReason::ConcealedIdentityOnly)
        );
        assert_eq!(party.to_string(), "UNATTRIBUTABLE:CONCEALED_IDENTITY_ONLY");
    }

    /// Whether reference data could fix it decides if a held record is worth
    /// retrying after a table load.
    #[test]
    fn only_some_unattributable_reasons_are_worth_retrying() {
        assert!(UnattributableReason::NetworkUnresolved.is_resolvable_with_reference_data());
        assert!(UnattributableReason::TenantUnresolved.is_resolvable_with_reference_data());
        assert!(!UnattributableReason::IdentityMalformed.is_resolvable_with_reference_data());
        assert!(!UnattributableReason::ConcealedIdentityOnly.is_resolvable_with_reference_data());
    }

    #[test]
    fn tenancy_reaches_the_party_through_the_subscription() {
        let tenant = TenantRef::new("mvno-alpha".parse().unwrap());
        let party = ChargedParty::Subscriber(subscriber().with_tenant(tenant.clone()));
        assert_eq!(party.tenant(), Some(&tenant));

        // Unresolved tenancy is `None`, not the host network.
        assert_eq!(ChargedParty::Subscriber(subscriber()).tenant(), None);
    }

    #[test]
    fn unattributable_reason_codes_round_trip() {
        assert_eq!(
            UnattributableReason::ALL.len(),
            UnattributableReason::CODES.len()
        );
        for (reason, code) in UnattributableReason::ALL
            .iter()
            .zip(UnattributableReason::CODES)
        {
            assert_eq!(reason.as_str(), *code);
            assert_eq!(code.parse::<UnattributableReason>().unwrap(), *reason);
        }
        assert!("NOPE".parse::<UnattributableReason>().is_err());
    }

    #[test]
    fn party_identifiers_reject_whitespace_and_overlong_input() {
        assert!("mvno-alpha".parse::<PartyId>().is_ok());
        assert_eq!(
            " mvno-alpha ".parse::<PartyId>().unwrap().as_str(),
            "mvno-alpha"
        );
        for s in ["", "has space", &"x".repeat(65)] {
            assert!(s.parse::<PartyId>().is_err(), "{s:?} must not parse");
        }
    }
}
