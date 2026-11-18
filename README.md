# TabKit

TabKit is a headless Tableau 2026.2.4 workbook runtime and MCP server.

## Target

- Tableau Server / Cloud workbook format: **26.2**
- Tableau product target: **2026.2.4**
- Tableau REST API: **3.29**
- Tableau workbook XSD contract: **2026.2.0**
- Rust: **1.89+**

TabKit does not ship a second Tableau schema validator. It keeps a typed projection of the workbook objects required for inspection, lineage, and supported edits. Final workbook validation is delegated to Tableau REST `validateWorkbook`.

## Scope

TabKit supports:

- TWB and TWBX open/repack;
- datasource, connection, relation, field, calculation, parameter, worksheet, and dashboard indexing;
- Initial SQL and Custom SQL lineage;
- SQL Server local `#temp` lineage across Initial SQL and Custom SQL;
- calculation and parameter creation;
- bar, line, KPI, and text-table worksheet creation from existing datasource fields;
- dashboard creation from existing worksheets;
- full-screen HTML/CSS/JavaScript web dashboards through the signed, standalone Dashboard Extension host;
- dashboard filter actions and parameter actions;
- dashboard layout, filter cards, and supported worksheet presentation styles;
- Tableau workbook search and download;
- Tableau `validateWorkbook`;
- Tableau publish and background job polling;
- MCP over stdio and Streamable HTTP.

TabKit does not automate Tableau Desktop.

## Deployment binaries

Deployment components:

- `tabkit-mcp` — TabKit MCP server;
- `tabkit-extension-host` — runtime required by `create_web_dashboard`.

The components can be deployed independently. The extension host can run on the same Windows VM as the official Tableau MCP server; they use separate local ports.

For Windows host setup, use [Extension host deployment](docs/EXTENSION_DEPLOYMENT.md).

The `tabkit` CLI is for local development and workbook operations.

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

Deployment outputs:

```text
target\release\tabkit-mcp.exe
target\release\tabkit-extension-host.exe
```

CI and GitHub releases publish Windows x64 deployment ZIPs only:

```text
tabkit-mcp-v<version>-windows-x64.zip
tabkit-extension-host-v<version>-windows-x64.zip
```

The extension-host binary embeds its browser assets.

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

TabKit performs local structural admission, indexing, edit preconditions, package integrity checks, stale-base checks, and post-edit reparse checks.

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

Unsupported or ambiguous paths are returned as explicit lineage gaps. TabKit does not invent missing lineage.

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

`create_worksheet` supports bar, line, KPI, and text-table views over existing datasource fields. `create_dashboard` supports nested rows/columns, fixed and weighted sizing, padding, gaps, text, worksheet zones, filter cards, and the documented worksheet presentation styles. `create_web_dashboard` creates one full-canvas Dashboard Extension and stores its HTML/CSS/JavaScript bundle in the workbook.

Dashboard filter cards expose filters that already exist on the source worksheet. Cross-sheet interaction is available through filter actions; TabKit does not synthesize persisted `Apply to Worksheets` sharing state.

When a new object depends on another object created by TabKit, chain `workbook_edit` calls so the dependency exists on the source handle before it is referenced. See [Authoring](docs/AUTHORING.md).

## Current limits

- worksheet authoring is limited to bar, line, KPI, and text-table views over existing fields;
- web dashboards require `tabkit-extension-host`; remote authoring also requires `TABKIT_EXTENSION_SIGNING_KEY`, HTTPS, Tableau safe-listing, and Full Data Access for direct datasource reads;
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

Published documentation: https://agredyaev.github.io/tabkit/

Source Markdown is under [`docs/`](docs/README.md):

- [Architecture](docs/ARCHITECTURE.md)
- [Authoring](docs/AUTHORING.md)
- [Dashboard authoring](docs/DASHBOARD.md)
- [Web dashboard extension](docs/WEB_DASHBOARD.md)
- [Extension deployment](docs/EXTENSION_DEPLOYMENT.md)
- [Performance](docs/PERFORMANCE.md)
- [Rust API map](docs/API.md)
- [Error contract](docs/ERRORS.md)
- [CHANGELOG.md](CHANGELOG.md)
