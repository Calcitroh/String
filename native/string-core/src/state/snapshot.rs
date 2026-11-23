//! Read legacy queue entries and encode the shared path dictionary.

use crate::{
    data::{array, object, take},
    paths,
    queue::{Item, Position, Queue},
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Value, json};
#[derive(Serialize, Default)]
struct Wire {
    #[serde(skip_serializing_if = "Option::is_none")]
    paths: Option<Vec<paths::Entry>>,
    items: Vec<Value>,
    orig: Option<Vec<Value>>,
    #[serde(flatten)]
    position: Position,
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
    }
    .serialize(s)
}
pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Queue, D::Error> {
    let mut fields = object(Value::deserialize(d)?);
    let dictionary = take(&mut fields, "paths")
        .as_array_mut()
        .map(|v| paths::expand(std::mem::take(v)));
    let items = array(take(&mut fields, "items"));
    let manual: std::collections::HashSet<_> = array(take(&mut fields, "man"))
        .into_iter()
        .filter_map(|v| v.as_u64())
        .collect();
    let orig = fields.remove("orig").unwrap_or_else(|| json!([]));
    let mut q = Queue::default();
    let pos = &mut q.position;
    let at = take(&mut fields, "at")
        .as_f64()
        .filter(|n| n.fract() == 0.)
        .unwrap_or(-1.) as isize;
    pos.pos = take(&mut fields, "pos").as_f64().unwrap_or_default();
    pos.shuffle = take(&mut fields, "shuffle").as_bool().unwrap_or(false);
    pos.repeat = serde_json::from_value(take(&mut fields, "repeat")).unwrap_or_default();
    let ctx = take(&mut fields, "ctx");
    if ctx.is_object() {
        pos.ctx = ctx;
    }
    let list_start = take(&mut fields, "listStart").as_u64().unwrap_or_default() as usize;
    let past = array(take(&mut fields, "past"));
    pos.extra = fields;

    let resolve = |v: &Value, allow_manual: bool| -> Option<Item> {
        let dictionary = dictionary.as_ref()?;
        let n = v.as_i64()?;
        let i = if allow_manual && n < 0 { !n } else { n };
        let id = dictionary.get(usize::try_from(i).ok()?)?.as_ref()?;
        Some(Item {
            id: id.clone(),
            manual: n < 0,
        })
    };
    // Map old positions to the surviving rows so one bad item cannot move the current song.
    let mut counts = vec![0];
    for (i, v) in items.into_iter().enumerate() {
        let item = if dictionary.is_some() {
            resolve(&v, true)
        } else if let Some(id) = v.as_str() {
            Some(Item {
                id: id.into(),
                manual: manual.contains(&(i as u64)),
            })
        } else {
            v["id"].as_str().map(|id| Item {
                id: id.into(),
                manual: v["m"] == 1 || v["m"] == true,
            })
        };
        if let Some(item) = item.filter(|x| !x.id.is_empty()) {
            q.items.push(item);
        }
        counts.push(q.items.len());
    }
    let count = |i: usize| counts[i.min(counts.len() - 1)];
    pos.at = count(at.saturating_add(1).max(0) as usize) as isize - 1;
    pos.list_start = count(list_start);
    let mut previous = 0;
    for value in past {
        let mut boundary = object(value);
        let Some(start) = take(&mut boundary, "start").as_u64() else {
            continue;
        };
        let start = count(start as usize);
        if start < previous || start > pos.list_start {
            continue;
        }
        let ctx = take(&mut boundary, "ctx");
        pos.past.push(crate::queue::Boundary {
            start,
            ctx: if ctx.is_object() { ctx } else { Value::Null },
            extra: boundary,
        });
        previous = start;
    }
    q.legacy_missing_order = dictionary.is_none()
        && orig.is_null()
        && matches!(
            pos.ctx["kind"].as_str(),
            Some("songs" | "album" | "artist" | "pl" | "liked")
        );
    q.orig = if let Value::Array(order) = orig {
        order
            .into_iter()
            .filter_map(|v| {
                if dictionary.is_some() {
                    resolve(&v, false).map(|item| item.id)
                } else {
                    v.as_str().map(str::to_owned)
                }
            })
            .collect()
    } else {
        q.items.iter().map(|item| item.id.clone()).collect()
    };
    q.changed();
    Ok(q)
}
