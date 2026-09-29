# tabkit

[![CI](https://img.shields.io/github/actions/workflow/status/agredyaev/tabkit/ci.yml?branch=dev&label=CI&logo=githubactions&logoColor=white&style=for-the-badge)](https://github.com/agredyaev/tabkit/actions/workflows/ci.yml)
[![Rust 1.88+](https://img.shields.io/badge/Rust-1.88%2B-EF6C00?logo=rust&logoColor=white&style=for-the-badge)](Cargo.toml)
[![MIT License](https://img.shields.io/badge/License-MIT-3949AB?logo=opensourceinitiative&logoColor=white&style=for-the-badge)](LICENSE)
[![Security policy](https://img.shields.io/badge/Security-Policy-00897B?style=for-the-badge)](SECURITY.md)

`tabkit` is a Rust CLI and MCP server for working with Tableau 2025 workbooks. It reads `.twb` and `.twbx` files from a chosen workspace, exposes their known structure, and makes supported edits through a reviewable plan. The CLI and MCP server use the same tools.

## What it does

- Inspect fields, calculations, parameters, filters, sheets, dependencies, and package contents.
- Navigate a read-only dependency graph from source fields through calculations, Rows, Columns, Marks and filters to worksheets and dashboards. Export every known link as JSON with explicit coverage gaps.
- Validate known workbook rules, compare workbooks, and run declarative assertions.
- Plan and apply typed calculation, parameter, and filter edits to a **new** workbook file while preserving unrelated content.

## Optional integrations

- **Tableau REST:** Configure a Tableau server and OAuth or PAT credentials to search and download workbooks. Publishing requires `--publish-enabled`, an allowed project, and separate prepare and confirm calls.
- **Hyper:** The standard build can extract `.hyper` files from `.twbx` packages. Read-only queries require a build with `--features hyper` and the official Hyper SDK/runtime.

## Try it locally

Requires Rust 1.88 or newer. The workspace must already exist and be given as an absolute path.

```sh
cargo build --release --locked
./target/release/tabkit --workspace "$(pwd)/examples" inspect synthetic.twb
./target/release/tabkit --workspace "$(pwd)/examples" validate synthetic.twb
```

On Windows, run `target\release\tabkit.exe` with an absolute Windows workspace path. Run `tabkit --workspace <absolute-path> tools` to see the available tools and their JSON schemas. Startup options go before the subcommand.

To use MCP, configure your client to launch `tabkit --workspace <absolute-path> mcp` over stdio. See the [MCP configuration examples](examples/quick-mcp.example.json) for Tableau OAuth and [PAT](examples/quick-mcp-pat.example.json). For local workbook work, Tableau credentials are unnecessary.

For lineage, call `workbook_lineage_open` once per MCP session, then `workbook_lineage_find`, `workbook_lineage_neighbors`, or the paged `workbook_lineage_impact`. Links point from an input to what depends on it; `upstream` reverses the navigation. Field nodes include their formulas; worksheet-local fields carry their worksheet and are read-only edit targets. Sort bindings, extract filters, and ordinary datasource filters have distinct links. Shared-view filters show their field, with unknown worksheet effects marked as gaps. Check `workbook_lineage_gaps` before concluding a path is absent. `workbook_lineage_export` writes the complete known graph and coverage gaps to a new JSON file. In a one-shot CLI call, export accepts `input` instead of a session `snapshot_id`. Global `field` node IDs match `workbook_inspect` field IDs used for edits; `local_field` IDs do not. `workbook_inspect` with `section=datasources` reports connection metadata, Initial SQL and Custom SQL without exposing raw XML; it never executes SQL.

For edits, copy a workbook into a separate workspace, inspect it, then call `workbook_plan` and `workbook_apply` with reviewed JSON arguments. See the [plan](examples/plan.json) and [apply](examples/apply.template.json) examples. The source workbook is never overwritten.

## Scope

Local validation checks XML and the Tableau structures `tabkit` understands; it does not execute Tableau or prove full workbook semantics. Lineage covers known global and worksheet-local calculations, Rows/Columns/Marks and sort bindings, worksheet, datasource and extract filters, worksheets and dashboards. Unresolved references and unmodeled constructs are reported as coverage gaps, never silently treated as absent. Unsupported edit shapes are rejected. Live Tableau, Amazon Quick Desktop, and native Hyper integration still require environment qualification.

Run `cargo test --locked` for the local test suite. See the [architecture decisions](docs/adr/README.md) for the supported scope.
