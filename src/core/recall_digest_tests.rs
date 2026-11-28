use super::*;

fn meta<'a>(
    hash: &'a str,
    command: &'a str,
    shown_upto: usize,
    truncated: bool,
    exit_code: Option<i32>,
    byte_size: usize,
) -> DigestMeta<'a> {
    DigestMeta {
        hash,
        command,
        shown_upto,
        truncated,
        exit_code,
        byte_size,
    }
}

#[test]
fn test_slice_last_lines() {
    let b = b"l1\nl2\nl3\n";
    assert_eq!(slice_last_lines(b, 0), b"");
    assert_eq!(slice_last_lines(b"", 2), b"");
    assert_eq!(slice_last_lines(b, 1), b"l3\n");
    assert_eq!(slice_last_lines(b, 2), b"l2\nl3\n");
    assert_eq!(slice_last_lines(b, 3), b);
    assert_eq!(slice_last_lines(b, 99), b);
    assert_eq!(slice_last_lines(b"l1\nl2\nl3", 1), b"l3");
    assert_eq!(slice_last_lines(b"l1\nl2\nl3", 2), b"l2\nl3");
    assert_eq!(slice_last_lines(b"a\n\n", 1), b"\n");
    assert_eq!(slice_last_lines(b"\n", 1), b"\n");
}

#[test]
fn test_select_range_to_and_start_lines() {
    let b = b"l1\nl2\nl3\nl4\nl5\n";
    let full = RecallArgs {
        full: true,
        ..RecallArgs::default()
    };
    assert_eq!(select_range(b, 3, &full), (&b[..], 1));
    assert_eq!(
        select_range(b, 3, &RecallArgs::default()),
        (&b"l3\nl4\nl5\n"[..], 3)
    );
    let to2 = RecallArgs {
        to: Some(2),
        ..RecallArgs::default()
    };
    assert_eq!(select_range(b, 3, &to2), (&b"l1\nl2\n"[..], 1));
    let r2_4 = RecallArgs {
        from: Some(2),
        to: Some(4),
        ..RecallArgs::default()
    };
    assert_eq!(select_range(b, 3, &r2_4), (&b"l2\nl3\nl4\n"[..], 2));
    let r0_2 = RecallArgs {
        from: Some(0),
        to: Some(2),
        ..RecallArgs::default()
    };
    assert_eq!(select_range(b, 3, &r0_2), (&b"l1\nl2\n"[..], 1));
    let inv = RecallArgs {
        from: Some(4),
        to: Some(2),
        ..RecallArgs::default()
    };
    assert_eq!(select_range(b, 3, &inv), (&b""[..], 4));
}

#[test]
fn test_select_range_tail() {
    let b = b"l1\nl2\nl3\nl4\nl5\n";
    let t2 = RecallArgs {
        tail: Some(2),
        ..RecallArgs::default()
    };
    assert_eq!(select_range(b, 3, &t2), (&b"l4\nl5\n"[..], 4));
    let t99 = RecallArgs {
        tail: Some(99),
        ..RecallArgs::default()
    };
    assert_eq!(select_range(b, 3, &t99), (&b[..], 1));
    let t1 = RecallArgs {
        tail: Some(1),
        ..RecallArgs::default()
    };
    assert_eq!(select_range(b"a\nb\nc", 2, &t1), (&b"c"[..], 3));
}

#[test]
fn test_number_lines_preserves_bytes_and_crlf() {
    assert_eq!(number_lines(b"", 1), b"");
    assert_eq!(
        number_lines(b"one\r\ntwo\n\xff", 10),
        b"10:one\r\n11:two\n12:\xff"
    );
}

#[test]
fn test_grep_bytes_with_context_line_numbers_and_crlf() {
    let input = b"a\r\nmatch1\r\nc\nd\ne\nmatch2\nf\n";
    assert_eq!(
        grep_bytes_with(input, "match1$", 1, 0, false),
        b"match1\r\n"
    );
    assert_eq!(
        grep_bytes_with(input, "match", 10, 1, true),
        b"10-a\r\n11:match1\r\n12-c\n--\n14-e\n15:match2\n16-f\n"
    );
    let close = b"l1\nhit1\nmid\nhit2\nl5\n";
    assert_eq!(grep_bytes_with(close, "hit", 1, 1, false), close);
    assert_eq!(grep_bytes_with(close, "absent", 1, 2, true), b"");
}

