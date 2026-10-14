use jf_db::Database;

const LIST_TABLES: &str = "SELECT name FROM sqlite_master WHERE type='table'";

/// Regression test: every pooled connection must see the migrated schema.
///
/// SQLite hands each `:memory:` connection a private database, so a pool larger
/// than one connection silently gives some queries an empty schema. This test
/// fails if `connect_memory` ever grows its pool again.
#[tokio::test]
async fn memory_db_has_tables() {
    let db = Database::connect_memory().await.expect("db");

    let first: Vec<String> = sqlx::query_scalar(LIST_TABLES)
        .fetch_all(db.pool())
        .await
        .expect("first query");
    println!("FIRST POOL QUERY: {first:?}");
    assert!(first.contains(&"libraries".to_string()), "migrations must be visible");

    let second: Vec<String> = sqlx::query_scalar(LIST_TABLES)
        .fetch_all(db.pool())
        .await
        .expect("second query");
    println!("SECOND POOL QUERY: {second:?}");
    assert_eq!(first, second, "every pooled connection must see the schema");

    let mut conn = db.pool().acquire().await.expect("acquire");
    let one: Vec<String> = sqlx::query_scalar(LIST_TABLES)
        .fetch_all(&mut *conn)
        .await
        .expect("single connection query");
    println!("SINGLE CONNECTION: {one:?}");
    assert_eq!(first, one);
}
