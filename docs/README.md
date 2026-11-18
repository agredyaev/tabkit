# Documentation

Technical documentation for TapKit.

- [Architecture](ARCHITECTURE.md) — runtime boundaries, workbook model, validation boundary, MCP and REST flow.
- [Authoring](AUTHORING.md) — semantic edit operations, parameters, worksheet creation, actions, sequencing, and limits.
- [Dashboard authoring](DASHBOARD.md) — supported native dashboard layout, filter cards, presentation controls, and limits.
- [Web dashboard extension](WEB_DASHBOARD.md) — single-extension dashboards, signed runtime bundle, sandbox bridge, and security boundary.
- [Extension deployment](EXTENSION_DEPLOYMENT.md) — Windows/Linux host deployment, signing key, HTTPS proxy, Tableau safe-list, and troubleshooting.
- [Performance](PERFORMANCE.md) — benchmark method, current measurements, and scaling notes.
- [Rust API map](API.md) — source modules, public entry points, and ownership boundaries.
- [Error contract](ERRORS.md) — stable error codes, MCP mapping, REST error handling, and lineage gaps.

Repository-level files remain in the project root:

- `README.md` — build, run, MCP tools, supported scope, and current limits.
- `CHANGELOG.md` — release changes.
- `SECURITY.md` — vulnerability reporting policy.
