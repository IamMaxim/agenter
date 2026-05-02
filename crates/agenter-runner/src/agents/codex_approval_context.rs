//! Correlates sparse Codex `item/*/requestApproval` RPCs with richer preceding `item/started`
//! (and incremental `patchUpdated`) notifications — mirroring Codex TUI bookkeeping.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

/// Per-turn cache: Codex correlates file-change approvals by `item.id` (itemId on the wire).
#[derive(Clone, Debug, Default)]
pub struct CodexApprovalItemCache {
    /// item id -> ordered list of per-path change snapshots (later rows replace same path).
    file_changes: HashMap<String, Vec<FileChangeRow>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileChangeRow {
    path: String,
    change_kind: String,
    unified_diff: Option<String>,
}

impl CodexApprovalItemCache {
    /// Observe JSON-RPC Codex notifications/requests addressed to this client (`method` present).
    pub fn observe_jsonrpc_message(&mut self, message: &Value) {
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return;
        };
        match method {
            "item/started" => self.observe_item_started(message),
            "item/fileChange/patchUpdated" => self.observe_patch_updated(message),
            _ => {}
        }
    }

    fn observe_item_started(&mut self, message: &Value) {
        let Some(item) = message.pointer("/params/item") else {
            return;
        };
        if item_get_type(item) != Some("fileChange") {
            return;
        };
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            return;
        };
        let Some(changes) = item.get("changes") else {
            return;
        };
        let rows = parse_changes_value(changes);
        if rows.is_empty() {
            return;
        }
        self.file_changes.insert(id.to_owned(), rows);
    }

    fn observe_patch_updated(&mut self, message: &Value) {
        let Some(item_id) = string_at(
            message,
            &[
                "/params/itemId",
                "/params/item/id",
                "/params/item_id",
                "/params/id",
            ],
        ) else {
            return;
        };
        let Some(changes) = message
            .pointer("/params/changes")
            .or_else(|| message.get("changes"))
        else {
            return;
        };
        let rows = parse_changes_value(changes);
        if rows.is_empty() {
            return;
        }
        merge_file_changes(&mut self.file_changes, item_id, rows);
    }

    pub fn presentation_for_file_change_approval(&self, params: &Value) -> Option<Value> {
        let item_id = string_at(params, &["/itemId", "/item/id", "/item_id"])?;
        let rows = self.file_changes.get(item_id)?;
        if rows.is_empty() {
            return None;
        }
        let paths: Vec<String> = rows.iter().map(|r| r.path.clone()).collect();
        let files: Vec<Value> = rows
            .iter()
            .map(|r| {
                json!({
                    "path": r.path,
                    "change_kind": r.change_kind,
                    "unified_diff": r.unified_diff,
                })
            })
            .collect();
        Some(json!({
            "variant": "codex_file_change",
            "provider_item_id": item_id,
            "paths": paths,
            "files": files,
        }))
    }

    #[cfg(test)]
    fn upsert_rows(&mut self, item_id: &str, rows: Vec<FileChangeRow>) {
        self.file_changes.insert(item_id.to_owned(), rows);
    }
}

fn merge_file_changes(
    map: &mut HashMap<String, Vec<FileChangeRow>>,
    item_id: &str,
    delta: Vec<FileChangeRow>,
) {
    let merged = map.entry(item_id.to_owned()).or_default();
    for row in delta {
        if let Some(pos) = merged.iter().position(|r| r.path == row.path) {
            merged[pos] = row;
        } else {
            merged.push(row);
        }
    }
}

fn item_get_type(item: &Value) -> Option<&str> {
    string_at(item, &["/type", "/kind"])
}

fn parse_changes_value(changes: &Value) -> Vec<FileChangeRow> {
    match changes {
        Value::Array(items) => items
            .iter()
            .filter_map(parse_change_array_element)
            .collect(),
        Value::Object(map) => parse_changes_object(map),
        _ => Vec::new(),
    }
}

