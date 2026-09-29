# Fuzz harness

This separate development crate imports the production Rust modules by path and enables its own `dev-tools` feature.
No production library API or alternate parser is introduced for fuzzing. No native Hyper or network path is executed.
Run `cargo +nightly fuzz build` from the project root, then `python3 scripts/fuzz_run.py --seconds 60`. The `lineage` target checks graph references, details, export, and paged impact on mutated XML and generated source/view structures.
Keep corpus growth and artifacts out of the source package; retain seeds and minimized regression inputs.
