# Suunto MCP Server

MCP server for accessing and analyzing Suunto activity, workout, and health data.

## Quick Start

### With a Suunto API subscription key
1. Set SUUNTO_SUBSCRIPTION_KEY, SUUNTO_CLIENT_ID, SUUNTO_CLIENT_SECRET, and SUUNTO_REDIRECT_URI
2. Use `suunto_get_authorization_url` to get a browser URL
3. After the user authorizes, use `suunto_exchange_authorization_code` to store their tokens
4. Use `suunto_list_accounts` to see active accounts
5. Start querying: `suunto_list_workouts`, `suunto_get_workout`, etc.

### Without Suunto API credentials (local / no-key mode)
1. Import FIT or GPX files from your local filesystem
2. Use `suunto_import_fit_file` or `suunto_import_gpx_file` to register them
3. Parse and analyze with `suunto_parse_fit_file`, `suunto_parse_gpx_file`, or
   `suunto_parse_health_export`
4. All imported data can be assigned to an `account_id` for multi-user workflows

## Available API Areas
- **Workouts**: list, get, export FIT, parse FIT
- **24/7 Data**: activity samples, sleep, recovery, daily statistics
- **Routes**: list, get, export GPX, import GPX
- **Local Imports**: parse FIT/GPX files, Apple Health exports
- **OAuth**: manage Suunto user authorizations
- **Guides**: SuuntoPlus guide management
- **Uploads**: push FIT workouts to Suunto App
