#![cfg(not(miri))]

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn test_cli_blocks_execution_in_private_ci_without_valid_license_or_timestamp() {
    // This test simulates running the gleon CLI in a private CI environment.
    // Because GLEON_BUILD_TIMESTAMP is not set to a valid future timestamp (or > 0)
    // during local `cargo test`, `is_valid_official_build` will evaluate to false.
    // In a generic private CI context, this must trigger an exit 42 with compliance error.

    let mut cmd = Command::cargo_bin("gleon").unwrap();
    cmd.env_clear();

    // Mock Generic CI environment (private repository)
    cmd.env("CI", "true");
    cmd.env("CI_PROJECT_PATH", "myorg/private-repo");

    // Explicitly zero out the timestamp in case the local environment leaked it
    cmd.env("GLEON_BUILD_TIMESTAMP", "0");

    // Provide a dummy invalid license key
    cmd.env("GLEON_LICENSE_KEY", "invalid_base64_or_signature");

    // Attempt to run the CLI (e.g. status)
    let assert = cmd.arg("--strict").arg("status").assert();

    // The CLI should fail with exit code 42 due to UnofficialBuildInPrivateCI
    // or ExpiredUnlicensedBinary
    assert.failure().code(42).stderr(predicate::str::contains(
        "[GLEON COMPLIANCE ERROR] Execution blocked.",
    ));
}
