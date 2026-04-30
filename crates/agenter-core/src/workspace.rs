use serde::{Deserialize, Serialize};

use crate::{RunnerId, WorkspaceId};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkspaceRef {
    pub workspace_id: WorkspaceId,
    pub runner_id: RunnerId,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}
