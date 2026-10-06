use anyhow::{Result, anyhow};
use std::sync::Arc;
use tokio::sync::{Notify, mpsc, oneshot};
use turso::Connection;

use super::models::{ChunkRow, Job, Note, SearchHit};
use super::repo;
use crate::config::JOB_MAX_ATTEMPTS;

type Reply<T> = oneshot::Sender<Result<T>>;

/// Db operations!
enum DbCmd {
    InsertNote {
        content: String,
        reply: Reply<i64>,
    },
    UpdateNote {
        id: i64,
        content: String,
        reply: Reply<bool>,
    },
    GetNote {
        id: i64,
        reply: Reply<Option<Note>>,
    },
    RecentNotes {
        limit: i64,
        reply: Reply<Vec<Note>>,
    },
    DeleteNote {
        id: i64,
        reply: Reply<()>,
    },
    Search {
        query_vec: Vec<f32>,
        limit: i64,
        reply: Reply<Vec<SearchHit>>,
    },
    ClaimEmbedJob {
        reply: Reply<Option<Job>>,
    },
    CompleteEmbedJob {
        job_id: i64,
        note_id: i64,
        chunks: Vec<ChunkRow>,
        reply: Reply<bool>,
    },
    FailJob {
        job_id: i64,
        error: String,
        reply: Reply<()>,
    },
    EnsureEmbeddingModel {
        model_id: String,
        reply: Reply<bool>,
    },
}

#[derive(Debug, Clone)]
pub struct DbHandle {
    tx: mpsc::Sender<DbCmd>,
    job_signal: Arc<Notify>,
}

pub fn spawn_actor(conn: Connection) -> DbHandle {
    let (tx, mut rx) = mpsc::channel::<DbCmd>(64);
    tokio::spawn(async move {
        while let Some(cmd) = rx.recv().await {
            // `let _ =`: if the caller stopped waiting, nobody wants the answer.
            match cmd {
                DbCmd::InsertNote { content, reply } => {
                    let _ = reply.send(repo::insert_note(&conn, &content).await);
                }
                DbCmd::UpdateNote { id, content, reply } => {
                    let _ = reply.send(repo::update_note(&conn, id, &content).await);
                }
                DbCmd::GetNote { id, reply } => {
                    let _ = reply.send(repo::get_note(&conn, id).await);
                }
                DbCmd::RecentNotes { limit, reply } => {
                    let _ = reply.send(repo::recent_notes(&conn, limit).await);
                }
                DbCmd::DeleteNote { id, reply } => {
                    let _ = reply.send(repo::delete_note(&conn, id).await);
                }
                DbCmd::Search {
                    query_vec,
                    limit,
                    reply,
                } => {
                    let _ = reply.send(repo::search_chunks(&conn, &query_vec, limit).await);
                }
                DbCmd::ClaimEmbedJob { reply } => {
                    let _ = reply.send(repo::claim_next_job(&conn, "embed").await);
                }
                DbCmd::CompleteEmbedJob {
                    job_id,
                    note_id,
                    chunks,
                    reply,
                } => {
                    let _ =
                        reply.send(repo::complete_embed_job(&conn, job_id, note_id, &chunks).await);
                }
                DbCmd::FailJob {
                    job_id,
                    error,
                    reply,
                } => {
                    let _ =
                        reply.send(repo::fail_job(&conn, job_id, &error, JOB_MAX_ATTEMPTS).await);
                }
                DbCmd::EnsureEmbeddingModel { model_id, reply } => {
                    let _ = reply.send(repo::ensure_embedding_model(&conn, &model_id).await);
                }
            }
        }
        // All handles dropped: the loop ends and `conn` is dropped cleanly.
    });

    DbHandle {
        tx,
        job_signal: Arc::new(Notify::new()),
    }
}

impl DbHandle {
    async fn request<T>(&self, make: impl FnOnce(Reply<T>) -> DbCmd) -> Result<T> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(make(reply))
            .await
            .map_err(|_| anyhow!("database task has stopped"))?;
        rx.await
            .map_err(|_| anyhow!("database task dropped the request"))?
    }
    pub async fn insert_note(&self, content: String) -> Result<i64> {
        let id = self
            .request(|reply| DbCmd::InsertNote { content, reply })
            .await?;
        self.job_signal.notify_one();
        Ok(id)
    }

    pub async fn update_note(&self, id: i64, content: String) -> Result<bool> {
        let changed = self
            .request(|reply| DbCmd::UpdateNote { id, content, reply })
            .await?;
        if changed {
            self.job_signal.notify_one();
        }
        Ok(changed)
    }

    pub async fn get_note(&self, id: i64) -> Result<Option<Note>> {
        self.request(|reply| DbCmd::GetNote { id, reply }).await
    }

    pub async fn recent_notes(&self, limit: i64) -> Result<Vec<Note>> {
        self.request(|reply| DbCmd::RecentNotes { limit, reply })
            .await
    }

    pub async fn delete_note(&self, id: i64) -> Result<()> {
        self.request(|reply| DbCmd::DeleteNote { id, reply }).await
    }

    pub async fn search(&self, query_vec: Vec<f32>, limit: i64) -> Result<Vec<SearchHit>> {
        self.request(|reply| DbCmd::Search {
            query_vec,
            limit,
            reply,
        })
        .await
    }

    pub async fn claim_embed_job(&self) -> Result<Option<Job>> {
        self.request(|reply| DbCmd::ClaimEmbedJob { reply }).await
    }

    pub async fn complete_embed_job(
        &self,
        job_id: i64,
        note_id: i64,
        chunks: Vec<ChunkRow>,
    ) -> Result<bool> {
        self.request(|reply| DbCmd::CompleteEmbedJob {
            job_id,
            note_id,
            chunks,
            reply,
        })
        .await
    }

    pub async fn fail_job(&self, job_id: i64, error: String) -> Result<()> {
        self.request(|reply| DbCmd::FailJob {
            job_id,
            error,
            reply,
        })
        .await
    }

    pub async fn ensure_embedding_model(&self, model_id: &str) -> Result<bool> {
        let model_id = model_id.to_string();
        let queued = self
            .request(|reply| DbCmd::EnsureEmbeddingModel { model_id, reply })
            .await?;
        if queued {
            self.job_signal.notify_one();
        }
        Ok(queued)
    }
    /// Resolves when a job was enqueued since the last call.
    pub async fn jobs_available(&self) {
        self.job_signal.notified().await;
    }
}