fn parse_changes_object(map: &serde_json::Map<String, Value>) -> Vec<FileChangeRow> {
    let mut out = Vec::new();
    let mut keys: Vec<String> = map.keys().cloned().collect();
    keys.sort_unstable();

    let looks_like_wire_record = keys
        .iter()
        .any(|k| matches!(k.as_str(), "path" | "kind" | "diff" | "changeKind"));

    if looks_like_wire_record && keys.len() <= 8 {
        if let Some(row) = parse_change_array_element(&Value::Object(map.clone())) {
            return vec![row];
        }
    }

    for path in keys {
        if let Some(val) = map.get(&path) {
            out.extend(rows_for_path_named(&path, val));
        }
    }
    out
}

fn parse_change_array_element(value: &Value) -> Option<FileChangeRow> {
    let path = string_at(
        value,
        &["/path", "/filePath", "/file_path", "/relPath", "/rel_path"],
    )?
    .to_owned();

    let (change_kind, diff) = row_from_codex_patch_object(value.pointer("/patch").unwrap_or(value))
        .or_else(|| classify_generic_change_object(value))?;

    Some(FileChangeRow {
        path,
        change_kind,
        unified_diff: diff,
    })
}

fn rows_for_path_named(path: &str, body: &Value) -> Vec<FileChangeRow> {
    if body.is_null() {
        return Vec::new();
    }

    row_from_codex_patch_object(body)
        .map(|(k, d)| {
            vec![FileChangeRow {
                path: path.to_owned(),
                change_kind: k,
                unified_diff: d,
            }]
        })
        .unwrap_or_else(|| {
            classify_generic_change_object(body)
                .map(|(k, d)| {
                    vec![FileChangeRow {
                        path: path.to_owned(),
                        change_kind: k,
                        unified_diff: d,
                    }]
                })
                .unwrap_or_default()
        })
}

fn row_from_codex_patch_object(patch: &Value) -> Option<(String, Option<String>)> {
    let obj = patch.as_object()?;
    let keys: HashSet<&str> = obj.keys().map(|s| s.as_str()).collect();

    let add_like = keys.contains("Add")
        || keys.contains("add")
        || patch.pointer("/Add").is_some()
        || patch.pointer("/add").is_some();
    let delete_like = keys.contains("Delete")
        || keys.contains("delete")
        || patch.pointer("/Delete").is_some()
        || patch.pointer("/delete").is_some();
    let update_like = keys.contains("Update")
        || keys.contains("update")
        || patch.pointer("/Update").is_some()
        || patch.pointer("/update").is_some();

    if update_like || (keys.contains("unified_diff") || keys.contains("unifiedDiff")) {
        let content = patch
            .pointer("/Update/unified_diff")
            .or_else(|| patch.pointer("/update/unified_diff"))
            .or_else(|| patch.pointer("/Update/unifiedDiff"))
            .or_else(|| patch.pointer("/update/unifiedDiff"))
            .or_else(|| patch.get("unified_diff"))
            .or_else(|| patch.get("unifiedDiff"))
            .and_then(Value::as_str)?
            .to_owned();
        return Some(("update".to_owned(), Some(content)));
    }
    if add_like {
        let content = patch
            .pointer("/Add/content")
            .or_else(|| patch.pointer("/add/content"))
            .or_else(|| patch.get("content"))
            .and_then(Value::as_str)?
            .to_owned();
        return Some(("add".to_owned(), Some(content)));
    }
    if delete_like {
        let content = patch
            .pointer("/Delete/content")
            .or_else(|| patch.pointer("/delete/content"))
            .or_else(|| patch.get("content"))
            .and_then(|v| v.as_str().map(|s| s.to_owned()));
        return Some(("delete".to_owned(), content));
    }
    None
}

fn classify_generic_change_object(body: &Value) -> Option<(String, Option<String>)> {
    let kind_raw = string_at(
        body,
        &["/kind/type", "/kind", "/changeKind", "/change_kind"],
    );

    let kind_norm = match kind_raw.map(str::trim) {
        Some(s) if s.eq_ignore_ascii_case("add") => Some("add"),
        Some(s)
            if s.eq_ignore_ascii_case("delete")
                || s.eq_ignore_ascii_case("remove")
                || s.eq_ignore_ascii_case("rm") =>
        {
            Some("delete")
        }
        Some(s)
            if s.eq_ignore_ascii_case("update")
                || s.eq_ignore_ascii_case("modify")
                || s.eq_ignore_ascii_case("change") =>
        {
            Some("update")
        }
        _ => None,
    };

    let diff_opt = pick_diff_text(body);
    match kind_norm {
        Some(k) => Some((k.to_owned(), diff_opt.or_else(|| body_text_preview(body)))),
        None => diff_opt
            .or_else(|| body_text_preview(body))
            .map(|d| ("update".to_owned(), Some(d))),
    }
}

