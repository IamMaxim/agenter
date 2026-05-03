use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use agenter_core::SessionId;
use agenter_protocol::{runner::RunnerEvent, RequestId};
use serde::{Deserialize, Serialize};
use tokio::{io::AsyncWriteExt, sync::Mutex};

const CLEANUP_RETENTION: usize = 256;

#[derive(Clone, Debug)]
pub struct RunnerWal {
    path: PathBuf,
    inner: Arc<Mutex<RunnerWalState>>,
}

#[derive(Clone, Debug, Default)]
struct RunnerWalState {
    next_seq: u64,
    acked_seq: u64,
    records: Vec<RunnerWalRecord>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RunnerWalRecord {
    pub runner_event_seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<RequestId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    pub event: RunnerEvent,
    #[serde(default)]
    pub acked: bool,
}

impl RunnerWal {
    pub async fn open(path: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let path = path.into();
        let records = read_records(&path).await?;
        let next_seq = records
            .iter()
            .map(|record| record.runner_event_seq)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
            .max(1);
        let acked_seq = records
            .iter()
            .filter(|record| record.acked)
            .map(|record| record.runner_event_seq)
            .max()
            .unwrap_or(0);
        Ok(Self {
            path,
            inner: Arc::new(Mutex::new(RunnerWalState {
                next_seq,
                acked_seq,
                records,
            })),
        })
    }

    pub async fn append(
        &self,
        request_id: Option<RequestId>,
        session_id: Option<SessionId>,
        event: RunnerEvent,
    ) -> anyhow::Result<RunnerWalRecord> {
        let mut inner = self.inner.lock().await;
        let record = RunnerWalRecord {
            runner_event_seq: inner.next_seq,
            request_id,
            session_id,
            event,
            acked: false,
        };
        inner.next_seq = inner.next_seq.saturating_add(1);
        inner.records.push(record.clone());
        persist_records(&self.path, &inner.records).await?;
        Ok(record)
    }

    pub async fn ack(&self, runner_event_seq: u64) -> anyhow::Result<()> {
        let mut inner = self.inner.lock().await;
        inner.acked_seq = inner.acked_seq.max(runner_event_seq);
        for record in &mut inner.records {
            if record.runner_event_seq <= runner_event_seq {
                record.acked = true;
            }
        }
        let acked_count = inner.records.iter().filter(|record| record.acked).count();
        if acked_count > CLEANUP_RETENTION {
            let remove_count = acked_count - CLEANUP_RETENTION;
            let mut removed = 0;
            inner.records.retain(|record| {
                if record.acked && removed < remove_count {
                    removed += 1;
                    false
                } else {
                    true
                }
            });
        }
        persist_records(&self.path, &inner.records).await
    }

    pub async fn unacked(&self) -> Vec<RunnerWalRecord> {
        self.inner
            .lock()
            .await
            .records
            .iter()
            .filter(|record| !record.acked)
            .cloned()
            .collect()
    }

    pub async fn acked_seq(&self) -> u64 {
        self.inner.lock().await.acked_seq
    }
}

async fn read_records(path: &Path) -> anyhow::Result<Vec<RunnerWalRecord>> {
    let Ok(contents) = tokio::fs::read_to_string(path).await else {
        return Ok(Vec::new());
    };
    let mut records = Vec::new();
    for (line_no, line) in contents.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<RunnerWalRecord>(line) else {
            tracing::warn!(
                path = %path.display(),
                line = line_no + 1,
                "ignoring corrupt trailing runner WAL records"
            );
            break;
        };
        records.push(record);
    }
    Ok(records)
}

async fn persist_records(path: &Path, records: &[RunnerWalRecord]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp_path = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("jsonl")
    ));
    let mut contents = String::new();
    for record in records {
        contents.push_str(&serde_json::to_string(record)?);
        contents.push('\n');
    }
    let mut file = tokio::fs::File::create(&tmp_path).await?;
    file.write_all(contents.as_bytes()).await?;
    file.sync_all().await?;
    drop(file);
    tokio::fs::rename(&tmp_path, path).await?;
    if let Some(parent) = path.parent() {
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use agenter_core::{AgentMessageDeltaEvent, AppEvent, SessionId};
    use agenter_protocol::runner::{AgentEvent, RunnerEvent};
    use uuid::Uuid;

    use super::*;

    fn wal_event(session_id: SessionId) -> RunnerEvent {
        RunnerEvent::AgentEvent(Box::new(AgentEvent {
            session_id,
            event: AppEvent::AgentMessageDelta(AgentMessageDeltaEvent {
                session_id,
                message_id: "msg-1".to_owned(),
                delta: "hello".to_owned(),
                provider_payload: None,
            }),
            universal_event: None,
        }))
    }

    #[tokio::test]
    async fn wal_append_replay_and_ack_cleanup() {
        let path =
            std::env::temp_dir().join(format!("agenter-runner-wal-test-{}.jsonl", Uuid::new_v4()));
        let wal = RunnerWal::open(&path).await.expect("open wal");
        let session_id = SessionId::new();
        let first = wal
            .append(
                Some(RequestId::from("req-1")),
                Some(session_id),
                wal_event(session_id),
            )
            .await
            .expect("append first");
        let second = wal
            .append(None, Some(session_id), first.event.clone())
            .await
            .expect("append second");

        assert_eq!(first.runner_event_seq, 1);
        assert_eq!(second.runner_event_seq, 2);
        assert_eq!(wal.unacked().await.len(), 2);

        wal.ack(1).await.expect("ack first");
        let reopened = RunnerWal::open(&path).await.expect("reopen wal");
        assert_eq!(reopened.acked_seq().await, 1);
        let unacked = reopened.unacked().await;
        assert_eq!(unacked.len(), 1);
        assert_eq!(unacked[0].runner_event_seq, 2);

        reopened.ack(2).await.expect("ack second");
        assert!(reopened.unacked().await.is_empty());
        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn wal_open_tolerates_corrupt_trailing_record_and_preserves_prefix() {
        let path =
            std::env::temp_dir().join(format!("agenter-runner-wal-test-{}.jsonl", Uuid::new_v4()));
        let session_id = SessionId::new();
        let wal = RunnerWal::open(&path).await.expect("open wal");
        let first = wal
            .append(None, Some(session_id), wal_event(session_id))
            .await
            .expect("append first");
        let mut contents = tokio::fs::read_to_string(&path).await.expect("read wal");
        contents.push_str("{not-json");
        tokio::fs::write(&path, contents)
            .await
            .expect("corrupt tail");

        let reopened = RunnerWal::open(&path).await.expect("open corrupt tail");
        let unacked = reopened.unacked().await;
        assert_eq!(unacked.len(), 1);
        assert_eq!(unacked[0].runner_event_seq, first.runner_event_seq);

        let second = reopened
            .append(None, Some(session_id), wal_event(session_id))
            .await
            .expect("append after corrupt tail");
        assert_eq!(second.runner_event_seq, first.runner_event_seq + 1);
        let recovered = RunnerWal::open(&path).await.expect("open recovered");
        assert_eq!(recovered.unacked().await.len(), 2);
        let _ = tokio::fs::remove_file(path).await;
    }
}
