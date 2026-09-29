# ADR-007 — Optional isolated read-only Hyper capability

Status: Accepted

## Context

Hyper access requires Tableau's native SDK/runtime. It remains optional and read-only; local workbook editing must not depend on it.

## Decision

- Keep Hyper behind an optional build feature.
- Call the official C++ SDK through the project's small C ABI bridge; the SDK has no supported C API.
- Base binary must run without the native Hyper SDK.
- Extract Hyper members explicitly from TWBX.
- Admit only bounded read-only SQL.
- Reject multiple statements and side-effecting statements.
- Snapshot/hash-check the source around query execution.
- Run native work behind the existing worker/process boundary and cancellation checks.

## Consequences

- Native Hyper must be qualified separately with the official SDK/runtime.
- The Windows release includes the official runtime as DLL and process sidecars; its packaged CLI and MCP paths are smoke-tested with a real extract.
- Generic write SQL is outside v1.
- SQL parser behavior is part of the admission boundary and receives resource-regression tests.

## Implementation

- [src/integrations/hyper.rs](../../src/integrations/hyper.rs)
- [src/integrations/sql.rs](../../src/integrations/sql.rs)
- [src/package.rs](../../src/workbook/package.rs)
- [Cargo.toml](../../Cargo.toml)
