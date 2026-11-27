use super::{extract_index_name, extract_table_name, filter_db_push};

#[test]
fn db_push_summarizes_schema_changes() {
    let output = concat!(
        "CREATE TABLE users (id INTEGER);\n",
        "CREATE TABLE sessions (id INTEGER);\n",
        "ALTER TABLE users ADD COLUMN email TEXT;\n",
        "ALTER TABLE users ADD COLUMN display_name TEXT;\n",
        "ALTER TABLE sessions ADD COLUMN user_id INTEGER;\n",
        "DROP TABLE legacy_sessions;\n",
    );

    assert_eq!(
        filter_db_push(output),
        "Schema pushed to database\n  + 2 tables, ~ 3 columns, - 1 dropped"
    );
    assert_eq!(
        filter_db_push("The database is already in sync."),
        "Schema pushed to database"
    );
}

#[test]
fn table_name_requires_a_name_after_the_table_keyword() {
    assert_eq!(
        extract_table_name(
            r#"ALTER TABLE "Session" ADD FOREIGN KEY (user_id) REFERENCES users(id);"#
        )
        .as_deref(),
        Some("Session")
    );
    assert_eq!(
        extract_table_name("DROP TABLE `legacy_sessions`;").as_deref(),
        Some("legacy_sessions")
    );
    assert_eq!(extract_table_name("ALTER TABLE"), None);
    assert_eq!(extract_table_name("DROP TABLESPACE archive;"), None);
}

#[test]
fn index_name_requires_a_name_after_the_index_keyword() {
    assert_eq!(
        extract_index_name(r#"CREATE UNIQUE INDEX "users_email_idx" ON users(email);"#).as_deref(),
        Some("users_email_idx")
    );
    assert_eq!(extract_index_name("CREATE UNIQUE INDEX"), None);
    assert_eq!(
        extract_index_name("CREATE TABLE INDEX_LOG (id INTEGER);"),
        None
    );
    assert_eq!(extract_index_name("CREATE TABLE users (id INTEGER);"), None);
}
