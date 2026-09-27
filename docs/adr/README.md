# Architecture Decision Records

These ADRs capture only decisions that constrain the current v1 implementation.

| ADR | Decision | Requirements |
|---|---|---|
| [ADR-001](0001-single-binary-single-core.md) | One Rust binary; CLI and MCP share one application core | M-001, M-002, M-003 |
| [ADR-002](0002-bounded-semantic-edits.md) | Typed bounded workbook edits; no generic XML mutation | M-008–M-016, W-001, W-002, W-009 |
| [ADR-003](0003-checked-streaming-xml.md) | Checked streaming XML with source-relative spans and compact indices | M-008, M-009, M-019, M-032–M-035 |
| [ADR-004](0004-authoritative-plan-apply.md) | Hash-bound plan/apply; recompute authority; no blind patch trust | M-017–M-019 |
| [ADR-005](0005-auth-boundary.md) | OAuth PKCE primary, explicit PAT fallback, secrets outside tool args | M-023–M-025, W-005 |
| [ADR-006](0006-focused-rest-and-two-phase-publish.md) | Focused Tableau REST surface and two-phase publish with no blind replay | M-022, M-026–M-028, W-004 |
| [ADR-007](0007-optional-read-only-hyper.md) | Hyper remains optional, isolated and read-only | M-029–M-031, W-003 |
| [ADR-008](0008-dod-yagni-runtime-shape.md) | Dense IDs, contiguous/range data, local indices; no speculative infrastructure | M-032–M-034, W-006, W-007 |
| [ADR-009](0009-independent-oracles-and-measured-optimization.md) | Independent behavioral oracles and measured performance gates | M-034–M-036 |
