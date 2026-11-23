use crate::{model::Song, playback::Session};
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use egui::{Context, Ui};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use string_lyrics::{LANGUAGES, LyricsResult, Ollama, Translation, TranslationOptions, pick_model};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct LanguageModel {
    lang: String,
    model: String,
}
impl LanguageModel {
    pub fn from_settings(s: &Session) -> Self {
        Self {
            lang: s
                .setting("trLang")
                .as_str()
                .filter(|l| !l.is_empty())
                .unwrap_or("English")
                .into(),
            model: s.setting("trModel").as_str().unwrap_or("").into(),
        }
    }
    pub fn save_changes(&self, s: &mut Session) {
        let old = Self::from_settings(s);
        if old.lang != self.lang {
            s.set_setting("trLang", self.lang.clone().into());
        }
        if old.model != self.model {
            s.set_setting("trModel", self.model.clone().into());
        }
    }
}

enum Event {
    Started(String),
    Line(usize, Option<String>),
    Done(Translation),
    Failed(String),
}
struct Job {
    ticket: u64,
    song: Song,
    lines: Vec<String>,
    options: LanguageModel,
    events: Sender<Event>,
    ctx: Context,
}
impl Job {
    fn post(&self, event: Event) {
        if self.events.send(event).is_ok() {
            self.ctx.request_repaint();
        }
    }
    fn run(&self, service: &Ollama, revision: &AtomicU64) {
        let current = || self.ticket == revision.load(Ordering::Acquire);
        if !current() {
            return;
        }
        let model = if self.options.model.is_empty() {
            match service.models() {
                Ok(models) => match pick_model(&models) {
                    Some(m) => m.to_owned(),
                    None => {
                        self.post(Event::Failed("Ollama has no models yet. Download one first, for example: ollama pull qwen3:14b".into()));
                        return;
                    }
                },
                Err(error) => {
                    self.post(Event::Failed(error.to_string()));
                    return;
                }
            }
        } else {
            self.options.model.clone()
        };
        if !current() {
            return;
        }
        self.post(Event::Started(model.clone()));
        let options = TranslationOptions {
            lang: &self.options.lang,
            model: &model,
        };
        let result =
            service.translate(&self.song.lyrics_song(), &self.lines, options, |i, line| {
                if current() {
                    self.post(Event::Line(i, line.map(str::to_owned)));
                }
            });
        self.post(Event::Done(result));
    }
}

