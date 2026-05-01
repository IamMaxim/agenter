use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};

const TRANSFER_ID_PREFIX: &str = "runner-transfer";
static NEXT_TRANSFER_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunnerTransportOutboundFrame {
    Text(String),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunnerTransportChunkFrame {
    #[serde(rename = "runner_chunk_start")]
    Start(RunnerChunkStart),
    #[serde(rename = "runner_chunk_data")]
    Data(RunnerChunkData),
    #[serde(rename = "runner_chunk_end")]
    End(RunnerChunkEnd),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunnerChunkStart {
    pub transfer_id: String,
    pub total_bytes: usize,
    pub total_chunks: usize,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunnerChunkData {
    pub transfer_id: String,
    pub index: usize,
    pub data_base64: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunnerChunkEnd {
    pub transfer_id: String,
}

#[derive(Debug, thiserror::Error)]
pub enum RunnerTransportError {
    #[error("runner transport chunk size must be greater than zero")]
    ZeroChunkSize,
    #[error("runner transport message exceeds maximum: {total_bytes} > {max_bytes}")]
    MessageTooLarge {
        total_bytes: usize,
        max_bytes: usize,
    },
    #[error("runner transport chunk received without start: {transfer_id}")]
    MissingStart { transfer_id: String },
    #[error("runner transport chunk transfer mismatch: expected {expected}, got {actual}")]
    TransferMismatch { expected: String, actual: String },
    #[error("runner transport chunk index out of range: {index} >= {total_chunks}")]
    ChunkIndexOutOfRange { index: usize, total_chunks: usize },
    #[error("runner transport chunk duplicate index: {index}")]
    DuplicateChunk { index: usize },
    #[error("runner transport chunk set incomplete: {received_chunks} != {total_chunks}")]
    Incomplete {
        received_chunks: usize,
        total_chunks: usize,
    },
    #[error("runner transport reassembled byte count mismatch: {actual_bytes} != {total_bytes}")]
    ByteCountMismatch {
        actual_bytes: usize,
        total_bytes: usize,
    },
    #[error("runner transport digest mismatch")]
    DigestMismatch,
    #[error("runner transport JSON serialization failed: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("runner transport base64 decode failed: {0}")]
    Base64(#[from] base64::DecodeError),
}

#[derive(Debug)]
pub struct RunnerTransportChunkReassembler {
    max_message_bytes: usize,
    current: Option<ChunkTransfer>,
}

#[derive(Debug)]
struct ChunkTransfer {
    transfer_id: String,
    total_bytes: usize,
    total_chunks: usize,
    sha256: String,
    chunks: BTreeMap<usize, Vec<u8>>,
}

impl RunnerTransportChunkReassembler {
    #[must_use]
    pub fn new(max_message_bytes: usize) -> Self {
        Self {
            max_message_bytes,
            current: None,
        }
    }
}

pub fn chunk_message<T>(
    message: &T,
    chunk_bytes: usize,
) -> Result<Vec<RunnerTransportOutboundFrame>, RunnerTransportError>
where
    T: Serialize,
{
    if chunk_bytes == 0 {
        return Err(RunnerTransportError::ZeroChunkSize);
    }

    let bytes = serde_json::to_vec(message)?;
    if bytes.len() <= chunk_bytes {
        return Ok(vec![RunnerTransportOutboundFrame::Text(
            String::from_utf8(bytes).expect("serde_json emits UTF-8"),
        )]);
    }

    let transfer_id = next_transfer_id();
    let total_chunks = bytes.len().div_ceil(chunk_bytes);
    let mut frames = Vec::with_capacity(total_chunks + 2);
    frames.push(serialize_frame(&RunnerTransportChunkFrame::Start(
        RunnerChunkStart {
            transfer_id: transfer_id.clone(),
            total_bytes: bytes.len(),
            total_chunks,
            sha256: sha256_hex(&bytes),
        },
    ))?);

    for (index, chunk) in bytes.chunks(chunk_bytes).enumerate() {
        frames.push(serialize_frame(&RunnerTransportChunkFrame::Data(
            RunnerChunkData {
                transfer_id: transfer_id.clone(),
                index,
                data_base64: BASE64.encode(chunk),
            },
        ))?);
    }

    frames.push(serialize_frame(&RunnerTransportChunkFrame::End(
        RunnerChunkEnd { transfer_id },
    ))?);
    Ok(frames)
}

pub fn reassemble_message<T>(
    reassembler: &mut RunnerTransportChunkReassembler,
    text: &str,
) -> Result<Option<T>, RunnerTransportError>
where
    T: DeserializeOwned,
{
    match serde_json::from_str::<RunnerTransportChunkFrame>(text) {
        Ok(frame) => reassemble_chunk_frame(reassembler, frame),
        Err(_) => Ok(Some(serde_json::from_str(text)?)),
    }
}

fn reassemble_chunk_frame<T>(
    reassembler: &mut RunnerTransportChunkReassembler,
    frame: RunnerTransportChunkFrame,
) -> Result<Option<T>, RunnerTransportError>
where
    T: DeserializeOwned,
{
    match frame {
        RunnerTransportChunkFrame::Start(start) => {
            if start.total_bytes > reassembler.max_message_bytes {
                return Err(RunnerTransportError::MessageTooLarge {
                    total_bytes: start.total_bytes,
                    max_bytes: reassembler.max_message_bytes,
                });
            }
            reassembler.current = Some(ChunkTransfer {
                transfer_id: start.transfer_id,
                total_bytes: start.total_bytes,
                total_chunks: start.total_chunks,
                sha256: start.sha256,
                chunks: BTreeMap::new(),
            });
            Ok(None)
        }
        RunnerTransportChunkFrame::Data(data) => {
            let Some(current) = reassembler.current.as_mut() else {
                return Err(RunnerTransportError::MissingStart {
                    transfer_id: data.transfer_id,
                });
            };
            if current.transfer_id != data.transfer_id {
                return Err(RunnerTransportError::TransferMismatch {
                    expected: current.transfer_id.clone(),
                    actual: data.transfer_id,
                });
            }
            if data.index >= current.total_chunks {
                return Err(RunnerTransportError::ChunkIndexOutOfRange {
                    index: data.index,
                    total_chunks: current.total_chunks,
                });
            }
            if current.chunks.contains_key(&data.index) {
                return Err(RunnerTransportError::DuplicateChunk { index: data.index });
            }
            current
                .chunks
                .insert(data.index, BASE64.decode(data.data_base64)?);
            Ok(None)
        }
        RunnerTransportChunkFrame::End(end) => {
            let Some(current) = reassembler.current.take() else {
                return Err(RunnerTransportError::MissingStart {
                    transfer_id: end.transfer_id,
                });
            };
            if current.transfer_id != end.transfer_id {
                return Err(RunnerTransportError::TransferMismatch {
                    expected: current.transfer_id,
                    actual: end.transfer_id,
                });
            }
            if current.chunks.len() != current.total_chunks {
                return Err(RunnerTransportError::Incomplete {
                    received_chunks: current.chunks.len(),
                    total_chunks: current.total_chunks,
                });
            }

            let mut bytes = Vec::with_capacity(current.total_bytes);
            for chunk in current.chunks.into_values() {
                bytes.extend(chunk);
            }
            if bytes.len() != current.total_bytes {
                return Err(RunnerTransportError::ByteCountMismatch {
                    actual_bytes: bytes.len(),
                    total_bytes: current.total_bytes,
                });
            }
            if sha256_hex(&bytes) != current.sha256 {
                return Err(RunnerTransportError::DigestMismatch);
            }

            Ok(Some(serde_json::from_slice(&bytes)?))
        }
    }
}

fn serialize_frame(
    frame: &RunnerTransportChunkFrame,
) -> Result<RunnerTransportOutboundFrame, RunnerTransportError> {
    Ok(RunnerTransportOutboundFrame::Text(serde_json::to_string(
        frame,
    )?))
}

fn next_transfer_id() -> String {
    format!(
        "{TRANSFER_ID_PREFIX}-{}",
        NEXT_TRANSFER_ID.fetch_add(1, Ordering::Relaxed)
    )
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}
