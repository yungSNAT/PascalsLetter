use super::*;

#[test]
fn blank_lines_do_not_interrupt_grouping_or_count_as_matches() {
    let input = "\nexample.rs:7: first \n \t\nexample.rs:12: second\n\n";
    let block = crate::testutil::block(input, crate::block::ContentFormat::Matches);
    let ctx = MethodCtx { source: "grep" };

    let Outcome::Continue(output) = GrepGroup.apply(&block, &ctx) else {
        panic!("grep-group should continue with grouped matches");
    };

    assert_eq!(
        output,
        "2 matches in 1F:\n\n[file] example.rs (2):\n     7: first\n    12: second"
    );
}
