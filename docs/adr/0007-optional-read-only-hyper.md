# ADR-007 — Optional isolated read-only Hyper capability

Status: Accepted

## Context

Hyper access requires Tableau's native SDK/runtime. It remains optional and read-only; local workbook editing must not depend on it.

## Decision

- Keep Hyper behind an optional build feature.
- Base binary must run without the native Hyper SDK.
- Extract Hyper members explicitly from TWBX.
- Admit only bounded read-only SQL.
- Reject multiple statements and side-effecting statements.
- Snapshot/hash-check the source around query execution.
- Run native work behind the existing worker/process boundary and cancellation checks.

## Consequences

- Native Hyper must be qualified separately with the official SDK/runtime.
- Generic write SQL is outside v1.
- SQL parser behavior is part of the admission boundary and receives resource-regression tests.

## Implementation

- [src/hyper.rs](../../src/hyper.rs)
- [src/sql.rs](../../src/sql.rs)
- [src/package.rs](../../src/package.rs)
- [Cargo.toml](../../Cargo.toml)

## Requirements

M-029, M-030, M-031, W-003.
