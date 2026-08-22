//! Which country's rules apply — and which question you are asking.
//!
//! Four different regimes attach to a usage record by country: roaming
//! regulation, retention, lawful interception and tax. Three of them turn on
//! **where the usage happened**. The fourth does not.
//!
//! # Two questions, two countries, and they differ for every roamer
//!
//! | Question | Answer | Source |
//! |---|---|---|
//! | Whose telecom rules govern this usage? | where it happened | the **serving** network |
//! | Which country taxes it? | where the customer is | the **SIM's** country |
//!
//! For EU VAT, Council Implementing Regulation (EU) 282/2011 Art. 24b(b)
//! presumes a customer supplied over a mobile network to be located in the
//! country of the **mobile country code of the IMSI on the SIM card**, and
//! Art. 24f lists that MCC as an item of evidence. A German subscriber using
//! data in Spain is governed by Spanish telecom rules and taxed in Germany, and
//! a mediation layer that answers "country" with one number has answered one of
//! the two questions wrong.
//!
//! So there is no `country()` here. There is [`usage_country`] and there is
//! [`customer_country_presumption`], each named for its question.
//!
//! ```rust
//! use mediation::{Jurisdiction, customer_country_presumption, usage_country};
//! # use mediation::{ChargedParty, RecordType, SourceRef, SubscriberRef, UsageRecord};
//! # let roamer = UsageRecord::new(
//! #     RecordType::PduSession,
//! #     ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
//! #     "2026-07-28T10:00:00+02:00".parse().unwrap(),
//! #     SourceRef::new("pgw-01".parse().unwrap(), 0),
//! # ).with_networks("214-07".parse().unwrap(), "262-02".parse().unwrap());
//! // A German SIM, used in Spain.
//! assert_eq!(usage_country(&roamer), Jurisdiction::Country("214".parse().unwrap()));
//! assert_eq!(
//!     customer_country_presumption(&roamer),
//!     Jurisdiction::Country("262".parse().unwrap()),
//! );
//! ```
//!
//! # The crate ships no resolver, and 3GPP has not resolved it either
//!
//! Reading a country from a serving network works because a terrestrial network
//! is licensed in one country. Satellite access breaks that: a beam crosses
//! borders and covers international waters, so the serving network's country
//! describes the **operator's licence**, not the subscriber's position. 3GPP SA1
//! opened a study on extra-territorial satellite coverage precisely because the
//! legal and regulatory answer is not settled.
//!
//! A library that returned a country there would be claiming more than the
//! standards body does. [`Ambiguity::ExtraTerritorialAccess`] says so instead.

use std::fmt;

use crate::plmn::Mcc;
use crate::record::UsageRecord;

// ── Ambiguity ────────────────────────────────────────────────────────────────

/// Why a record does not fix a country.
///
/// Each variant is a different problem with a different remedy, which is why
/// they are not one "unknown".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Ambiguity {
    /// The record carries nothing to read a country from.
    ///
    /// A decoding or provenance gap: fix the feed.
    NoEvidence,
    /// The access is extra-territorial, so the serving network's country is not
    /// evidence of where the subscriber was.
    ///
    /// Satellite access. The serving network is still recorded — it just does
    /// not answer this question — so a caller who has an independent position
    /// fix can still resolve it. Nothing else can.
    ExtraTerritorialAccess,
    /// The serving network's country code names no country.
    ///
    /// ITU-T E.212 sets aside codes for test networks, trial networks, internal
    /// use, and the shared codes international operators use — see
    /// [`Mcc::is_geographic`]. They are well-formed and they answer no question
    /// about where the subscriber was, which is a different problem from a
    /// missing field and has a different remedy: the record needs a position
    /// from somewhere else, or it is a test record that should never have
    /// entered the feed.
    NonGeographicNetwork {
        /// The code the record carried.
        mcc: Mcc,
    },
    /// Two sources in the record name different countries.
    ///
    /// A fault, not a tie to break: the serving network and the reported cell
    /// disagree, and picking one by precedence would file the usage under a
    /// country no evidence supports. Both are carried so a caller can see which
    /// feed is wrong.
    Conflicting {
        /// The country the serving network is in.
        serving: Mcc,
        /// The country the reported location is in.
        reported: Mcc,
    },
}

