# MCP Tool Reference

This page lists the tools exposed by Suunto MCP. The README stays focused on
setup and examples; this file is the reference surface for MCP clients, reviewers,
and contributors.

## Access Levels

- **Suunto API**: requires Suunto API credentials, a subscription key, and a
  stored account token.
- **Local/no-key**: works with local files and does not require Suunto API
  credentials.
- **Write-gated**: mutates Suunto data or uploads content. These tools require
  `SUUNTO_ENABLE_WRITE_TOOLS=true`.

## OAuth And Accounts

Suunto API account setup and token management.

| Tool | Access | Description |
| --- | --- | --- |
| `suunto_get_authorization_url` | Suunto API | Build the Suunto OAuth browser authorization URL. |
| `suunto_exchange_authorization_code` | Suunto API | Exchange a Suunto OAuth authorization code for tokens and store them locally. |
| `suunto_import_token_set` | Suunto API | Store an existing Suunto access token and optional refresh token without exposing token values. |
| `suunto_import_token_file` | Suunto API | Import a token JSON response file created by a trusted OAuth test flow. |
| `suunto_refresh_token` | Suunto API | Refresh one stored Suunto account, or all accounts that have refresh tokens. |
| `suunto_list_accounts` | Local token store | List stored Suunto accounts without secrets. |
| `suunto_auth_status` | Local token store | Report whether stored account tokens are usable. |
| `suunto_deauthorize` | Suunto API | Call Suunto OAuth deauthorize for an account and optionally remove local tokens. |

## Workouts

Read workout history, fetch FIT files, and parse workout details.

| Tool | Access | Description |
| --- | --- | --- |
| `suunto_list_workouts` | Suunto API | List workouts for an authorized Suunto account using the current `/v3/workouts` API. |
| `suunto_get_workout` | Suunto API | Fetch one Suunto workout JSON document. |
| `suunto_get_workout_fit` | Suunto API | Download a workout FIT file as metadata, base64, file, or both. |
| `suunto_parse_workout_fit` | Suunto API | Download a workout FIT file and parse sessions, laps, events, and optional records. |
| `suunto_get_complete_workout` | Suunto API | Fetch workout JSON plus optional FIT metadata and parsed FIT summary. |
| `suunto_add_workout_description_info` | Write-gated | Add information to a workout description. |
| `suunto_list_workouts_deprecated_v2` | Suunto API | List workouts using Suunto's deprecated `/v2/workouts` API. Prefer `suunto_list_workouts`. |
| `suunto_get_workout_deprecated_v2` | Suunto API | Fetch one workout using Suunto's deprecated `/v2/workouts` API. Prefer `suunto_get_workout`. |
| `suunto_get_workout_fit_deprecated_v2` | Suunto API | Download a FIT file using Suunto's deprecated `/v2/workouts` API. Prefer `suunto_get_workout_fit`. |

## 24/7 Activity, Sleep, And Recovery

Read Suunto 24/7 data. The summary tools help plan and chunk date ranges so API
quota usage is visible before broad fetches.

| Tool | Access | Description |
| --- | --- | --- |
| `suunto_get_activity_samples` | Suunto API | Fetch 24/7 activity samples for a Suunto account. |
| `suunto_get_sleep_data` | Suunto API | Fetch 24/7 sleep samples where available for the account/API subscription. |
| `suunto_get_recovery_data` | Suunto API | Fetch 24/7 recovery samples where available for the account/API subscription. |
| `suunto_get_daily_activity_statistics` | Suunto API | Fetch daily activity statistics for a date range. |
| `suunto_estimate_247_summary_calls` | Local quota planner | Estimate Suunto API calls for a 24/7 summary without fetching data. |
| `suunto_get_247_summary` | Suunto API | Fetch activity, sleep, recovery, and daily statistics using 28-day chunks. |
| `suunto_get_legacy_daily_activity` | Suunto API | Fetch the legacy daily activity API surface when enabled for the subscription. |
| `suunto_get_legacy_daily_activity_statistics` | Suunto API | Fetch legacy daily activity statistics when enabled for the subscription. |

## Routes

List Suunto routes, export GPX, parse GPX, and optionally import routes.

| Tool | Access | Description |
| --- | --- | --- |
| `suunto_list_routes` | Suunto API | List routes for an authorized Suunto account. |
| `suunto_get_route` | Suunto API | Fetch one route metadata document by route id. |
| `suunto_export_route_gpx` | Suunto API | Export a Suunto route as GPX. |
| `suunto_parse_route_gpx` | Suunto API | Export a Suunto route as GPX and parse tracks, routes, and waypoints. |
| `suunto_import_route_gpx` | Write-gated | Import a local GPX route into Suunto App. |

## Workout Uploads

Initialize and upload FIT workout files to Suunto App. Signed upload URLs are
redacted in tool responses where possible.

