# ADR-002 — Typed bounded semantic edits, not generic XML mutation

Status: Accepted

## Context

Tableau workbook XML contains supported semantic objects plus unknown/vendor content that must survive edits. A general XML writer would broaden authority and risk unrelated byte changes.

## Decision

- Expose only typed edit operations in `ChangeSet`.
- Supported v1 mutations are calculations, admitted static parameters, categorical filters and quantitative range filters.
- Represent changes as bounded source-relative patches.
- Preserve unrelated source bytes exactly.
- Reject unsupported editable shapes; never fall back to arbitrary XML replacement.
- Preserve unknown content when it is not the edit target.

## Consequences

- v1 cannot express arbitrary workbook rewrites.
- Adding an edit shape requires a typed operation, preconditions, preservation proof and postcondition tests.
- Exact formatting/comments/vendor nodes outside patch spans survive.

## Implementation

- [src/edit.rs](../../src/edit.rs)
- [src/patch.rs](../../src/patch.rs)
- [src/workbook.rs](../../src/workbook.rs)
- [src/xml.rs](../../src/xml.rs)

## Requirements

M-008–M-016, W-001, W-002, W-009.
