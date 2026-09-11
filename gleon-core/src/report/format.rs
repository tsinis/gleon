//! Path-formatting helpers shared by the HTML and `JUnit` XML report generators.

use serde::{Serialize, Serializer};

/// Computes a relative path from `base` to `target`.
///
/// Precondition: `target` and `base` must share the same coordinate frame (both absolute or both relative).
/// If one path is absolute and the other is relative, returns `target` unchanged.
/// For example, if `target` is `.gleon/diffs/image.png` and `base` is `.gleon/reports`,
/// the result is `../diffs/image.png`.
pub(super) fn make_relative_path(
    target: &std::path::Path,
    base: &std::path::Path,
) -> std::path::PathBuf {
    use std::path::{Component, PathBuf};

    if target.is_absolute() != base.is_absolute() {
        return target.to_path_buf();
    }

    let mut norm_target = Vec::new();
    for comp in target.components() {
        match comp {
            Component::ParentDir => {
                if let Some(Component::Normal(_)) = norm_target.last() {
                    norm_target.pop();
                } else {
                    norm_target.push(comp);
                }
            }
            Component::CurDir => {}
            _ => norm_target.push(comp),
        }
    }

    let mut norm_base = Vec::new();
    for comp in base.components() {
        match comp {
            Component::ParentDir => {
                if let Some(Component::Normal(_)) = norm_base.last() {
                    norm_base.pop();
                } else {
                    norm_base.push(comp);
                }
            }
            Component::CurDir => {}
            _ => norm_base.push(comp),
        }
    }

    let mut target_comps = norm_target.into_iter();
    let mut base_comps = norm_base.into_iter();

    #[cfg(windows)]
    if let (Some(Component::Prefix(p1)), Some(Component::Prefix(p2))) =
        (target_comps.clone().next(), base_comps.clone().next())
    {
        if p1 != p2 {
            return target.to_path_buf();
        }
    }

    let mut target_comp = target_comps.next();
    let mut base_comp = base_comps.next();

    while let (Some(t), Some(b)) = (target_comp, base_comp) {
        if t == b {
            target_comp = target_comps.next();
            base_comp = base_comps.next();
        } else {
            break;
        }
    }

    let mut rel = PathBuf::new();

    if base_comp.is_some() {
        rel.push("..");
        for _ in base_comps {
            rel.push("..");
        }
    }

    if let Some(t) = target_comp {
        rel.push(t);
        for comp in target_comps {
            rel.push(comp);
        }
    }

    if rel.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        rel
    }
}

/// Zero-copy serialization wrapper formatting a path relative to `report_dir` (or absolute/as-is
/// if `report_dir` is `None`), always using forward slashes so links work cross-platform.
pub(super) struct FormattedPath<'a> {
    pub(super) path: &'a std::path::Path,
    pub(super) report_dir: Option<&'a std::path::Path>,
}

impl std::fmt::Display for FormattedPath<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use std::path::Component;
        let path_to_format = self
            .report_dir
            .map_or(std::borrow::Cow::Borrowed(self.path), |base| {
                std::borrow::Cow::Owned(make_relative_path(self.path, base))
            });

        let mut first = true;
        let mut last_was_slash = false;
        let mut has_output = false;

        for comp in path_to_format.components() {
            if !first
                && !last_was_slash
                && !matches!(comp, Component::RootDir | Component::Prefix(_))
            {
                write!(f, "/")?;
            }
            first = false;
            match comp {
                Component::Normal(os_str) => {
                    write!(f, "{}", os_str.to_string_lossy())?;
                    last_was_slash = false;
                    has_output = true;
                }
                Component::ParentDir => {
                    write!(f, "..")?;
                    last_was_slash = false;
                    has_output = true;
                }
                Component::CurDir => {
                    write!(f, ".")?;
                    last_was_slash = false;
                    has_output = true;
                }
                Component::RootDir => {
                    write!(f, "/")?;
                    last_was_slash = true;
                    has_output = true;
                }
                Component::Prefix(prefix) => {
                    write!(f, "{}", prefix.as_os_str().to_string_lossy())?;
                    last_was_slash = false;
                    first = true;
                    has_output = true;
                }
            }
        }
        if !has_output {
            write!(f, ".")?;
        }
        Ok(())
    }
}

impl Serialize for FormattedPath<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::pedantic,
    clippy::nursery
)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_make_relative_path() {
        let target = PathBuf::from(".gleon/diffs/billing/form.png");
        let base = PathBuf::from(".gleon/reports");
        let rel = make_relative_path(&target, &base);
        assert_eq!(rel, PathBuf::from("../diffs/billing/form.png"));
    }

    #[test]
    fn test_make_relative_path_edge_cases() {
        let abs = PathBuf::from("/a/b/c");
        let rel = PathBuf::from("a/b/c");
        // Mixed absolute and relative returns target unchanged
        assert_eq!(make_relative_path(&abs, &rel), abs);

        #[cfg(windows)]
        {
            // Prefix mismatches on Windows
            let mut p1 = PathBuf::new();
            p1.push("C:\\a\\b");
            let mut p2 = PathBuf::new();
            p2.push("D:\\a\\b");
            assert_eq!(super::make_relative_path(&p1, &p2), p1);
        }
    }

    #[test]
    fn test_make_relative_path_curdir_normalization() {
        let target = PathBuf::from("./a/./b/../c");
        let base = PathBuf::from("./a/./d/../e");
        let rel = make_relative_path(&target, &base);
        assert!(!rel.to_string_lossy().is_empty());
    }

    #[test]
    fn test_make_relative_path_lexical_normalization() {
        // Test that `..` is correctly normalized lexically without touching FS.
        let base = PathBuf::from("runs/latest");
        let target = PathBuf::from("runs/latest/../baseline/auth_login.png");
        let expected = PathBuf::from("../baseline/auth_login.png");
        assert_eq!(make_relative_path(&target, &base), expected);

        let target2 = PathBuf::from("baseline/auth_login.png");
        let base2 = PathBuf::from("runs/latest");
        let expected2 = PathBuf::from("../../baseline/auth_login.png");
        assert_eq!(make_relative_path(&target2, &base2), expected2);

        let target3 = PathBuf::from("../outside/image.png");
        let base3 = PathBuf::from("reports");
        let expected3 = PathBuf::from("../../outside/image.png");
        assert_eq!(make_relative_path(&target3, &base3), expected3);
    }

    #[test]
    fn test_formatted_path_display() {
        let path1 = std::path::Path::new("foo/bar/baz.png");
        assert_eq!(
            FormattedPath {
                path: path1,
                report_dir: None
            }
            .to_string(),
            "foo/bar/baz.png"
        );

        #[cfg(windows)]
        {
            let path2 = std::path::Path::new("C:\\foo\\bar.png");
            let formatted2 = FormattedPath {
                path: path2,
                report_dir: None,
            }
            .to_string();
            assert_eq!(formatted2, "C:/foo/bar.png");
        }
    }

    #[test]
    fn test_formatted_path_all_components() {
        let root_path = std::path::Path::new("/a/.././b");
        let formatted = FormattedPath {
            path: root_path,
            report_dir: None,
        }
        .to_string();
        assert!(formatted.contains('a'));

        let empty_path = std::path::Path::new("");
        let formatted_empty = FormattedPath {
            path: empty_path,
            report_dir: None,
        }
        .to_string();
        assert_eq!(formatted_empty, ".");
    }
}
