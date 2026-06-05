# Changelog

All notable changes to this project will be documented here.

## Unreleased

### Fixed

- `/247samples/*` endpoints (sleep, activity, recovery) now send `from`/`to` as
  Unix millisecond timestamps instead of ISO 8601 strings, fixing HTTP 400
  "Invalid parameter 'from': Type mismatch" errors from the Suunto Cloud API.
- Hardened upload validation, local storage paths, webhook payload handling,
  and binary export security.

## 0.1.0 - 2026-06-03

Initial public alpha release.

- Added Suunto OAuth account tooling.
- Added workout, FIT, route, 24/7 activity, sleep, recovery, and daily statistics
  tools.
- Added local no-key parsing/imports for FIT, GPX, JSON, CSV/TSV, and Apple
  Health exports.
- Added gated route import, workout upload, guide mutation, and workout add-info
  write tools.
- Added webhook ingestion routes, optional HMAC verification, local event storage,
  and follow-up suggestions.
- Added local API rate and weekly quota guards.
- Added CI, tests, typing checks, release metadata, security notes, and setup docs.
