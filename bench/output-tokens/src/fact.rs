//! When a fact stands whole in a line, shared by the facts check and the `ab` grader.

/// How many times `fact` stands whole in `line`: no letter, digit, `_`, `.` or `-` just
/// before it and no letter, digit or `_` just after, so `io.rs` is in neither `old.io.rs`
/// nor `io.rsx`. A short hash may run on into more hex digits, as its full hash does; git
/// abbreviates a hash only as far as it stays unique in the repository.
pub fn copies(line: &str, fact: &str) -> usize {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let short_hash = fact.len() >= 7 && fact.chars().all(|c| c.is_ascii_hexdigit());
    line.match_indices(fact)
        .filter(|(at, _)| {
            let mut after = &line[at + fact.len()..];
            if short_hash {
                after = after.trim_start_matches(|c: char| c.is_ascii_hexdigit());
            }
            !line[..*at].ends_with(|c: char| word(c) || c == '.' || c == '-')
                && !after.starts_with(word)
        })
        .count()
}
