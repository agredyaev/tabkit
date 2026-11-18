# Documentation

TabKit targets Tableau **2026.2.4**, TWB **26.2**, workbook XSD **2026.2.0**, and REST **3.29**.

## Start here

| Task | Page |
| --- | --- |
| understand runtime boundaries and lineage | [Architecture](ARCHITECTURE.md) |
| understand TWB/TWBX admission and preservation | [Workbook and package model](WORKBOOK.md) |
| configure/use MCP tools and handles | [MCP server](MCP.md) |
| understand Tableau auth/validate/publish | [Tableau REST](TABLEAU_REST.md) |
| create calculations, parameters, worksheets, actions | [Authoring](AUTHORING.md) |
| create Tableau-native dashboards | [Native dashboards](DASHBOARD.md) |
| create HTML/CSS/JavaScript dashboards | [Web dashboards](WEB_DASHBOARD.md) |
| deploy the web-dashboard host on Windows | [Extension host deployment](EXTENSION_DEPLOYMENT.md) |
| understand error codes | [Error contract](ERRORS.md) |
| inspect benchmark behavior | [Performance](PERFORMANCE.md) |
| use the Rust API | [Rust API](API.md) |

Published site: https://agredyaev.github.io/tabkit/

Repository root:

- [README](../README.md) — scope, build, deployment binaries, MCP flow;
- [SECURITY](../SECURITY.md) — security policy;
- [CHANGELOG](../CHANGELOG.md) — release history.

## Build the site locally

```bash
cargo install mdbook --version 0.5.4 --locked
cargo install mdbook-mermaid --version 0.17.1 --locked
mdbook-mermaid install .
mdbook build
```
