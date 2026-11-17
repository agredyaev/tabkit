# Changelog

## 0.9.0 - 2026-11-17

- Replaced the legacy implementation with the current headless Tableau 2026.2.4 runtime/compiler.
- Added MCP stdio and Streamable HTTP transports through `rmcp`.
- Added typed workbook inspection and bounded lineage.
- Added Initial SQL, Custom SQL, CTE, derived-query, and SQL Server local `#temp` lineage.
- Added calculation authoring, including same-batch dependencies.
- Added dashboard authoring from existing worksheets, filter cards, layout, and presentation styles.
- Added source-preserving TWB/TWBX edits and package preservation checks.
- Added Tableau REST 3.29 search, download, `validateWorkbook`, publish, and job polling.
- Added Windows/Linux/macOS CI, cargo-deny, RustSec audit, Dependabot, and release packaging.
- Set the public package and CLI version to `0.9.0`.

Earlier releases belong to the legacy implementation and remain available in Git history and existing GitHub releases.
