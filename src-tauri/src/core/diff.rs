use similar::{ChangeTag, TextDiff};

pub fn unified_diff(old: &str, new: &str) -> String {
    let diff = TextDiff::from_lines(old, new);
    let mut output = String::new();

    for change in diff.iter_all_changes() {
        let prefix = match change.tag() {
            ChangeTag::Delete => "-",
            ChangeTag::Insert => "+",
            ChangeTag::Equal => " ",
        };
        output.push_str(prefix);
        output.push_str(change.value());
        if !change.value().ends_with('\n') {
            output.push('\n');
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::unified_diff;

    #[test]
    fn diff_separates_non_newline_terminated_changes() {
        let diff = unified_diff("hello", "hello world");

        assert!(diff.contains("-hello"));
        assert!(diff.contains("+hello world"));
        assert!(diff.contains("-hello\n+hello world"));
    }
}
