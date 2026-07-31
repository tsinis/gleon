//! UI helpers for progress indicators and terminal feedback.

use indicatif::{ProgressBar, ProgressStyle};

/// Creates a standardized progress bar for batch operations.
pub fn create_progress_bar(total: u64) -> ProgressBar {
    let pb = ProgressBar::new(total);
    let template = "{pos}/{len} [{wide_bar}] {msg}";
    if let Ok(style) = ProgressStyle::default_bar().template(template) {
        pb.set_style(style.progress_chars("=>."));
    }
    pb
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(not(miri))]
    fn test_create_progress_bar() {
        let pb = create_progress_bar(100);
        pb.set_message("testing");
        pb.inc(1);
        assert_eq!(pb.position(), 1);
        pb.finish_and_clear();
    }
}
