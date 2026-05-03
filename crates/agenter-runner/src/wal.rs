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
    ack_path: PathBuf,
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
        let ack_path = ack_path_for(&path);
        let read = read_records(&path).await?;
        let mut records = read.records;
        let original_record_count = records.len();
        records.retain(|record| event_is_replayable(&record.event));
        let removed_non_replayable = original_record_count - records.len();
        if read.corrupt_tail || removed_non_replayable > 0 {
            persist_records(&path, &records).await?;
            if removed_non_replayable > 0 {
                tracing::warn!(
                    path = %path.display(),
                    removed_count = removed_non_replayable,
                    "removed source non-replayable records from runner WAL"
                );
            }
        }
        let next_seq = records
            .iter()
            .map(|record| record.runner_event_seq)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
            .max(1);
        let record_acked_seq = records
            .iter()
            .filter(|record| record.acked)
            .map(|record| record.runner_event_seq)
            .max()
            .unwrap_or(0);
        let acked_seq = record_acked_seq.max(read_ack_cursor(&ack_path).await?);
        Ok(Self {
            path,
            ack_path,
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
        append_record(&self.path, &record).await?;
        Ok(record)
    }

    pub async fn ack(&self, runner_event_seq: u64) -> anyhow::Result<()> {
        let mut inner = self.inner.lock().await;
        inner.acked_seq = inner.acked_seq.max(runner_event_seq);
        persist_ack_cursor(&self.ack_path, inner.acked_seq).await?;
        let acked_count = inner
            .records
            .iter()
            .filter(|record| record.runner_event_seq <= inner.acked_seq)
            .count();
        if acked_count > CLEANUP_RETENTION {
            let retention_start = inner
                .acked_seq
                .saturating_sub(u64::try_from(CLEANUP_RETENTION).unwrap_or(u64::MAX));
            inner
                .records
                .retain(|record| record.runner_event_seq > retention_start);
            persist_records(&self.path, &inner.records).await?;
        }
        Ok(())
    }

    pub async fn unacked(&self) -> Vec<RunnerWalRecord> {
        let inner = self.inner.lock().await;
        inner
            .records
            .iter()
            .filter(|record| record.runner_event_seq > inner.acked_seq)
            .cloned()
            .collect()
    }

    pub async fn acked_seq(&self) -> u64 {
        self.inner.lock().await.acked_seq
    }
}

#[derive(Debug, Default)]
struct ReadRecords {
    records: Vec<RunnerWalRecord>,
    corrupt_tail: bool,
}

pub(crate) fn event_is_replayable(event: &RunnerEvent) -> bool {
    matches!(event, RunnerEvent::AgentEvent(_))
}

fn ack_path_for(path: &Path) -> PathBuf {
    path.with_extension(format!(
        "{}.ack",
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("jsonl")
    ))
}

async fn read_ack_cursor(path: &Path) -> anyhow::Result<u64> {
    let Ok(contents) = tokio::fs::read_to_string(path).await else {
        return Ok(0);
    };
    Ok(contents.trim().parse::<u64>().unwrap_or(0))
}

async fn persist_ack_cursor(path: &Path, acked_seq: u64) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp_path = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("ack")
    ));
    let mut file = tokio::fs::File::create(&tmp_path).await?;
    file.write_all(acked_seq.to_string().as_bytes()).await?;
    file.write_all(b"\n").await?;
    file.sync_all().await?;
    drop(file);
    tokio::fs::rename(&tmp_path, path).await?;
    Ok(())
}

async fn append_record(path: &Path, record: &RunnerWalRecord) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    file.write_all(serde_json::to_string(record)?.as_bytes())
        .await?;
    file.write_all(b"\n").await?;
    file.sync_all().await?;
    Ok(())
}

