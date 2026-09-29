# Retained fuzz regression

json-number-roundtrip.json is a minimized libFuzzer input from the 0.1.5 wire_json campaign.
Before float_roundtrip it changes the parsed f64 across JSON serialization/reparse.
Run `cargo test --locked behavioral::fuzz_regression_json_number_roundtrip_is_stable -- --exact`.
