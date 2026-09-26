# tabkit 0.1.5 — tested local implementation

Base release built on this Mac (arm64, Rust/cargo 1.98.1).
73 Rust tests passed, including nine generated properties (1,024 cases each).
16 executable CLI/MCP smoke, 28 HTTPS REST and 22 full OAuth/PKCE fixture checks passed.
Nine AddressSanitizer/libFuzzer targets have successful final runs: eight x60 seconds and SQL x180 seconds after repair (3,072,517 executions). Earlier failed runs are retained.
This is not yet a live Tableau/Hyper/Quick qualification.

## Reproduce

    cargo check --all-targets --locked
    cargo test --locked
    cargo build --release --locked
    python3 scripts/smoke.py

CLI startup arguments precede the subcommand; --workspace is required.
Executable: target/release/tabkit
Independent MCP client checked initialize, 23 tools, tool calls and clean EOF.
A smoke-detected SDK identity bug was corrected; server identifies as tabkit 0.1.5.

## Changes

- Invalid reqwest form feature removed; .form() API retained.
- clap retained with smaller feature set, valid required workspace argument.
- SQL parser is optional for the production Hyper feature, present for dev tests.
- Generic SELECT and HyperScalar contracts retained.
- One flate2 Rust ZIP backend, no Zopfli; TWBX preservation tested.
- Narrow SHA-256 hex helper; hex dependency and unused Tokio signal removed.
- Base normal/build dependency tree: 157 to 144 package+version entries.
- 401 invalidates cached Tableau session; current request is never replayed.
- Browser launcher cannot inherit MCP stdin/stdout.
- Exact OAuth discovery issuer, strict callback state/duplicates/path/framing, bounded deadlines.
- Existing OAuth and explicit PAT fallback remain.

## Testing additions in 0.1.5

See TESTING.md and reports/testing-0.1.5/summary.json for actual runs, fixtures, fuzz replay and fault-injection results.
Three defects were reproduced and fixed: malformed preservation ranges, JSON numeric round-trip drift and SQL nested-function admission resource exhaustion. All have regressions; a separate SQL comment-oracle issue was corrected in the harness.
`proptest` is dev-only; libFuzzer lives in a separate development workspace. No end-user runtime was added.

## Historical 0.1.4 evidence and remaining limitations

reports/remote-stage.json, test-final.log, check-final.log, build-release.log,
smoke.json, dependency-change.json, clippy.log and compiled-source-manifest.json.
Clippy exits 0 with 15 remaining suggestions; not a zero-warning lint acceptance.
No rustfmt check or Rust 1.88 execution; no actual corporate auth, Tableau,
native Hyper, Amazon Quick UI or Windows/Linux build was run.

No Quick configuration changes, real credential reads or live Tableau publications.
Base runtime needs no Python; Python is used by developer smoke only.
The Hyper build needs the official SDK/library/runtime and has not been built.
PAT supplied via a Quick env setting is stored in that setting; keep secret out of argv.
Next: read-only real Tableau access, real 2025 book corpus, explicit test project
publish, native Hyper lifecycle, then actual Quick integration.
