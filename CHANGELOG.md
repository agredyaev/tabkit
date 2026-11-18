# Changelog

## Unreleased

## 0.9.1 - 2026-11-18

### MCP

- added `workbook_create` for TabKit-owned blank TWB 26.2 handles;
- added `workbook_create -> create_web_dashboard -> validate -> publish` lifecycle coverage.

### Lineage

- resolved SQL Server `WITH ... INSERT INTO #temp` writes after CTE definitions;
- resolved chained CTE and local-temp dependencies in statement order;
- forward or missing local-temp producers now emit explicit gaps without false lineage edges;
- added bidirectional source-to-dashboard coverage for CTE and local-temp chains.

### Quality

- added package, patch, formula, signing, XML, workbook, REST-boundary, and lineage edge-case tests;
- `cargo llvm-cov --all-targets` reports 74.45% line coverage and 81.01% for `lineage.rs`;
- live Tableau REST sign-in, download, validation, publish, and job paths remain external integration boundaries.

Public package/CLI version: `0.9.1`.

## 0.9.0 - 2026-11-18

First release of the Tableau 2026.2.4 implementation.

### Runtime

- Tableau 2026.2.4 / TWB 26.2 / XSD 2026.2.0 / REST 3.29 compatibility boundary.
- Typed workbook index over TWB/TWBX with source-preserving package handling.
- MCP stdio and Streamable HTTP transports.
- Tableau REST search, download, `validateWorkbook`, publish, and job lookup.

### Lineage

- datasource/connection/relation/field/calculation/worksheet/dashboard graph.
- Initial SQL, Custom SQL, CTE, derived-query, and SQL Server local `#temp` source tracking.
- Exact field-origin precedence with explicit gaps for unresolved/unsupported provenance.

### Authoring

- calculations, including supported same-batch calculation dependencies;
- parameters;
- bar, line, KPI, and text-table worksheets;
- filter actions and parameter actions;
- native dashboards with layout, filter cards, and supported worksheet styles;
- `create_web_dashboard` for signed HTML/CSS/JavaScript Dashboard Extensions.

### Web-dashboard runtime

- standalone `tabkit-extension-host`;
- one `host.html` entry point for the extension runtime;
- HMAC-SHA256 signed workbook bundles;
- URL validation, HTML sanitization, sandbox/CSP isolation;
- bounded Tableau bridge requests and paged data reads.

### Delivery and quality

- Windows x64 deployment ZIPs for `tabkit-mcp` and `tabkit-extension-host`;
- Linux/Windows test jobs;
- format/Clippy/Rustdoc gates;
- cargo-deny, RustSec audit, Dependabot;
- mdBook + Mermaid + Rustdoc documentation on GitHub Pages.

Public package/CLI version: `0.9.0`.

Earlier releases belong to the legacy implementation and remain in Git history and existing GitHub releases.