pub(super) struct Translator {
    pub(super) service: Arc<Ollama>,
    revision: Arc<AtomicU64>,
    jobs: Sender<Job>,
    events: Option<Receiver<Event>>,
    active: Option<(String, String, LanguageModel)>,
    retry_on_open: bool,
    model: String,
    models: Option<Result<Vec<String>, String>>,
    listing: Option<Receiver<Result<Vec<String>, String>>>,
    pub status: String,
    pub rows: Vec<Option<String>>,
    pub layout_revision: u64,
}
impl Translator {
    pub fn new(service: Ollama) -> Self {
        let service = Arc::new(service);
        let revision = Arc::new(AtomicU64::new(0));
        let (jobs, rx) = unbounded::<Job>();
        let (backend, current) = (service.clone(), revision.clone());
        // Serialize translations so a late start can never cancel the newer song.
        std::thread::spawn(move || {
            for job in rx {
                job.run(&backend, &current);
            }
        });
        Self {
            service,
            revision,
            jobs,
            events: None,
            active: None,
            retry_on_open: false,
            model: String::new(),
            models: None,
            listing: None,
            status: String::new(),
            rows: vec![],
            layout_revision: 0,
        }
    }
    pub fn page_opened(&mut self, song: &str, force: bool) {
        self.poll();
        if force || self.retry_on_open || self.active.as_ref().is_some_and(|(id, _, _)| id != song)
        {
            self.clear();
        }
    }
    pub fn clear(&mut self) {
        if self.active.take().is_some() {
            self.retry_on_open = false;
            self.revision.fetch_add(1, Ordering::AcqRel);
            self.service.stop();
            self.events = None;
            self.rows.clear();
            self.layout_revision += 1;
            self.status.clear();
        }
    }
    pub fn update(
        &mut self,
        ctx: &Context,
        song: &Song,
        lyrics: &LyricsResult,
        options: LanguageModel,
    ) {
        if self.active.as_ref().is_none_or(|(id, text, current)| {
            id != &song.id || text != &lyrics.text || current != &options
        }) {
            self.clear();
            let (tx, rx) = unbounded();
            self.events = Some(rx);
            self.active = Some((song.id.clone(), lyrics.text.clone(), options.clone()));
            self.rows.resize(lyrics.lines.len(), None);
            self.status = format!("Translating into {}…", options.lang);
            let job = Job {
                ticket: self.revision.load(Ordering::Acquire),
                song: song.clone(),
                lines: lyrics.lines.iter().map(|l| l.text.clone()).collect(),
                options: options.clone(),
                events: tx,
                ctx: ctx.clone(),
            };
            let _ = self.jobs.send(job);
        }
        self.poll();
    }
    pub fn poll(&mut self) {
        let Some((_, _, options)) = &self.active else {
            return;
        };
        let lang = &options.lang;
        for event in self.events.iter().flat_map(|events| events.try_iter()) {
            match event {
                Event::Started(m) => {
                    self.status = format!("Translating into {lang} with {m}, on this PC…");
                    self.model = m;
                }
                Event::Line(i, line) => {
                    if let Some(row) = self.rows.get_mut(i) {
                        *row = line;
                        self.layout_revision += 1;
                    }
                }
                Event::Failed(e) => {
                    self.retry_on_open = true;
                    self.status = format!("Couldn't translate: {e}");
                }
                Event::Done(done) if !done.stopped => {
                    self.retry_on_open = done.error.is_some();
                    self.rows = done.lines;
                    self.layout_revision += 1;
                    self.status = if let Some(error) = &done.error
                        && self.rows.iter().all(Option::is_none)
                    {
                        format!("Couldn't translate: {error}")
                    } else if done.same {
                        format!("Already in {lang}")
                    } else {
                        let extra = done.error.map_or(String::new(), |e| format!(" ({e})"));
                        format!(
                            "Translated into {lang} by {}, on this PC{extra}",
                            self.model
                        )
                    };
                }
                Event::Done(_) => {}
            }
        }
    }
    pub fn settings(&mut self, ui: &mut Ui, options: &mut LanguageModel) {
        if self.models.is_none() && self.listing.is_none() {
            let (service, ctx) = (self.service.clone(), ui.ctx().clone());
            let (tx, rx) = bounded(1);
            self.listing = Some(rx);
            std::thread::spawn(move || {
                let _ = tx.send(service.models().map_err(|e| e.to_string()));
                ctx.request_repaint();
            });
        }
        if let Some(models) = self.listing.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.models = Some(models);
            self.listing = None;
        }
        egui::ComboBox::from_label("Language")
            .selected_text(&options.lang)
            .show_ui(ui, |ui| {
                for &name in LANGUAGES {
                    ui.selectable_value(&mut options.lang, name.to_owned(), name);
                }
            });
        let name = if options.model.is_empty() {
            "Automatic"
        } else {
            &options.model
        };
        egui::ComboBox::from_label("Model")
            .selected_text(name)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut options.model, String::new(), "Automatic");
                if let Some(Ok(names)) = &self.models {
                    for name in names {
                        ui.selectable_value(&mut options.model, name.clone(), name);
                    }
                }
            });
        match &self.models {
            Some(Err(error)) => {
                ui.label(error);
            }
            None => {
                ui.label("Checking local models…");
            }
            _ => {}
        }
        if self.listing.is_none() && ui.small_button("Refresh models").clicked() {
            self.models = None;
        }
    }
}
impl Drop for Translator {
    fn drop(&mut self) {
        self.revision.fetch_add(1, Ordering::AcqRel);
        self.service.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reopening_lyrics_retries_failed_jobs_once_and_keeps_other_jobs() {
        use super::super::tests::fixture;

        let (_dir, song, mut view, options) = fixture();
        let ctx = Context::default();
        let lyrics = view.lyrics.get(&song.lyrics_song(), false, false);
        // Handle the submitted jobs here so a failure can arrive before or after leaving.
        let (tx, jobs) = unbounded();
        view.translator.jobs = tx;
        let draw = |view: &mut super::super::LyricsView| {
            view.update(&ctx, &song, None);
            view.translator
                .update(&ctx, &song, &lyrics, options.clone());
        };
        draw(&mut view);
        let mut job = jobs.try_recv().unwrap();
        view.check_visible(None, false, Some(&song.id));
        draw(&mut view);
        assert!(jobs.is_empty(), "keeps a running job");
        job.post(Event::Failed("No models".into()));
        draw(&mut view);
        draw(&mut view);
        assert!(view.translator.status.contains("No models") && jobs.is_empty());
        view.check_visible(None, false, Some(&song.id));
        draw(&mut view);
        job = jobs.try_recv().expect("retry the failed model lookup");
        view.check_visible(None, false, Some(&song.id));
        job.post(Event::Done(Translation {
            error: Some("Disconnected".into()),
            ..Default::default()
        }));
        draw(&mut view);
        job = jobs
            .try_recv()
            .expect("retry the translation that failed while away");
        job.post(Event::Done(Translation {
            lines: vec![Some("words".into())],
            ..Default::default()
        }));
        draw(&mut view);
        view.check_visible(None, false, Some(&song.id));
        draw(&mut view);
        assert!(jobs.is_empty(), "keeps a successful job");
        assert_eq!(view.translator.rows, [Some("words".into())]);
    }
}
