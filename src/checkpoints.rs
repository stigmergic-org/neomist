use std::collections::VecDeque;
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::Event;
use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;

const CHECKPOINT_PREFIX: &str = "saved checkpoint to DB:";

#[derive(Clone)]
pub struct CheckpointLayer {
    history: Arc<RwLock<VecDeque<String>>>,
    capacity: usize,
}

impl CheckpointLayer {
    pub fn new(history: Arc<RwLock<VecDeque<String>>>, capacity: usize) -> Self {
        Self { history, capacity }
    }
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
                tokio::spawn(async move {
                    let mut guard = history.write().await;
                    if guard.front().map(|item| item == &hash).unwrap_or(false) {
                        return;
                    }
                    guard.push_front(hash);
                    while guard.len() > capacity {
                        guard.pop_back();
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
