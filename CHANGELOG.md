# Changelog

## 0.9.0 - 2026-11-17

- Replaced the legacy implementation with the current headless Tableau 2026.2.4 runtime/compiler.
- Added MCP stdio and Streamable HTTP transports through `rmcp`.
- Added typed workbook inspection and bounded lineage.
- Added Initial SQL, Custom SQL, CTE, derived-query, and SQL Server local `#temp` lineage.
- Added calculation authoring, including same-batch dependencies.
- Added parameter authoring and parameter indexing.
- Added bounded bar, line, KPI, and text-table worksheet authoring from existing datasource fields.
- Added dashboard filter actions and parameter actions.
- Added dashboard authoring from existing worksheets, filter cards, layout, and presentation styles.
- Added `create_web_dashboard` and a standalone `tabkit-extension-host` for HTML/CSS/JavaScript dashboards.
- Added stable `host.html` runtime versioning, HMAC-SHA256 bundle signing, stricter URL validation, HTML sanitization, bridge quotas, and paged data reads for web dashboards.
- Added Windows x64 deployment ZIPs for `tabkit-mcp` and `tabkit-extension-host` using `<component>-v<version>-windows-x64.zip` names.
- Added source-preserving TWB/TWBX edits and package preservation checks.
- Added Tableau REST 3.29 search, download, `validateWorkbook`, publish, and job polling.
- Added Windows/Linux test coverage, cargo-deny, RustSec audit, Dependabot, and Windows-only deployment packaging.
- Set the public package and CLI version to `0.9.0`.

Earlier releases belong to the legacy implementation and remain available in Git history and existing GitHub releases.
