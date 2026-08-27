//! Minimal unified-diff generator used to preview staged edits (`view_diff`,
//! `/diff`) before they are applied.
//!
//! The algorithm trims common prefix/suffix lines first — surgical edits
//! leave almost everything else untouched — then runs an LCS dynamic program
//! over the remaining middle section only. If that middle section is too
//! large to diff cheaply it degrades to one coarse delete+insert block
//! instead of blowing up memory.

/// Lines of unchanged context shown around each hunk (unified-diff default).
const CONTEXT_LINES: usize = 3;
/// Largest middle section (per side) the LCS table runs on. Beyond this the
/// middle collapses to a single replace block: `N²` table cells stay bounded.
const MAX_LCS_SIDE: usize = 2000;

#[derive(Debug, PartialEq, Eq)]
enum Op<'a> {
    Same(&'a str),
    Del(&'a str),
    Add(&'a str),
}

fn split_lines(text: &str) -> Vec<&str> {
    // A trailing newline produces no phantom final line for diff purposes.
    text.lines().collect()
}

fn lcs_ops<'a>(old: &[&'a str], new: &[&'a str]) -> Vec<Op<'a>> {
    let n = old.len();
    let m = new.len();
    if n > MAX_LCS_SIDE || m > MAX_LCS_SIDE {
        let mut ops: Vec<Op<'_>> = old.iter().map(|l| Op::Del(l)).collect();
        ops.extend(new.iter().map(|l| Op::Add(l)));
        return ops;
    }
    // Flat table: table[i * stride + j] = LCS length of old[i..] vs new[j..].
    // A single contiguous allocation is more cache-friendly than Vec<Vec>.
    let stride = m + 1;
    let mut table = vec![0u32; (n + 1) * stride];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i * stride + j] = if old[i] == new[j] {
                table[(i + 1) * stride + (j + 1)] + 1
            } else {
                table[i * stride + (j + 1)].max(table[(i + 1) * stride + j])
            }
        }
    }
    let mut ops = Vec::new();
    let mut i = 0;
    let mut j = 0;
    while i < n && j < m {
        if old[i] == new[j] {
            ops.push(Op::Same(old[i]));
            i += 1;
            j += 1;
        } else if table[(i + 1) * stride + j] >= table[i * stride + (j + 1)] {
            ops.push(Op::Del(old[i]));
            i += 1;
        } else {
            ops.push(Op::Add(new[j]));
            j += 1;
        }
    }
    ops.extend(old[i..].iter().map(|l| Op::Del(l)));
    ops.extend(new[j..].iter().map(|l| Op::Add(l)));
    ops
}

/// Formats one range per unified-diff rules: `start` alone for a single
/// line, `start,count` otherwise, and the *preceding* position when empty.
fn range(start_1based: usize, len: usize) -> String {
    if len == 0 {
        format!("{},0", start_1based.saturating_sub(1))
    } else if len == 1 {
        format!("{start_1based}")
    } else {
        format!("{start_1based},{len}")
    }
}

