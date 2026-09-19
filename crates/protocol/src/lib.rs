//! ntfy wire models and bounded newline-delimited JSON decoding.

use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};
use thiserror::Error;
use url::Url;

pub const MAX_EVENT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventKind {
    Open,
    Keepalive,
    Message,
    PollRequest,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct Priority(pub u8);

impl Priority {
    pub const MIN: Self = Self(1);
    pub const LOW: Self = Self(2);
    pub const DEFAULT: Self = Self(3);
    pub const HIGH: Self = Self(4);
    pub const MAX: Self = Self(5);
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NtfyAction {
    pub action: String,
    pub label: String,
    #[serde(default)]
    pub url: Option<Url>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub headers: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub clear: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub r#type: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub expires: Option<i64>,
    #[serde(default)]
    pub url: Option<Url>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NtfyEvent {
    pub id: String,
    pub time: i64,
    pub event: EventKind,
    pub topic: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub priority: Option<Priority>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub click: Option<Url>,
    #[serde(default)]
    pub icon: Option<Url>,
    #[serde(default)]
    pub actions: Vec<NtfyAction>,
    #[serde(default)]
    pub attachment: Option<Attachment>,
}

impl NtfyEvent {
    pub fn notification_title(&self, server: &Url) -> String {
        self.title
            .as_deref()
            .filter(|title| !title.trim().is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("{}@{}", self.topic, server))
    }
}

#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("ntfy event exceeded {MAX_EVENT_BYTES} bytes")]
    FrameTooLarge,
    #[error("invalid ntfy JSON event: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Default)]
pub struct NdjsonDecoder {
    pending: Vec<u8>,
}

impl NdjsonDecoder {
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<NtfyEvent>, DecodeError> {
        if self.pending.len().saturating_add(chunk.len()) > MAX_EVENT_BYTES {
            self.pending.clear();
            return Err(DecodeError::FrameTooLarge);
        }
        self.pending.extend_from_slice(chunk);

        let mut events = Vec::new();
        let mut consumed = 0;
        for newline in self
            .pending
            .iter()
            .enumerate()
            .filter_map(|(i, b)| (*b == b'\n').then_some(i))
        {
            let line = &self.pending[consumed..newline];
            consumed = newline + 1;
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if !line.is_empty() {
                events.push(serde_json::from_slice(line)?);
            }
        }
        if consumed > 0 {
            self.pending.drain(..consumed);
        }
        Ok(events)
    }

    pub fn finish(mut self) -> Result<Option<NtfyEvent>, DecodeError> {
        while self.pending.last().is_some_and(u8::is_ascii_whitespace) {
            self.pending.pop();
        }
        if self.pending.is_empty() {
            Ok(None)
        } else {
            Ok(Some(serde_json::from_slice(&self.pending)?))
        }
    }
}

/// A small, bounded event-id window prevents duplicate notifications after reconnects.
#[derive(Debug)]
pub struct Deduplicator {
    capacity: usize,
    order: VecDeque<String>,
    ids: HashSet<String>,
}

impl Deduplicator {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            order: VecDeque::new(),
            ids: HashSet::new(),
        }
    }

    /// Returns true only for an id not present in the bounded window.
    pub fn insert(&mut self, id: &str) -> bool {
        if self.ids.contains(id) {
            return false;
        }
        let owned = id.to_owned();
        self.ids.insert(owned.clone());
        self.order.push_back(owned);
        if self.order.len() > self.capacity
            && let Some(oldest) = self.order.pop_front()
        {
            self.ids.remove(&oldest);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_fragmented_unicode_ndjson() {
        let mut decoder = NdjsonDecoder::default();
        let json = "{\"id\":\"a\",\"time\":1,\"event\":\"message\",\"topic\":\"提醒\",\"message\":\"你好\",\"tags\":[\"warning\"]}\n";
        let bytes = json.as_bytes();
        let split = json.find('你').unwrap() + 1;
        assert!(decoder.push(&bytes[..split]).unwrap().is_empty());
        let result = decoder.push(&bytes[split..]).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].message, "你好");
        assert_eq!(result[0].tags, ["warning"]);
    }

    #[test]
    fn deduplicator_is_bounded() {
        let mut ids = Deduplicator::new(2);
        assert!(ids.insert("a"));
        assert!(!ids.insert("a"));
        assert!(ids.insert("b"));
        assert!(ids.insert("c"));
        assert!(ids.insert("a"));
    }
}
