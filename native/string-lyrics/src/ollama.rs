mod server;
use crate::{
    Song, cache,
    translation::{
        Plan, ReplyParser, Translation, TranslationOptions, cloud_name, messages, plan, thinks,
    },
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use server::Server;
use std::{
    io::{self, BufRead, BufReader},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
use ureq::Agent;

fn local_http() -> Agent {
    Agent::config_builder()
        .proxy(None)
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_millis(1500)))
        .build()
        .into()
}
struct Job {
    http: Agent,
    cancelled: AtomicBool,
}
impl Job {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.http.clone().shutdown();
    }
    fn stopped(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[derive(Deserialize, Serialize)]
struct Record {
    v: u32,
    #[serde(default)]
    model: String,
    #[serde(default)]
    lang: String,
    #[serde(default)]
    same: bool,
    out: Vec<Option<String>>,
    #[serde(default)]
    at: i64,
}

pub struct Ollama {
    dir: PathBuf,
    server: Server,
    active: Mutex<Option<Arc<Job>>>,
}
impl Ollama {
    /// Reuse a running Ollama or start the installed server on demand.
    pub fn new(data: impl AsRef<Path>) -> Self {
        Self {
            dir: data.as_ref().join("translations"),
            server: Server::installed(),
            active: Mutex::default(),
        }
    }
    /// An explicit loopback HTTP origin is externally managed. This mode never starts a process.
    pub fn with_base_url(data: impl AsRef<Path>, base: &str) -> io::Result<Self> {
        let base = base.trim_end_matches('/');
        let uri = base.parse::<ureq::http::Uri>().map_err(io::Error::other)?;
        if uri.scheme_str() != Some("http")
            || uri.authority().is_none_or(|a| a.as_str().contains('@'))
            || !crate::loopback(&uri)
            || !matches!(uri.path(), "" | "/")
            || uri.query().is_some()
        {
            return Err(io::Error::other("Ollama needs a loopback HTTP origin"));
        }
        Ok(Self {
            dir: data.as_ref().join("translations"),
            server: Server::external(base),
            active: Mutex::default(),
        })
    }
    /// Verified local models, in the same order as /api/tags.
    pub fn models(&self) -> io::Result<Vec<String>> {
        let http = local_http();
        self.server.ensure(&http, || false)?;
        let mut res = http
            .get(format!("{}/api/tags", self.server.base))
            .config()
            .timeout_global(Some(Duration::from_secs(10)))
            .build()
            .call()
            .map_err(io::Error::other)?;
        if !res.status().is_success() {
            return Err(io::Error::other(format!(
                "Ollama answered {}",
                res.status()
            )));
        }
        let tags: Value = res.body_mut().read_json().map_err(io::Error::other)?;
        let names: Vec<_> = tags["models"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|tag| tag["name"].as_str())
            .collect();
        let mut local = Vec::new();
        for group in names.chunks(8) {
            thread::scope(|scope| -> io::Result<()> {
                let jobs: Vec<_> = group
                    .iter()
                    .map(|&name| {
                        let http = &http;
                        (name, scope.spawn(move || self.is_local(http, name)))
                    })
                    .collect();
                for (name, job) in jobs {
                    if job
                        .join()
                        .map_err(|_| io::Error::other("Model check interrupted"))??
                    {
                        local.push(name.into());
                    }
                }
                Ok(())
            })?;
        }
        Ok(local)
    }
    fn is_local(&self, http: &Agent, model: &str) -> io::Result<bool> {
        if crate::trim(model).is_empty() || cloud_name(model) {
            return Ok(false);
        }
        let mut res = http
            .post(format!("{}/api/show", self.server.base))
            .config()
            .timeout_global(Some(Duration::from_secs(10)))
            .build()
            .send_json(json!({"model":model}))
            .map_err(io::Error::other)?;
        if !res.status().is_success() {
            return Ok(false);
        }
        let info: Value = res.body_mut().read_json().map_err(io::Error::other)?;
        Ok(["remote_host", "remote_model"]
            .iter()
            .all(|k| info[*k].is_null() || info[*k].as_str() == Some(""))
            && info["model_info"]
                .as_object()
                .is_some_and(|m| !m.is_empty()))
    }
    /// Interrupt the current translation, including a blocked network read.
    pub fn stop(&self) {
        if let Some(job) = self.active.lock().unwrap().take() {
            job.cancel();
        }
    }
    /// Close this service and stop any server process tree it owns.
    pub fn shutdown(&self) {
        self.server.close();
        self.stop();
    }
    /// Start a translation, replacing the previous job. The callback receives a zero-based
    /// original lyric index (including chorus repeats) and the translated text, on this worker.
    /// Start with empty rows and use the full final `lines` for the completed screen.
    pub fn translate<S: AsRef<str>>(
        &self,
        song: &Song<'_>,
        lines: &[S],
        options: TranslationOptions<'_>,
        mut update: impl FnMut(usize, Option<&str>),
    ) -> Translation {
        let job = Arc::new(Job {
            http: local_http(),
            cancelled: AtomicBool::new(false),
        });
        if let Some(old) = self.active.lock().unwrap().replace(job.clone()) {
            old.cancel();
        }
        let result = self.run(&job, song, plan(lines), options, &mut update);
        let mut active = self.active.lock().unwrap();
        if active.as_ref().is_some_and(|a| Arc::ptr_eq(a, &job)) {
            active.take();
        }
        if job.stopped() {
            Translation {
                stopped: true,
                ..Default::default()
            }
        } else {
            result
        }
    }

    fn run(
        &self,
        job: &Job,
        song: &Song<'_>,
        plan: Plan<'_>,
        options: TranslationOptions<'_>,
        update: &mut impl FnMut(usize, Option<&str>),
    ) -> Translation {
        let TranslationOptions { lang, model } = options;
        let n = plan.uniq.len();
        if n == 0 {
            return Translation {
                lines: plan.spread(&[]),
                same: true,
                ..Default::default()
            };
        }
        let text = plan.uniq.join("\n");
        let key = cache::hash(&format!("{model}\n{lang}\n{text}"));
        let path = self
            .dir
            .join(format!("{key:x}-{}.json", text.encode_utf16().count()));
        if let Some(c) = cache::read::<Record>(&path)
            && c.v == 1
            && c.out.len() == n
        {
            let lines = plan.spread(&c.out);
            for (i, line) in lines.iter().enumerate() {
                if job.stopped() {
                    break;
                }
                update(i, line.as_deref());
            }
            return Translation {
                lines,
                cached: true,
                same: c.same,
                ..Default::default()
            };
        }
        let mut request = json!({"model":model, "messages":messages(&plan.uniq, lang, song.title, song.artist),
            "stream":true,"keep_alive":"3m","options":{"temperature":0.2,"num_ctx":8192}});
        if thinks(model) {
            request["think"] = false.into();
        }
        let mut rows = vec![Vec::new(); n];
        for (row, &k) in plan.map.iter().enumerate() {
            if let Some(k) = k {
                rows[k].push(row);
            }
        }
        let mut out = vec![None; n];
        let mut parser = ReplyParser::default();
        let mut emit = |i: usize, words: Option<&str>| {
            if let Some(slot) = out.get_mut(i) {
                *slot = words.map(str::to_owned);
                for &row in &rows[i] {
                    if job.stopped() {
                        break;
                    }
                    update(row, words);
                }
            }
        };
        let error = self
            .chat(job, model, &request.to_string(), |chunk| {
                parser.push(chunk, &mut emit)
            })
            .err();
        if job.stopped() {
            return Translation {
                stopped: true,
                ..Default::default()
            };
        }
        parser.finish(emit);
        let filled = out.iter().flatten().count();
        let same = filled > 0
            && out.iter().zip(&plan.uniq).all(|(x, y)| {
                x.as_ref()
                    .is_none_or(|s| s.to_lowercase() == y.to_lowercase())
            });
        let lines = plan.spread(&out);
        // A superseded worker must never commit a partial result over its replacement.
        let active = self.active.lock().unwrap();
        if active
            .as_deref()
            .is_some_and(|current| std::ptr::eq(current, job))
            && filled * 5 >= n * 4
        {
            let _ = cache::write(
                &path,
                &Record {
                    v: 1,
                    model: model.into(),
                    lang: lang.into(),
                    same,
                    out,
                    at: Utc::now().timestamp_millis(),
                },
            );
        }
        let error = (filled < n).then(|| {
            error.map_or_else(
                || format!("{} lines came back empty", n - filled),
                |e| e.to_string(),
            )
        });
        Translation {
            lines,
            same,
            error,
            ..Default::default()
        }
    }
    fn chat(
        &self,
        job: &Job,
        model: &str,
        body: &str,
        mut chunk: impl FnMut(&str),
    ) -> io::Result<()> {
        if body.encode_utf16().count() > 400_000 {
            return Err(io::Error::other("body must be a chat request"));
        }
        self.server.ensure(&job.http, || job.stopped())?;
        if !self.is_local(&job.http, model)? {
            return Err(io::Error::other(
                "Choose an installed local model in Playback to translate privately.",
            ));
        }
        let mut res = job
            .http
            .post(format!("{}/api/chat", self.server.base))
            .header("Content-Type", "application/json")
            .send(body)
            .map_err(io::Error::other)?;
        if !res.status().is_success() {
            return Err(io::Error::other(format!(
                "Ollama answered {}: {}",
                res.status().as_u16(),
                res.body_mut().read_to_string().unwrap_or_default()
            )));
        }
        let mut error = None;
        for line in BufReader::new(res.body_mut().as_reader()).lines() {
            if job.stopped() {
                break;
            }
            let Ok(event) = serde_json::from_str::<Value>(&line?) else {
                continue;
            };
            if let Some(e) = event["error"].as_str().filter(|e| !e.is_empty()) {
                error = Some(io::Error::other(e.to_owned()));
            } else if let Some(s) = event["message"]["content"].as_str() {
                chunk(s);
            }
        }
        error.map_or(Ok(()), Err)
    }
}
impl Drop for Ollama {
    fn drop(&mut self) {
        self.shutdown();
    }
}
