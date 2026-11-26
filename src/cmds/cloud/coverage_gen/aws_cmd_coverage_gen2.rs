use super::*;
use serde_json::json;
use std::iter::{once, repeat_n};

fn assert_overflow<'a>(
    result: &FilterResult,
    visible_lines: impl IntoIterator<Item = &'a str>,
    summary: &str,
) {
    assert!(result.truncated, "missing truncation flag for {summary}");
    let mut actual = result.text.lines();
    for expected in visible_lines {
        assert_eq!(actual.next(), Some(expected), "{summary}");
    }
    assert_eq!(actual.next().map(str::trim), Some(summary));
    assert_eq!(actual.next(), None);
}

#[test]
fn bounded_inventory_filters_keep_the_first_items_and_report_overflow() {
    let total = MAX_ITEMS + 2;
    let count_header = format!("Count: {total}/{total}");
    let cases = [
        (
            filter_dynamodb_items as fn(&str) -> Option<FilterResult>,
            "Items",
            json!({"id": {"N": "7"}}),
            r#"{"id":7}"#,
            "items",
            Some(count_header.as_str()),
        ),
        (
            filter_ecs_tasks,
            "tasks",
            json!({
                "taskArn": "arn:aws:ecs:us-east-1:123:task/test-cluster/task-7",
                "lastStatus": "STOPPED",
                "containers": [{"name": "worker", "lastStatus": "STOPPED", "exitCode": 3}]
            }),
            "task-7 STOPPED containers:[worker:STOPPED(exit:3)]",
            "tasks",
            None,
        ),
        (
            filter_s3_objects,
            "Contents",
            json!({"Key": "report.csv", "Size": 1024, "LastModified": "2024-02-20T14:00:00Z"}),
            "report.csv 1.0 KB 2024-02-20",
            "objects",
            None,
        ),
        (
            filter_sqs_messages,
            "Messages",
            json!({
                "MessageId": "12345678-0000-4000-8000-000000000000",
                "Body": "queue fixture",
                "ReceiptHandle": "ignored-fixture-handle"
            }),
            "12345678 queue fixture",
            "messages",
            None,
        ),
    ];

    for (filter, key, item, expected_item, noun, header) in cases {
        let mut items = vec![item; MAX_ITEMS];
        items.resize(total, json!({}));
        let result = filter(&json!({(key): items}).to_string()).expect(key);
        let visible = header.into_iter().chain(repeat_n(expected_item, MAX_ITEMS));
        assert_overflow(&result, visible, &format!("… +2 more {noun}"));
    }
}

#[test]
fn security_groups_show_port_ranges_sources_and_empty_permissions() {
    let mut groups = vec![
        json!({
            "GroupName": "isolated",
            "GroupId": "sg-isolated",
            "IpPermissions": [],
            "IpPermissionsEgress": []
        }),
        json!({
            "GroupName": "workers",
            "GroupId": "sg-workers",
            "IpPermissions": [
                {
                    "IpProtocol": "tcp", "FromPort": 8000, "ToPort": 8010,
                    "IpRanges": [{"CidrIp": "192.0.2.0/24"}, {"Description": "no CIDR"}],
                    "Ipv6Ranges": [{"CidrIpv6": "2001:db8::/32"}, {"CidrIpv6": null}],
                    "UserIdGroupPairs": [{"GroupId": "sg-peer"}, {}]
                },
                {"IpProtocol": "udp", "FromPort": 53, "ToPort": 53}
            ],
            "IpPermissionsEgress": [{"IpProtocol": "-1"}]
        }),
    ];
    groups.resize(
        MAX_ITEMS,
        json!({"GroupName": "spare", "GroupId": "sg-spare"}),
    );
    groups.push(json!({"GroupName": "omitted", "GroupId": "sg-omitted"}));

    let result = filter_security_groups(&json!({"SecurityGroups": groups}).to_string()).unwrap();
    let visible = [
        "isolated (sg-isolated) ingress: none | egress: none",
        "workers (sg-workers) ingress: tcp/8000-8010<-192.0.2.0/24,2001:db8::/32,sg-peer,?, udp/53<-? | egress: all<-?",
    ]
    .into_iter()
    .chain(repeat_n("spare (sg-spare) ingress: none | egress: none", MAX_ITEMS - 2));
    assert_overflow(&result, visible, "… +1 more groups");
}

#[test]
fn logs_query_formats_scalar_fields_within_the_row_limit() {
    let row = json!([
        {"field": "@message", "value": "processed batch"},
        {"field": "count", "value": 3},
        {"field": "ok", "value": true},
        {"field": "@ptr", "value": "internal-pointer"},
        {"value": "missing field name"}
    ]);
    let mut rows = vec![row; MAX_ITEMS];
    rows[1] = json!({"malformed": "not a field array"});
    rows.push(json!([{"field": "@message", "value": "outside the preview"}]));

    let input = json!({"status": "Complete", "results": rows});
    let result = filter_logs_query_results(&input.to_string()).unwrap();
    let visible = once("Status: Complete").chain(repeat_n(
        "@message=processed batch count=3 ok=true",
        MAX_ITEMS - 1,
    ));
    assert_overflow(&result, visible, "… +1 more rows");

    assert!(filter_logs_query_results(r#"{"status":"Running"}"#).is_none());
}

#[test]
fn s3_transfer_summarizes_recognized_lines_and_preserves_unknown_output() {
    let operations = [
        "upload: local-a",
        "download: remote-b",
        "delete: old-c",
        "copy: source-d",
        "move: source-e",
        "move: source-f",
        "progress: checking remaining files",
    ];
    let errors: Vec<_> = once("error: remote unavailable".to_owned())
        .chain((1..=10).map(|n| format!("retry {n} failed")))
        .collect();
    let input = format!("{}\n{}\n", operations.join("\n"), errors.join("\n"));
    let result = filter_s3_transfer(&input);
    // The 11th error is dropped without a notice or the truncation flag
    // today, so only the summary and the first 10 errors are pinned.
    assert!(
        result.text.starts_with(&format!(
            "S3 transfer: 1 uploaded, 1 downloaded, 1 deleted, 1 copied, 2 moved, 11 errors\n{}",
            errors[..10].join("\n")
        )),
        "{}",
        result.text
    );

    let progress_only = "progress: waiting for move: pending\n".repeat(10);
    let result = filter_s3_transfer(&progress_only);
    assert_eq!(result.text, progress_only);
    assert!(!result.truncated);
}

#[test]
fn secrets_filter_handles_independently_missing_name_and_value() {
    let named = filter_secrets_get(r#"{"Name":"fixture","VersionId":"test-version"}"#).unwrap();
    assert_eq!(named.text, "Name: fixture");
    assert!(!named.truncated);

    let unnamed = filter_secrets_get(r#"{"SecretString":"{\n  \"demo\": true\n}"}"#).unwrap();
    assert_eq!(unnamed.text, r#"Secret: {"demo":true}"#);
    assert!(!unnamed.truncated);

    assert!(
        filter_secrets_get(r#"{"SecretBinary":"Zml4dHVyZQ==","VersionId":"test-version"}"#)
            .is_none()
    );
}
