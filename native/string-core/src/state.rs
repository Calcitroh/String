//! Shared state.json and its compact queue. Unowned settings and add-on records survive edits.

mod snapshot;

use crate::{Fields, queue::Queue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(from = "SavedState")]
pub struct State {
    pub v: u32,
    #[serde(default, with = "snapshot")]
    pub queue: Queue,
    #[serde(flatten)]
    pub fields: Fields,
}
#[derive(Deserialize)]
struct SavedState {
    v: u32,
    #[serde(default, with = "snapshot")]
    queue: Queue,
    #[serde(flatten)]
    fields: Fields,
}
impl From<SavedState> for State {
    fn from(mut saved: SavedState) -> Self {
        let mut fields = Self::default().fields;
        for key in ["set", "sort"] {
            if let Some(Value::Object(base)) = fields.get_mut(key)
                && let Some(Value::Object(values)) = saved.fields.remove(key)
            {
                base.extend(values);
            }
        }
        fields.extend(saved.fields);
        Self {
            v: saved.v,
            queue: saved.queue,
            fields,
        }
    }
}
impl Default for State {
    fn default() -> Self {
        let Value::Object(fields) = json!({
            "roots":[],"files":[],"playlists":[],"liked":{},"plays":{},"edits":{},"hidden":{},"notDupes":{},"recent":[],
            "vol":0.8,"muted":false,"set":{"fade":0,"autoplay":true,"timeLeft":false,"artMono":true,"viz":true,
            "discord":false,"discordId":"","dcCovers":false,"dcShow":"song"},"sort":{"key":"title","dir":1},
            "albumSort":"artist","route":"home"
        }) else {
            unreachable!()
        };
        Self {
            v: 1,
            queue: Queue::default(),
            fields,
        }
    }
}
impl State {
    pub fn play_ids(
        &mut self,
        ids: &[String],
        at: isize,
        ctx: Value,
        lib: &crate::queue::Catalog,
        now_ms: i64,
    ) -> bool {
        if !self.queue.play_ids(ids, at, ctx.clone(), lib) {
            return false;
        }
        self.remember_context(&ctx, now_ms);
        true
    }
    pub fn commit_next(
        &mut self,
        plan: crate::queue::AdvancePlan,
        options: crate::queue::PlaybackOptions,
        lib: &crate::queue::Catalog,
        now_ms: i64,
    ) -> bool {
        let new_list = plan.starts_list();
        if !self.queue.commit_next(plan, options, lib) {
            return false;
        }
        let context = self.queue.context(None);
        if new_list && matches!(context["kind"].as_str(), Some("album" | "artist" | "pl")) {
            self.remember_context(&context.clone(), now_ms);
        }
        true
    }
    pub fn hidden(&self, id: &str) -> bool {
        match self.fields.get("hidden").and_then(|v| v.get(id)) {
            None | Some(Value::Null | Value::Bool(false)) => false,
            Some(Value::Number(n)) => n.as_f64() != Some(0.),
            Some(Value::String(s)) => !s.is_empty(),
            _ => true,
        }
    }
    /// Record a selected list, or a newly started album, artist or playlist.
    pub fn remember_context(&mut self, ctx: &Value, now_ms: i64) {
        let Some(mut entry) = ctx.as_object().cloned() else {
            return;
        };
        entry.insert("at".into(), now_ms.into());
        let recent = self.fields.entry("recent").or_insert_with(|| json!([]));
        if let Some(rows) = recent.as_array_mut() {
            rows.retain(|r| r.get("kind") != ctx.get("kind") || r.get("key") != ctx.get("key"));
            rows.insert(0, Value::Object(entry));
            rows.truncate(16);
        }
    }
}
