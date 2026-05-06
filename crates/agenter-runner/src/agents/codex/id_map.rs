#![allow(dead_code)]

use std::collections::HashMap;

use agenter_core::{ItemId, SessionId, TurnId};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct CodexIdMap {
    namespace: Uuid,
    turn_ids: HashMap<String, TurnId>,
    item_ids: HashMap<String, ItemId>,
}

impl CodexIdMap {
    #[must_use]
    pub fn for_session(session_id: SessionId) -> Self {
        let namespace = Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("agenter:codex:session:{session_id}").as_bytes(),
        );
        Self {
            namespace,
            turn_ids: HashMap::new(),
            item_ids: HashMap::new(),
        }
    }

    #[must_use]
    pub fn with_namespace(namespace: Uuid) -> Self {
        Self {
            namespace,
            turn_ids: HashMap::new(),
            item_ids: HashMap::new(),
        }
    }

    pub fn turn_id(&mut self, native_thread_id: &str, native_turn_id: &str) -> TurnId {
        let key = scoped_key(native_thread_id, native_turn_id);
        *self
            .turn_ids
            .entry(key.clone())
            .or_insert_with(|| TurnId::from_uuid(stable_uuid(self.namespace, "turn", &key)))
    }

    pub fn item_id(&mut self, native_thread_id: &str, native_item_id: &str) -> ItemId {
        let key = scoped_key(native_thread_id, native_item_id);
        *self
            .item_ids
            .entry(key.clone())
            .or_insert_with(|| ItemId::from_uuid(stable_uuid(self.namespace, "item", &key)))
    }
}

fn scoped_key(native_thread_id: &str, native_id: &str) -> String {
    format!("{native_thread_id}:{native_id}")
}

fn stable_uuid(namespace: Uuid, kind: &str, key: &str) -> Uuid {
    Uuid::new_v5(&namespace, format!("codex:{kind}:{key}").as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_id_map_reuses_ids_and_is_stable_for_repeated_imports() {
        let session_id = SessionId::new();
        let mut first = CodexIdMap::for_session(session_id);
        let mut second = CodexIdMap::for_session(session_id);

        let first_turn = first.turn_id("thread-a", "turn-1");
        assert_eq!(first.turn_id("thread-a", "turn-1"), first_turn);
        assert_eq!(second.turn_id("thread-a", "turn-1"), first_turn);
        assert_ne!(first.turn_id("thread-b", "turn-1"), first_turn);

        let first_item = first.item_id("thread-a", "item-1");
        assert_eq!(first.item_id("thread-a", "item-1"), first_item);
        assert_eq!(second.item_id("thread-a", "item-1"), first_item);
        assert_ne!(first.item_id("thread-a", "item-2"), first_item);
    }
}
