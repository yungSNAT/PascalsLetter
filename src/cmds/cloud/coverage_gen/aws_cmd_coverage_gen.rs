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

    /// One case a line, tab-separated: the `aws` service and operation, the JSON the
    /// stand-in prints for it, and a line rtk must show.
    const CASES: &str = "\
sts get-caller-identity\t{\"UserId\":\"AIDAEXAMPLE\",\"Account\":\"123456789012\",\"Arn\":\"arn:aws:iam::123456789012:user/dev\"}\tAWS: 123456789012 arn:aws:iam::123456789012:user/dev\n\
ec2 describe-instances\t{\"Reservations\":[{\"Instances\":[{\"InstanceId\":\"i-0abc\",\"State\":{\"Name\":\"running\"},\"InstanceType\":\"t3.micro\"}]}]}\ti-0abc running t3.micro\n\
ecs list-services\t{\"serviceArns\":[\"arn:aws:ecs:us-east-1:123:service/cluster/api-service\"]}\tapi-service\n\
ecs describe-services\t{\"services\":[{\"serviceName\":\"api\",\"status\":\"ACTIVE\",\"runningCount\":3,\"desiredCount\":3,\"launchType\":\"FARGATE\"}]}\tapi ACTIVE 3/3 (FARGATE)\n\
rds describe-db-instances\t{\"DBInstances\":[{\"DBInstanceIdentifier\":\"mydb\",\"Engine\":\"postgres\",\"EngineVersion\":\"15.4\",\"DBInstanceClass\":\"db.t3.micro\",\"DBInstanceStatus\":\"available\",\"Endpoint\":{\"Address\":\"mydb.example.com\",\"Port\":5432}}]}\tmydb postgres 15.4 db.t3.micro available mydb.example.com:5432\n\
cloudformation list-stacks\t{\"StackSummaries\":[{\"StackName\":\"my-stack\",\"StackStatus\":\"CREATE_COMPLETE\",\"CreationTime\":\"2024-01-15T10:30:00Z\"}]}\tmy-stack CREATE_COMPLETE 2024-01-15\n\
cloudformation describe-stacks\t{\"Stacks\":[{\"StackName\":\"prod-stack\",\"StackStatus\":\"UPDATE_COMPLETE\",\"CreationTime\":\"2024-02-20T14:00:00Z\",\"Outputs\":[{\"OutputKey\":\"Url\",\"OutputValue\":\"https://x.example\"}]}]}\tUrl=https://x.example\n\
cloudformation describe-stack-events\t{\"StackEvents\":[{\"Timestamp\":\"2024-01-15T10:30:00Z\",\"LogicalResourceId\":\"Bucket\",\"ResourceType\":\"AWS::S3::Bucket\",\"ResourceStatus\":\"CREATE_FAILED\",\"ResourceStatusReason\":\"Already exists\"}]}\tCloudFormation: 1 events (1 failed, 0 successful)\n\
logs get-log-events\t{\"events\":[{\"timestamp\":1705312200000,\"message\":\"INFO: server started\",\"ingestionTime\":1705312201000}]}\t2024-01-15 09:50:00 INFO: server started\n\
logs filter-log-events\t{\"events\":[{\"timestamp\":1705312260000,\"message\":\"ERROR: connection lost\",\"ingestionTime\":1705312261000}]}\t2024-01-15 09:51:00 ERROR: connection lost\n\
lambda list-functions\t{\"Functions\":[{\"FunctionName\":\"api\",\"Runtime\":\"python3.12\",\"MemorySize\":512,\"Timeout\":30,\"State\":\"Active\"}]}\tapi python3.12 512MB 30s Active\n\
lambda get-function\t{\"Configuration\":{\"FunctionName\":\"my-fn\",\"Runtime\":\"nodejs20.x\",\"Handler\":\"index.handler\",\"MemorySize\":128,\"Timeout\":10,\"State\":\"Active\",\"LastModified\":\"2024-02-20T14:00:00.000+0000\"}}\tmy-fn nodejs20.x index.handler 128MB 10s Active 2024-02-20\n\
iam list-roles\t{\"Roles\":[{\"RoleName\":\"admin-role\",\"CreateDate\":\"2024-01-15T10:30:00Z\"}]}\tadmin-role 2024-01-15\n\
iam list-users\t{\"Users\":[{\"UserName\":\"alice\",\"CreateDate\":\"2024-01-15T10:30:00Z\"}]}\talice created:2024-01-15\n\
dynamodb scan\t{\"Items\":[{\"id\":{\"N\":\"1\"},\"name\":{\"S\":\"Alice\"}}],\"Count\":1,\"ScannedCount\":1}\tCount: 1/1\n\
dynamodb query\t{\"Items\":[{\"id\":{\"N\":\"1\"},\"name\":{\"S\":\"Alice\"}}],\"Count\":1,\"ScannedCount\":1}\t{\"id\":1,\"name\":\"Alice\"}\n\
ecs describe-tasks\t{\"tasks\":[{\"taskArn\":\"arn:aws:ecs:us-east-1:123:task/my-cluster/abc123def456\",\"lastStatus\":\"RUNNING\",\"containers\":[{\"name\":\"web\",\"lastStatus\":\"RUNNING\"}]}]}\tabc123def456 RUNNING containers:[web:RUNNING]\n\
ec2 describe-security-groups\t{\"SecurityGroups\":[{\"GroupName\":\"web\",\"GroupId\":\"sg-1\",\"IpPermissions\":[{\"IpProtocol\":\"tcp\",\"FromPort\":443,\"ToPort\":443,\"IpRanges\":[{\"CidrIp\":\"0.0.0.0/0\"}]}],\"IpPermissionsEgress\":[{\"IpProtocol\":\"-1\",\"IpRanges\":[{\"CidrIp\":\"0.0.0.0/0\"}]}]}]}\tweb (sg-1) ingress: tcp/443<-0.0.0.0/0 | egress: all<-0.0.0.0/0\n\
s3api list-objects-v2\t{\"Contents\":[{\"Key\":\"logs/app.log\",\"Size\":1024,\"LastModified\":\"2024-02-20T14:00:00Z\"}]}\tlogs/app.log 1.0 KB 2024-02-20\n\
eks describe-cluster\t{\"cluster\":{\"name\":\"my-cluster\",\"status\":\"ACTIVE\",\"version\":\"1.28\",\"endpoint\":\"https://abc.eks.amazonaws.com\",\"certificateAuthority\":{\"data\":\"LS0tLS1CRUdJTiBDRVJUSUZJQ0FURS0tLS0t\"}}}\tmy-cluster ACTIVE k8s/1.28 https://abc.eks.amazonaws.com\n\
sqs receive-message\t{\"Messages\":[{\"MessageId\":\"12345678-abcd-efgh-ijkl-1234567890ab\",\"ReceiptHandle\":\"AQEBwJnKyrHigUMZj6rYigCgxlaS3SLy0a\",\"Body\":\"hello world\"}]}\t12345678 hello world\n\
dynamodb get-item\t{\"Item\":{\"id\":{\"N\":\"7\"},\"name\":{\"S\":\"Bob\"}}}\t{\"id\":7,\"name\":\"Bob\"}\n\
logs get-query-results\t{\"status\":\"Complete\",\"results\":[[{\"field\":\"@message\",\"value\":\"boom\"},{\"field\":\"@ptr\",\"value\":\"ptr-xyz\"}]]}\t@message=boom\n\
secretsmanager get-secret-value\t{\"Name\":\"my-secret\",\"SecretString\":\"plain-pw\",\"ARN\":\"arn:aws:secretsmanager:us-east-1:123:secret:my-secret\"}\tSecret: plain-pw\n\
";

    fn cases() -> impl Iterator<Item = Case> {
        CASES.lines().map(|line| {
            let mut field = line.split('\t');
            let mut next = || field.next().expect("three tab-separated fields");
            Case {
                key: next(),
                json: next(),
                expect: next(),
            }
        })
    }

    fn padded(json: &str) -> String {
        format!("{{{},{}", PAD, &json[1..])
    }

    fn write_fake_aws(dir: &Path) {
        let mut s = String::from(
            "#!/bin/sh\nfor a in \"$@\"; do\n  if [ \"$a\" = table ]; then echo 'table flag leaked' >&2; exit 9; fi\ndone\ncase \"$1 $2\" in\n",
        );
        for c in cases() {
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
        perms.set_mode(0o700);
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

        for c in cases() {
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