/// Counts added/removed lines in a unified diff, ignoring the `+++`/`---`
/// file headers. Used by `/review` for per-file summaries.
pub fn count_changes(diff: &str) -> (usize, usize) {
    let mut added = 0usize;
    let mut removed = 0usize;
    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
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

/// Renders a unified diff between two texts for display to the user or the
/// model. Identical inputs yield an empty string.
pub fn unified_diff(path: &str, old: &str, new: &str) -> String {
    if old == new {
        return String::new();
    }
    let a = split_lines(old);
    let b = split_lines(new);

    // Trim the common head/tail so LCS sees only the edited middle.
    let mut prefix = 0usize;
    while prefix < a.len() && prefix < b.len() && a[prefix] == b[prefix] {
        prefix += 1;
    }
    let mut suffix = 0usize;
    while suffix < a.len() - prefix
        && suffix < b.len() - prefix
        && a[a.len() - 1 - suffix] == b[b.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let mid_a = &a[prefix..a.len() - suffix];
    let mid_b = &b[prefix..b.len() - suffix];

    let header = format!("--- a/{path}\n+++ b/{path}\n");
    if mid_a.is_empty() && mid_b.is_empty() {
        return header;
    }

    // The trimmed head/tail stay in the op stream as Same entries so hunk
    // line numbers and context windows cover the whole file uniformly.
    let mut ops: Vec<Op<'_>> = a[..prefix].iter().map(|l| Op::Same(l)).collect();
    ops.extend(lcs_ops(mid_a, mid_b));
    ops.extend(a[a.len() - suffix..].iter().map(|l| Op::Same(l)));

    // Indices of changed entries, grouped into hunks: a gap of more than
    // 2*CONTEXT equal lines between consecutive changes splits the hunks,
    // so their ±CONTEXT context windows never touch.
    let change_idxs: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| !matches!(op, Op::Same(_)))
        .map(|(i, _)| i)
        .collect();
    // old != new guarantees at least one Del/Add op exists.
    debug_assert!(!change_idxs.is_empty());

    let mut out = header;
    let mut group_start = 0usize;
    let mut old_ln = 1usize;
    let mut new_ln = 1usize;
    while group_start < change_idxs.len() {
        let mut group_end = group_start + 1;
        // Changes closer together than 2*CONTEXT lines share one hunk.
        while group_end < change_idxs.len()
            && change_idxs[group_end] - change_idxs[group_end - 1] <= 2 * CONTEXT_LINES
        {
            group_end += 1;
        }
        let first = change_idxs[group_start];
        let last = change_idxs[group_end - 1];
        let hunk_start = first.saturating_sub(CONTEXT_LINES);
        let hunk_end = (last + 1 + CONTEXT_LINES).min(ops.len());

        // Advance the line counters past anything before this hunk.
        for op in &ops[..hunk_start] {
            match op {
                Op::Same(_) => {
                    old_ln += 1;
                    new_ln += 1;
                }
                Op::Del(_) => old_ln += 1,
                Op::Add(_) => new_ln += 1,
            }
        }
        let old_first = old_ln;
        let new_first = new_ln;

        let mut old_len = 0usize;
        let mut new_len = 0usize;
        let mut body = String::new();
        for op in &ops[hunk_start..hunk_end] {
            match op {
                Op::Same(line) => {
                    old_len += 1;
                    new_len += 1;
                    body.push(' ');
                    body.push_str(line);
                    body.push('\n');
                }
                Op::Del(line) => {
                    old_len += 1;
                    body.push('-');
                    body.push_str(line);
                    body.push('\n');
                }
                Op::Add(line) => {
                    new_len += 1;
                    body.push('+');
                    body.push_str(line);
                    body.push('\n');
                }
            }
        }
        out.push_str(&format!(
            "@@ -{} +{} @@\n",
            range(old_first, old_len),
            range(new_first, new_len)
        ));
        out.push_str(&body);
        group_start = group_end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_changes_ignores_headers_and_hunk_lines() {
        let d = "--- a/f.rs\n+++ b/f.rs\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n";
        assert_eq!(count_changes(d), (1, 1));
        assert_eq!(count_changes(""), (0, 0));
    }

    #[test]
    fn identical_inputs_produce_empty_diff() {
        assert_eq!(unified_diff("f.rs", "same\n", "same\n"), "");
    }

    #[test]
    fn simple_replacement_hunk() {
        let d = unified_diff("f.rs", "a\nb\nc\n", "a\nB\nc\n");
        assert!(d.starts_with("--- a/f.rs\n+++ b/f.rs\n"), "{d}");
        assert!(d.contains("@@ -1,3 +1,3 @@"), "{d}");
        assert!(d.contains("-b\n+B\n"), "{d}");
        assert!(d.contains(" a\n"), "{d}");
    }

    #[test]
    fn insertion_appends_line() {
        let d = unified_diff("f.rs", "a\n", "a\nb\n");
        assert!(d.contains("+b\n"), "{d}");
        assert!(d.contains("@@ -1 +1,2 @@"), "{d}");
    }

    #[test]
    fn deletion_shows_removed_line() {
        let d = unified_diff("f.rs", "a\nb\nc\n", "a\nc\n");
        assert!(d.contains("-b\n"), "{d}");
        let added = d.lines().filter(|l| l.starts_with('+')).count();
        assert_eq!(added, 1, "{d}"); // only the '+++' header line
    }

    #[test]
    fn distant_changes_make_two_hunks() {
        let filler: String = (0..40).map(|i| format!("line{i}\n")).collect();
        let mut modified = filler.clone();
        modified = modified.replacen("line5", "CHANGED5", 1);
        modified = modified.replacen("line35", "CHANGED35", 1);
        let d = unified_diff("f.txt", &filler, &modified);
        assert_eq!(d.matches("@@").count(), 4, "{d}"); // 2 hunks × open+close
        assert!(d.contains("-line5\n+CHANGED5\n"), "{d}");
        assert!(d.contains("-line35\n+CHANGED35\n"), "{d}");
    }

    #[test]
    fn huge_middle_falls_back_to_block_replace() {
        let old: String = std::iter::repeat_n("x\n", MAX_LCS_SIDE + 10).collect();
        let new: String = std::iter::repeat_n("y\n", MAX_LCS_SIDE + 10).collect();
        let d = unified_diff("big.txt", &old, &new);
        assert!(d.contains("@@"), "{d}");
        assert_eq!(d.matches("\n-y\n").count(), 0); // all deletes are 'x'
        assert!(d.contains("+y\n"), "{d}");
    }

    #[test]
    fn new_file_diff_starts_at_zero() {
        let d = unified_diff("new.txt", "", "hello\nworld\n");
        assert!(d.contains("@@ -0,0 +1,2 @@"), "{d}");
        assert!(d.contains("+hello\n+world\n"), "{d}");
    }
}
