use iced::widget::{button, column, row, space, text, text_editor};
use iced::{Element, Fill, Padding, Size, Task, Theme, keyboard, window};
use std::sync::Arc;

use notelook::config;
use notelook::db::{self, DbHandle};
use notelook::embed::Embedder;
use notelook::worker;

pub fn main() -> iced::Result {
    iced::application(App::boot, App::update, App::view)
        .title("Notelook")
        .theme(theme)
        .window(window::Settings {
            size: Size::new(900.0, 600.0),
            icon: load_window_icon(),
            ..Default::default()
        })
        .run()
}

fn load_window_icon() -> Option<window::Icon> {
    const PNG_BYTES: &[u8] = include_bytes!("../assets/logo.png");
    let img = image::load_from_memory(PNG_BYTES)
        .map_err(|e| eprintln!("failed to decode icon: {e}"))
        .ok()?
        .to_rgba8();
    let (width, height) = img.dimensions();
    window::icon::from_rgba(img.into_raw(), width, height)
        .map_err(|e| eprintln!("failed to build icon: {e}"))
        .ok()
}

fn theme(_state: &App) -> Theme {
    Theme::TokyoNightStorm
}

// ─── state ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelState {
    Loading,
    Ready,
    Failed,
}

struct App {
    content: text_editor::Content,
    /// Incremented on every edit.
    revision: u64,
    /// Revision that was last written to the DB.
    saved_revision: u64,
    char_count: usize,
    saving: bool,
    status: String,
    current_note_id: Option<i64>,
    db: Option<DbHandle>,
    model: ModelState,
}

impl App {
    fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }

    fn can_save(&self) -> bool {
        self.db.is_some() && !self.saving && self.is_dirty()
    }
}

#[derive(Debug, Clone, Copy)]
enum SaveOutcome {
    Created(i64),
    Updated(i64),
    Unchanged(i64),
}

impl SaveOutcome {
    fn note_id(self) -> i64 {
        match self {
            Self::Created(id) | Self::Updated(id) | Self::Unchanged(id) => id,
        }
    }
}

#[derive(Debug, Clone)]
enum Message {
    DbReady(Result<DbHandle, String>),
    ModelReady(Result<(), String>),
    Edit(text_editor::Action),
    NewNote,
    SaveNote,
    NoteSaved {
        revision: u64,
        result: Result<SaveOutcome, String>,
    },
    Quit,
}

// ─── async startup steps (plain functions: easy to read and test) ───────────

async fn open_database() -> anyhow::Result<DbHandle> {
    let path = config::db_path()?;
    let conn = db::open(&path).await?;
    Ok(db::spawn_actor(conn))
}

async fn start_model(db: DbHandle) -> anyhow::Result<()> {
    let embedder = tokio::task::spawn_blocking(Embedder::try_new).await??;
    let chunker = embedder.chunker(config::CHUNK_MAX_TOKENS, config::CHUNK_OVERLAP)?;
    tokio::spawn(worker::run_embed_worker(
        db,
        Arc::new(embedder),
        Arc::new(chunker),
    ));
    Ok(())
}

// ─── update ─────────────────────────────────────────────────────────────────

