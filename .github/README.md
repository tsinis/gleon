# gleon

⛵ `gleon` is a high-performance, developer-first, framework-agnostic visual regression testing CLI built in Rust. It isolates screenshot baselines by platform and Git branch and uses a content-addressed storage (CAS) model for baseline artifacts, minimizing bandwidth and storage overhead in CI pipelines.

## CI/CD Prerequisites (Shallow Clone Constraint)

> [!IMPORTANT]
> Gleon computes baseline manifests by resolving the `merge-base` commit between the current branch and the target branch (default `main`).
> Because default CI checkout actions (such as `actions/checkout` in GitHub Actions) perform a **shallow clone** (e.g. `fetch-depth: 1`), the local repository will lack the historical ancestry needed to compute the `merge-base`.
>
> **You must configure your checkout step to fetch full history:**
>
> ```yaml
> - name: Checkout code
>   uses: actions/checkout@v4
>   with:
>     fetch-depth: 0 # Required for Gleon merge-base resolution
> ```
>
> If a shallow clone is detected, Gleon will fail immediately returning a hard `GitError::ShallowClone`.

## How to Build and Run Locally

### Prerequisites

You need the stable Rust toolchain (Edition 2024, Rust 1.97+).

### Building the CLI

To compile the binary in release mode:

```bash
cargo build --release --workspace
```

The compiled binary will be located at `target/release/gleon`.

### Running the CLI

You can execute the binary directly or via `cargo`:

```bash
# Run status command locally
cargo run --package gleon -- status

# Run with custom config file
cargo run --package gleon -- --config path/to/config.yaml status

# Run status with target branch override
cargo run --package gleon -- --target-branch dev status
```

### Running Tests

To run the full suite of unit and integration tests:

```bash
cargo test --workspace
```

To run clippy lints:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

To format code:

```bash
cargo fmt --all
```

## Storage Layout and Terminology

Gleon uses two primary types of metadata files to achieve concurrent-safe, low-bandwidth visual diffs:

1. **`manifest.json`**: Stored locally in each golden screenshots test folder (e.g. `test/goldens/login_test/manifest.json`). It maps individual screenshot files to their content-addressed hashes, width, height, and author metadata.
2. **`manifest_index.json`**: The top-level index file that maps test folder paths to their respective `manifest.json` file checksums. It is stored remotely under:
   - `branches/<hashed-branch-name>/<platform>/manifest_index.json`
   - `commits/<commit-sha>/<platform>/manifest_index.json`
