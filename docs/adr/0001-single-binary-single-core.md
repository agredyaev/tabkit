# ADR-001 — One binary and one application core

Status: Accepted

## Context

The product needs CLI and MCP access to the same local/remote capabilities without duplicated domain behavior or separate services.

## Decision

- Ship one Rust `tabkit` binary.
- Keep the base runtime self-contained; Python remains developer tooling only.
- Route CLI and MCP commands through `App::dispatch`.
- Keep MCP on stdio.
- Do not introduce a daemon, service container or second business-logic stack.

## Consequences

- CLI/MCP differences are transport/presentation concerns only.
- A behavior fix in the application core applies to both interfaces.
- Long-lived server infrastructure is outside v1.

## Implementation

- [src/main.rs](../../src/main.rs)
- [src/app.rs](../../src/app.rs)
- [src/mcp.rs](../../src/mcp.rs)
- [src/wire.rs](../../src/wire.rs)
