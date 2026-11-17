# TapKit

TapKit is a headless Tableau 2026.2.4 workbook runtime and MCP server.

Version: **0.9.0**

## Target

- Tableau Server / Cloud workbook format: **26.2**
- Tableau product target: **2026.2.4**
- Tableau REST API: **3.29**
- Tableau workbook XSD contract: **2026.2.0**
- Rust: **1.88+**

TapKit does not ship a second Tableau schema validator. It keeps a small typed projection of the workbook objects required for inspection, lineage, and supported edits. Final workbook validation is delegated to Tableau REST `validateWorkbook`.

## Scope

TapKit supports:

- TWB and TWBX open/repack;
- datasource, connection, relation, field, calculation, parameter, worksheet, and dashboard indexing;
- Initial SQL and Custom SQL lineage;
- SQL Server local `#temp` lineage across Initial SQL and Custom SQL;
- calculation and parameter creation;
- basic bar, line, KPI, and text-table worksheet creation from existing datasource fields;
- dashboard creation from existing worksheets;
- full-screen HTML/CSS/JavaScript web dashboards through the bundled Dashboard Extension host;
- dashboard filter actions and parameter actions;
- dashboard layout, filter cards, and bounded presentation styling;
- Tableau workbook search and download;
- Tableau `validateWorkbook`;
- Tableau publish and background job polling;
- MCP over stdio and Streamable HTTP.

TapKit does not automate Tableau Desktop.

## Binaries

The repository builds two binaries:

- `tabkit` — CLI;
- `tabkit-mcp` — MCP server.

## Build

```bash
cargo build --release --locked
```

Run the test suite:

```bash
cargo test --release --locked --all-targets
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
```

## Windows

Install Rust with the MSVC toolchain, clone the repository, then run:

```powershell
cargo build --release --locked
cargo test --release --locked --all-targets
```

Outputs:

```text
target\release\tabkit.exe
target\release\tabkit-mcp.exe
```

CI also publishes a `tabkit-windows-x64` artifact for each successful `main` build. The archive includes the `extension/` directory with the TapKit Web Dashboard `.trex` and static host runtime.

## Tableau configuration

Set Tableau connection settings through environment variables:

```text
TABLEAU_SERVER=https://tableau.example.com
TABLEAU_SITE=site-content-url
TABLEAU_PAT_NAME=pat-name
TABLEAU_PAT_SECRET=pat-secret
```

The PAT secret is read from an environment variable. It is not accepted as a CLI argument.

## MCP

### stdio

```bash
tabkit-mcp --transport stdio
```

Local paths are allowed with stdio.

### Streamable HTTP

```bash
tabkit-mcp --transport http --bind 127.0.0.1:8000
```

Endpoint:

```text
http://127.0.0.1:8000/mcp
```

Set `TABKIT_MCP_TOKEN` to require a bearer token.

Remote HTTP does not allow arbitrary local workbook paths unless explicitly enabled.

## MCP tools

```text
tableau_search_workbooks
workbook_open
workbook_inspect
workbook_lineage
workbook_edit
workbook_validate
workbook_publish
tableau_job
workbook_close
```

Typical flow:

```text
tableau_search_workbooks
-> workbook_open
-> workbook_inspect
-> workbook_lineage
-> workbook_edit
-> workbook_validate
-> workbook_publish
-> tableau_job
-> workbook_close
```

## Validation boundary

TapKit performs local structural admission, indexing, edit preconditions, package integrity checks, stale-base checks, and post-edit reparse checks.

Tableau Server remains the authority for Tableau workbook validity:

```text
candidate TWB
-> REST 3.29 validateWorkbook
-> Tableau XSD validation
-> Tableau semantic validation
```

For TWBX, `workbook_validate` submits the embedded TWB. Package/resource acceptance is exercised by publish.

## Lineage

Lineage covers:

```text
SQL source
-> Initial SQL / Custom SQL
-> relation / datasource
-> field
-> calculation
-> worksheet
-> dashboard
```

SQL Server local temporary tables are connection-scoped:

```text
source table
-> #temp
-> Custom SQL
-> Tableau field
```

Unsupported or ambiguous paths are returned as explicit lineage gaps. TapKit does not invent missing lineage.

## Authoring

Supported edit operations:

```text
create_calculation
create_parameter
create_worksheet
create_filter_action
create_parameter_action
create_web_dashboard
create_dashboard
```

Worksheet creation is intentionally narrow: bar, line, KPI, and text-table views over existing datasource fields. Native dashboard creation supports nested rows/columns, fixed and weighted sizing, padding, gaps, text, worksheet zones, filter cards, and bounded worksheet presentation styles. `create_web_dashboard` instead emits one full-canvas Dashboard Extension object and runs a bounded HTML/CSS/JavaScript bundle in the bundled sandbox host.

Dashboard filter cards expose filters that already exist on the source worksheet. Cross-sheet interaction is available through filter actions; TapKit does not synthesize persisted `Apply to Worksheets` sharing state.

When a new object depends on another object created by TapKit, chain `workbook_edit` calls so the dependency exists on the source handle before it is referenced. See [Authoring](docs/AUTHORING.md).

## Current limits

- worksheet authoring is limited to bar, line, KPI, and text-table views over existing fields;
- web dashboards require the TapKit extension host; remote hosts must use HTTPS and Tableau may require safe-listing/full-data permission;
- arbitrary raw XML mutation is not exposed;
- action authoring is limited to filter actions and parameter actions;
- full action/set/story lineage is not implemented;
- exact source-column attribution through arbitrary SQL expressions and mutations is incomplete;
- SQL Server `##global` temp-table producer scope is not modeled;
- Tableau authentication uses PAT;
- rendering and screenshot feedback are not implemented.

## Security

Repository checks include:

- `cargo fmt`;
- Clippy with warnings denied;
- release tests on Linux and Windows;
- `cargo-deny` dependency, source, license, and advisory policy checks;
- RustSec audit;
- Dependabot for Cargo and GitHub Actions;
- `unsafe_code = "deny"`;
- denied `unwrap`, `expect`, `dbg!`, `todo!`, and `unimplemented!` in production policy, with explicit test-only allowances where required.

See [SECURITY.md](SECURITY.md).

## Documentation

Technical documentation is under [`docs/`](docs/README.md):

- [Architecture](docs/ARCHITECTURE.md)
- [Authoring](docs/AUTHORING.md)
- [Dashboard authoring](docs/DASHBOARD.md)
- [Web dashboard extension](docs/WEB_DASHBOARD.md)
- [Performance](docs/PERFORMANCE.md)
- [Rust API map](docs/API.md)
- [Error contract](docs/ERRORS.md)
- [CHANGELOG.md](CHANGELOG.md)
