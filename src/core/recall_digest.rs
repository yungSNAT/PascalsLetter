//! Structural handle digests and ranged/context recall formatting for `rtk recall`.

use super::retriever::{RecallArgs, count_lines, slice_first_lines, slice_from_line};
use std::io::Write;
use std::sync::LazyLock;

const DIGEST_HEAD_LINES: usize = 5;
const DIGEST_TAIL_LINES: usize = 5;
const DIGEST_MAX_SIGNALS: usize = 8;
const DIGEST_MAX_LINE_CHARS: usize = 200;

static DIGEST_SIGNAL_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)(?:(?:error|exception)\b|\b(?:fatal|panic(?:ked)?|fail(?:ed|ure)?|traceback|assert(?:ion)?|warn(?:ing)?)\b)",
    )
    .expect("static digest signal regex")
});

pub(super) struct DigestMeta<'a> {
    pub(super) hash: &'a str,
    pub(super) command: &'a str,
    pub(super) shown_upto: usize,
    pub(super) truncated: bool,
    pub(super) exit_code: Option<i32>,
    pub(super) byte_size: usize,
}

fn slice_last_lines(bytes: &[u8], n: usize) -> &[u8] {
    if n == 0 || bytes.is_empty() {
        return &[];
    }
    let body = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    let mut seen = 0usize;
    for (i, &b) in body.iter().enumerate().rev() {
        if b == b'\n' {
            seen += 1;
            if seen == n {
                return &bytes[i + 1..];
            }
        }
    }
    bytes
}

fn number_lines(input: &[u8], start_line: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    for (idx, line) in input.split_inclusive(|&b| b == b'\n').enumerate() {
        let _ = write!(out, "{}:", start_line + idx);
        out.extend_from_slice(line);
    }
    out
}

fn expand_context(matched: &[bool], context: usize) -> Vec<bool> {
    let mut include = matched.to_vec();
    if context == 0 {
        return include;
    }
    let last = matched.len().saturating_sub(1);
    for (i, &is_match) in matched.iter().enumerate() {
        if is_match {
            let lo = i.saturating_sub(context);
            let hi = i.saturating_add(context).min(last);
            include[lo..=hi].fill(true);
        }
    }
    include
}

#[cfg(test)]
pub(super) fn grep_bytes(input: &[u8], pattern: &str) -> Vec<u8> {
    grep_bytes_with(input, pattern, 1, 0, false)
}

fn grep_bytes_with(
    input: &[u8],
    pattern: &str,
    start_line: usize,
    context: usize,
    line_numbers: bool,
) -> Vec<u8> {
    use regex::bytes::Regex;
    let re = Regex::new(pattern)
        .or_else(|_| Regex::new(&regex::escape(pattern)))
        .ok();
    let lines: Vec<&[u8]> = input.split_inclusive(|&b| b == b'\n').collect();
    let matched: Vec<bool> = lines
        .iter()
        .map(|line| {
            let body = line.strip_suffix(b"\n").unwrap_or(line);
            let body = body.strip_suffix(b"\r").unwrap_or(body);
            re.as_ref().is_none_or(|r| r.is_match(body))
        })
        .collect();
    let include = expand_context(&matched, context);
    let mut out = Vec::new();
    let mut prev_emitted: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        if !include[i] {
            continue;
        }
        if context > 0 && prev_emitted.is_some_and(|p| i > p + 1) {
            out.extend_from_slice(b"--\n");
        }
        if line_numbers {
            let sep = if matched[i] { ':' } else { '-' };
            let _ = write!(out, "{}{}", start_line + i, sep);
        }
        out.extend_from_slice(line);
        prev_emitted = Some(i);
    }
    out
}

fn clip_digest_line(raw: &[u8]) -> (String, bool) {
    let stripped = raw.strip_suffix(b"\n").unwrap_or(raw);
    let stripped = stripped.strip_suffix(b"\r").unwrap_or(stripped);
    let text = String::from_utf8_lossy(stripped);
    if text.chars().count() <= DIGEST_MAX_LINE_CHARS {
        (text.into_owned(), false)
    } else {
        let head: String = text.chars().take(DIGEST_MAX_LINE_CHARS).collect();
        (format!("{head}…"), true)
    }
}

fn digest_header(meta: &DigestMeta<'_>, total_lines: usize) -> String {
    let line_word = if total_lines == 1 { "line" } else { "lines" };
    let mut header = format!(
        "recall {} | {} | {} {} | {} B",
        meta.hash,
        meta.command,
        total_lines,
        line_word,
        crate::core::utils::format_tokens(meta.byte_size),
    );
    if let Some(code) = meta.exit_code {
        header.push_str(&format!(" | exit {code}"));
    }
    if meta.shown_upto > 1 && !meta.truncated && meta.shown_upto <= total_lines {
        header.push_str(&format!(" | hidden {}-{}", meta.shown_upto, total_lines));
    }
    if meta.truncated {
        header.push_str(" | truncated");
    }
    header
}

fn write_digest_line(
    out: &mut Vec<u8>,
    idx: usize,
    raw: &[u8],
    width: usize,
    clipped_any: &mut bool,
) {
    let (text, clipped) = clip_digest_line(raw);
    *clipped_any |= clipped;
    let _ = writeln!(out, "{:>width$}: {text}", idx + 1);
}

