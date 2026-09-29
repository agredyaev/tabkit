# ADR-006 — Focused Tableau REST surface and two-phase publish

Status: Accepted

## Context

The product needs enough Tableau REST capability to discover, inspect, download, verify and publish workbooks without becoming a generic REST client. Publish is a remote side effect whose outcome can become uncertain.

## Decision

- Keep the REST surface focused on login/logout, explore, search, workbook metadata, jobs, download, view image/data and publish.
- Require explicit project/destination policy.
- Split publish into prepare and confirm.
- Prepare binds candidate hash, destination identity, local validation, optional assertion suite, remote baseline and expiry into an approval artifact.
- Confirm re-runs policy/local/freshness checks immediately before POST.
- Create a durable local attempt marker before remote commit.
- Never automatically replay an attempted publish whose outcome is failed/unknown.
- Overwrite requires explicit operator policy and a reviewed remote baseline.

## Consequences

- Publish is intentionally more expensive than a single POST.
- Unknown remote outcomes require inspection and a new approval.
- Additional REST resources are added only for concrete workflows.

## Implementation

- [src/app.rs](../../src/app.rs)
- [src/rest.rs](../../src/integrations/rest.rs)
- [src/fs.rs](../../src/fs.rs)