fn body_text_preview(body: &Value) -> Option<String> {
    let s = serde_json::to_string_pretty(body).ok()?;
    if s.len() > 2400 {
        Some(format!("{}… [truncated]", &s[..2400]))
    } else {
        Some(s)
    }
}

fn pick_diff_text(body: &Value) -> Option<String> {
    string_at(
        body,
        &[
            "/diff",
            "/unified_diff",
            "/unifiedDiff",
            "/patch",
            "/hunks",
            "/snapshot",
            "/preview",
            "/contents",
            "/content",
        ],
    )
    .map(str::to_owned)
}

pub fn sparse_file_change_fallback_details(params: &Value) -> Option<String> {
    let mut lines = Vec::new();
    if let Some(r) = string_at(params, &["/reason"]) {
        lines.push(format!("Reason: {r}"));
    }
    if let Some(g) = string_at(params, &["/grantRoot", "/grant_root"]) {
        lines.push(format!("Grant root: {g}"));
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

pub fn presentation_for_command_execution_approval(params: &Value) -> Option<Value> {
    let command = params
        .get("command")
        .and_then(Value::as_str)
        .or_else(|| params.pointer("/item/command").and_then(Value::as_str))?;
    let cwd = params
        .get("cwd")
        .and_then(Value::as_str)
        .or_else(|| params.pointer("/item/cwd").and_then(Value::as_str));
    let choices = params
        .get("availableDecisions")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty());

    Some(json!({
        "variant": "codex_command",
        "command": command,
        "cwd": cwd,
        "available_decisions": choices.unwrap_or_else(|| {
            vec![
                "accept".to_owned(),
                "acceptForSession".to_owned(),
                "decline".to_owned(),
                "cancel".to_owned(),
            ]
        }),
    }))
}

fn string_at<'a>(message: &'a Value, pointers: &[&str]) -> Option<&'a str> {
    pointers
        .iter()
        .find_map(|pointer| message.pointer(pointer).and_then(Value::as_str))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn observes_item_started_file_change_array() {
        let msg = json!({
            "method": "item/started",
            "params": {
                "item": {
                    "id": "call_abc",
                    "type": "fileChange",
                    "changes": [
                        {"path": "src/lib.rs", "kind": {"type": "Update"}, "diff": "--- a\n+++ b"}
                    ]
                },
                "threadId": "th",
                "turnId": "tu"
            }
        });
        let mut c = CodexApprovalItemCache::default();
        c.observe_jsonrpc_message(&msg);
        let params = json!({"itemId": "call_abc"});
        let p = c
            .presentation_for_file_change_approval(&params)
            .expect("presentation");
        assert_eq!(p["variant"], "codex_file_change");
        assert_eq!(p["paths"], json!(["src/lib.rs"]));
    }

    #[test]
    fn patch_updated_merges_by_path() {
        let mut c = CodexApprovalItemCache::default();
        c.upsert_rows(
            "call_x",
            vec![FileChangeRow {
                path: "a".to_owned(),
                change_kind: "add".to_owned(),
                unified_diff: Some("old_a".to_owned()),
            }],
        );
        let upd = json!({
            "method": "item/fileChange/patchUpdated",
            "params": {
                "itemId": "call_x",
                "changes": [{"path": "a", "kind": "update", "diff": "--- newdiff"}],
            },
        });
        c.observe_jsonrpc_message(&upd);
        let p = c
            .presentation_for_file_change_approval(&json!({"itemId": "call_x"}))
            .expect("presentation");
        let files = p["files"].as_array().expect("files");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0]["unified_diff"], "--- newdiff");
    }
}