| Tool | Access | Description |
| --- | --- | --- |
| `suunto_init_workout_upload` | Write-gated | Initialize a Suunto FIT workout upload. |
| `suunto_upload_workout_fit_to_signed_url` | Write-gated | Upload a FIT file to the signed object-storage URL returned by `suunto_init_workout_upload`. |
| `suunto_get_upload_status` | Suunto API | Fetch Suunto workout upload status. |
| `suunto_wait_for_upload_status` | Suunto API | Poll Suunto workout upload status until a terminal state or attempt limit. |
| `suunto_upload_workout_fit` | Write-gated | Initialize and upload a FIT file to Suunto App. |

## SuuntoPlus Guides

Read, validate, package, and optionally mutate SuuntoPlus Guides.

| Tool | Access | Description |
| --- | --- | --- |
| `suunto_list_guides` | Suunto API | List SuuntoPlus Guides for an account. |
| `suunto_get_guide` | Suunto API | Fetch a SuuntoPlus Guide ZIP/file by id. |
| `suunto_validate_guide_json` | Local/no-key | Validate a SuuntoPlus guide JSON object or `guide.json` file before upload. |
| `suunto_create_guide_zip` | Local/no-key | Create a local SuuntoPlus guide ZIP from `guide.json` and `icon.png`. |
| `suunto_create_guide` | Write-gated | Upload a SuuntoPlus guide ZIP. |
| `suunto_update_guide` | Write-gated | Replace a SuuntoPlus guide ZIP. |
| `suunto_delete_guide` | Write-gated | Delete a SuuntoPlus Guide. |

## Local Imports

Local import tools are supporting utilities for files you already have on disk.
They do not connect to Apple Health, iCloud, or the Health app directly; Apple
Health support means parsing a local `export.xml` file.

| Tool | Access | Description |
| --- | --- | --- |
| `suunto_parse_fit_file` | Local/no-key | Parse a local FIT file without storing it. |
| `suunto_parse_gpx_file` | Local/no-key | Parse a local GPX file without storing it. |
| `suunto_import_fit_file` | Local/no-key | Register a local FIT file and cache a parsed summary in the local import index. |
| `suunto_import_gpx_file` | Local/no-key | Register a local GPX route/track file and cache a parsed summary. |
| `suunto_import_activity_json` | Local/no-key | Register a local normalized activity JSON file. |
| `suunto_parse_activity_csv` | Local/no-key | Parse a local CSV/TSV activity file without storing it. |
| `suunto_import_activity_csv` | Local/no-key | Register a local CSV/TSV activity file and cache a bounded preview summary. |
| `suunto_parse_health_export` | Local/no-key | Parse a local Apple Health `export.xml` file without storing it. |
| `suunto_import_health_export` | Local/no-key | Register a local Apple Health `export.xml` file and cache a bounded summary. |
| `suunto_list_imported_files` | Local/no-key | List locally imported files. |
| `suunto_get_imported_activity` | Local/no-key | Inspect and parse an imported file. |

## Webhook Tools

These tools manage webhook payload verification and local event storage. HTTP
webhook ingestion routes are listed below.

| Tool | Access | Description |
| --- | --- | --- |
| `suunto_verify_webhook_signature` | Local/no-key | Verify a Suunto webhook HMAC SHA-256 signature. |
| `suunto_store_webhook_event` | Local/no-key | Store a webhook notification payload in the configured local webhook store. |
| `suunto_list_webhook_events` | Local/no-key | List locally stored webhook events. |
| `suunto_get_webhook_event` | Local/no-key | Get one locally stored webhook event by id. |
| `suunto_delete_webhook_event` | Local/no-key | Delete one locally stored webhook event by id. |
| `suunto_suggest_webhook_followups` | Local/no-key | Suggest MCP fetch tools to call after receiving a Suunto webhook event. |

## Safety And Quota

| Tool | Access | Description |
| --- | --- | --- |
| `suunto_api_quota_status` | Local/no-key | Inspect local Suunto API quota usage. |

## Resources

| Resource | Description |
| --- | --- |
| `suunto://config` | Redacted runtime configuration. |
| `suunto://api-catalog` | Tool groups exposed by the server. |
| `suunto://coverage` | Suunto API coverage map for implemented tools and routes. |
| `suunto://quota` | Local API quota ledger status. |

## Webhook Endpoints

These endpoints are available when the server is running over an HTTP transport.

| Endpoint | Description |
| --- | --- |
| `POST /webhooks/suunto` | Generic JSON Suunto webhook ingestion. |
| `POST /webhooks/suunto/workout` | Workout notification ingestion. |
| `POST /webhooks/suunto/route` | Route notification ingestion. |
| `POST /webhooks/suunto/247/activity` | 24/7 activity notification ingestion. |
| `POST /webhooks/suunto/247/sleep` | 24/7 sleep notification ingestion. |
| `POST /webhooks/suunto/247/recovery` | 24/7 recovery notification ingestion. |
| `POST /webhooks/suunto/legacy-workout` | Legacy workout form notification ingestion. |

When `SUUNTO_WEBHOOK_SECRET` is configured, webhook routes require a valid
`X-HMAC-SHA256-Signature` header.