fn write_digest_middle(out: &mut Vec<u8>, lines: &[&[u8]], width: usize, clipped_any: &mut bool) {
    let mid_start = DIGEST_HEAD_LINES;
    let mid_end = lines.len() - DIGEST_TAIL_LINES;
    let omitted_count = mid_end - mid_start;
    let signals: Vec<usize> = (mid_start..mid_end)
        .filter(|&idx| DIGEST_SIGNAL_RE.is_match(&String::from_utf8_lossy(lines[idx])))
        .collect();
    if signals.is_empty() {
        let _ = writeln!(
            out,
            "... {omitted_count} lines omitted (lines {}-{})",
            mid_start + 1,
            mid_end
        );
        return;
    }
    let sig_word = if signals.len() == 1 { "line" } else { "lines" };
    let shown_note = if signals.len() > DIGEST_MAX_SIGNALS {
        format!(" (showing first {DIGEST_MAX_SIGNALS})")
    } else {
        String::new()
    };
    let _ = writeln!(
        out,
        "... {omitted_count} lines omitted (lines {}-{}); {} diagnostic {sig_word}{shown_note}:",
        mid_start + 1,
        mid_end,
        signals.len()
    );
    for &idx in signals.iter().take(DIGEST_MAX_SIGNALS) {
        write_digest_line(out, idx, lines[idx], width, clipped_any);
    }
    if signals.len() > DIGEST_MAX_SIGNALS {
        let extra = signals.len() - DIGEST_MAX_SIGNALS;
        let _ = writeln!(out, "... +{extra} more diagnostic lines (use --grep)");
    }
}

fn format_digest(meta: &DigestMeta<'_>, full: &[u8]) -> Vec<u8> {
    let total_lines = count_lines(full);
    let header = digest_header(meta, total_lines);
    let mut out = Vec::new();
    if full.contains(&0) {
        let _ = writeln!(out, "[{header} | binary]");
        let _ = writeln!(
            out,
            "(binary payload; use `rtk recall {} --full` to retrieve raw bytes)",
            meta.hash
        );
        return out;
    }
    let _ = writeln!(out, "[{header}]");
    let lines: Vec<&[u8]> = full.split_inclusive(|&b| b == b'\n').collect();
    let width = total_lines.max(1).to_string().len();
    let mut clipped_any = false;
    let omitted = lines.len() > DIGEST_HEAD_LINES + DIGEST_TAIL_LINES + 2;
    if !omitted {
        for (idx, line) in lines.iter().enumerate() {
            write_digest_line(&mut out, idx, line, width, &mut clipped_any);
        }
    } else {
        for (idx, line) in lines.iter().enumerate().take(DIGEST_HEAD_LINES) {
            write_digest_line(&mut out, idx, line, width, &mut clipped_any);
        }
        write_digest_middle(&mut out, &lines, width, &mut clipped_any);
        let tail_start = lines.len() - DIGEST_TAIL_LINES;
        for (idx, line) in lines.iter().enumerate().skip(tail_start) {
            write_digest_line(&mut out, idx, line, width, &mut clipped_any);
        }
    }
    if omitted || clipped_any {
        let _ = writeln!(
            out,
            "[drill-down: rtk recall {} --from A --to B | --tail N | --grep PAT | --full]",
            meta.hash
        );
    }
    out
}

/// The lines `rtk recall` prints together with the 1-based line number where
/// the returned slice starts in `full`.
pub(super) fn select_range<'a>(
    full: &'a [u8],
    shown_upto: usize,
    args: &RecallArgs<'_>,
) -> (&'a [u8], usize) {
    if args.full {
        return (full, 1);
    }
    if let Some(n) = args.tail {
        let slice = slice_last_lines(full, n);
        let prefix_len = full.len() - slice.len();
        let start = count_lines(&full[..prefix_len]) + 1;
        return (slice, start);
    }
    let effective_lines = match args.to {
        Some(to) => {
            let start = args.from.unwrap_or(1).max(1);
            Some(to.checked_sub(start).map_or(0, |d| d.saturating_add(1)))
        }
        None => args.lines,
    };
    let (start_slice, start_line) = match (args.from, effective_lines) {
        (Some(n), _) => (slice_from_line(full, n), n.max(1)),
        (None, Some(_)) => (full, 1),
        (None, None) => (slice_from_line(full, shown_upto), shown_upto.max(1)),
    };
    let sliced = match effective_lines {
        Some(n) => slice_first_lines(start_slice, n),
        None => start_slice,
    };
    (sliced, start_line)
}

#[cfg(test)]
pub(crate) fn select_lines(
    full: &[u8],
    shown_upto: usize,
    all: bool,
    from: Option<usize>,
    lines: Option<usize>,
) -> &[u8] {
    let args = RecallArgs {
        full: all,
        from,
        lines,
        ..RecallArgs::default()
    };
    select_range(full, shown_upto, &args).0
}

pub(super) fn format_recall_output(
    meta: &DigestMeta<'_>,
    full: &[u8],
    args: &RecallArgs<'_>,
) -> Vec<u8> {
    if args.digest {
        return format_digest(meta, full);
    }
    let (sliced, start_line) = select_range(full, meta.shown_upto, args);
    match args.grep {
        Some(pat) => {
            if regex::bytes::Regex::new(pat).is_err() {
                eprintln!(
                    "rtk recall: note: --grep pattern is not a valid regex, matching it literally"
                );
            }
            grep_bytes_with(sliced, pat, start_line, args.context, args.line_numbers)
        }
        None if args.line_numbers => number_lines(sliced, start_line),
        None => sliced.to_vec(),
    }
}

#[cfg(test)]
#[path = "recall_digest_tests.rs"]
mod tests;
