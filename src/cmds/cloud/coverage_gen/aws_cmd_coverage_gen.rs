use super::*;

fn owned(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn structured_operation_detection_covers_all_branches() {
    for op in ["sync", "cp"] {
        assert!(
            !is_structured_operation(&owned(&[op, "--dryrun"])),
            "{op} must be treated as a text transfer operation"
        );
    }
    for op in [
        "describe-vpcs",
        "list-buckets",
        "get-bucket-policy",
        "scan",
        "query",
        "receive-message",
    ] {
        assert!(
            is_structured_operation(&owned(&[op])),
            "{op} must be treated as a structured JSON operation"
        );
    }
    for op in ["deploy", "mb", "put-item"] {
        assert!(
            !is_structured_operation(&owned(&[op])),
            "{op} must not get --output json injected"
        );
    }
    assert!(!is_structured_operation(&[]));
}

#[cfg(unix)]
mod via_binary {
    use crate::core::test_isolation::{rtk_binary_is_built, rtk_command, tempdir};
    use std::ffi::OsString;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process::Output;

    /// Verbose top-level metadata that every specialised filter must drop.
    const PAD: &str = r#""ResponseMetadata":{"RequestId":"0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0","HTTPStatusCode":200,"RetryAttempts":0,"HTTPHeaders":{"x-amzn-requestid":"0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0","content-type":"application/x-amz-json-1.1","date":"Mon, 15 Jan 2024 10:30:00 GMT"}}"#;

    struct Case {
        key: &'static str,
        json: &'static str,
        expect: &'static str,
    }

    const CASES: &[Case] = &[
        Case {
            key: "sts get-caller-identity",
            json: r#"{"UserId":"AIDAEXAMPLE","Account":"123456789012","Arn":"arn:aws:iam::123456789012:user/dev"}"#,
            expect: "AWS: 123456789012 arn:aws:iam::123456789012:user/dev",
        },
        Case {
            key: "ec2 describe-instances",
            json: r#"{"Reservations":[{"Instances":[{"InstanceId":"i-0abc","State":{"Name":"running"},"InstanceType":"t3.micro"}]}]}"#,
            expect: "i-0abc running t3.micro",
        },
        Case {
            key: "ecs list-services",
            json: r#"{"serviceArns":["arn:aws:ecs:us-east-1:123:service/cluster/api-service"]}"#,
            expect: "api-service",
        },
        Case {
            key: "ecs describe-services",
            json: r#"{"services":[{"serviceName":"api","status":"ACTIVE","runningCount":3,"desiredCount":3,"launchType":"FARGATE"}]}"#,
            expect: "api ACTIVE 3/3 (FARGATE)",
        },
        Case {
            key: "rds describe-db-instances",
            json: r#"{"DBInstances":[{"DBInstanceIdentifier":"mydb","Engine":"postgres","EngineVersion":"15.4","DBInstanceClass":"db.t3.micro","DBInstanceStatus":"available","Endpoint":{"Address":"mydb.example.com","Port":5432}}]}"#,
            expect: "mydb postgres 15.4 db.t3.micro available mydb.example.com:5432",
        },
        Case {
            key: "cloudformation list-stacks",
            json: r#"{"StackSummaries":[{"StackName":"my-stack","StackStatus":"CREATE_COMPLETE","CreationTime":"2024-01-15T10:30:00Z"}]}"#,
            expect: "my-stack CREATE_COMPLETE 2024-01-15",
        },
        Case {
            key: "cloudformation describe-stacks",
            json: r#"{"Stacks":[{"StackName":"prod-stack","StackStatus":"UPDATE_COMPLETE","CreationTime":"2024-02-20T14:00:00Z","Outputs":[{"OutputKey":"Url","OutputValue":"https://x.example"}]}]}"#,
            expect: "Url=https://x.example",
        },
        Case {
            key: "cloudformation describe-stack-events",
            json: r#"{"StackEvents":[{"Timestamp":"2024-01-15T10:30:00Z","LogicalResourceId":"Bucket","ResourceType":"AWS::S3::Bucket","ResourceStatus":"CREATE_FAILED","ResourceStatusReason":"Already exists"}]}"#,
            expect: "CloudFormation: 1 events (1 failed, 0 successful)",
        },
        Case {
            key: "logs get-log-events",
            json: r#"{"events":[{"timestamp":1705312200000,"message":"INFO: server started","ingestionTime":1705312201000}]}"#,
            expect: "2024-01-15 09:50:00 INFO: server started",
        },
        Case {
            key: "logs filter-log-events",
            json: r#"{"events":[{"timestamp":1705312260000,"message":"ERROR: connection lost","ingestionTime":1705312261000}]}"#,
            expect: "2024-01-15 09:51:00 ERROR: connection lost",
        },
        Case {
            key: "lambda list-functions",
            json: r#"{"Functions":[{"FunctionName":"api","Runtime":"python3.12","MemorySize":512,"Timeout":30,"State":"Active"}]}"#,
            expect: "api python3.12 512MB 30s Active",
        },
        Case {
            key: "lambda get-function",
            json: r#"{"Configuration":{"FunctionName":"my-fn","Runtime":"nodejs20.x","Handler":"index.handler","MemorySize":128,"Timeout":10,"State":"Active","LastModified":"2024-02-20T14:00:00.000+0000"}}"#,
            expect: "my-fn nodejs20.x index.handler 128MB 10s Active 2024-02-20",
        },
        Case {
            key: "iam list-roles",
            json: r#"{"Roles":[{"RoleName":"admin-role","CreateDate":"2024-01-15T10:30:00Z"}]}"#,
            expect: "admin-role 2024-01-15",
        },
        Case {
            key: "iam list-users",
            json: r#"{"Users":[{"UserName":"alice","CreateDate":"2024-01-15T10:30:00Z"}]}"#,
            expect: "alice created:2024-01-15",
        },
        Case {
            key: "dynamodb scan",
            json: r#"{"Items":[{"id":{"N":"1"},"name":{"S":"Alice"}}],"Count":1,"ScannedCount":1}"#,
            expect: "Count: 1/1",
        },
        Case {
            key: "dynamodb query",
            json: r#"{"Items":[{"id":{"N":"1"},"name":{"S":"Alice"}}],"Count":1,"ScannedCount":1}"#,
            expect: r#"{"id":1,"name":"Alice"}"#,
        },
        Case {
            key: "ecs describe-tasks",
            json: r#"{"tasks":[{"taskArn":"arn:aws:ecs:us-east-1:123:task/my-cluster/abc123def456","lastStatus":"RUNNING","containers":[{"name":"web","lastStatus":"RUNNING"}]}]}"#,
            expect: "abc123def456 RUNNING containers:[web:RUNNING]",
        },
        Case {
            key: "ec2 describe-security-groups",
            json: r#"{"SecurityGroups":[{"GroupName":"web","GroupId":"sg-1","IpPermissions":[{"IpProtocol":"tcp","FromPort":443,"ToPort":443,"IpRanges":[{"CidrIp":"0.0.0.0/0"}]}],"IpPermissionsEgress":[{"IpProtocol":"-1","IpRanges":[{"CidrIp":"0.0.0.0/0"}]}]}]}"#,
            expect: "web (sg-1) ingress: tcp/443<-0.0.0.0/0 | egress: all<-0.0.0.0/0",
        },
        Case {
            key: "s3api list-objects-v2",
            json: r#"{"Contents":[{"Key":"logs/app.log","Size":1024,"LastModified":"2024-02-20T14:00:00Z"}]}"#,
            expect: "logs/app.log 1.0 KB 2024-02-20",
        },
        Case {
            key: "eks describe-cluster",
            json: r#"{"cluster":{"name":"my-cluster","status":"ACTIVE","version":"1.28","endpoint":"https://abc.eks.amazonaws.com","certificateAuthority":{"data":"LS0tLS1CRUdJTiBDRVJUSUZJQ0FURS0tLS0t"}}}"#,
            expect: "my-cluster ACTIVE k8s/1.28 https://abc.eks.amazonaws.com",
        },
        Case {
            key: "sqs receive-message",
            json: r#"{"Messages":[{"MessageId":"12345678-abcd-efgh-ijkl-1234567890ab","ReceiptHandle":"AQEBwJnKyrHigUMZj6rYigCgxlaS3SLy0a","Body":"hello world"}]}"#,
            expect: "12345678 hello world",
        },
        Case {
            key: "dynamodb get-item",
            json: r#"{"Item":{"id":{"N":"7"},"name":{"S":"Bob"}}}"#,
            expect: r#"{"id":7,"name":"Bob"}"#,
        },
        Case {
            key: "logs get-query-results",
            json: r#"{"status":"Complete","results":[[{"field":"@message","value":"boom"},{"field":"@ptr","value":"ptr-xyz"}]]}"#,
            expect: "@message=boom",
        },
        Case {
            key: "secretsmanager get-secret-value",
            json: r#"{"Name":"my-secret","SecretString":"plain-pw","ARN":"arn:aws:secretsmanager:us-east-1:123:secret:my-secret"}"#,
            expect: "Secret: plain-pw",
        },
    ];

    fn padded(json: &str) -> String {
        format!("{{{},{}", PAD, &json[1..])
    }

    fn write_fake_aws(dir: &Path) {
        let mut s = String::from(
            "#!/bin/sh\nfor a in \"$@\"; do\n  if [ \"$a\" = table ]; then echo 'table flag leaked' >&2; exit 9; fi\ndone\ncase \"$1 $2\" in\n",
        );
        for c in CASES {
            s.push_str(&format!(
                "  \"{}\") printf '%s\\n' '{}'; exit 0 ;;\n",
                c.key,
                padded(c.json)
            ));
        }
        s.push_str("  \"ec2 describe-vpcs\") echo 'fake access denied' >&2; exit 3 ;;\n");
        s.push_str("  \"configure list\") echo 'plain text output from fake'; exit 0 ;;\n");
        s.push_str(
            "  \"s3 cp\"|\"s3 sync\") echo 'upload: a.txt to s3://bucket/a.txt'; exit 0 ;;\n",
        );
        s.push_str("  *) echo \"unexpected fake aws call: $*\" >&2; exit 42 ;;\nesac\n");
        let path = dir.join("aws");
        std::fs::write(&path, s).expect("write fake aws");
        let mut perms = std::fs::metadata(&path)
            .expect("stat fake aws")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).expect("chmod fake aws");
    }

    fn run_rtk(dir: &Path, args: &[&str]) -> Output {
        let mut cmd = rtk_command();
        let inherited: OsString = cmd
            .get_envs()
            .find(|(k, _)| *k == "PATH")
            .and_then(|(_, v)| v.map(|v| v.to_os_string()))
            .unwrap_or_default();
        let mut path = dir.as_os_str().to_os_string();
        if !inherited.is_empty() {
            path.push(":");
            path.push(&inherited);
        }
        cmd.env("PATH", path).args(args).output().expect("run rtk")
    }

    #[test]
    fn specialised_routes_filter_fake_aws_output() {
        if !rtk_binary_is_built() {
            return;
        }
        let dir = tempdir();
        write_fake_aws(dir.path());

        for c in CASES {
            let mut args: Vec<&str> = vec!["aws"];
            args.extend(c.key.split(' '));
            args.extend(["--output", "table"]);
            let out = run_rtk(dir.path(), &args);
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert_eq!(
                out.status.code(),
                Some(0),
                "route {:?} failed\nstdout:\n{stdout}\nstderr:\n{stderr}",
                c.key
            );
            assert!(
                stdout.contains(c.expect),
                "route {:?}: expected {:?} in stdout\nstdout:\n{stdout}\nstderr:\n{stderr}",
                c.key,
                c.expect
            );
            assert!(
                !stdout.contains("RequestId"),
                "route {:?}: filter was not applied\nstdout:\n{stdout}",
                c.key
            );
        }

        for op in ["cp", "sync"] {
            let out = run_rtk(dir.path(), &["aws", "s3", op, "a.txt", "s3://bucket/a.txt"]);
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert_eq!(out.status.code(), Some(0), "s3 {op} stdout:\n{stdout}");
            assert!(
                stdout.contains("upload: a.txt to s3://bucket/a.txt"),
                "s3 {op} stdout:\n{stdout}"
            );
        }
    }

    #[test]
    fn generic_route_propagates_failure_and_passes_plain_text() {
        if !rtk_binary_is_built() {
            return;
        }
        let dir = tempdir();
        write_fake_aws(dir.path());

        let out = run_rtk(dir.path(), &["-v", "aws", "ec2", "describe-vpcs"]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(3), "stderr:\n{stderr}");
        assert!(stderr.contains("fake access denied"), "stderr:\n{stderr}");
        assert!(stderr.contains("Running: aws ec2"), "stderr:\n{stderr}");

        let out = run_rtk(dir.path(), &["aws", "configure", "list"]);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(
            out.status.code(),
            Some(0),
            "stdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(
            stdout.contains("plain text output from fake"),
            "stdout:\n{stdout}"
        );
    }
}
