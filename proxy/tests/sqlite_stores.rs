use rtk_proxy::{memory::IcmMemory, store::Store};
use rusqlite::Connection;

#[tokio::test]
async fn both_stores_reopen_committed_wal_records_with_the_patched_engine() {
    assert!(
        rusqlite::version_number() >= 3_054_000,
        "SQLite {}",
        rusqlite::version()
    );
    let tmp = tempfile::tempdir().unwrap();
    let usage = tmp.path().join("usage.db");
    let memory = tmp.path().join("memory.db");
    // Keep both ordinary WAL databases open while new store handles read the writes.
    let usage_db = Connection::open(&usage).unwrap();
    let memory_db = Connection::open(&memory).unwrap();
    for db in [&usage_db, &memory_db] {
        db.pragma_update(None, "journal_mode", "WAL").unwrap();
    }

    let usage_path = usage.to_str().unwrap();
    let memory_path = memory.to_str().unwrap();
    let store = Store::open(usage_path).unwrap();
    store.put_setting("marker", "saved").await;
    drop(store);
    let cache = IcmMemory::open(memory_path).unwrap();
    cache
        .put_exact("topic".into(), "remembered".into())
        .await
        .unwrap();
    drop(cache);

    assert_eq!(
        Store::open(usage_path).unwrap().all_settings_sync()["marker"],
        "saved"
    );
    let reopened = IcmMemory::open(memory_path).unwrap();
    assert_eq!(
        reopened.get_exact("topic".into()).await.unwrap().as_deref(),
        Some("remembered")
    );
    assert_eq!(reopened.count().await.unwrap(), 1);
}
