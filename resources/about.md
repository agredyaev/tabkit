# tabkit scope
One application, CLI and MCP stdio, for selected Tableau Server 2025 REST journeys and local TWB/TWBX inspection and bounded edits. Start with system_status. Read operator policy; missing Hyper, data output or publishing is an explicit capability limit, not permission to substitute shell commands.

This is not full Tableau plugin parity. Supported edits change existing global calculations, static parameters and simple filters. No arbitrary XML, field creation/deletion/rename, layouts, dynamic parameters or admin features. Local validation distinguishes known invalid content from preserved unsupported constructs. It never certifies Tableau execution. Read-only lineage covers named calculation, shelf, filter, worksheet and dashboard routes and reports gaps; it is not complete Tableau lineage.

Base build needs no Hyper SDK. A Hyper-enabled build also requires the official native runtime and libraries; it is not one physical self-contained file. AWS Quick handles model execution independently; return only authorized minimal content to the model.
