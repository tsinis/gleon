use gleon_core::git::GitResolver;
use std::env;
use std::path::PathBuf;

#[test]
fn test_verify_ignored_with_real_fixtures() {
    let current_dir = env::current_dir().unwrap();
    // In cargo tests, current_dir is the crate root (e.g., gleon-core)
    // We want to access gleon/tests/fixtures/git
    let workspace_root = current_dir.parent().unwrap();
    let fixtures_dir = workspace_root.join("gleon/tests/fixtures/git");

    // We assume this is run within the actual git repository of gleon
    // Check paths relative to fixtures_dir
    let ignored_golden = fixtures_dir.join("goldens/login/test.png");
    let ignored_scratch = fixtures_dir.join("scratch/temp.txt");
    let ignored_secret = fixtures_dir.join("secret.txt");

    // Should be ignored
    let result = GitResolver::verify_ignored_impl(&[ignored_golden], &fixtures_dir)
        .expect("Failed to verify ignored");
    assert!(result, "Expected goldens to be ignored");

    let result = GitResolver::verify_ignored_impl(&[ignored_scratch], &fixtures_dir)
        .expect("Failed to verify ignored");
    assert!(result, "Expected scratch/ to be ignored");

    let result = GitResolver::verify_ignored_impl(&[ignored_secret], &fixtures_dir)
        .expect("Failed to verify ignored");
    assert!(result, "Expected secret.txt to be ignored");

    // Check a file that shouldn't be ignored
    let not_ignored = fixtures_dir.join("src/main.rs");
    let result = GitResolver::verify_ignored_impl(&[not_ignored], &fixtures_dir)
        .expect("Failed to verify ignored");
    assert!(!result, "Expected src/main.rs NOT to be ignored");
}

#[test]
fn test_verify_ignored_outside_repo() {
    let current_dir = env::current_dir().unwrap();
    let workspace_root = current_dir.parent().unwrap();

    // A path completely outside the repository (e.g., /tmp)
    let outside_path = PathBuf::from("/tmp/some_fake_file.png");

    let result = GitResolver::verify_ignored_impl(&[outside_path], workspace_root);
    assert!(
        matches!(result, Err(gleon_core::git::GitError::OutsideRepository(_))),
        "Expected OutsideRepository error, got {:?}",
        result
    );
}

#[test]
fn test_verify_ignored_relative_outside_repo() {
    let current_dir = env::current_dir().unwrap();
    let workspace_root = current_dir.parent().unwrap();

    // A relative path escaping the workspace (e.g., ../../../../../../etc/passwd)
    let outside_path = PathBuf::from("tests/fixtures/git/../../../../../../etc/passwd");

    let result = GitResolver::verify_ignored_impl(&[outside_path], workspace_root);
    assert!(
        matches!(result, Err(gleon_core::git::GitError::OutsideRepository(_))),
        "Expected OutsideRepository error, got {:?}",
        result
    );
}
