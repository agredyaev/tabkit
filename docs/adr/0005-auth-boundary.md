# ADR-005 — OAuth PKCE primary auth with explicit PAT fallback

Status: Accepted

## Context

The product needs interactive corporate authentication and a non-browser fallback without exposing credentials through MCP schemas, argv, protocol output or logs.

## Decision

- Use OAuth 2.0 Authorization Code + PKCE for the interactive path.
- Require exact issuer discovery and HTTPS authorization/token endpoints.
- Bind callbacks to state and configured loopback path; enforce bounded framing/timeouts.
- Accept only JWT access tokens for the Tableau connected-app sign-in path.
- Keep PAT as a separately configured fallback.
- Keep secrets in configuration/environment boundaries, not tool arguments.
- Do not let the browser launcher inherit MCP stdin/stdout.
- A failed authenticated request is not silently replayed after session invalidation.

## Consequences

- Authentication configuration is explicit and external to tool payloads.
- Corporate SSO behavior still requires environment qualification.
- No generic OAuth framework or token cache is introduced.

## Implementation

- [src/oauth.rs](../../src/oauth.rs)
- [src/rest.rs](../../src/rest.rs)
- [src/config.rs](../../src/config.rs)
- [src/app.rs](../../src/app.rs)

## Requirements

M-023, M-024, M-025, W-005.
