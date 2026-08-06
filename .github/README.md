[![codecov](https://codecov.io/gh/tsinis/gleon/graph/badge.svg?token=KIUODCEVAK)](https://codecov.io/gh/tsinis/gleon)

# gleon

⛵ `gleon` is a high-performance, developer-first, framework-agnostic visual regression testing CLI built in Rust. It isolates screenshot baselines by platform and Git branch and uses a content-addressed storage (CAS) model for baseline artifacts, minimizing bandwidth and storage overhead in CI pipelines.

## CI/CD Prerequisites (Shallow Clone Constraint)

> [!IMPORTANT]
> gleon computes baseline manifests by resolving the `merge-base` commit between the current branch and the target branch (default `main`).
> Because default CI checkout actions (such as `actions/checkout` in GitHub Actions) perform a **shallow clone** (e.g. `fetch-depth: 1`), the local repository will lack the historical ancestry needed to compute the `merge-base`.
>
> **You must configure your checkout step to fetch full history:**
>
> ```yaml
> - name: Checkout code
>   uses: actions/checkout@v4
>   with:
>     fetch-depth: 0 # Required for gleon merge-base resolution
> ```
>
> If a shallow clone is detected, gleon will fail immediately returning a hard `GitError::ShallowClone`.

## GitHub Action Usage

Gleon provides a Composite GitHub Action (`tsinis/gleon`) for running visual regression testing in CI:

```yaml
steps:
  - name: Checkout repository
    uses: actions/checkout@v4
    with:
      fetch-depth: 0 # Required for merge-base resolution

  - name: Run Gleon Visual Regression
    uses: tsinis/gleon@v1
    with:
      license-key: ${{ secrets.GLEON_LICENSE_KEY }}
      strict: "false"
```

### Action Inputs

| Input | Description | Default |
| :--- | :--- | :--- |
| `version` | Gleon release version to download (`'latest'` or specific tag) | `'latest'` |
| `github-token` | GitHub token (`${{ secrets.GITHUB_TOKEN }}`) to prevent API rate limits when downloading the binary. **Highly recommended** for active CI pipelines | `''` |
| `license-key` | Commercial BSL license key for private repositories | `''` |
| `strict` | Fail build on license violation (`'true'` / `'false'`) | `'false'` |
| `target-branch` | Target branch for baseline comparison | PR base ref or default branch |
| `args` | Additional flags for `gleon diff` | `''` |


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

## FAQ: Architecture & Best Practices

### Why does gleon enforce `.gitignore` for baseline images?

gleon separates the **control plane** (manifests) from the **data plane** (images) by employing a **Content-Addressable Storage (CAS)** architecture.

In modern software engineering, committing binary blobs directly to Git is an anti-pattern. Git is optimized for text; committing thousands of screenshot revisions inherently bloats the repository, severely degrades clone times, and makes PR diffs unmanageable. This is the exact problem that tools like Git LFS or Bazel Remote Execution solve.

To provide enterprise-grade scale, gleon uses a **Git-First Control Plane with Dumb Blob Storage**:

- **Manifests in Git:** gleon tracks tiny, deterministic JSON files (`.gleon/manifests/**/*.json`) in your Git repository. These files contain the cryptographic hashes (SHA-256) of your baseline images and their spatial dimensions.
- **Blobs in Cloud Storage:** The actual PNG images (`.gleon/blobs/`) are aggressively ignored from Git. They are uploaded to an S3-compatible bucket (like AWS S3, Cloudflare R2, or Google Cloud Storage) using `gleon push` and downloaded using `gleon pull`.

This architecture guarantees that your Git repository remains lightning-fast and lightweight indefinitely, while immutable graphical artifacts are offloaded to purpose-built object storage.

### How do I handle cross-platform rendering diffs?

When running visual tests across diverse operating systems (e.g., generating on macOS, verifying on Ubuntu CI), you will inevitably encounter minor pixel differences caused by native OS font rendering and anti-aliasing algorithms.

**Do NOT arbitrarily increase the global error threshold (e.g., 2%) to ignore these!** Inflating the tolerance threshold masks genuine regressions and defeats the purpose of visual testing.

Instead, gleon natively embraces **Platform-Specific Baselines**.
