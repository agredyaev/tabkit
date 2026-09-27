# ADR-008 — Data-oriented runtime shape and YAGNI constraints

Status: Accepted

## Context

Workbook/XML processing is dominated by sequential scans, identity lookup, dependency traversal and bounded edit preparation. Heap-per-object models, global caches and generic infrastructure increase allocation, indirection and ownership complexity.

## Decision

- Use typed dense IDs for domain identity.
- Prefer contiguous vectors, ranges, sorted relation slices, owner side tables and request-local scratch.
- Hash maps are admission/lookup indexes only; canonical arrays define observable identity/order.
- Borrow source strings/spans where ownership permits.
- Do not add global mutable workbook caches.
- Do not add a DI container, plugin framework, event bus, background service or generic backend abstraction without a demonstrated requirement.
- Optimize only measured dominant paths and preserve the simpler fallback when a fast path is conditional.

## Consequences

- Some builder/admission structures differ from frozen runtime structures.
- Fast paths carry explicit equivalence tests.
- Performance work must remove demonstrated work/state rather than add speculative infrastructure.

## Implementation

- [src/workbook.rs](../../src/workbook.rs)
- [src/xml.rs](../../src/xml.rs)
- [src/xml_stream.rs](../../src/xml_stream.rs)
- [src/edit.rs](../../src/edit.rs)

## Requirements

M-032, M-033, M-034, W-006, W-007.
