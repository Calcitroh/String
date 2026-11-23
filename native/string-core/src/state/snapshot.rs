//! Read legacy queue entries and encode the shared path dictionary.

use crate::{
    paths,
    queue::{Item, Position, Queue},
};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};
use serde_json::{Value, json};
#[derive(Deserialize, Serialize, Default)]
struct Wire {
    #[serde(skip_serializing_if = "Option::is_none")]
    paths: Option<Vec<paths::Entry>>,
    #[serde(default)]
    items: Vec<Value>,
    #[serde(default = "empty_order")]
    orig: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    man: Option<Vec<usize>>,
    #[serde(flatten)]
    position: Position,
}
fn empty_order() -> Option<Vec<Value>> {
    Some(vec![])
}
pub fn serialize<S: Serializer>(q: &Queue, s: S) -> Result<S::Ok, S::Error> {
    if q.legacy_missing_order {
        let items = q
            .items
            .iter()
            .map(|x| json!({"id":x.id,"m":u8::from(x.manual)}))
            .collect();
        return Wire {
            items,
            position: q.position.clone(),
            ..Default::default()
        }
        .serialize(s);
    }
    let (paths, map) = paths::compress(
        q.items
            .iter()
            .map(|x| x.id.as_str())
            .chain(q.orig.iter().map(String::as_str)),
    );
    let items = q
        .items
        .iter()
        .map(|x| {
            let i = map[x.id.as_str()];
            json!(if x.manual { !i } else { i })
        })
        .collect();
    let orig = (!q.orig.iter().eq(q.items.iter().map(|x| &x.id)))
        .then(|| q.orig.iter().map(|id| json!(map[id.as_str()])).collect());
    Wire {
        paths: Some(paths),
        items,
        orig,
        position: q.position.clone(),
        man: None,
    }
    .serialize(s)
}
pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Queue, D::Error> {
    let w = Wire::deserialize(d)?;
    let mut q = Queue::default();
    q.position = w.position;
    if let Some(entries) = w.paths {
        let ids = paths::expand(&entries).map_err(D::Error::custom)?;
        let resolve = |v: &Value, manual: bool| -> Result<(String, bool), D::Error> {
            let n = v
                .as_i64()
                .ok_or_else(|| D::Error::custom("invalid queue index"))?;
            let i = if manual && n < 0 { !n } else { n };
            let id = usize::try_from(i)
                .ok()
                .and_then(|i| ids.get(i))
                .ok_or_else(|| D::Error::custom("queue index outside path table"))?;
            Ok((id.clone(), n < 0))
        };
        for v in w.items {
            let (id, manual) = resolve(&v, true)?;
            q.items.push(Item { id, manual });
        }
        q.orig = if let Some(orig) = w.orig {
            orig.iter()
                .map(|v| resolve(v, false).map(|x| x.0))
                .collect::<Result<_, _>>()?
        } else {
            q.items.iter().map(|x| x.id.clone()).collect()
        };
    } else {
        let manual: std::collections::HashSet<_> = w.man.unwrap_or_default().into_iter().collect();
        for (i, v) in w.items.into_iter().enumerate() {
            let item = if let Some(id) = v.as_str() {
                Item {
                    id: id.into(),
                    manual: manual.contains(&i),
                }
            } else {
                let id = v["id"]
                    .as_str()
                    .ok_or_else(|| D::Error::custom("missing legacy song id"))?;
                Item {
                    id: id.into(),
                    manual: v["m"] == 1 || v["m"] == true,
                }
            };
            if !item.id.is_empty() {
                q.items.push(item);
            }
        }
        q.legacy_missing_order = w.orig.is_none()
            && matches!(
                q.position.ctx["kind"].as_str(),
                Some("songs" | "album" | "artist" | "pl" | "liked")
            );
        q.orig = if let Some(orig) = w.orig {
            orig.into_iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| D::Error::custom("invalid legacy original order"))
                })
                .collect::<Result<_, _>>()?
        } else {
            q.items.iter().map(|x| x.id.clone()).collect()
        };
    }
    q.position.at = q.position.at.clamp(-1, q.items.len() as isize - 1);
    let mut start = 0;
    for b in &q.position.past {
        if b.start < start || b.start > q.position.list_start {
            return Err(D::Error::custom("invalid previous list boundary"));
        }
        start = b.start;
    }
    if q.position.list_start > q.items.len() {
        return Err(D::Error::custom("invalid current list boundary"));
    }
    q.changed();
    Ok(q)
}