impl Ambiguity {
    /// A short code naming the reason.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoEvidence => "NO_EVIDENCE",
            Self::ExtraTerritorialAccess => "EXTRA_TERRITORIAL_ACCESS",
            Self::NonGeographicNetwork { .. } => "NON_GEOGRAPHIC_NETWORK",
            Self::Conflicting { .. } => "CONFLICTING",
        }
    }

    /// `true` when the record contradicts itself rather than merely being
    /// silent.
    ///
    /// The difference between a feed to fix and a record to enrich.
    #[must_use]
    pub const fn is_contradiction(self) -> bool {
        matches!(self, Self::Conflicting { .. })
    }
}

impl fmt::Display for Ambiguity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflicting { serving, reported } => {
                write!(f, "CONFLICTING (serving {serving}, reported {reported})")
            }
            Self::NonGeographicNetwork { mcc } => {
                write!(f, "NON_GEOGRAPHIC_NETWORK ({mcc})")
            }
            other => f.write_str(other.as_str()),
        }
    }
}

// ── Jurisdiction ─────────────────────────────────────────────────────────────

/// A country, or the reason there is not one.
///
/// Deliberately not an `Option<Mcc>`: "no country" has several causes with
/// different remedies, and an `Option` would collapse a contradiction and a
/// silence into the same `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Jurisdiction {
    /// The record fixes a country.
    Country(Mcc),
    /// It does not, for this reason.
    Unresolved(Ambiguity),
}

impl Jurisdiction {
    /// The country, when there is one.
    ///
    /// `None` for every unresolved case, so a caller who genuinely does not
    /// care why can still say so — but has to say it.
    #[must_use]
    pub const fn country(self) -> Option<Mcc> {
        match self {
            Self::Country(mcc) => Some(mcc),
            Self::Unresolved(_) => None,
        }
    }

    /// Why there is no country, when there is not one.
    #[must_use]
    pub const fn ambiguity(self) -> Option<Ambiguity> {
        match self {
            Self::Country(_) => None,
            Self::Unresolved(reason) => Some(reason),
        }
    }

    /// `true` when a country was determined.
    #[must_use]
    pub const fn is_resolved(self) -> bool {
        matches!(self, Self::Country(_))
    }
}

impl fmt::Display for Jurisdiction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Country(mcc) => write!(f, "{mcc}"),
            Self::Unresolved(reason) => write!(f, "unresolved: {reason}"),
        }
    }
}

// ── Where the usage happened ─────────────────────────────────────────────────

