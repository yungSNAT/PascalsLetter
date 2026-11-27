#![cfg(feature = "engine")]

use super::{claude_matcher, claude_tools_for_subject};
use rtk_engine::block::Subject;

#[test]
fn claude_subjects_map_to_their_tool_names() {
    let cases: &[(Subject, &[&str])] = &[
        (Subject::Write, &["Write"]),
        (Subject::Edit, &["Edit"]),
        (Subject::Subagent, &["Task", "Agent"]),
        (Subject::Mcp, &["mcp__.*"]),
        (
            Subject::Any,
            &[
                "Bash",
                "Read",
                "Write",
                "Edit",
                "Grep",
                "Glob",
                "WebFetch",
                "WebSearch",
                "Task",
                "Agent",
            ],
        ),
    ];

    for &(subject, expected) in cases {
        assert_eq!(claude_tools_for_subject(subject), expected);
    }
}

#[test]
fn claude_matcher_deduplicates_overlapping_subjects() {
    let subjects = [
        Subject::Subagent,
        Subject::Edit,
        Subject::Any,
        Subject::Write,
    ];

    assert_eq!(
        claude_matcher(&subjects).as_deref(),
        Some("Bash|Read|Write|Edit|Grep|Glob|WebFetch|WebSearch|Task|Agent")
    );
}
