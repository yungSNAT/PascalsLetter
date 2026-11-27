use super::*;

#[test]
fn merge_sources_with_no_sources_refuses_to_publish() {
    let err = merge_sources("demo", &[]).unwrap_err();
    assert!(
        err.to_string().contains("nothing to publish"),
        "unexpected error: {err:#}"
    );
}

#[test]
fn merge_sources_reports_which_file_failed_to_parse() {
    let good = "[pipeline]\nname = \"good\"\n".to_string();
    let bad = "this is = = not toml\n".to_string();
    let err = merge_sources(
        "demo",
        &[
            ("user pipeline.toml".to_string(), good),
            ("project broken.toml".to_string(), bad),
        ],
    )
    .unwrap_err();
    assert_eq!(err.to_string(), "parse project broken.toml");
}

#[test]
fn merge_sources_unions_extends_overrides_scalars_and_renames() {
    let user = r#"
[pipeline]
name = "first"
extends = "rtk:default"

[settings]
level = 1
keep = "yes"
"#;
    let project = r#"
[pipeline]
name = "second"
extends = ["rtk:default", "rtk:proxy", "rtk:proxy", 7]

[settings]
level = 2
"#;
    let merged = merge_sources(
        "combined",
        &[
            ("user pipeline.toml".to_string(), user.to_string()),
            ("project pipeline.toml".to_string(), project.to_string()),
        ],
    )
    .unwrap();
    let table: toml::Table = merged.parse().unwrap();
    let pipeline = table["pipeline"].as_table().unwrap();
    assert_eq!(pipeline["name"].as_str(), Some("combined"));
    let extends = pipeline["extends"].as_array().unwrap();
    let strs: Vec<&str> = extends.iter().filter_map(|e| e.as_str()).collect();
    assert_eq!(strs, vec!["rtk:default", "rtk:proxy"]);
    // Non-string entries are kept, not deduplicated away.
    assert!(extends.iter().any(|e| e.as_integer() == Some(7)));
    assert_eq!(extends.len(), 3);

    let settings = table["settings"].as_table().unwrap();
    assert_eq!(settings["level"].as_integer(), Some(2));
    assert_eq!(settings["keep"].as_str(), Some("yes"));
}

#[test]
fn merge_sources_without_pipeline_table_merges_plain_tables() {
    let a = "[alpha]\nx = 1\n".to_string();
    let b = "[beta]\ny = 2\n".to_string();
    let merged = merge_sources(
        "plain",
        &[("a.toml".to_string(), a), ("b.toml".to_string(), b)],
    )
    .unwrap();
    let table: toml::Table = merged.parse().unwrap();
    assert!(table.get("pipeline").is_none());
    assert_eq!(table["alpha"]["x"].as_integer(), Some(1));
    assert_eq!(table["beta"]["y"].as_integer(), Some(2));
}
