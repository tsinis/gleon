//! Core library for the gleon visual regression testing CLI.

pub mod cli;
pub mod config;
/// Resolves the effective run context (platform, branch, renderer) from CLI flags, config, and environment.
pub mod context;
pub mod engine;
pub mod env;
pub mod git;
pub mod io;
/// License validation and enforcement for gated features.
pub mod license;
pub mod manifest;
pub mod masking;
/// High-level workspace operations (init, stage, approve, diff, push, pull, etc.) invoked by the CLI.
pub mod ops;
/// Platform key resolution (os/arch/renderer/label) and conflict detection.
pub mod platform;
/// Rendering of run results into HTML, `JUnit` XML, markdown, and PR comment formats.
pub mod report;
pub mod scanner;
pub mod storage;
pub mod ui;
