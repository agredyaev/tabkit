# tabkit

`tabkit` is a Rust CLI and MCP server for working with Tableau 2025 workbooks. It reads `.twb` and `.twbx` files from a chosen workspace, exposes their known structure, and makes supported edits through a reviewable plan. The CLI and MCP server use the same tools.

## What it does

- Inspect fields, calculations, parameters, filters, sheets, dependencies, and package contents.
- Validate known workbook rules, compare workbooks, and run declarative assertions.
- Plan and apply typed calculation, parameter, and filter edits to a **new** workbook file while preserving unrelated content.
- Optionally connect to Tableau REST for discovery, download, and explicitly approved publishing.
- Optionally inspect Hyper extracts with the native SDK; the default build needs no Hyper runtime.

## Try it locally

Requires Rust 1.88 or newer. The workspace must already exist and be given as an absolute path.

```sh
cargo build --release --locked
./target/release/tabkit --workspace "$(pwd)/examples" inspect synthetic.twb
./target/release/tabkit --workspace "$(pwd)/examples" validate synthetic.twb
```

On Windows, run `target\release\tabkit.exe` with an absolute Windows workspace path. Run `tabkit --workspace <absolute-path> tools` to see the available tools and their JSON schemas. Startup options go before the subcommand.

To use MCP, configure your client to launch `tabkit --workspace <absolute-path> mcp` over stdio. See the [MCP configuration examples](examples/quick-mcp.example.json) for Tableau OAuth and [PAT](examples/quick-mcp-pat.example.json). For local workbook work, Tableau credentials are unnecessary.

For edits, copy a workbook into a separate workspace, inspect it, then call `workbook_plan` and `workbook_apply` with reviewed JSON arguments. See the [plan](examples/plan.json) and [apply](examples/apply.template.json) examples. The source workbook is never overwritten.

## Scope

Local validation checks XML and the Tableau structures `tabkit` understands; it does not execute Tableau or prove full workbook semantics. Unsupported edit shapes are rejected. Publishing is disabled unless explicitly enabled at startup. Live Tableau, Amazon Quick Desktop, and native Hyper integration still require environment qualification.

Run `cargo test --locked` for the local test suite. See the [architecture decisions](docs/adr/README.md) for the supported scope.
