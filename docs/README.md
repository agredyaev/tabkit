# Documentation

Technical documentation for TabKit.

Published site: https://agredyaev.github.io/tabkit/

- [Architecture](ARCHITECTURE.md) — runtime boundaries, workbook model, validation boundary, MCP and REST flow.
- [Authoring](AUTHORING.md) — semantic edit operations, parameters, worksheet creation, actions, sequencing, and limits.
- [Dashboard authoring](DASHBOARD.md) — supported native dashboard layout, filter cards, presentation controls, and limits.
- [Web dashboard extension](WEB_DASHBOARD.md) — single-extension dashboards, signed runtime bundle, sandbox bridge, and security boundary.
- [Extension deployment](EXTENSION_DEPLOYMENT.md) — Windows host startup, signing key, HTTPS route, Tableau extension allowlist, and acceptance checks.
- [Performance](PERFORMANCE.md) — benchmark method, current measurements, and scaling notes.
- [Rust API](API.md) — source modules, public entry points, and ownership boundaries.
- [Error contract](ERRORS.md) — stable error codes, MCP mapping, REST error handling, and lineage gaps.

Repository-level files remain in the project root:

- `README.md` — build, run, MCP tools, supported scope, and current limits.
- `CHANGELOG.md` — release changes.
- `SECURITY.md` — vulnerability reporting policy.

## Local documentation build

The published site uses mdBook 0.5.4 and mdbook-mermaid 0.17.1.

```bash
cargo install mdbook --version 0.5.4 --locked
cargo install mdbook-mermaid --version 0.17.1 --locked
mdbook-mermaid install .
mdbook build
```

`mdbook-mermaid install .` writes the Mermaid browser assets referenced by `book.toml`.