/// The country whose telecom rules govern this usage.
///
/// The question behind roaming regulation, retention and lawful interception:
/// **where the subscriber was**, not where their subscription is.
///
/// Read from the serving network, cross-checked against the reported location.
/// The two must agree — they both carry an MCC, and a record where they differ
/// has a decoding fault or a mislabelled element, which
/// [`Ambiguity::Conflicting`] reports rather than resolving by precedence.
///
/// Satellite access yields [`Ambiguity::ExtraTerritorialAccess`] whatever the
/// serving network says, because a beam crossing borders makes that MCC a
/// statement about the operator's licence.
///
/// ```rust
/// use mediation::{Ambiguity, Jurisdiction, RatType, SessionContext, usage_country};
/// # use mediation::{ChargedParty, RecordType, SourceRef, SubscriberRef, UsageRecord};
/// # let base = UsageRecord::new(
/// #     RecordType::PduSession,
/// #     ChargedParty::Subscriber(SubscriberRef::new("imsi-262021234567890".parse().unwrap())),
/// #     "2026-07-28T10:00:00+02:00".parse().unwrap(),
/// #     SourceRef::new("pgw-01".parse().unwrap(), 0),
/// # ).with_networks("214-07".parse().unwrap(), "262-02".parse().unwrap());
/// assert_eq!(usage_country(&base), Jurisdiction::Country("214".parse().unwrap()));
///
/// // Over satellite, the same serving network no longer answers.
/// let orbital = base.with_session(SessionContext::none().with_rat(RatType::NonTerrestrial));
/// assert_eq!(
///     usage_country(&orbital),
///     Jurisdiction::Unresolved(Ambiguity::ExtraTerritorialAccess),
/// );
/// ```
#[must_use]
pub fn usage_country(record: &UsageRecord) -> Jurisdiction {
    if record.session.jurisdiction_is_ambiguous() {
        return Jurisdiction::Unresolved(Ambiguity::ExtraTerritorialAccess);
    }

    let serving = record.serving_plmn.map(|plmn| plmn.mcc());
    let reported = record.session.serving_country();

    let country = match (serving, reported) {
        (Some(serving), Some(reported)) if serving != reported => {
            return Jurisdiction::Unresolved(Ambiguity::Conflicting { serving, reported });
        }
        (Some(country), _) | (None, Some(country)) => country,
        (None, None) => return Jurisdiction::Unresolved(Ambiguity::NoEvidence),
    };
    // A well-formed code that names no territory is not an answer to "where".
    if !country.is_geographic() {
        return Jurisdiction::Unresolved(Ambiguity::NonGeographicNetwork { mcc: country });
    }
    Jurisdiction::Country(country)
}

// ── Where the customer is ────────────────────────────────────────────────────

/// The customer's country under the EU VAT presumption.
///
/// Council Implementing Regulation (EU) 282/2011 Art. 24b(b): a customer
/// supplied over a mobile network is **presumed** to be located in the country
/// of the mobile country code of the IMSI on the SIM card. Art. 24f lists that
/// MCC among the items of evidence.
///
/// So this reads the **subscriber's IMSI**, not the serving network and not the
/// home PLMN field. The three normally agree for a domestic record and diverge
/// for every roamer, and the home PLMN is a routing fact that an MVNO
/// arrangement can set to something the SIM does not say.
///
/// # It is a presumption, not a finding
///
/// The regulation allows it to be rebutted, and only the supplier holds what
/// would rebut it — a billing address, a fixed-line contract. This crate
/// returns the presumption and marks nothing as final. A subscriber with no
/// IMSI-form identity yields [`Ambiguity::NoEvidence`]: there is no presumption
/// to make, and falling back to the serving network would answer the *other*
/// question.
///
/// A SIM whose MCC names no territory — a test range, a shared international
/// code; see [`Mcc::is_geographic`] — yields
/// [`Ambiguity::NonGeographicNetwork`], for the same reason
/// [`usage_country`] does. The presumption is that the customer is *in* the
/// country of the MCC, and `001` is not a country anyone can be taxed in.
///
/// ```rust
/// use mediation::{Ambiguity, Jurisdiction, customer_country_presumption};
/// # use mediation::{ChargedParty, RecordType, SourceRef, SubscriberRef, UsageRecord};
/// # let test_sim = UsageRecord::new(
/// #     RecordType::PduSession,
/// #     ChargedParty::Subscriber(SubscriberRef::new("imsi-001010000000001".parse().unwrap())),
/// #     "2026-07-28T10:00:00+02:00".parse().unwrap(),
/// #     SourceRef::new("pgw-01".parse().unwrap(), 0),
/// # );
/// // A test IMSI has no tax country, and saying "001" would invent one.
/// assert!(matches!(
///     customer_country_presumption(&test_sim),
///     Jurisdiction::Unresolved(Ambiguity::NonGeographicNetwork { .. }),
/// ));
/// ```
#[must_use]
pub fn customer_country_presumption(record: &UsageRecord) -> Jurisdiction {
    let Some(imsi) = record
        .party
        .subscriber()
        .and_then(|subscriber| subscriber.supi.as_imsi())
    else {
        return Jurisdiction::Unresolved(Ambiguity::NoEvidence);
    };
    let mcc = imsi.mcc();
    if !mcc.is_geographic() {
        return Jurisdiction::Unresolved(Ambiguity::NonGeographicNetwork { mcc });
    }
    Jurisdiction::Country(mcc)
}

