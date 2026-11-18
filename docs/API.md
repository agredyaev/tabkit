# Rust API map

The public Rust API is organized by responsibility rather than by Tableau XML element.

## Core modules

| Module | Responsibility | Main public entry points |
| --- | --- | --- |
| `workbook` | Parse TWB XML into the typed semantic index used by inspection, lineage, and edits | `Workbook::open`, `Workbook::summary`, lookup methods, `inspect_limited` |
| `xml` | Source-backed XML index with stable byte spans and preorder subtree ranges | `Document::parse`, `Document::descendants`, `Span` |
| `package` | TWB/TWBX admission, bounded ZIP handling, stale-base checks, source-preserving repack | `Package::open`, `Package::write_candidate`, `Limits` |
| `formula` | Tableau formula reference extraction; not an expression evaluator | `analyze`, `qualified` |
| `lineage` | Build and query the semantic dependency graph | `Lineage::build`, `find_kind`, `trace_limited` |
| `edit` | Typed calculation, parameter, worksheet, action, native-dashboard, and web-dashboard authoring lowered into source-preserving XML patches | `Batch`, `Edit`, `apply`, `Candidate` |
| `patch` | Admit, apply, and verify non-overlapping byte patches | `Patch`, `apply` |
| `rest` | Tableau REST 3.29 authentication, search, download, validation, publish, and job polling | `Config`, `Rest` |
| `mcp` | MCP server state and tool handlers built on `rmcp` | `TabKitMcp` |

## Main data flow

```text
TWB/TWBX
  -> package admission
  -> XML parse/index
  -> Workbook typed index
  -> Lineage / inspection

Workbook + typed edit batch
  -> semantic dependency/precondition checks
  -> bounded source-preserving patches
  -> candidate reparse
  -> candidate TWB/TWBX
  -> Tableau validateWorkbook
  -> publish
```

## Validation ownership

TabKit does not maintain a second Tableau XSD validator.

Local code is responsible for:
- package and size limits;
- XML parsing and supported-version admission;
- semantic edit preconditions and supported authoring bounds;
- patch preservation;
- candidate reparse;
- explicit lineage gaps where provenance cannot be proven.

Tableau Server is responsible for:
- XSD validation through REST `validateWorkbook`;
- Tableau semantic validation;
- final packaged-workbook acceptance during publish.

## Public version constants

Defined in `src/lib.rs`:

```text
TABLEAU_PRODUCT_TARGET = 2026.2.4
TWB_VERSION            = 26.2
TWB_XSD_VERSION        = 2026.2.0
REST_API_VERSION       = 3.29
```

These values are treated as one compatibility boundary.

## Rustdoc policy

Rustdoc documents contracts that are not recoverable from a signature: invariants, ownership/lifetime rules, bounds, validation authority, failure semantics, and postconditions. It should not restate a function or field name in prose.

Rustdoc must not claim that the semantic index is a complete Tableau object model, that local checks replace `validateWorkbook`, or that workbook-local/node-local IDs survive reparsing.

CI builds Rustdoc with warnings denied so broken links and other Rustdoc diagnostics fail the quality job.


## Generated API reference

The generated Rust API reference is published inside the TabKit documentation site:

https://agredyaev.github.io/tabkit/api/tabkit/

It is generated from the Rust doc comments in `src/` by `cargo doc`.
