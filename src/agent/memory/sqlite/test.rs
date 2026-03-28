use std::env::temp_dir;

use chrono::Utc;
use futures::{StreamExt, TryStreamExt};
use llama_runner::ImageOrText;
use sea_orm::{ConnectionTrait, Database, Schema};
use serde::{Deserialize, Serialize};

use crate::{
    agent::memory::{
        Decision, DecisionMemory,
        sqlite::{SqliteDecisionMemory, error::CreateDecisionMemoryError},
    },
    source::LlmComprehendable,
};

use super::{decision, material};

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// A minimal `LlmComprehendable` with KIND = Some(RssItem) that also
/// satisfies the Serialize + DeserializeOwned bounds required by push().
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MockRssItem {
    title: String,
}

impl LlmComprehendable for MockRssItem {
    const KIND: Option<material::Kind> = Some(material::Kind::RssItem);

    fn get_message<'s>(&'s self) -> Vec<ImageOrText<'s>> {
        vec![ImageOrText::Text(self.title.as_str())]
    }
}

/// A type whose KIND is None, used to exercise the UnsupportedMaterialType path.
struct NoKindItem;

impl LlmComprehendable for NoKindItem {
    fn get_message<'s>(&'s self) -> Vec<ImageOrText<'s>> {
        vec![]
    }
}

/// Creates the `material` and `decision_mem` tables in the given SQLite
/// connection using SeaORM's schema builder.
async fn create_schema(db: &sea_orm::DatabaseConnection) {
    let backend = db.get_database_backend();
    let schema = Schema::new(backend);

    db.execute(&schema.create_table_from_entity(material::Entity))
        .await
        .expect("create material table");

    db.execute(&schema.create_table_from_entity(decision::Entity))
        .await
        .expect("create decision_mem table");
}

/// Opens an in-memory SQLite connection and creates the schema.
async fn in_memory_db() -> sea_orm::DatabaseConnection {
    let db = Database::connect("sqlite::memory:")
        .await
        .expect("connect in-memory sqlite");
    create_schema(&db).await;
    db
}

fn mock_decision(title: &str, is_truthy: bool) -> Decision<MockRssItem> {
    Decision {
        material: MockRssItem {
            title: title.to_owned(),
        },
        is_truthy,
        time: Utc::now(),
    }
}

// ---------------------------------------------------------------------------
// new() tests
// ---------------------------------------------------------------------------

/// A valid directory with a supported KIND produces Ok.
#[tokio::test]
async fn new_valid_path_succeeds() {
    let dir = std::env::temp_dir();
    let result: Result<SqliteDecisionMemory<MockRssItem>, _> =
        SqliteDecisionMemory::new(in_memory_db().await, &dir).await;
    result.expect("new() should succeed for a valid path");
}

/// KIND = None must return CreateDecisionMemoryError::UnsupportedMaterialType.
#[tokio::test]
async fn new_unsupported_kind_returns_error() {
    let dir = std::env::temp_dir();
    let result: Result<SqliteDecisionMemory<NoKindItem>, _> =
        SqliteDecisionMemory::new(in_memory_db().await, &dir).await;
    assert!(
        matches!(
            result,
            Err(CreateDecisionMemoryError::UnsupportedMaterialType)
        ),
        "expected UnsupportedMaterialType"
    );
}

// ---------------------------------------------------------------------------
// push() test
// ---------------------------------------------------------------------------

/// push() should persist a decision without returning an error.
#[tokio::test]
async fn push_succeeds() {
    let db = in_memory_db().await;
    let mut mem = SqliteDecisionMemory::<MockRssItem> {
        db,
        _marker: std::marker::PhantomData,
        material_dir: temp_dir(),
    };

    let result = mem.push(mock_decision("hello", true)).await;
    assert!(result.is_ok(), "push() returned error: {result:?}");
}

// ---------------------------------------------------------------------------
// iter_newest_first() test
// ---------------------------------------------------------------------------

/// After pushing two decisions, iter_newest_first() should return items in
/// newest-first order.  Because `into_decision()` is currently `todo!()`,
/// this test is expected to panic — it documents the current state and will
/// pass once the implementation is complete.
#[tokio::test]
async fn iter_newest_first_panics_until_into_decision_implemented() {
    let db = in_memory_db().await;
    let mut mem = SqliteDecisionMemory::<MockRssItem> {
        db,
        _marker: std::marker::PhantomData,
        material_dir: temp_dir(),
    };

    // Push an older decision first, then a newer one.
    let older = Decision {
        material: MockRssItem {
            title: "older".into(),
        },
        is_truthy: false,
        time: Utc::now() - chrono::Duration::seconds(60),
    };
    let newer = Decision {
        material: MockRssItem {
            title: "newer".into(),
        },
        is_truthy: true,
        time: Utc::now(),
    };

    mem.push(older).await.unwrap();
    mem.push(newer).await.unwrap();

    // This call will panic at `into_decision()` which is `todo!()`.
    let items: Vec<_> = mem.iter_newest_first().try_collect().await.unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].as_ref().material.title, "newer");
    assert_eq!(items[1].as_ref().material.title, "older");
}

// ---------------------------------------------------------------------------
// clear() test
// ---------------------------------------------------------------------------

/// After pushing decisions and then calling clear(), iter_newest_first()
/// must return an empty iterator.
#[tokio::test]
async fn clear_yields_empty_iter() {
    let db = in_memory_db().await;
    let mut mem = SqliteDecisionMemory::<MockRssItem> {
        db,
        _marker: std::marker::PhantomData,
        material_dir: temp_dir(),
    };

    mem.push(mock_decision("a", true)).await.unwrap();
    mem.push(mock_decision("b", false)).await.unwrap();

    mem.clear().await.expect("clear() should not error");

    // iter_newest_first() would panic inside into_decision() if it reached
    // any rows, but after clear() the query should return zero rows and the
    // map closure is never called, so this is safe to call.
    let count = mem.iter_newest_first().count().await;
    assert_eq!(count, 0, "expected empty iterator after clear()");
}