// ── The two together ─────────────────────────────────────────────────────────

/// `true` when the usage happened outside the customer's own country.
///
/// Not the same as [`UsageRecord::is_roaming`], which compares **networks**: a
/// subscriber on a second domestic network is roaming and is not abroad, and
/// the regulations that turn on being abroad — Roam Like At Home, the fair-use
/// window — mean abroad.
///
/// `None` when either country is unresolved, which is an answer: a record that
/// cannot establish this is not a domestic record, and defaulting it to `false`
/// understates roaming in exactly the direction that costs money.
#[must_use]
pub fn is_abroad(record: &UsageRecord) -> Option<bool> {
    let here = usage_country(record).country()?;
    let home = customer_country_presumption(record).country()?;
    Some(here != home)
}

// ── Serde ────────────────────────────────────────────────────────────────────

#[cfg(feature = "serde")]
mod serde_impl {
    use super::{Ambiguity, Jurisdiction};
    use crate::plmn::Mcc;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// Internally tagged on `reason`, so the conflicting case can carry its two
    /// countries without a second shape.
    #[derive(Serialize, Deserialize)]
    #[serde(tag = "reason", rename = "Ambiguity")]
    enum AmbiguityRepr {
        #[serde(rename = "NO_EVIDENCE")]
        NoEvidence,
        #[serde(rename = "EXTRA_TERRITORIAL_ACCESS")]
        ExtraTerritorialAccess,
        #[serde(rename = "NON_GEOGRAPHIC_NETWORK")]
        NonGeographicNetwork { mcc: Mcc },
        #[serde(rename = "CONFLICTING")]
        Conflicting { serving: Mcc, reported: Mcc },
    }

    impl Serialize for Ambiguity {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            match *self {
                Ambiguity::NoEvidence => AmbiguityRepr::NoEvidence,
                Ambiguity::ExtraTerritorialAccess => AmbiguityRepr::ExtraTerritorialAccess,
                Ambiguity::NonGeographicNetwork { mcc } => {
                    AmbiguityRepr::NonGeographicNetwork { mcc }
                }
                Ambiguity::Conflicting { serving, reported } => {
                    AmbiguityRepr::Conflicting { serving, reported }
                }
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for Ambiguity {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            Ok(match AmbiguityRepr::deserialize(deserializer)? {
                AmbiguityRepr::NoEvidence => Ambiguity::NoEvidence,
                AmbiguityRepr::ExtraTerritorialAccess => Ambiguity::ExtraTerritorialAccess,
                AmbiguityRepr::NonGeographicNetwork { mcc } => {
                    Ambiguity::NonGeographicNetwork { mcc }
                }
                AmbiguityRepr::Conflicting { serving, reported } => {
                    Ambiguity::Conflicting { serving, reported }
                }
            })
        }
    }

    /// Internally tagged on `kind`: a resolved country and an unresolved reason
    /// must never read alike.
    #[derive(Serialize, Deserialize)]
    #[serde(tag = "kind", rename = "Jurisdiction")]
    enum JurisdictionRepr {
        #[serde(rename = "COUNTRY")]
        Country { mcc: Mcc },
        #[serde(rename = "UNRESOLVED")]
        Unresolved { ambiguity: Ambiguity },
    }

