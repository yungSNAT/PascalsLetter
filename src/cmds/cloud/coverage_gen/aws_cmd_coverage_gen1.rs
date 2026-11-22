use super::*;
use serde_json::json;

type FilterFn = fn(&str) -> Option<FilterResult>;

fn many(key: &str, item: &str, n: usize) -> String {
    let items: Vec<&str> = std::iter::repeat_n(item, n).collect();
    format!(r#"{{"{}":[{}]}}"#, key, items.join(","))
}

#[test]
fn list_filters_report_truncation_when_over_cap() {
    let cases: Vec<(FilterFn, &str, &str, &str)> = vec![
        (
            filter_ecs_list_services,
            "serviceArns",
            r#""arn:aws:ecs:r:1:service/c/s""#,
            "services",
        ),
        (
            filter_ecs_describe_services,
            "services",
            r#"{"serviceName":"s"}"#,
            "services",
        ),
        (
            filter_cfn_list_stacks,
            "StackSummaries",
            r#"{"StackName":"st"}"#,
            "stacks",
        ),
        (
            filter_cfn_describe_stacks,
            "Stacks",
            r#"{"StackName":"st"}"#,
            "stacks",
        ),
        (
            filter_lambda_list,
            "Functions",
            r#"{"FunctionName":"f"}"#,
            "functions",
        ),
        (filter_iam_roles, "Roles", r#"{"RoleName":"r"}"#, "roles"),
        (filter_iam_users, "Users", r#"{"UserName":"u"}"#, "users"),
    ];
    for (f, key, item, noun) in cases {
        let json = many(key, item, MAX_ITEMS + 2);
        let r = f(&json).expect("filter must parse valid JSON");
        assert!(r.truncated, "{key} should be truncated");
        assert!(
            r.text.contains(&format!("+2 more {}", noun)),
            "{key}: unexpected text {}",
            r.text
        );
    }
}

#[test]
fn ec2_truncates_and_skips_reservations_without_instances() {
    let inst: Vec<String> = (0..MAX_ITEMS + 3)
        .map(|i| format!(r#"{{"InstanceId":"i-{}"}}"#, i))
        .collect();
    let json = format!(
        r#"{{"Reservations":[{{}},{{"Instances":[{}]}}]}}"#,
        inst.join(",")
    );
    let r = filter_ec2_instances(&json).unwrap();
    assert!(r.truncated);
    assert!(
        r.text
            .starts_with(&format!("EC2: {} instances", MAX_ITEMS + 3))
    );
    assert!(r.text.contains("… +3 more"));
    assert!(r.text.contains("i-0 ? ? - pub:- vpc:- subnet:- sg:[-] (-)"));
    assert!(!r.text.contains(&format!("i-{} ", MAX_ITEMS + 1)));
}

#[test]
fn logs_events_handle_missing_timestamps_and_broken_json_messages() {
    let json = r#"{"events":[{"message":"{broken json"},{"timestamp":0,"message":"plain"}]}"#;
    let r = filter_logs_events(json).unwrap();
    assert_eq!(r.text, "??:??:?? {broken json\n??:??:?? plain");
    assert!(!r.truncated);
}

#[test]
fn cfn_events_truncate_large_streams_and_omit_empty_reason() {
    let mut events: Vec<String> = (0..100)
        .map(|i| {
            format!(
                r#"{{"Timestamp":"2024-01-15T10:30:00Z","LogicalResourceId":"Ok{}","ResourceType":"AWS::S3::Bucket","ResourceStatus":"CREATE_COMPLETE"}}"#,
                i
            )
        })
        .collect();
    events.push(
        r#"{"Timestamp":"2024-01-15T10:31:00Z","LogicalResourceId":"Fn","ResourceType":"AWS::Lambda::Function","ResourceStatus":"UPDATE_ROLLBACK_COMPLETE"}"#
            .to_string(),
    );
    let json = format!(r#"{{"StackEvents":[{}]}}"#, events.join(","));
    let r = filter_cfn_events(&json).unwrap();
    assert!(r.truncated);
    assert!(r.text.contains("101 events (1 failed, 100 successful)"));
    assert!(
        r.text
            .contains("Fn Lambda::Function UPDATE_ROLLBACK_COMPLETE")
    );
    assert!(!r.text.contains("REASON"));
    assert!(r.text.contains("+ 100 successful resources"));
}

#[test]
fn lambda_get_handles_odd_layer_arns_and_missing_fields() {
    let json = r#"{"Configuration":{"Layers":[{"Arn":"plainlayer"},{"Arn":"a:b"},{}]}}"#;
    let r = filter_lambda_get(json).unwrap();
    assert_eq!(r.text, "? ? ? 0MB 0s active ?\n  layers: plainlayer, a:b");
    assert!(!r.truncated);
}

#[test]
fn iam_roles_extract_all_principal_shapes() {
    let json = r#"{"Roles":[
        {"RoleName":"star","AssumeRolePolicyDocument":{"Statement":[{"Principal":"*"}]}},
        {"RoleName":"multi","AssumeRolePolicyDocument":{"Statement":[{"Principal":{"Service":["a.amazonaws.com",5,"b.amazonaws.com"]}}]}},
        {"RoleName":"acct","AssumeRolePolicyDocument":{"Statement":[{"Principal":{"AWS":"111122223333"}}]}},
        {"RoleName":"accts","AssumeRolePolicyDocument":{"Statement":[{"Principal":{"AWS":["111111111111",null,"222222222222"]}}]}},
        {"RoleName":"fed","AssumeRolePolicyDocument":{"Statement":[{"Principal":{"Federated":"cognito"}}]}},
        {"RoleName":"bare","CreateDate":"2024-03-01T00:00:00Z","Description":"d"}
    ]}"#;
    let r = filter_iam_roles(json).unwrap();
    assert!(!r.truncated);
    let lines: Vec<&str> = r.text.lines().collect();
    assert!(lines.contains(&"star ? assume:[*]"), "{}", r.text);
    assert!(
        lines.contains(&"multi ? assume:[a.amazonaws.com,b.amazonaws.com]"),
        "{}",
        r.text
    );
    assert!(
        lines.contains(&"acct ? assume:[111122223333]"),
        "{}",
        r.text
    );
    assert!(
        lines.contains(&"accts ? assume:[111111111111,222222222222]"),
        "{}",
        r.text
    );
    assert!(lines.contains(&"fed ?"), "{}", r.text);
    assert!(lines.contains(&"bare 2024-03-01 [d]"), "{}", r.text);
}

#[test]
fn unwrap_dynamodb_value_edge_cases() {
    assert_eq!(unwrap_dynamodb_value(&json!({"N": "abc"}), 0), json!("abc"));
    assert_eq!(unwrap_dynamodb_value(&json!({"N": 5}), 0), json!(5));
    assert_eq!(
        unwrap_dynamodb_value(&json!({"L": "x"}), 0),
        json!({"L": "x"})
    );
    assert_eq!(unwrap_dynamodb_value(&json!({"M": 3}), 0), json!({"M": 3}));
    assert_eq!(
        unwrap_dynamodb_value(&json!({"NS": ["1", "2.5", "abc"]}), 0),
        json!([1, 2.5, "abc"])
    );
    assert_eq!(unwrap_dynamodb_value(&json!({"NS": "x"}), 0), json!("x"));
    assert_eq!(
        unwrap_dynamodb_value(&json!({"BS": ["AQ=="]}), 0),
        json!(["AQ=="])
    );
    assert_eq!(
        unwrap_dynamodb_value(&json!({"X": {"S": "y"}}), 0),
        json!({"X": "y"})
    );
    assert_eq!(unwrap_dynamodb_value(&json!(7), 0), json!(7));
    assert_eq!(unwrap_dynamodb_value(&json!("s"), 0), json!("s"));

    // Shallow nesting is fully unwrapped.
    let shallow = json!({"a": {"b": {"S": "deep"}}});
    assert_eq!(
        unwrap_dynamodb_value(&shallow, 0),
        json!({"a": {"b": "deep"}})
    );

    // Very deep nesting stops unwrapping past the depth limit.
    let mut deep = json!({"S": "deep"});
    for _ in 0..12 {
        deep = json!({ "k": deep });
    }
    let out = serde_json::to_string(&unwrap_dynamodb_value(&deep, 0)).unwrap();
    assert!(out.contains(r#"{"S":"deep"}"#), "{}", out);
}

#[test]
fn dynamodb_items_truncate_over_cap() {
    let json = many("Items", r#"{"id":{"N":"1"}}"#, MAX_ITEMS + 2);
    let r = filter_dynamodb_items(&json).unwrap();
    assert!(r.truncated);
    let total = MAX_ITEMS + 2;
    assert!(r.text.starts_with(&format!("Count: {}/{}", total, total)));
    assert!(r.text.ends_with("… +2 more items"));
    assert_eq!(r.text.lines().count(), 1 + MAX_ITEMS + 1);
}

#[cfg(unix)]
fn run_with_fake_aws(body: &str, args: &[&str]) -> std::process::Output {
    use std::os::unix::fs::PermissionsExt;
    let dir = crate::core::test_isolation::tempdir();
    let script = dir.path().join("aws");
    std::fs::write(&script, format!("#!/bin/sh\n{}\n", body)).unwrap();
    let mut perms = std::fs::metadata(&script).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script, perms).unwrap();

    let mut cmd = crate::core::test_isolation::rtk_command();
    let inherited = cmd
        .get_envs()
        .find(|(k, _)| *k == std::ffi::OsStr::new("PATH"))
        .and_then(|(_, v)| v.map(|v| v.to_os_string()));
    let mut path = dir.path().as_os_str().to_os_string();
    if let Some(p) = inherited {
        path.push(":");
        path.push(p);
    }
    cmd.env("PATH", path);
    cmd.args(args);
    let out = cmd.output().expect("failed to spawn rtk");
    drop(dir);
    out
}

#[cfg(unix)]
#[test]
fn binary_sts_rewrites_output_flag_and_filters() {
    if !crate::core::test_isolation::rtk_binary_is_built() {
        return;
    }
    let argdir = crate::core::test_isolation::tempdir();
    let argfile = argdir.path().join("args.txt");
    let body = format!(
        "printf '%s\\n' \"$@\" > '{}'\nprintf '%s\\n' '{{\"Account\":\"111122223333\",\"Arn\":\"arn:aws:iam::111122223333:user/dev\"}}'",
        argfile.display()
    );
    let out = run_with_fake_aws(
        &body,
        &[
            "-v",
            "aws",
            "sts",
            "get-caller-identity",
            "--output",
            "table",
            "--output=text",
            "--region",
            "us-east-1",
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {}", stderr);
    assert!(stdout.contains("AWS: 111122223333"), "stdout: {}", stdout);
    assert!(
        stderr.contains("Running: aws sts get-caller-identity"),
        "stderr: {}",
        stderr
    );
    let recorded = std::fs::read_to_string(&argfile).unwrap();
    let lines: Vec<&str> = recorded.lines().collect();
    assert_eq!(
        lines,
        vec![
            "sts",
            "get-caller-identity",
            "--region",
            "us-east-1",
            "--output",
            "json"
        ]
    );
}

#[cfg(unix)]
#[test]
fn binary_filtered_failure_none_and_truncation_paths() {
    if !crate::core::test_isolation::rtk_binary_is_built() {
        return;
    }
    // Failure: exit code and stderr propagate.
    let out = run_with_fake_aws(
        "echo boom-sts >&2\nexit 3",
        &["aws", "sts", "get-caller-identity"],
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("boom-sts"));

    // Filter returns None: raw output passes through with a warning.
    let out = run_with_fake_aws(
        "echo not-json-output",
        &["aws", "ec2", "describe-instances"],
    );
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("not-json-output"));
    assert!(String::from_utf8_lossy(&out.stderr).contains("filter warning"));

    // Truncated result.
    let arns: Vec<String> = (0..MAX_ITEMS + 10)
        .map(|i| format!("\"arn:aws:ecs:r:1:service/c/svc-{}\"", i))
        .collect();
    let body = format!("printf '%s\\n' '{{\"serviceArns\":[{}]}}'", arns.join(","));
    let out = run_with_fake_aws(&body, &["aws", "ecs", "list-services"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(stdout.contains("svc-0"), "stdout: {}", stdout);
    assert!(stdout.contains("+10 more services"), "stdout: {}", stdout);
}

#[cfg(unix)]
#[test]
fn binary_s3_ls_paths() {
    if !crate::core::test_isolation::rtk_binary_is_built() {
        return;
    }
    let n = MAX_ITEMS + 13;
    let body: String = (0..n)
        .map(|i| format!("echo '2024-01-01 bucket-{}'\n", i))
        .collect();
    let out = run_with_fake_aws(&body, &["-v", "aws", "s3", "ls"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(stdout.contains("bucket-0"), "stdout: {}", stdout);
    assert!(stdout.contains("+3 more items"), "stdout: {}", stdout);
    assert!(String::from_utf8_lossy(&out.stderr).contains("Running: aws s3 ls"));

    let out = run_with_fake_aws("echo '2024-01-01 only-bucket'", &["aws", "s3", "ls"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("only-bucket"));

    let out = run_with_fake_aws("echo boom-ls >&2\nexit 2", &["aws", "s3", "ls"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("boom-ls"));
}

#[cfg(unix)]
#[test]
fn binary_s3_transfer_paths() {
    if !crate::core::test_isolation::rtk_binary_is_built() {
        return;
    }
    let body: String = (0..12)
        .map(|i| format!("echo 'upload: f{}.txt to s3://b/f{}.txt'\n", i, i))
        .collect();
    let out = run_with_fake_aws(&body, &["-v", "aws", "s3", "cp", "src", "s3://b/"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(
        stdout.contains("S3 transfer: 12 uploaded, 0 errors"),
        "stdout: {}",
        stdout
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("Running: aws s3 cp"));

    let out = run_with_fake_aws(
        "echo boom-sync >&2\nexit 4",
        &["aws", "s3", "sync", "a", "b"],
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("boom-sync"));
}
