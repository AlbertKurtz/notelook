pub mod actor;
pub mod models;
pub mod repo;
mod schema;

pub use actor::{DbHandle, spawn_actor};
pub use models::*;

use anyhow::{Context, Result};
use std::path::Path;
use turso::{Builder, Connection};

/// Opens (or creates) the database, applies migrations and recovers jobs.
/// Pass `Path::new(":memory:")` for tests.
pub async fn open(path: &Path) -> Result<Connection> {
    let path_str = path.to_str().context("database path is not valid UTF-8")?;
    let db = Builder::new_local(path_str)
        .build()
        .await
        .with_context(|| format!("failed to open database at {path_str}"))?;
    let conn = db.connect()?;

    conn.execute("PRAGMA foreign_keys = ON", ()).await?;
    schema::migrate(&conn).await?;
    repo::reset_running_jobs(&conn).await?;
    Ok(conn)
}
