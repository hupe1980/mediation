+++
title = "Design constraints and testing"
description = "Zero I/O, no async, no clock, exact integers, no unsafe: how each constraint is enforced and tested."
weight = 170
+++

Each constraint is enforced by something that fails the build.

## Design constraints

| Constraint | Enforced by |
|---|---|
| 🧊 **No I/O**, 🕰️ **no clock** | `just purity` greps `src/`; evaluation instants are parameters |
| 🔢 **Exact quantities** | `just no-floats`; octets are `u64`, durations `u32` |
| 🎯 **Deterministic** | property tests: same input, same output, in any order |
| ♻️ **Idempotent** | a record fed twice gives one billable outcome and one duplicate |
| 🧱 **Immutable records** | a correction is a new record or a `Restatement` |
| 🛡️ **Total on untrusted input** | `tests/robustness.rs`: no parser or decoder panics, at any input |
| 🔒 **No `unsafe`** | `#![deny(unsafe_code)]` |
| 🪶 **Small dependency set** | `time`, `thiserror`; `serde` optional |

A decoder that panics on a partner's malformed field is a denial of service, so
every `FromStr` and decoder returns a `Result`. Anything sized by the input is
bounded (`SequenceTracker::observe`, `PartialRun::assess`, BER nesting depth)
while counts stay exact, and nothing is skipped silently: an end-of-contents
marker inside a definite-length structure is an error.

A clock read would make two values built from identical inputs unequal and an
archived batch replay to today's verdicts. Callers pass the instant in.

## Testing

```bash
just ci     # fmt-check, clippy, purity, no-floats, tests, docs, wasm, site check, package
```

The integration suites under
[`tests/`](https://github.com/hupe1980/mediation/tree/main/tests) cover each
module against published examples, and `proptest` checks the three promises:
**determinism**, **conservation** (no record created or destroyed) and
**totality** (no panic). `serde_representation.rs` pins every serialised tag and
field name.

Every Rust block in the README and on this site runs as a doctest
(`ReadmeDoctests` and `site_doctests` in
[`src/lib.rs`](https://github.com/hupe1980/mediation/blob/main/src/lib.rs)), so a
page naming an API the crate does not have fails the build.

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
- **3GPP TS 32.297** — CDR file format and transfer; the per-node file counter
- **3GPP TS 32.254** — Exposure Function Northbound API charging
- **3GPP TS 28.201 / 28.202** — network slice performance and management charging
- **ITU-T E.212** — IMSI, MCC and MNC assignment; geographic areas holding more than one MCC
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
- **§ 10 TDDDG** — billing traffic data, up to six months after the invoice is dispatched
- **§ 11 TDDDG** — the itemised bill: shortened destinations, omitted counselling numbers
- **RFC 9557 §2.2** — `Z` and `-00:00` mean an unknown local offset, so a CDR timestamp refuses them
- **§ 176 TKG** — retention; the crate ships no default period
- **GDPR Art. 4(5) and EDPB Guidelines 01/2025** — pseudonymisation, and that its output remains personal data
- **GSMA TD.35** — NRTRDE, the four-hour exchange window; the roaming extract that makes fraud visible before it is paid for
- **ITU-T E.164 / national number plans** — allocation ranges nest, so a lookup resolves by longest prefix
- **GSMA FF.21** — fraud loss survey: IRSF and Wangiri as the dominant revenue-share vectors
- **RFC 1982** — serial-number arithmetic, which is how a sequence wrap is not a gap

> ℹ️ Rules derived from the sources above. Not legal advice; verify against the
> current text for your use case.