impl App {
    fn boot() -> (Self, Task<Message>) {
        let app = Self {
            content: text_editor::Content::new(),
            revision: 0,
            saved_revision: 0,
            char_count: 0,
            saving: false,
            status: String::from("Opening database…"),
            current_note_id: None,
            db: None,
            model: ModelState::Loading,
        };

        let task = Task::perform(
            async { open_database().await.map_err(|e| format!("{e:#}")) },
            Message::DbReady,
        );
        (app, task)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::DbReady(Ok(db)) => {
                self.db = Some(db.clone());
                self.status = String::from("Ready");
                // Step 2 of startup: load the model in the background.
                // This is to be understood if i want the model local from the get go
                Task::perform(
                    async move { start_model(db).await.map_err(|e| format!("{e:#}")) },
                    Message::ModelReady,
                )
            }
            Message::DbReady(Err(e)) => {
                self.status = format!("Database error: {e}");
                eprintln!("{e}");
                Task::none()
            }

            Message::ModelReady(Ok(())) => {
                self.model = ModelState::Ready;
                Task::none()
            }
            Message::ModelReady(Err(e)) => {
                // Notes still save; their jobs wait in the queue until next start.
                self.model = ModelState::Failed;
                self.status = format!("Model error (notes still save): {e}");
                eprintln!("{e}");
                Task::none()
            }

            Message::Edit(action) => {
                let is_edit = action.is_edit();
                self.content.perform(action);
                if is_edit {
                    self.revision += 1;
                    // Once per keystroke is fine; once per frame is not.
                    self.char_count = self.content.text().chars().count();
                }
                Task::none()
            }

            Message::NewNote => {
                if self.is_dirty() {
                    self.status = String::from("Unsaved changes: press Ctrl+S first");
                    return Task::none();
                }
                self.content = text_editor::Content::new();
                self.current_note_id = None;
                self.revision = 0;
                self.saved_revision = 0;
                self.char_count = 0;
                self.status = String::from("New note");
                Task::none()
            }

            Message::SaveNote => {
                if !self.can_save() {
                    return Task::none();
                }
                let note_text = self.content.text();
                if note_text.trim().is_empty() {
                    self.status = String::from("Cannot save an empty note");
                    return Task::none();
                }
                let Some(db) = self.db.clone() else {
                    return Task::none();
                };

                self.saving = true;
                self.status = String::from("Saving…");
                let revision = self.revision;
                // note id same for a revision, +1 for a new one
                let note_id = self.current_note_id;

                Task::perform(
                    async move {
                        let outcome = match note_id {
                            None => SaveOutcome::Created(db.insert_note(note_text).await?),
                            Some(id) => {
                                if db.update_note(id, note_text).await? {
                                    SaveOutcome::Updated(id)
                                } else {
                                    SaveOutcome::Unchanged(id)
                                }
                            }
                        };
                        anyhow::Ok(outcome)
                    },
                    move |result| Message::NoteSaved {
                        revision,
                        result: result.map_err(|e| format!("{e:#}")),
                    },
                )
            }

            Message::NoteSaved { revision, result } => {
                self.saving = false;
                match result {
                    Ok(outcome) => {
                        self.current_note_id = Some(outcome.note_id());
                        self.saved_revision = revision;
                        self.status = match outcome {
                            SaveOutcome::Created(id) => format!("Note {id} created"),
                            SaveOutcome::Updated(id) => format!("Note {id} updated"),
                            SaveOutcome::Unchanged(id) => format!("Note {id}: no changes"),
                        };
                    }
                    Err(e) => self.status = format!("Save failed: {e}"),
                }
                Task::none()
            }

            // not gently quit, need to look into this
            Message::Quit => iced::exit(),
        }
    }
    // ─── view ───────────────────────────────────────────────────────────────

    fn view(&self) -> Element<'_, Message> {
        let size = 13;

        // signal unsaved file
        let dirty_mark = if self.is_dirty() { "● " } else { "" };
        let model_label = match self.model {
            ModelState::Loading => "model loading…",
            ModelState::Ready => "search ready",
            ModelState::Failed => "model unavailable",
        };
        let stats = format!(
            "{dirty_mark}Lines: {} | Chars: {} | {model_label}",
            self.content.line_count(),
            self.char_count
        );

        // maybe this will be changed with more Win95 feel dropdown menu lol
        let action_bar = row![
            button("New").on_press(Message::NewNote),
            button("Save").on_press_maybe(self.can_save().then_some(Message::SaveNote)),
            space::horizontal(),
            text(&self.status).size(size),
            text(" · ").size(size),
            text(stats).size(size),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);

        let editor = text_editor(&self.content)
            .placeholder("Type your note here…")
            .on_action(Message::Edit)
            .key_binding(editor_bindings)
            .height(Fill);

        column![action_bar, editor]
            .spacing(12)
            .padding(Padding::new(16.0))
            .width(Fill)
            .height(Fill)
            .into()
    }
}

/// Ctrl+S / Ctrl+N / Ctrl+Q while the editor has focus; everything else
/// falls through to the editor's default bindings.
fn editor_bindings(key_press: text_editor::KeyPress) -> Option<text_editor::Binding<Message>> {
    use keyboard::Key;

    if key_press.modifiers.command() {
        let message = match key_press.key.as_ref() {
            Key::Character("s") => Some(Message::SaveNote),
            Key::Character("n") => Some(Message::NewNote),
            Key::Character("q") => Some(Message::Quit),
            _ => None,
        };
        if let Some(message) = message {
            return Some(text_editor::Binding::Custom(message));
        }
    }

    text_editor::Binding::from_key_press(key_press)
}
