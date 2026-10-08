use similar::TextDiff;

/// 生成标准的 Unified Diff 字符串
pub fn unified_diff(file_path: &str, before: &str, after: &str) -> String {
    if before == after {
        return String::new();
    }

    let diff = TextDiff::from_lines(before, after);
    diff.unified_diff()
        .context_radius(3)
        .header(&format!("a/{}", file_path), &format!("b/{}", file_path))
        .to_string()
}

/// 计算 Patch 中的增删统计
pub fn patch_stats(patch: &str) -> (usize, usize) {
    let mut added = 0;
    let mut removed = 0;
    for line in patch.lines() {
        if line.starts_with("+++") || line.starts_with("---") || line.starts_with("@@") {
            continue;
        }
        if line.starts_with('+') {
            added += 1;
        } else if line.starts_with('-') {
            removed += 1;
        }
    }
    (added, removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unified_diff_generation() {
        let before = "line1\nline2\nline3\n";
        let after = "line1\nline2_modified\nline3\nline4\n";
        let patch = unified_diff("src/test.txt", before, after);
        assert!(patch.contains("--- a/src/test.txt"));
        assert!(patch.contains("+++ b/src/test.txt"));
        assert!(patch.contains("-line2"));
        assert!(patch.contains("+line2_modified"));
        assert!(patch.contains("+line4"));

        let (added, removed) = patch_stats(&patch);
        assert_eq!(added, 2);
        assert_eq!(removed, 1);
    }
}
