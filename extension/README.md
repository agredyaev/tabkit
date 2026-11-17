# TapKit Web Dashboard extension

This directory is the trusted Tableau Dashboard Extension host used by TapKit web dashboards.

## Development

Serve this directory on localhost:

```sh
python -m http.server 8765 --directory extension
```

Add `tapkit-web-dashboard.trex` to Tableau. The development manifest points to:

```text
http://localhost:8765/host.html
```

For Tableau Server, host the same files on HTTPS and replace the manifest source URL. The extension may also need to be enabled or safe-listed by the site administrator.

## Runtime contract

TapKit stores one byte-preserving base64 bundle in extension instance settings:

- `tapkit.bundle_b64`

The decoded payload is UTF-8 JSON with `html`, `css`, and `javascript` string fields.

The host creates a sandboxed iframe with scripts enabled but without same-origin or network permissions. Generated JavaScript receives a narrow `window.tapkit` API:

- `listDataSources()`
- `listLogicalTables(datasource)`
- `getLogicalTableData(datasource, table, maxRows?)`
- `listWorksheets()`
- `getSummaryData(worksheet, maxRows?)`
- `applyFilter(worksheet, field, values)`
- `clearFilter(worksheet, field)`
- `listParameters()`
- `setParameter(name, value)`
- `on(event, handler)`

The child frame cannot access `tableau.extensions` directly. Data and summary-data reads are capped at 100,000 rows per request. Direct logical-table reads require the `full data` permission declared by the manifest. Inline HTML/CSS/JavaScript is capped at 1 MiB by the runtime.

## Vendored dependency

`vendor/tableau.extensions.1.latest.min.js` is the official Tableau Extensions API runtime from `tableau/extensions-api`. It is redistributed under Apache-2.0; the upstream license is retained as `vendor/LICENSE-tableau-extensions-api.txt`.
