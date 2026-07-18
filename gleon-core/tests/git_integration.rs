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

    // A path completely outside the repository (sibling of workspace root)
    let outside_path = workspace_root.parent().unwrap().join("some_fake_file.png");

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

    // A relative path escaping the workspace
    let outside_path = PathBuf::from("tests/fixtures/git/../../../../../../some_outside_file.txt");

    let result = GitResolver::verify_ignored_impl(&[outside_path], workspace_root);
    assert!(
        matches!(result, Err(gleon_core::git::GitError::OutsideRepository(_))),
        "Expected OutsideRepository error, got {:?}",
        result
    );
}

#[test]
#[cfg(not(miri))]
fn test_resolve_branch_real_repo() {
    let current_dir = env::current_dir().unwrap();
    let mut repo_root = current_dir.clone();
    while !repo_root.join(".git").exists() {
        if let Some(parent) = repo_root.parent() {
            repo_root = parent.to_path_buf();
        } else {
            break;
        }
    }

    let result = GitResolver::resolve_branch_impl(None, &repo_root, &gleon_core::git::OsEnv);
    assert!(
        result.is_ok(),
        "Expected branch resolution to succeed on real repo, got {:?}",
        result
    );
    let branch = result.unwrap();
    assert!(!branch.is_empty(), "Branch name should not be empty");
}

#[test]
#[cfg(not(miri))]
fn test_verify_ignored_real_repo_files() {
    let current_dir = env::current_dir().unwrap();
    let mut repo_root = current_dir.clone();
    while !repo_root.join(".git").exists() {
        if let Some(parent) = repo_root.parent() {
            repo_root = parent.to_path_buf();
        } else {
            break;
        }
    }

    let cargo_toml = repo_root.join("Cargo.toml");
    let result = GitResolver::verify_ignored_impl(&[cargo_toml], &repo_root).unwrap();
    assert!(!result, "Cargo.toml should not be ignored");

    let target_dir = repo_root.join("target");
    if target_dir.exists() {
        let result = GitResolver::verify_ignored_impl(&[target_dir], &repo_root).unwrap();
        assert!(result, "target/ directory should be ignored");
    }
}

#[test]
#[cfg(not(miri))]
fn test_resolve_merge_base_real_repo() {
    let current_dir = env::current_dir().unwrap();
    let mut repo_root = current_dir.clone();
    while !repo_root.join(".git").exists() {
        if let Some(parent) = repo_root.parent() {
            repo_root = parent.to_path_buf();
        } else {
            break;
        }
    }

    let result = GitResolver::resolve_merge_base(&repo_root, "HEAD");
    if repo_root.join(".git/shallow").exists() {
        assert!(matches!(
            result,
            Err(gleon_core::git::GitError::ShallowClone(_))
        ));
    } else {
        assert!(
            result.is_ok(),
            "Expected merge-base to succeed, got {:?}",
            result
        );
        let sha = result.unwrap();
        assert_eq!(sha.len(), 40, "SHA should be 40 characters");
    }
}

#[test]
#[cfg(not(miri))]
fn test_get_commit_author_real_repo() {
    let current_dir = env::current_dir().unwrap();
    let mut repo_root = current_dir.clone();
    while !repo_root.join(".git").exists() {
        if let Some(parent) = repo_root.parent() {
            repo_root = parent.to_path_buf();
        } else {
            break;
        }
    }

    let sha = GitResolver::resolve_merge_base(&repo_root, "HEAD");
    if let Ok(sha) = sha {
        let author = GitResolver::get_commit_author(&repo_root, &sha);
        assert!(
            author.is_ok(),
            "Expected get_commit_author to succeed, got {:?}",
            author
        );
        let author_str = author.unwrap();
        assert!(!author_str.is_empty(), "Author string should not be empty");
    }
}
