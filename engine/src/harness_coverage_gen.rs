use super::*;

#[test]
fn hook_planning_requires_both_an_event_name_and_a_capability() {
    let used = [
        (Event::AfterTool, vec![Subject::Shell]),
        (Event::BeforeTool, vec![Subject::Shell]),
    ];
    let (planned, skipped) = plan_hooks(profile("openclaw").unwrap(), &used, false);
    assert_eq!(
        planned,
        [PlannedHook {
            event: "before_tool_call",
            which: Position::Tool(Event::BeforeTool),
            matcher: None,
        }]
    );
    assert_eq!(
        skipped,
        [SkippedHook {
            which: Position::Tool(Event::AfterTool),
            reason: "this agent cannot change a tool's output",
        }]
    );

    let claude = profile("claude-code").unwrap();
    // The after event has a name but no capability; the before event has the reverse.
    let incomplete = HarnessProfile {
        events: &[BEFORE_REPLACE_INPUT],
        wiring: Some(Wiring {
            before_tool: None,
            ..claude.wiring.unwrap()
        }),
        ..*claude
    };
    let (planned, skipped) = plan_hooks(&incomplete, &used, false);
    assert!(planned.is_empty());
    assert_eq!(
        skipped,
        [
            SkippedHook {
                which: Position::Tool(Event::AfterTool),
                reason: "this agent does not offer this event",
            },
            SkippedHook {
                which: Position::Tool(Event::BeforeTool),
                reason: "this agent does not offer this event",
            },
        ]
    );
}

#[test]
fn matcher_handles_wildcard_dynamic_and_unmapped_subjects() {
    let cursor = profile("cursor").unwrap();
    assert_eq!(matcher_for(cursor, &[Subject::Mcp, Subject::Any]), None);
    assert_eq!(
        matcher_for(cursor, &[Subject::Mcp]).as_deref(),
        Some("^(?:mcp__.*)$")
    );
    assert_eq!(matcher_for(cursor, &[Subject::Edit]), None);
}

#[test]
fn output_resolution_uses_directives_or_stays_inert_without_a_deny_hook() {
    assert!(profile("cursor").unwrap().can_replace_output());
    assert!(!profile("codex").unwrap().can_replace_output());

    let passive = HarnessProfile {
        events: &[EventCapability::NONE],
        directive_channels: &[Channel::PerTurn, Channel::AwarenessFile],
        ..*profile("claude-code").unwrap()
    };
    assert_eq!(
        resolve_output_transform(&passive, Subject::Shell, false),
        Resolution::Directive(Channel::PerTurn)
    );

    let no_directives = HarnessProfile {
        directive_channels: &[],
        ..passive
    };
    assert_eq!(
        resolve_output_transform(&no_directives, Subject::Shell, false),
        Resolution::Inert
    );
}
