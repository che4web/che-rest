use che_orm::{Database, Uuid};
use futures_util::FutureExt;
use std::future::Future;

/// A separate schema for each case; clean it even if an assertion panics.
pub async fn isolated<F, Fut>(scenario: F)
where
    F: FnOnce(Database, String) -> Fut,
    Fut: Future<Output = ()>,
{
    let url = std::env::var("TEST_POSTGRES_URL").expect("set TEST_POSTGRES_URL");
    let admin = Database::connect(&url).await.unwrap();
    let schema = format!("che_rest_test_{}", Uuid::now_v7().simple());
    admin
        .execute_sql(format!("CREATE SCHEMA \"{schema}\""), vec![])
        .await
        .unwrap();
    let separator = if url.contains('?') { '&' } else { '?' };
    let scoped_url = format!("{url}{separator}options=-csearch_path%3D{schema}");
    let database = Database::connect(&scoped_url).await.unwrap();
    let result = std::panic::AssertUnwindSafe(scenario(database, scoped_url))
        .catch_unwind()
        .await;
    admin
        .execute_sql(format!("DROP SCHEMA \"{schema}\" CASCADE"), vec![])
        .await
        .unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
