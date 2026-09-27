# ADR-003 — Checked streaming XML with source-relative indices

Status: Accepted

## Context

The original DOM-style admission retained too much memory and performed excessive allocation. The product still needs XML 1.0 structural validation, namespace correctness, exact source spans and stable node identity.

## Decision

- Use a pinned streaming tokenizer for production XML admission.
- Validate structure, namespaces, expanded attribute-name uniqueness, references, root cardinality, encoding and node/input limits during admission.
- Reject DTD declarations.
- Retain original bytes plus compact element/link/owner/span indices required by product behavior.
- Do not retain ordinary attribute strings/rows; scan admitted start-tag spans and store only sparse normalized values.
- Keep the independent DOM parser in developer tests as an oracle, not in the production hot path.
- Candidate product proof may use admitted source + typed bounded delta instead of a second full production reparse; full re-admission stays in tests.

## Consequences

- Most attribute access is borrowed and allocation-free.
- Source bytes remain the preservation authority.
- XML admission remains a full correctness boundary even though representation is compact.
- Specialized fast paths must fall back conservatively and preserve oracle equivalence.

## Implementation

- [src/xml_stream.rs](../../src/xml_stream.rs)
- [src/xml_fast.rs](../../src/xml_fast.rs)
- [src/xml.rs](../../src/xml.rs)
- [src/xml_attrs.rs](../../src/xml_attrs.rs)
- [src/xml_index_tests.rs](../../src/xml_index_tests.rs)
- [src/ownership_tests.rs](../../src/ownership_tests.rs)

## Requirements

M-008, M-009, M-019, M-032–M-035.
