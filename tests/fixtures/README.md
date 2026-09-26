# Synthetic audit regression inputs

These small hand-authored inputs reproduce source-audit edge cases. They are NOT exports
from a qualified Tableau 2025 installation. Some files are intentionally invalid.

- `new-reference.before.twb`: global field exists but is absent from a using worksheet's dependencies.
- `new-reference.attribute-only-candidate.twb`: counterexample for formula-only replacement; not a valid expected output.
- `invalid-parameter.twb`: value outside the declared static range.
- `known-local-error.twb`: an unresolved known calculation reference.
- `inline-calculation.twb`: a worksheet-local definition that must remain visible as unsupported.
- `layout-only.twb`: changed dashboard layout despite stable object inventory.
- `duplicate-keys.json`: intentionally ambiguous JSON; a general JSON loader can hide this error.

The regression functions are in `src/regression.rs`. Run `cargo test --locked` after
resolving dependencies with Cargo. They have not been compiled or executed in this delivery.
