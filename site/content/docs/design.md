+++
title = "Design constraints and testing"
description = "Zero I/O, no async, no clock, exact integers, no unsafe — how each constraint is enforced, and what the property suites actually prove."
weight = 170
+++

Each constraint below is enforced by something that fails the build, not by a
convention somebody has to remember. That is the difference between a guarantee
and an intention.

## Design constraints

| Constraint | Detail |
|---|---|
| 🧊 **No I/O** | All inputs are arguments. CI-enforced. |
| ⏱️ **No async** | Synchronous throughout. |
| 🕰️ **No clock** | No clock read anywhere; evaluation instants are parameters. CI-enforced. |
| 🔢 **Exact quantities** | Octets are `u64`, durations `u32`. No float reaches a quantity. CI-enforced. |
| 🎯 **Deterministic** | Same inputs always produce the same output — replayable and cacheable. |
| ♻️ **Idempotent** | Feeding the same record twice yields one billable outcome and one recorded duplicate — never two, never zero. |
| 🧱 **Immutable records** | A `UsageRecord` is never mutated; a correction is a new record. |
| 🛡️ **Total on untrusted input** | No parser and no decoder panics, at any input. A `proptest` suite hammers all of them on every build. |
| 🔒 **No `unsafe`** | `#![deny(unsafe_code)]`, also checked by the purity job. |
| 🪶 **Small dependency set** | `time`, `thiserror` — `serde` optional. |

### Why nothing panics

A mediation layer reads records that arrived over a network from a partner's
equipment. Every byte and every character of them is chosen by somebody else, so
a decoder that aborts the process on a malformed field is not a validation
failure — it is a denial of service with a partner's file as the payload.

Two consequences are visible in the API. Anything sized by the input is
**bounded**: `SequenceTracker::observe` caps the missing identities it lists,
`PartialRun::assess` caps the holes it names while still counting them exactly,
and `BerReader` bounds how deep indefinite lengths may nest. And nothing is
silently skipped: a decoder that stepped over a field it could not read would
produce a record missing exactly the fields it failed on, which is worse than no
record — so an end-of-contents marker inside a definite-length structure is an
error, not a terminator.

