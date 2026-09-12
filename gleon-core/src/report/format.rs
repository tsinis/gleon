//! Path-formatting helpers shared by the HTML and `JUnit` XML report generators.

use serde::{Serialize, Serializer};

/// Computes a relative path from `base` to `target`.
///
/// Precondition: `target` and `base` must share the same coordinate frame (both absolute or both relative).
/// If one path is absolute and the other is relative, returns `target` unchanged.
/// For example, if `target` is `.gleon/diffs/image.png` and `base` is `.gleon/reports`,
/// the result is `../diffs/image.png`.
/// Lexically normalizes a path's components: collapses `foo/../` pairs and drops `.` segments,
/// without touching the filesystem.
fn normalize_components(path: &std::path::Path) -> Vec<std::path::Component<'_>> {
    use std::path::Component;

    let mut normalized = Vec::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                if let Some(Component::Normal(_)) = normalized.last() {
                    normalized.pop();
                } else {
                    normalized.push(comp);
                }
            }
            Component::CurDir => {}
            _ => normalized.push(comp),
        }
    }
    normalized
}

/// Resolves `path` against the current working directory, if it is relative.
///
/// Meant to be called **once**, up front, on a `report_dir` before it is threaded through many
/// [`FormattedPath`]s — pre-absolutizing it here means every one of those can hit the
/// same-coordinate-frame fast path in [`make_relative_path`] instead of each independently
/// falling back to `current_dir()`. Falls back to returning `path` unchanged if the working
/// directory can't be read.
pub(super) fn to_absolute(path: &std::path::Path) -> std::path::PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
    }
}

pub(super) fn make_relative_path(
    target: &std::path::Path,
    base: &std::path::Path,
) -> std::path::PathBuf {
    #[cfg(windows)]
    use std::path::Component;
    use std::path::PathBuf;

    // Bring both paths into one coordinate frame first. `gleon report html --out report.html`
    // hands us a relative (often empty) report dir while the recorded image paths are absolute;
    // bailing out with the target unchanged would embed `file:///...` links in the artifact.
    let (absolute_target, absolute_base);
    let (target, base) = if target.is_absolute() == base.is_absolute() {
        (target, base)
    } else if let Ok(cwd) = std::env::current_dir() {
        absolute_target = if target.is_absolute() {
            target.to_path_buf()
        } else {
            cwd.join(target)
        };
        absolute_base = if base.is_absolute() {
            base.to_path_buf()
        } else {
            cwd.join(base)
        };
        (absolute_target.as_path(), absolute_base.as_path())
    } else {
        // No usable working directory to anchor against: leave the target untouched rather
        // than inventing a relationship between the two paths.
        return target.to_path_buf();
    };

    let mut target_comps = normalize_components(target).into_iter();
    let mut base_comps = normalize_components(base).into_iter();

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
        // Mixed frames are anchored to the working directory rather than bailing out with the
        // absolute target (which used to leak `file:///...` links into `--out` HTML reports).
        let cwd = std::env::current_dir().unwrap();
        let target = cwd.join("reports").join("img.png");
        let relative_base = PathBuf::from("reports");
        assert_eq!(
            make_relative_path(&target, &relative_base),
            PathBuf::from("img.png"),
            "relative base resolves against the cwd the report is written from"
        );

        // Nothing sensible to relate them by once anchoring is impossible is covered by the
        // same-frame path below.
        let outside = PathBuf::from("/definitely/not/under/cwd/img.png");
        let rel = make_relative_path(&outside, &relative_base);
        assert!(
            rel.is_relative(),
            "still produces a relative link, got {rel:?}"
        );

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
