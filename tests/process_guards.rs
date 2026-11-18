#![allow(clippy::expect_used)]

use std::process::Command;
use tempfile::tempdir;

fn stderr_text(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn mcp_binary_rejects_incomplete_tableau_configuration() {
    for args in [
        vec!["--tableau-server", "https://tableau.example.test"],
        vec!["--tableau-pat-name", "test-pat"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_tabkit-mcp"))
            .args(args)
            .output()
            .expect("run tabkit-mcp");
        assert!(!output.status.success());
        let stderr = stderr_text(&output);
        assert!(stderr.contains("CONFIG:"), "{stderr}");
        assert!(
            stderr.contains("TABLEAU_SERVER and TABLEAU_PAT_NAME must be provided together"),
            "{stderr}"
        );
    }
}

#[test]
fn mcp_binary_rejects_unauthenticated_non_loopback_http_before_bind() {
    let output = Command::new(env!("CARGO_BIN_EXE_tabkit-mcp"))
        .args(["--transport", "http", "--bind", "0.0.0.0:18765"])
        .env_remove("TABKIT_MCP_TOKEN")
        .output()
        .expect("run tabkit-mcp");
    assert!(!output.status.success());
    let stderr = stderr_text(&output);
    assert!(stderr.contains("MCP_AUTH:"), "{stderr}");
    assert!(stderr.contains("non-loopback"), "{stderr}");
}

#[test]
fn extension_host_binary_rejects_invalid_base_path_and_unsigned_non_loopback_bind() {
    let invalid_path = Command::new(env!("CARGO_BIN_EXE_tabkit-extension-host"))
        .args(["--base-path", "/tabkit/"])
        .output()
        .expect("run extension host");
    assert!(!invalid_path.status.success());
    let stderr = stderr_text(&invalid_path);
    assert!(stderr.contains("CONFIG:"), "{stderr}");
    assert!(stderr.contains("base_path"), "{stderr}");

    let unsigned = Command::new(env!("CARGO_BIN_EXE_tabkit-extension-host"))
        .args(["--bind", "0.0.0.0:18766"])
        .env_remove("TABKIT_EXTENSION_SIGNING_KEY")
        .output()
        .expect("run extension host");
    assert!(!unsigned.status.success());
    let stderr = stderr_text(&unsigned);
    assert!(stderr.contains("EXTENSION_HOST_AUTH:"), "{stderr}");
    assert!(stderr.contains("TABKIT_EXTENSION_SIGNING_KEY"), "{stderr}");
}

#[test]
fn cli_validate_rejects_non_https_tableau_before_pat_lookup() {
    let dir = tempdir().expect("temp dir");
    let workbook = dir.path().join("fixture.twb");
    std::fs::write(
        &workbook,
        br#"<workbook version="26.2" source-build="20262.test"></workbook>"#,
    )
    .expect("write fixture");

    let output = Command::new(env!("CARGO_BIN_EXE_tabkit"))
        .arg("validate")
        .arg("--server")
        .arg("http://tableau.example.test")
        .arg("--pat-name")
        .arg("test-pat")
        .arg(&workbook)
        .env_remove("TABLEAU_PAT_SECRET")
        .output()
        .expect("run tabkit validate");
    assert!(!output.status.success());
    let stderr = stderr_text(&output);
    assert!(stderr.contains("CONFIG:"), "{stderr}");
    assert!(stderr.contains("HTTPS"), "{stderr}");
}

#[test]
fn extension_host_binary_rejects_malformed_signing_keys_before_bind() {
    for value in ["not-base64!", "AA=="] {
        let output = Command::new(env!("CARGO_BIN_EXE_tabkit-extension-host"))
            .env("TABKIT_EXTENSION_SIGNING_KEY", value)
            .output()
            .expect("run extension host");
        assert!(!output.status.success());
        let stderr = stderr_text(&output);
        assert!(stderr.contains("CONFIG:"), "{stderr}");
        assert!(stderr.contains("TABKIT_EXTENSION_SIGNING_KEY"), "{stderr}");
    }
}
