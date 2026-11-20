# Changelog

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Pre-1.0: a minor
release may break the API; each one lists its migrations.

## [0.3.0] - 2026-11-20

### Added

- **Fixed-line voice.** `dialplan` module: `DialContext` (`germany()`, `italy()`, `nanp()`,
  builders for other plans), `Destination`, `ShortCode`, `DestinationClass`, `NumberingPlan`,
  `GermanNumberingPlan` (BNetzA, as of 2025-11-06). `UsageRecord::call`, `CallDirection`,
  `calling` / `called` fields, `CallId`, `RecordKey::Call`.
- Wireline SUPIs: `Supi::GlobalLine` (`gli-`), `Supi::GlobalCable` (`gci-`).
- § 11 TDDDG itemisation: `itemise_called`, `itemise_calling`, `Itemisation`,
  `ItemisedDestination`, `ShortenedNumber`, `ProtectedDestinations`.
- `Mcc::same_country` (ITU-T E.212 multi-MCC areas).
- `RetentionPolicy::billing_tdddg10`, `ObservationWindow::months`,
  `SequenceTracker::observe_all`, `DedupStore::refresh` / `remove`, `score_dispositions`,
  `NumberPrefix::without_last`, `ParseError::is_withheld`.

### Fixed

- **Double billing and lost usage across batches**: dedup keys are stamped on arrival and
  refreshed while a record is held; rejected records no longer consume their key; partial runs
  missing their first record are held (V20) instead of rejected; partial keys use the partial
  sequence; originating and terminating call records no longer share a key; sequence gaps are
  order-independent, late fills close their hole, counter regressions raise V18.
- **Panics on input**: timestamps at the year range edge, centuries, BER offsets, retention and
  lateness durations from serde, non-ASCII pseudonyms.
- **Wire decoding**: IMEI wire form with spare digit; long-form BER lengths; `Z` / `-00:00`
  refused (RFC 9557); `KB` refused; E.212 addresses never become MSISDNs; NAI ≤ 253 octets.
- **Countries, not MCCs** in jurisdiction and impossible travel; VAT presumption only for a
  subscriber on radio access.
- **Regulatory arithmetic**: a day at home counts as domestic presence only (EU 2016/2286
  Art. 4(4)); § 10 TDDDG periods capped at six months; dispute holds have no deadline;
  `split_measured` across rating groups; wangiri per subscriber within a window; AIT on
  originated SMS only.
- Release gate ignores findings on records already rejected or held; V09 reported once.

### Changed

- Releases publish by crates.io Trusted Publishing; CI builds `wasm32-unknown-unknown`.

### Migration

| Before | After |
|---|---|
| `RecordType::MobileOriginatedCall` / `MobileTerminatedCall` (`MO_CALL` / `MT_CALL`) | `OriginatedCall` / `TerminatedCall` (`ORIGINATED_CALL` / `TERMINATED_CALL`) |
| `RecordType::FixedSession`, `ServiceDomain::Fixed` | `FixedAccessSession`, `ServiceDomain::FixedAccess` |
| `identity::NumberingPlan` | `NumberingPlanIndicator` |
| `fraud::CalledParty` and the resolver argument | set `UsageRecord::called` |
| `wangiri_pattern(records, ring, min)` | `wangiri_pattern(records, ring, min, within)` → `Vec<Indication>` |
| `CdrTimestamp::new`, `CdrTimestamp::from_utc` | return `Option` (UTC years 1–9998) |
| `Century::year` | returns `Option<i32>`; `Century::new` takes multiples of 100 in 0–9900 |
| `Suci` public fields | accessors; scheme is one hex digit |
| `ObservationWindow::new(days)`, `days()` | `ObservationWindow::months(n)`, `month_count()`; `start()` returns `Option` |
| `FairUseAssessment::from_counts(…)` | gains `window_start` |
| `LatenessPolicy::late_at` | returns `Option` |
| `RetentionPolicy::new` | refuses dated dispute holds and § 10 periods over six months |
| `DialContext::new(cc, intl, trunk, lead)` | `DialContext::new(cc, intl)?` plus builders |
| `RecordKey::Call { entity, call_id, sequence }` | gains `record_type` |
| `OrphanReason::FirstRecordMissing` | removed — such runs are held as `Incomplete` |
| `DedupWindow::from_bound(<1 s)` | `None` |
| `Imei::to_tbcd` | writes the spare digit 0, not the check digit |
| `"KB"`, `"Z"`, `"-00:00"` | refused |