    impl Serialize for Jurisdiction {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            match *self {
                Jurisdiction::Country(mcc) => JurisdictionRepr::Country { mcc },
                Jurisdiction::Unresolved(ambiguity) => JurisdictionRepr::Unresolved { ambiguity },
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for Jurisdiction {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            Ok(match JurisdictionRepr::deserialize(deserializer)? {
                JurisdictionRepr::Country { mcc } => Jurisdiction::Country(mcc),
                JurisdictionRepr::Unresolved { ambiguity } => Jurisdiction::Unresolved(ambiguity),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Nai;
    use crate::location::{Ncgi, Nci, UserLocation};
    use crate::party::{ChargedParty, SubscriberRef};
    use crate::record::{RecordType, SourceRef};
    use crate::session::{RatType, SessionContext};

    fn record(imsi: &str, serving: &str, home: &str) -> UsageRecord {
        UsageRecord::new(
            RecordType::PduSession,
            ChargedParty::Subscriber(SubscriberRef::new(imsi.parse().unwrap())),
            "2026-07-28T10:00:00+02:00".parse().unwrap(),
            SourceRef::new("pgw-01".parse().unwrap(), 0),
        )
        .with_networks(serving.parse().unwrap(), home.parse().unwrap())
    }

    fn cell(plmn: &str) -> UserLocation {
        UserLocation::Ncgi(Ncgi::new(
            plmn.parse().unwrap(),
            Nci::new(0x0_1234_5678).unwrap(),
        ))
    }

    // ── The two questions ────────────────────────────────────────────────────

    /// The whole reason the module exists: a German SIM used in Spain is
    /// governed by Spanish telecom rules and taxed in Germany, and one number
    /// cannot say both.
    #[test]
    fn a_roamer_has_two_countries_and_they_are_not_the_same() {
        let roamer = record("imsi-262021234567890", "214-07", "262-02");

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

    /// At home the two agree, which is exactly why conflating them survives
    /// testing on domestic traffic.
    #[test]
    fn at_home_the_two_questions_have_one_answer() {
        let domestic = record("imsi-262021234567890", "262-02", "262-02");

        assert_eq!(
            usage_country(&domestic),
            customer_country_presumption(&domestic)
        );
        assert_eq!(is_abroad(&domestic), Some(false));
    }

    /// Being on another network in the same country is roaming and is not
    /// abroad, and the regulations that turn on being abroad mean abroad.
    #[test]
    fn national_roaming_is_roaming_but_not_abroad() {
        let national = record("imsi-262021234567890", "262-01", "262-02");

        assert_eq!(national.is_roaming(), Some(true), "a different network");
        assert_eq!(is_abroad(&national), Some(false), "the same country");
    }

    // ── The VAT presumption reads the SIM, not the routing ───────────────────

    /// Art. 24b(b) names the MCC of the IMSI on the SIM. The home PLMN is a
    /// routing fact an MVNO arrangement can set elsewhere.
    #[test]
    fn the_presumption_reads_the_sim_not_the_home_network() {
        // A German SIM whose home network field names an Austrian host.
        let mvno = record("imsi-262021234567890", "262-02", "232-01");

        assert_eq!(
            customer_country_presumption(&mvno),
            Jurisdiction::Country("262".parse().unwrap()),
            "the SIM decides, not the routing"
        );
    }

    /// No IMSI means no presumption to make — and falling back to the serving
    /// network would answer the other question entirely.
    #[test]
    fn a_subscriber_without_an_imsi_yields_no_presumption() {
        let nai: Nai = "user@example.net".parse().unwrap();
        let non_3gpp = UsageRecord::new(
            RecordType::PduSession,
            ChargedParty::Subscriber(SubscriberRef::new(crate::identity::Supi::NetworkSpecific(
                nai,
            ))),
            "2026-07-28T10:00:00+02:00".parse().unwrap(),
            SourceRef::new("pgw-01".parse().unwrap(), 0),
        )
        .with_networks("214-07".parse().unwrap(), "262-02".parse().unwrap());

        assert_eq!(
            customer_country_presumption(&non_3gpp),
            Jurisdiction::Unresolved(Ambiguity::NoEvidence)
        );
        // The other question still answers.
        assert!(usage_country(&non_3gpp).is_resolved());
        assert_eq!(is_abroad(&non_3gpp), None);
    }

    // ── Ambiguity ────────────────────────────────────────────────────────────

    /// A beam crosses borders, so the serving network's country describes the
    /// operator's licence rather than the subscriber's position.
    #[test]
    fn satellite_access_does_not_fix_a_country() {
        let orbital = record("imsi-262021234567890", "214-07", "262-02")
            .with_session(SessionContext::none().with_rat(RatType::NonTerrestrial));

        assert_eq!(
            usage_country(&orbital),
            Jurisdiction::Unresolved(Ambiguity::ExtraTerritorialAccess)
        );
        assert_eq!(is_abroad(&orbital), None);

        // The customer question is unaffected: the SIM did not move.
        assert_eq!(
            customer_country_presumption(&orbital),
            Jurisdiction::Country("262".parse().unwrap())
        );

        // A terrestrial access over the same networks resolves.
        let terrestrial = record("imsi-262021234567890", "214-07", "262-02")
            .with_session(SessionContext::none().with_rat(RatType::Nr));
        assert!(usage_country(&terrestrial).is_resolved());
    }

    /// An unrecorded access technology is not evidence of ambiguity, or every
    /// legacy record would be flagged.
    #[test]
    fn an_unrecorded_access_is_not_ambiguous() {
        let plain = record("imsi-262021234567890", "214-07", "262-02");
        assert!(plain.session.rat.is_none());
        assert!(usage_country(&plain).is_resolved());
    }

    /// Two sources naming two countries is a fault, and picking one by
    /// precedence would file the usage under a country no evidence supports.
    #[test]
    fn conflicting_evidence_is_reported_not_resolved() {
        let confused = record("imsi-262021234567890", "214-07", "262-02")
            .with_session(SessionContext::none().with_location(cell("232-01")));

        let verdict = usage_country(&confused);
        assert_eq!(
            verdict,
            Jurisdiction::Unresolved(Ambiguity::Conflicting {
                serving: "214".parse().unwrap(),
                reported: "232".parse().unwrap(),
            })
        );
        assert_eq!(verdict.country(), None);
        assert!(verdict.ambiguity().unwrap().is_contradiction());
        assert!(verdict.to_string().contains("serving 214"));

        // Agreeing sources resolve to the country both name.
        let agreeing = record("imsi-262021234567890", "214-07", "262-02")
            .with_session(SessionContext::none().with_location(cell("214-07")));
        assert_eq!(
            usage_country(&agreeing),
            Jurisdiction::Country("214".parse().unwrap())
        );
    }

    /// A location alone answers the question when no serving network was
    /// recorded — it carries an MCC too.
    #[test]
    fn a_location_alone_fixes_the_country() {
        let mut located = record("imsi-262021234567890", "214-07", "262-02");
        located.serving_plmn = None;
        located.session = SessionContext::none().with_location(cell("214-07"));

        assert_eq!(
            usage_country(&located),
            Jurisdiction::Country("214".parse().unwrap())
        );
    }

    /// Silence and contradiction are different problems with different
    /// remedies, which is why this is not an `Option`.
    #[test]
    fn nothing_recorded_is_its_own_reason() {
        let mut blank = record("imsi-262021234567890", "214-07", "262-02");
        blank.serving_plmn = None;

        assert_eq!(
            usage_country(&blank),
            Jurisdiction::Unresolved(Ambiguity::NoEvidence)
        );
        assert!(!Ambiguity::NoEvidence.is_contradiction());
        assert_eq!(Ambiguity::NoEvidence.to_string(), "NO_EVIDENCE");
        assert_eq!(
            Ambiguity::ExtraTerritorialAccess.as_str(),
            "EXTRA_TERRITORIAL_ACCESS"
        );
    }

    #[test]
    fn a_resolved_jurisdiction_renders_as_its_country() {
        let resolved = Jurisdiction::Country("262".parse().unwrap());
        assert_eq!(resolved.to_string(), "262");
        assert_eq!(resolved.ambiguity(), None);
        assert!(
            Jurisdiction::Unresolved(Ambiguity::NoEvidence)
                .to_string()
                .starts_with("unresolved")
        );
    }
}
