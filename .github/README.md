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
>   uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
>   with:
>     fetch-depth: 0 # Required for gleon merge-base resolution
> ```
>
> If a shallow clone is detected, gleon will fail immediately returning a hard `GitError::ShallowClone`.

## GitHub Action Usage

`gleon` provides a Composite GitHub Action (`tsinis/gleon`) for running visual regression testing in CI:

```yaml
steps:
  - name: Checkout repository
    uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
    with:
      fetch-depth: 0 # Required for merge-base resolution

  - name: Run gleon Visual Regression Verify
    uses: tsinis/gleon@main
    with:
      command: "verify"
      github-token: ${{ secrets.GITHUB_TOKEN }}
```

### Action Inputs

| Input               | Description                                                                                                                                         | Default                       |
| :------------------ | :-------------------------------------------------------------------------------------------------------------------------------------------------- | :---------------------------- |
| `version`           | gleon release version tag to download (e.g. `'v0.1.0'` or `'latest'`)                                                                               | `'latest'`                    |
| `github-token`      | GitHub token (`${{ secrets.GITHUB_TOKEN }}`) to prevent API rate limits when downloading the binary. **Highly recommended** for active CI pipelines | `${{ github.token }}`         |
| `checksum`          | Expected SHA256 digest of the binary for independent trust root verification (optional)                                                             | `''`                          |
| `license-key`       | Commercial BSL license key for private repositories                                                                                                 | `''`                          |
| `strict`            | Fail build on license violation (`'true'` / `'false'`)                                                                                              | `'false'`                     |
| `target-branch`     | Target branch for baseline comparison                                                                                                               | PR base ref or default branch |
| `command`           | gleon command to execute (`'verify'`, `'diff'`, `'pull'`, `'approve'`)                                                                              | `'diff'`                      |
| `working-directory` | Directory to run gleon from (useful for monorepos)                                                                                                  | `'.'`                         |
| `args`              | Additional flags for the selected gleon command (e.g. `'--from=.gleon/diffs'` for approve)                                                          | `''`                          |
| `pr-number`         | Pull Request number for markdown report generation                                                                                                  | PR number                     |
| `artifact-name`     | Name of uploaded artifact on verification failure                                                                                                   | `gleon-artifacts-...`         |

### Ephemeral Diff Branch Cleanup Workflow

When visual diffs are detected in a PR, ephemeral branches (`gleon/diffs/pr-<PR_NUMBER>`) are pushed to store diff artifacts.

To prevent orphan branches from accumulating in consumer repositories, add `.github/workflows/gleon-cleanup.yml` to your repository:

```yaml
name: gleon Ephemeral Branch Cleanup

on:
  pull_request_target:
    types: [closed]

jobs:
  cleanup:
    permissions:
      contents: write
    uses: tsinis/gleon/.github/workflows/cleanup.yml@main
```

When a Pull Request is closed or merged, this workflow automatically deletes the ephemeral `gleon/diffs/pr-<PR_NUMBER>` branch from your repository.

## 📸 Approving Visual Baseline Changes in Pull Requests

When `gleon` detects visual regressions during a PR CI run, it posts a detailed Markdown report with diff previews in the PR comment section.

To accept the new visual changes as the new baseline:

1. **Approve All Changed Screenshots**:
   Comment directly on the PR:

   ```text
   /gleon approve
   ```

2. **Approve Specific Tests Only**:

   ```text
   /gleon approve auth/login
   ```

### GitHub Actions Workflow Setup

To enable `/gleon approve` comments in your repository, simply add `.github/workflows/gleon-approve.yml` referencing the shipped reusable workflow:

```yaml
name: Gleon Approve

on:
  issue_comment:
    types: [created]

jobs:
  approve:
    permissions:
      contents: write
      pull-requests: read
    uses: tsinis/gleon/.github/workflows/approve.yml@main
    secrets:
      R2_ACCOUNT_ID: ${{ secrets.R2_ACCOUNT_ID }}
      GLEON_STORAGE_URL: ${{ secrets.GLEON_STORAGE_URL }}
      AWS_ACCESS_KEY_ID: ${{ secrets.AWS_ACCESS_KEY_ID }}
      AWS_SECRET_ACCESS_KEY: ${{ secrets.AWS_SECRET_ACCESS_KEY }}
```

## How to Build and Run Locally

### Prerequisites

You need the stable Rust toolchain (Edition 2024, Rust 1.97+).

### Building and Installing the CLI

To compile the binary in release mode:

```bash
cargo build --release --workspace
```

The compiled binary will be located at `target/release/gleon`.

To install the CLI binary into your local cargo environment (`~/.cargo/bin`):

```bash
cargo install --path gleon --force
```

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
