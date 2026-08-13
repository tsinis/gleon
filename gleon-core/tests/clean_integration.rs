#![cfg(all(test, not(miri)))]

use gleon_core::cli::{Cli, Commands};
use gleon_core::context::ResolvedContext;
use gleon_core::ops::clean::{CleanOptions, clean_workspace};
use std::fs;
use tempfile::tempdir;

const VALID_PNG_BYTES: &[u8] = include_bytes!("fixtures/baseline_100x100.png");

#[test]
fn test_clean_workspace_full_flow_with_git_untracking() {
    let temp = tempdir().unwrap();
    let base_path = temp.path();

    // 1. Initialize a real git repo using gix
    let repo = gix::init(base_path).unwrap();

    // 2. Setup .gleon structure
    let gleon_dir = base_path.join(".gleon");
    fs::create_dir_all(&gleon_dir).unwrap();
    let runs_dir = gleon_dir.join("runs");
    fs::create_dir_all(&runs_dir).unwrap();
    fs::write(runs_dir.join("run_metadata.json"), b"{}").unwrap();
    let diffs_dir = gleon_dir.join("diffs");
    fs::create_dir_all(&diffs_dir).unwrap();
    fs::write(diffs_dir.join("diff.png"), b"diff").unwrap();

    let config_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include: "packages/app/test/goldens/**/*.png"
    mode: pixel
"#;
    fs::write(gleon_dir.join("gleon.yaml"), config_yaml).unwrap();

    // 3. Create sample golden files
    let goldens_dir = base_path
        .join("packages")
        .join("app")
        .join("test")
        .join("goldens");
    fs::create_dir_all(&goldens_dir).unwrap();
    let golden_file = goldens_dir.join("login.png");
    fs::write(&golden_file, VALID_PNG_BYTES).unwrap();

    // 4. Create and populate git index
    let index_path = base_path.join(".git").join("index");
    let state = gix::index::State::new(gix::hash::Kind::Sha1);
    let mut index = gix::index::File::from_state(state, index_path);
    let rel_path_bstr = "packages/app/test/goldens/login.png";
    index.dangerously_push_entry(
        gix::index::entry::Stat::default(),
        gix::hash::ObjectId::empty_tree(gix::hash::Kind::Sha1),
        gix::index::entry::Flags::empty(),
        gix::index::entry::Mode::FILE,
        rel_path_bstr.as_bytes().into(),
    );
    index.write(gix::index::write::Options::default()).unwrap();

    // Verify entry is currently in index
    let index_before = repo.open_index().unwrap();
    assert!(
        index_before
            .entry_index_by_path(rel_path_bstr.into())
            .is_ok()
    );

    let cli = Cli::for_test(Commands::Clean {
        dry_run: false,
        skip_gitignore: false,
        keep_runs: false,
    });
    let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();

    // 5. Run clean
    let opts = CleanOptions::default();
    let res = clean_workspace(&ctx, base_path, &opts).unwrap();

    assert_eq!(res.deleted_files.len(), 1);
    assert_eq!(res.untracked_files.len(), 1);
    assert!(!golden_file.exists());
    assert!(!runs_dir.exists());
    assert!(!diffs_dir.exists());

    // 6. Verify entry was removed from Git index
    let index_after = repo.open_index().unwrap();
    assert!(
        index_after
            .entry_index_by_path(rel_path_bstr.into())
            .is_err()
    );

    // 7. Verify .gitignore wildcard entry
    let gitignore = fs::read_to_string(base_path.join(".gitignore")).unwrap();
    assert!(gitignore.contains("**/packages/app/test/goldens/**/*.png"));
}

#[test]
fn test_clean_workspace_dry_run_leaves_state_intact() {
    let temp = tempdir().unwrap();
    let base_path = temp.path();

    let gleon_dir = base_path.join(".gleon");
    fs::create_dir_all(&gleon_dir).unwrap();
    let runs_dir = gleon_dir.join("runs");
    fs::create_dir_all(&runs_dir).unwrap();
    fs::write(runs_dir.join("data.txt"), b"temp").unwrap();

    let config_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include: "test/goldens/**/*.png"
    mode: pixel
"#;
    fs::write(gleon_dir.join("gleon.yaml"), config_yaml).unwrap();

    let goldens_dir = base_path.join("test").join("goldens");
    fs::create_dir_all(&goldens_dir).unwrap();
    let golden_file = goldens_dir.join("button.png");
    fs::write(&golden_file, VALID_PNG_BYTES).unwrap();

    let cli = Cli::for_test(Commands::Clean {
        dry_run: true,
        skip_gitignore: false,
        keep_runs: false,
    });
    let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();

    let opts = CleanOptions {
        dry_run: true,
        skip_gitignore: false,
        keep_runs: false,
    };
    let res = clean_workspace(&ctx, base_path, &opts).unwrap();

    assert_eq!(res.deleted_files.len(), 1);
    assert!(golden_file.exists());
    assert!(runs_dir.exists());
    assert!(!base_path.join(".gitignore").exists());
}

#[test]
fn test_clean_workspace_outside_git_repository() {
    let temp = tempdir().unwrap();
    let base_path = temp.path();

    let gleon_dir = base_path.join(".gleon");
    fs::create_dir_all(&gleon_dir).unwrap();

    let config_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include: "test/goldens/**/*.png"
    mode: pixel
"#;
    fs::write(gleon_dir.join("gleon.yaml"), config_yaml).unwrap();

    let goldens_dir = base_path.join("test").join("goldens");
    fs::create_dir_all(&goldens_dir).unwrap();
    let golden_file = goldens_dir.join("header.png");
    fs::write(&golden_file, VALID_PNG_BYTES).unwrap();

    let cli = Cli::for_test(Commands::Clean {
        dry_run: false,
        skip_gitignore: false,
        keep_runs: false,
    });
    let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();

    let opts = CleanOptions::default();
    let res = clean_workspace(&ctx, base_path, &opts).unwrap();

    assert_eq!(res.deleted_files.len(), 1);
    assert!(!golden_file.exists());
    assert!(base_path.join(".gitignore").exists());
}
