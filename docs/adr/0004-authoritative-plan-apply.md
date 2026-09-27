# ADR-004 — Authoritative hash-bound plan/apply

Status: Accepted

## Context

Plans are review artifacts, not authority to execute arbitrary stored byte patches. Inputs can change between inspection, planning and apply.

## Decision

- Bind plans to schema/engine version and admitted source identity.
- Validate operation-specific expected state before planning.
- On apply, read and hash the plan, reopen the source, recompute the authoritative plan from typed changes and require equality with the supplied plan.
- Do not execute caller-supplied patch offsets as authority.
- Keep filesystem freshness and no-clobber checks at commit boundaries.
- Hash candidate output during admitted emission when the candidate is produced by the bounded patch emitter; keep filesystem freshness checks unchanged.

## Consequences

- Tampering/stale sources fail before commit.
- Plan files are reproducible review artifacts.
- Apply costs include recomputation by design.

## Implementation

- [src/edit.rs](../../src/edit.rs)
- [src/app.rs](../../src/app.rs)
- [src/package.rs](../../src/package.rs)
- [src/patch.rs](../../src/patch.rs)

## Requirements

M-017, M-018, M-019.