async fn read_records(path: &Path) -> anyhow::Result<ReadRecords> {
    let Ok(contents) = tokio::fs::read_to_string(path).await else {
        return Ok(ReadRecords::default());
    };
    let mut records = Vec::new();
    let mut corrupt_tail = false;
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
            corrupt_tail = true;
            break;
        };
        records.push(record);
    }
    Ok(ReadRecords {
        records,
        corrupt_tail,
    })
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
    use agenter_core::{
        AgentProviderId, NativeRef, RunnerId, SessionId, UniversalEventKind, UniversalEventSource,
        WorkspaceId, WorkspaceRef,
    };
    use agenter_protocol::runner::{
        AgentUniversalEvent, DiscoveredSession, DiscoveredSessionHistoryItem,
        DiscoveredSessionHistoryStatus, DiscoveredSessions, RunnerEvent,
    };
    use uuid::Uuid;

    use super::*;

    fn wal_event(session_id: SessionId) -> RunnerEvent {
        RunnerEvent::AgentEvent(Box::new(AgentUniversalEvent {
            session_id,
            event_id: None,
            turn_id: None,
            item_id: None,
            ts: None,
            source: UniversalEventSource::Native,
            native: Some(NativeRef {
                protocol: "test".to_owned(),
                method: Some("agentMessage/delta".to_owned()),
                kind: None,
                native_id: None,
                summary: Some("hello".to_owned()),
                hash: None,
                pointer: None,
            }),
            event: UniversalEventKind::NativeUnknown {
                summary: Some("hello".to_owned()),
            },
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

    #[tokio::test]
    async fn wal_ack_persists_cursor_without_rewriting_record_log() {
        let path =
            std::env::temp_dir().join(format!("agenter-runner-wal-test-{}.jsonl", Uuid::new_v4()));
        let wal = RunnerWal::open(&path).await.expect("open wal");
        let session_id = SessionId::new();
        let first = wal
            .append(None, Some(session_id), wal_event(session_id))
            .await
            .expect("append first");
        let second = wal
            .append(None, Some(session_id), first.event.clone())
            .await
            .expect("append second");
        let before_ack = tokio::fs::read_to_string(&path).await.expect("read wal");

        wal.ack(first.runner_event_seq).await.expect("ack first");

        let after_ack = tokio::fs::read_to_string(&path).await.expect("read wal");
        assert_eq!(after_ack, before_ack);
        let reopened = RunnerWal::open(&path).await.expect("reopen wal");
        assert_eq!(reopened.acked_seq().await, first.runner_event_seq);
        assert_eq!(
            reopened
                .unacked()
                .await
                .into_iter()
                .map(|record| record.runner_event_seq)
                .collect::<Vec<_>>(),
            vec![second.runner_event_seq]
        );
        let _ = tokio::fs::remove_file(&path).await;
        let _ = tokio::fs::remove_file(ack_path_for(&path)).await;
    }

    #[tokio::test]
    async fn wal_open_drops_source_loaded_discovery_records() {
        let path =
            std::env::temp_dir().join(format!("agenter-runner-wal-test-{}.jsonl", Uuid::new_v4()));
        let session_id = SessionId::new();
        let source_discovery = RunnerWalRecord {
            runner_event_seq: 1,
            request_id: Some(RequestId::from("refresh-1")),
            session_id: None,
            event: RunnerEvent::SessionsDiscovered(DiscoveredSessions {
                workspace: WorkspaceRef {
                    workspace_id: WorkspaceId::new(),
                    runner_id: RunnerId::new(),
                    path: "/tmp/workspace".to_owned(),
                    display_name: Some("workspace".to_owned()),
                },
                provider_id: AgentProviderId::from("codex"),
                sessions: vec![DiscoveredSession {
                    external_session_id: "native-session".to_owned(),
                    title: Some("Native session".to_owned()),
                    updated_at: None,
                    history_status: DiscoveredSessionHistoryStatus::Loaded,
                    history: vec![DiscoveredSessionHistoryItem::UserMessage {
                        message_id: None,
                        content: "large imported history".repeat(1024),
                    }],
                }],
            }),
            acked: false,
        };
        let agent_event = RunnerWalRecord {
            runner_event_seq: 2,
            request_id: None,
            session_id: Some(session_id),
            event: wal_event(session_id),
            acked: false,
        };
        tokio::fs::write(
            &path,
            format!(
                "{}\n{}\n",
                serde_json::to_string(&source_discovery).expect("serialize discovery"),
                serde_json::to_string(&agent_event).expect("serialize agent event")
            ),
        )
        .await
        .expect("write wal");

        let wal = RunnerWal::open(&path).await.expect("open wal");
        let unacked = wal.unacked().await;

        assert_eq!(unacked.len(), 1);
        assert_eq!(unacked[0].runner_event_seq, agent_event.runner_event_seq);
        assert!(matches!(unacked[0].event, RunnerEvent::AgentEvent(_)));
        let rewritten = tokio::fs::read_to_string(&path)
            .await
            .expect("read repaired wal");
        assert!(!rewritten.contains("sessions_discovered"));
        let _ = tokio::fs::remove_file(&path).await;
        let _ = tokio::fs::remove_file(ack_path_for(&path)).await;
    }

    #[test]
    fn only_agent_events_are_runner_wal_replayable() {
        let session_id = SessionId::new();
        assert!(event_is_replayable(&wal_event(session_id)));
        assert!(!event_is_replayable(&RunnerEvent::SessionsDiscovered(
            DiscoveredSessions {
                workspace: WorkspaceRef {
                    workspace_id: WorkspaceId::new(),
                    runner_id: RunnerId::new(),
                    path: "/tmp/workspace".to_owned(),
                    display_name: None,
                },
                provider_id: AgentProviderId::from("codex"),
                sessions: Vec::new(),
            }
        )));
    }
}