[`tests/robustness.rs`](https://github.com/hupe1980/mediation/blob/main/tests/robustness.rs) throws generated strings and bytes
at every `FromStr` and every wire decoder. It has already caught three real
defects: a UTF-8 boundary in the offset parser, another in the timestamp parser,
and a stack overflow on nested indefinite BER lengths — none of which needed
anything more than a few hundred kilobytes of `30 80`.

### Why no clock

A clock read is ambient state in the same family as I/O: it makes construction
non-deterministic, so two values built from identical inputs are never equal and
no storage layer can write a round-trip test. It also breaks replay — an
archived batch reprocessed years later must yield the verdicts it originally
did, not the verdicts of today. Callers that *have* a clock pass the instant in;
callers replaying an archive pass the archived instant and get the archived
answer back.


## Testing

```bash
cargo test --all-features
```

| Suite | Covers |
|---|---|
| [`offset_arithmetic.rs`](https://github.com/hupe1980/mediation/blob/main/tests/offset_arithmetic.rs) | Every quarter-hour zone, both 2026 German DST transitions, local-day vs UTC-day bucketing, instant ordering, hand-computed semi-octet vectors |
| [`identity_checkdigits.rs`](https://github.com/hupe1980/mediation/blob/main/tests/identity_checkdigits.rs) | Every single-digit error and adjacent transposition on a reference IMEI — including the known Luhn `09`/`90` blind spot — and IMEISV explicitly *not* checksummed |
| [`string_canonicalisation.rs`](https://github.com/hupe1980/mediation/blob/main/tests/string_canonicalisation.rs) | Stability, idempotence, totality and injectivity under `proptest`, for both the string and the `serde` form |
| [`pipeline.rs`](https://github.com/hupe1980/mediation/blob/main/tests/pipeline.rs) | The whole layer on one batch: sequence tracking, dedup, validation, disposition and the gate, plus determinism, archive replay, a hole at one gateway that does not block another, and `ProcessState` surviving a restart |
| [`period_and_correlation.rs`](https://github.com/hupe1980/mediation/blob/main/tests/period_and_correlation.rs) | The two seams where usage disappears — between batches and between nodes — plus corrections after a period closed |
| [`wire_codecs.rs`](https://github.com/hupe1980/mediation/blob/main/tests/wire_codecs.rs) | TBCD and plain BCD against the published examples, the nine-octet timestamp over every quarter-hour zone, and slice identities on the wire |
| [`ber_framing.rs`](https://github.com/hupe1980/mediation/blob/main/tests/ber_framing.rs) | Walking a record shaped like a real CDR, against an independently written encoder — then truncation, pathological nesting and non-minimal encodings |
| [`roaming_batch.rs`](https://github.com/hupe1980/mediation/blob/main/tests/roaming_batch.rs) | A partner's batch through every layer at once, and both RAP return classes against the conservation law |
| [`jurisdiction.rs`](https://github.com/hupe1980/mediation/blob/main/tests/jurisdiction.rs) | The regulator and the tax authority in different countries; a national roamer that is roaming and not abroad; each reason a country cannot be determined |
| [`timeband_and_location.rs`](https://github.com/hupe1980/mediation/blob/main/tests/timeband_and_location.rs) | `proptest`: apportionment never creates or destroys usage, for any total and any weights; a session through midnight; measured versus estimated; the cell-identity split that cannot be guessed |
| [`numbering_and_fraud.rs`](https://github.com/hupe1980/mediation/blob/main/tests/numbering_and_fraud.rs) | Settlement against the wrong partner for every ported number, an unresolvable number reported as such, and each fraud signal against the shape it does *not* catch |
| [`regulatory.rs`](https://github.com/hupe1980/mediation/blob/main/tests/regulatory.rs) | The observe-alert-wait-surcharge sequence and its two-week floor, the safe harbour under `proptest`, retention anchors and the hold that has no deadline, and the reduction/pseudonym distinction |
| [`conservation.rs`](https://github.com/hupe1980/mediation/blob/main/tests/conservation.rs) | The conservation law under `proptest`: every batch balances, in any order, split and recombined — and the total's evidence matches the pass that produced it |
| [`sequence_and_dedup.rs`](https://github.com/hupe1980/mediation/blob/main/tests/sequence_and_dedup.rs) | Wrap-safe sequence arithmetic across the whole TAP space, a two-billion-file hole counted exactly in one run, dedup idempotence over any number of copies, and both stores round-tripping |
| [`serde_representation.rs`](https://github.com/hupe1980/mediation/blob/main/tests/serde_representation.rs) | Every enum tag and struct field name, pinned literally |
| [`user_location_wire.rs`](https://github.com/hupe1980/mediation/blob/main/tests/user_location_wire.rs) | Every ULI identity's width and field order, the 5GS tracking area the element cannot carry, the reserved area codes, and the string round trip |
| [`robustness.rs`](https://github.com/hupe1980/mediation/blob/main/tests/robustness.rs) | `proptest`: no parser and no decoder panics, at any input |

### Documentation cannot drift

Every Rust block in the README **and on every page of this site** is compiled and
run as a doctest — see `ReadmeDoctests` and `site_doctests` in
[`src/lib.rs`](https://github.com/hupe1980/mediation/blob/main/src/lib.rs). A page
that names a function the crate does not have, or asserts a value it no longer
returns, fails the build.

Documentation that claims to be tested and is not is worse than documentation
that claims nothing, because a reader trusts it — so the check is structural
rather than a parallel file somebody has to remember to update.

### Dev tasks

A [`justfile`](https://github.com/hupe1980/mediation/blob/main/justfile) wraps the common commands ([just](https://just.systems)):

```bash
just            # list all recipes
just ci         # fmt-check + lint + purity + no-floats + test + doc + site-check + package
just test       # cargo test --all-features
just lint       # clippy with -D warnings, default + all features
just purity     # assert no clock read, no I/O, no unsafe in src/
just no-floats  # assert no float type reaches a quantity
just msrv       # compile on the pinned MSRV
just doc-open   # build the API docs and open them
just site-serve # serve this site with live reload
```


## Standards and regulatory basis

- **3GPP TS 32.298** — CDR parameter description; the nine-octet BCD `TimeStamp`
- **3GPP TS 29.060 §7.7.52 / TS 24.008 / TS 23.040 §9.2.3.11** — MS Time Zone:
  semi-octet quarter-hour offset, DST-adjusted, separate DST field
- **3GPP TS 23.003** — numbering, addressing and identification (IMSI, MSISDN,
  IMEI/IMEISV, SUPI, SUCI, GPSI, PEI); S-NSSAI and DNN structure; CGI, SAI, ECGI
  (28-bit ECI), NCGI (36-bit NCI, 22–32-bit gNB ID) and the TAI, whose code is 16
  bits in EPS and 24 in 5GS
- **3GPP TS 24.008** — the two location-area codes reserved to mean *no valid
  area*, reused at tracking-area width
- **3GPP TS 23.501** — slice/service types, and the 5QI a container runs under
- **3GPP TS 29.002** — TBCD-STRING: swapped nibbles, `0xF` filler; `AddressString`,
  whose leading octet is a nature-of-address and numbering-plan indicator
- **3GPP TS 24.008 §10.5.1.13** — the three-octet PLMN identity, where the `0xF`
  filler is the only thing that says how wide the MNC is
- **3GPP TS 29.274 §8.21** — the User Location Information element: flags, field
  order, and each identity's width
- **3GPP TS 32.251** — packet-switched charging; the Charging ID is unique only together with the P-GW address
- **3GPP TS 32.240** — the first partial CDR of a run is fully qualified; reduced partials carry only what changed
- **3GPP TS 32.260** — IMS charging; the ICID and the linked ICID
- **3GPP TS 32.298** — `CauseForRecClosing`, `ChangeCondition` and `ListOfServiceData`; a container closed on `tariffTimeChange` is the measured alternative to pro-rata apportionment
- **3GPP TS 23.501** — 5QI, the quality class a container's traffic ran under
- **3GPP TS 32.297** — CDR file format and transfer; the per-node file counter
- **3GPP TS 32.254** — Exposure Function Northbound API charging
- **3GPP TS 28.201 / 28.202** — network slice performance and management charging
- **ITU-T E.212** — IMSI, MCC and MNC assignment
- **ITU-T E.164** — international public telecommunication numbering
- **ITU-T E.118** — the SIM card identifier
- **ISO/IEC 7812-1 Annex B** — the Luhn check digit
- **ITU-T X.680 / X.690** — ASN.1 and the Basic Encoding Rules a record is framed in
- **IEC 80000-13** — binary and decimal multiples of the octet
- **GSMA TD.57** — TAP file sequence numbers `00001`…`99999`, the 30-day inclusion deadline, and monetary values as integers scaled by `tapDecimalPlaces`
- **GSMA TD.32** — RAP: a fatal return rejects the file, a severe return rejects individual events
- **Implementing Regulation (EU) 2016/2286** — fair use: Art. 4(4), a ≥ 4-month observation window with prevailing domestic presence *or* consumption as the safe harbour; Art. 5(3)–(4), the alert and the ≥ 2-week period before a surcharge
- **Regulation (EU) 2022/612** — Roam Like At Home; the 80 % fair-use alert; in force to 2032
- **Council Implementing Regulation (EU) 282/2011 Art. 24b(b) and 24f** — the customer of a mobile supply is presumed located in the country of the SIM's IMSI MCC, which is not where the usage happened
- **3GPP SA1 study on extra-territorial satellite coverage** — the jurisdiction question a beam crossing borders raises, still open at the standards level
- **§ 10 TDDDG** — billing traffic data, up to six months after the invoice is dispatched
- **§ 176 TKG** — retention; rewritten three times, which is why this crate ships no default
- **GDPR Art. 4(5) and EDPB Guidelines 01/2025** — pseudonymisation, and that its output remains personal data
- **GSMA TD.35** — NRTRDE, the four-hour exchange window; the roaming extract that makes fraud visible before it is paid for
- **ITU-T E.164 / national number plans** — allocation ranges nest, so a lookup resolves by longest prefix
- **GSMA FF.21** — fraud loss survey: IRSF and Wangiri as the dominant revenue-share vectors
- **RFC 1982** — serial-number arithmetic, which is how a sequence wrap is not a gap
- **GSMA TD.57** — resubmission is permitted only where the event was returned through RAP

> ℹ️ This crate implements arithmetic and classification rules derived from the
> sources listed on this page. It is **not legal advice** and carries no
> warranty of regulatory compliance — verify against the current text for your
> use case. Standards versions move; where one is not pinned it is because the
> crate targets the rule rather than a revision.