#[test]
fn test_format_digest_short_clipped_and_binary() {
    let short_bytes = b"alpha\nbeta\ngamma\n";
    let short_meta = meta("1111111111111111", "ls", 1, false, None, short_bytes.len());
    let short_digest = String::from_utf8(format_digest(&short_meta, short_bytes)).unwrap();
    assert_eq!(
        short_digest,
        "[recall 1111111111111111 | ls | 3 lines | 17 B]\n1: alpha\n2: beta\n3: gamma\n"
    );

    let long_line = format!("{}\r\n", "é".repeat(205));
    let one_meta = meta(
        "2222222222222222",
        "cat",
        1,
        false,
        Some(0),
        long_line.len(),
    );
    let one_digest = String::from_utf8(format_digest(&one_meta, long_line.as_bytes())).unwrap();
    assert!(one_digest.starts_with("[recall 2222222222222222 | cat | 1 line | "));
    assert!(one_digest.contains(" | exit 0]\n"));
    assert!(one_digest.contains(&format!("1: {}…\n", "é".repeat(200))));
    assert!(one_digest.contains("[drill-down: rtk recall 2222222222222222 "));

    let bin = b"\x00\x01\xffbinary";
    let bin_meta = meta("6666666666666666", "cat", 1, false, None, bin.len());
    let bin_digest = String::from_utf8(format_digest(&bin_meta, bin)).unwrap();
    assert!(bin_digest.contains("| binary]\n"));
    assert!(bin_digest.contains("use `rtk recall 6666666666666666 --full`"));
}

#[test]
fn test_format_digest_long_quiet_and_single_signal() {
    let quiet: String = (1..=20).map(|i| format!("ok line {i}\n")).collect();
    let quiet_meta = meta("3333333333333333", "git-log", 11, false, None, quiet.len());
    let quiet_digest = String::from_utf8(format_digest(&quiet_meta, quiet.as_bytes())).unwrap();
    assert!(quiet_digest.contains("| hidden 11-20]\n"));
    assert!(quiet_digest.contains("... 10 lines omitted (lines 6-15)\n"));
    assert!(quiet_digest.contains(" 1: ok line 1\n"));
    assert!(quiet_digest.contains("20: ok line 20\n"));

    let one_sig: String = (1..=20)
        .map(|i| {
            if i == 10 {
                "TypeError: bad value\n".into()
            } else {
                format!("step {i}\n")
            }
        })
        .collect();
    let one_sig_meta = meta(
        "4444444444444444",
        "pytest",
        1,
        false,
        Some(1),
        one_sig.len(),
    );
    let one_sig_digest =
        String::from_utf8(format_digest(&one_sig_meta, one_sig.as_bytes())).unwrap();
    assert!(one_sig_digest.contains("... 10 lines omitted (lines 6-15); 1 diagnostic line:\n"));
    assert!(one_sig_digest.contains("10: TypeError: bad value\n"));
}

#[test]
fn test_format_digest_truncated_and_capped_signals() {
    let many_sigs: String = (1..=25)
        .map(|i| {
            if (6..=15).contains(&i) {
                format!("warning: issue {i}\n")
            } else {
                format!("ok {i}\n")
            }
        })
        .collect();
    let trunc_meta = meta("5555555555555555", "cargo", 3, true, Some(101), 99_999);
    let trunc_digest = String::from_utf8(format_digest(&trunc_meta, many_sigs.as_bytes())).unwrap();
    assert!(trunc_digest.contains("| exit 101 | truncated]\n"));
    assert!(!trunc_digest.contains("hidden"));
    assert!(
        trunc_digest.contains(
            "... 15 lines omitted (lines 6-20); 10 diagnostic lines (showing first 8):\n"
        )
    );
    assert!(trunc_digest.contains(" 6: warning: issue 6\n"));
    assert!(trunc_digest.contains("13: warning: issue 13\n"));
    assert!(!trunc_digest.contains("14: warning: issue 14\n"));
    assert!(trunc_digest.contains("... +2 more diagnostic lines (use --grep)\n"));
}
