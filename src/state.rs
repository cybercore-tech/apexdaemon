//! Small persisted key/value store so plugins remember things across
//! restarts (last-seen PR state, last restart-attempt timestamp, etc.)
//! without each plugin inventing its own file format. One JSON object on
//! disk, keyed by plugin-chosen strings, read once at startup and
//! rewritten (atomically, via a temp-file rename) after every change —
//! infrequent enough that a simple full-file rewrite is plenty.

use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct StateStore {
    path: PathBuf,
    data: Arc<Mutex<HashMap<String, Value>>>,
}

impl StateStore {
    pub fn load(path: PathBuf) -> Self {
        let data = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self { path, data: Arc::new(Mutex::new(data)) }
    }

    pub fn get(&self, key: &str) -> Option<Value> {
        self.data.lock().unwrap().get(key).cloned()
    }

    pub fn get_str(&self, key: &str) -> Option<String> {
        self.get(key).and_then(|v| v.as_str().map(String::from))
    }

    /// Sets `key` and persists the whole store immediately. Errors are
    /// logged, not propagated — a failed write here shouldn't take down
    /// whichever plugin just made real progress (e.g. a successful ban
    /// notification) over losing one bookkeeping update.
    pub fn set(&self, key: &str, value: Value) {
        {
            let mut map = self.data.lock().unwrap();
            map.insert(key.to_string(), value);
        }
        if let Err(e) = self.persist() {
            eprintln!("[state] failed to persist {}: {e:#}", self.path.display());
        }
    }

    fn persist(&self) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let snapshot = self.data.lock().unwrap().clone();
        let json = serde_json::to_string_pretty(&snapshot)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_disk() {
        let dir = std::env::temp_dir().join(format!("apexd-state-test-{}", std::process::id()));
        let path = dir.join("state.json");

        let store = StateStore::load(path.clone());
        store.set("last_pr_state", Value::String("open".into()));
        assert_eq!(store.get_str("last_pr_state").as_deref(), Some("open"));

        let reloaded = StateStore::load(path.clone());
        assert_eq!(reloaded.get_str("last_pr_state").as_deref(), Some("open"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_yields_empty_store() {
        let store = StateStore::load(PathBuf::from("/nonexistent/path/state.json"));
        assert_eq!(store.get("anything"), None);
    }
}
