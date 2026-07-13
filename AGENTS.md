# AI Agent Guidelines for Gleon

Welcome to the **Gleon** repository. This document defines the workspace layout,
coding guidelines, and rules for all AI agents collaborating on this project.

## 1. Project Context & Workspace Layout

Gleon is a universal visual regression testing CLI built in Rust (Edition 2024).

The workspace is organized as a Cargo workspace:

- **`gleon-core/`**: Core logic library crate (platform resolution, config parsing, diff engines, storage sync, licensing).
- **`gleon/`**: CLI binary wrapper crate (Clap argument parsing, displaying status, orchestrating runs).

### Important: Hidden Plan Files

- [Antigravity folder](.antigravity/) contain the product requirements, architectural details, and roadmap for Gleon.
- Files in that folder are listed in `.gitignore` to prevent exposing them in public repositories.
- **Do NOT** remove these files from `.gitignore` or expose their contents in public commits.
- **Do** read these files to understand the expected behavior and implementation steps for each task/phase.

## 2. Core Rules for AI Agents

1. **Rust Toolchain**: Use the latest stable Rust toolchain (currently 1.97+), Edition 2024.
2. **Quality & Formatting**:
   - Run `Cargo fmt --all` before completing tasks.
   - Run `Cargo clippy --workspace --all-targets -- -D warnings` to ensure there are no lints or compiler warnings.
   - Run `Cargo test --workspace` to verify all test suites pass.
3. **Rust Coding Practices**:
   - Favor pattern matching and switch expressions where appropriate.
   - Keep error handling robust using crate `thiserror` (as specified in the implementation plan).
   - Write clean, modular, and well-documented Rust code. Preserve existing comments and docstrings.
4. **Verifications**:
   - When adding a feature or fixing a bug, write corresponding unit tests.
   - Run local tests to verify your changes.
