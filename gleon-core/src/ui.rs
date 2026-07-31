//! UI helpers for progress indicators and terminal feedback.

use indicatif::{ProgressBar, ProgressStyle};

/// Creates a standardized progress bar for batch operations.
pub fn create_progress_bar(total: u64) -> ProgressBar {
    let pb = ProgressBar::new(total);
    let template = "{pos}/{len} [{wide_bar}] {msg}";
    match ProgressStyle::default_bar().template(template) {
        Ok(style) => pb.set_style(style.progress_chars("=>.")),
        Err(e) => tracing::debug!("Failed to build progress bar style: {}", e),
    }
    pb
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;

    #[test]
    fn test_create_progress_bar() {
        let pb = create_progress_bar(100);
        pb.set_message("testing");
        pb.inc(1);
        assert_eq!(pb.position(), 1);
        pb.finish_and_clear();
    }
}
