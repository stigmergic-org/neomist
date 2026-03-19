use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eyre::{Result, WrapErr};
use tokio::sync::RwLock;
use tracing::field::{Field, Visit};
use tracing::{Event, warn};
use tracing_subscriber::Layer;

const CHECKPOINT_PREFIX: &str = "saved checkpoint to DB:";
const CHECKPOINTS_FILE: &str = "checkpoints.json";

#[derive(Clone)]
pub struct CheckpointLayer {
    history: Arc<RwLock<VecDeque<String>>>,
    capacity: usize,
    cache_path: PathBuf,
}

impl CheckpointLayer {
    pub fn new(
        history: Arc<RwLock<VecDeque<String>>>,
        capacity: usize,
        cache_path: PathBuf,
    ) -> Self {
        Self {
            history,
            capacity,
            cache_path,
        }
    }
}

pub fn checkpoint_cache_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join(CHECKPOINTS_FILE)
}

pub fn load_checkpoint_history(path: &Path, capacity: usize) -> VecDeque<String> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(_) => return VecDeque::new(),
    };

    let list: Vec<String> = match serde_json::from_str(&contents) {
        Ok(list) => list,
        Err(err) => {
            warn!("Failed to parse cached checkpoints: {err}");
            return VecDeque::new();
        }
    };

    let mut deque: VecDeque<String> = list.into();
    while deque.len() > capacity {
        deque.pop_back();
    }
    deque
}

impl<S> Layer<S> for CheckpointLayer
where
    S: tracing::Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        let mut visitor = MessageVisitor { message: None };
        event.record(&mut visitor);
        let message = match visitor.message {
            Some(message) => message,
            None => return,
        };

        if let Some(idx) = message.find(CHECKPOINT_PREFIX) {
            let hash = message[idx + CHECKPOINT_PREFIX.len()..].trim();
            if hash.starts_with("0x") {
                let history = self.history.clone();
                let hash = hash.to_string();
                let capacity = self.capacity;
                let cache_path = self.cache_path.clone();
                tokio::spawn(async move {
                    let mut guard = history.write().await;
                    if guard.front().map(|item| item == &hash).unwrap_or(false) {
                        return;
                    }
                    guard.push_front(hash);
                    while guard.len() > capacity {
                        guard.pop_back();
                    }
                    let snapshot: Vec<String> = guard.iter().cloned().collect();
                    drop(guard);
                    if let Err(err) = persist_checkpoints(&cache_path, &snapshot) {
                        warn!("Failed to persist checkpoints: {err}");
                    }
                });
            }
        }
    }
}

struct MessageVisitor {
    message: Option<String>,
}

impl Visit for MessageVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(value.to_string());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = Some(format!("{value:?}"));
        }
    }
}

fn persist_checkpoints(path: &Path, checkpoints: &[String]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).wrap_err("Failed to create checkpoint cache dir")?;
    }
    let payload = serde_json::to_string(&checkpoints).wrap_err("Failed to encode checkpoints")?;
    fs::write(path, payload).wrap_err("Failed to write checkpoint cache")?;
    Ok(())
}
