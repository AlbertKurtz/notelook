use iced::widget::{
    button, column, container, row, scrollable, space, text, text_editor, text_input,
};
use iced::{
    Alignment, Border, Color, Element, Fill, Length, Padding, Size, Task, Theme, keyboard, window,
};
use std::sync::Arc;

use notelook::chunk::Chunker;
use notelook::config;
use notelook::db::{Db, Note, SearchHit};
use notelook::embed::Embedder;
use notelook::worker;

pub fn main() -> iced::Result {
    iced::application(App::boot, App::update, App::view)
        .title("Notelook")
        .theme(theme)
        .window(window::Settings {
            size: Size::new(1050.0, 680.0),
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
    revision: u64,
    saved_revision: u64,
    char_count: usize,
    saving: bool,
    status: String,
    current_note_id: Option<i64>,

    // Data & Models
    db: Option<Db>,
    embedder: Option<Arc<Embedder>>,
    chunker: Option<Arc<Chunker>>,
    model_state: ModelState,

    // Sidebar & Search State
    notes: Vec<Note>,
    search_query: String,
    search_results: Vec<SearchHit>,
    is_searching: bool,
}

impl App {
    fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }

    fn can_save(&self) -> bool {
        self.db.is_some() && !self.saving && self.is_dirty()
    }
}

#[derive(Debug, Clone)]
enum Message {
    DbReady(Result<Db, String>),
    ModelReady(Result<(Arc<Embedder>, Arc<Chunker>), String>),
    NotesLoaded(Result<Vec<Note>, String>),
    SelectNote(i64),
    NoteLoaded(Result<Option<Note>, String>),
    Edit(text_editor::Action),
    NewNote,
    SaveNote,
    NoteSaved(Result<(i64, bool, String), String>),
    DeleteNote,
    NoteDeleted(Result<(), String>),
    SearchInputChanged(String),
    PerformSearch,
    SearchResultsLoaded(Result<Vec<SearchHit>, String>),
    ClearSearch,
    Quit,
}

// ─── async startup steps ────────────────────────────────────────────────────

async fn open_database() -> anyhow::Result<Db> {
    let path = config::db_path()?;
    Db::open(&path).await
}

async fn load_model() -> anyhow::Result<(Arc<Embedder>, Arc<Chunker>)> {
    let embedder = tokio::task::spawn_blocking(Embedder::try_new).await??;
    let chunker = embedder.chunker(config::CHUNK_MAX_TOKENS, config::CHUNK_OVERLAP)?;
    Ok((Arc::new(embedder), Arc::new(chunker)))
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
            embedder: None,
            chunker: None,
            model_state: ModelState::Loading,
            notes: Vec::new(),
            search_query: String::new(),
            search_results: Vec::new(),
            is_searching: false,
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
                self.status = String::from("Loading notes…");

                // 1. Fetch recent notes list
                let load_notes = Task::perform(
                    {
                        let db = db.clone();
                        async move { db.recent_notes(50).await.map_err(|e| format!("{e:#}")) }
                    },
                    Message::NotesLoaded,
                );

                // 2. Load the embedding model in background
                let load_model_task = Task::perform(
                    async { load_model().await.map_err(|e| format!("{e:#}")) },
                    Message::ModelReady,
                );

                Task::batch([load_notes, load_model_task])
            }
            Message::DbReady(Err(e)) => {
                self.status = format!("Database error: {e}");
                eprintln!("{e}");
                Task::none()
            }

            Message::NotesLoaded(Ok(notes)) => {
                self.notes = notes;
                self.status = String::from("Ready");
                Task::none()
            }
            Message::NotesLoaded(Err(e)) => {
                self.status = format!("Could not load notes: {e}");
                Task::none()
            }

            Message::ModelReady(Ok((embedder, chunker))) => {
                self.embedder = Some(Arc::clone(&embedder));
                self.chunker = Some(Arc::clone(&chunker));
                self.model_state = ModelState::Ready;

                // Sync any unindexed notes in the background
                if let Some(db) = self.db.clone() {
                    tokio::spawn(async move {
                        let _ = worker::index_unindexed_notes(db, embedder, chunker).await;
                    });
                }
                Task::none()
            }
            Message::ModelReady(Err(e)) => {
                self.model_state = ModelState::Failed;
                self.status = format!("Model error: {e}");
                eprintln!("{e}");
                Task::none()
            }

            Message::SelectNote(id) => {
                if self.is_dirty() {
                    self.status = String::from("Unsaved changes: save before switching notes");
                    return Task::none();
                }
                let Some(db) = self.db.clone() else {
                    return Task::none();
                };
                self.status = String::from("Loading note…");
                Task::perform(
                    async move { db.get_note(id).await.map_err(|e| format!("{e:#}")) },
                    Message::NoteLoaded,
                )
            }

            Message::NoteLoaded(Ok(Some(note))) => {
                self.content = text_editor::Content::with_text(&note.content);
                self.current_note_id = Some(note.id);
                self.revision = 0;
                self.saved_revision = 0;
                self.char_count = self.content.text().chars().count();
                self.status = format!("Loaded note {}", note.id);
                Task::none()
            }
            Message::NoteLoaded(Ok(None)) => {
                self.status = String::from("Note not found");
                Task::none()
            }
            Message::NoteLoaded(Err(e)) => {
                self.status = format!("Error loading note: {e}");
                Task::none()
            }

            Message::Edit(action) => {
                let is_edit = action.is_edit();
                self.content.perform(action);
                if is_edit {
                    self.revision += 1;
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
                let text = self.content.text();
                if text.trim().is_empty() {
                    self.status = String::from("Cannot save an empty note");
                    return Task::none();
                }
                let Some(db) = self.db.clone() else {
                    return Task::none();
                };

                self.saving = true;
                self.status = String::from("Saving…");
                let note_id = self.current_note_id;
                let text_clone = text.clone();

                Task::perform(
                    async move {
                        let (id, updated) = match note_id {
                            None => (db.insert_note(&text_clone).await?, true),
                            Some(id) => {
                                let changed = db.update_note(id, &text_clone).await?;
                                (id, changed)
                            }
                        };
                        anyhow::Ok((id, updated, text_clone))
                    },
                    move |result| Message::NoteSaved(result.map_err(|e| format!("{e:#}"))),
                )
            }

            Message::NoteSaved(Ok((id, changed, content))) => {
                self.saving = false;
                self.current_note_id = Some(id);
                self.saved_revision = self.revision;
                self.status = if changed {
                    format!("Note {id} saved")
                } else {
                    format!("Note {id}: unchanged")
                };

                // Asynchronously trigger embedding in background if content changed
                if changed {
                    if let (Some(db), Some(emb), Some(chk)) =
                        (self.db.clone(), self.embedder.clone(), self.chunker.clone())
                    {
                        tokio::spawn(async move {
                            let _ = worker::embed_note(db, emb, chk, id, content).await;
                        });
                    }
                }

                // Refresh the sidebar notes list
                if let Some(db) = self.db.clone() {
                    Task::perform(
                        async move { db.recent_notes(50).await.map_err(|e| format!("{e:#}")) },
                        Message::NotesLoaded,
                    )
                } else {
                    Task::none()
                }
            }
            Message::NoteSaved(Err(e)) => {
                self.saving = false;
                self.status = format!("Save failed: {e}");
                Task::none()
            }

            Message::DeleteNote => {
                let Some(id) = self.current_note_id else {
                    return Task::none();
                };
                let Some(db) = self.db.clone() else {
                    return Task::none();
                };

                self.status = format!("Deleting note {id}…");
                Task::perform(
                    async move { db.delete_note(id).await.map_err(|e| format!("{e:#}")) },
                    Message::NoteDeleted,
                )
            }

            Message::NoteDeleted(Ok(())) => {
                self.content = text_editor::Content::new();
                self.current_note_id = None;
                self.revision = 0;
                self.saved_revision = 0;
                self.char_count = 0;
                self.status = String::from("Note deleted");

                // Refresh sidebar
                if let Some(db) = self.db.clone() {
                    Task::perform(
                        async move { db.recent_notes(50).await.map_err(|e| format!("{e:#}")) },
                        Message::NotesLoaded,
                    )
                } else {
                    Task::none()
                }
            }
            Message::NoteDeleted(Err(e)) => {
                self.status = format!("Delete failed: {e}");
                Task::none()
            }

            Message::SearchInputChanged(query) => {
                self.search_query = query;
                if self.search_query.trim().is_empty() {
                    self.is_searching = false;
                    self.search_results.clear();
                }
                Task::none()
            }

            Message::PerformSearch => {
                let query = self.search_query.trim().to_string();
                if query.is_empty() {
                    self.is_searching = false;
                    self.search_results.clear();
                    return Task::none();
                }
                let (Some(db), Some(embedder)) = (self.db.clone(), self.embedder.clone()) else {
                    self.status = String::from("Embedding model not ready yet");
                    return Task::none();
                };

                self.is_searching = true;
                self.status = format!("Searching for \"{query}\"…");

                Task::perform(
                    async move {
                        let query_vec = tokio::task::spawn_blocking({
                            let emb = Arc::clone(&embedder);
                            let q = query.clone();
                            move || emb.compute(&q)
                        })
                        .await??;

                        db.search_semantic(&query_vec, 15).await
                    },
                    move |result| {
                        Message::SearchResultsLoaded(result.map_err(|e| format!("{e:#}")))
                    },
                )
            }

            Message::SearchResultsLoaded(Ok(results)) => {
                self.status = format!("Found {} semantic matches", results.len());
                self.search_results = results;
                Task::none()
            }
            Message::SearchResultsLoaded(Err(e)) => {
                self.status = format!("Search error: {e}");
                Task::none()
            }

            Message::ClearSearch => {
                self.search_query.clear();
                self.search_results.clear();
                self.is_searching = false;
                self.status = String::from("Search cleared");
                Task::none()
            }

            Message::Quit => iced::exit(),
        }
    }

    // ─── view ───────────────────────────────────────────────────────────────

    fn view(&self) -> Element<'_, Message> {
        let sidebar = self.view_sidebar();
        let editor = self.view_editor();

        row![sidebar, editor].width(Fill).height(Fill).into()
    }

    fn view_sidebar(&self) -> Element<'_, Message> {
        let title_row = row![
            text("Notes").size(18),
            space::horizontal(),
            button("+ New").on_press(Message::NewNote).padding([4, 10]),
        ]
        .align_y(Alignment::Center);

        let search_box = row![
            text_input("Semantic search…", &self.search_query)
                .on_input(Message::SearchInputChanged)
                .on_submit(Message::PerformSearch)
                .padding(8),
            if self.is_searching {
                button("✕").on_press(Message::ClearSearch).padding([6, 8])
            } else {
                button("🔍")
                    .on_press(Message::PerformSearch)
                    .padding([6, 8])
            }
        ]
        .spacing(6);

        let list_content: Element<'_, Message> = if self.is_searching {
            if self.search_results.is_empty() {
                container(text("No matches found").size(13))
                    .padding(16)
                    .into()
            } else {
                let items: Vec<Element<'_, Message>> = self
                    .search_results
                    .iter()
                    .map(|hit| {
                        let preview: String = hit.content.chars().take(80).collect();
                        let sim_badge = format!("{:.0}% match", hit.similarity_percent());

                        button(
                            column![
                                row![
                                    text(format!("Note {}", hit.note_id)).size(13),
                                    space::horizontal(),
                                    text(sim_badge).size(12),
                                ],
                                text(preview).size(12),
                            ]
                            .spacing(4),
                        )
                        .on_press(Message::SelectNote(hit.note_id))
                        .width(Fill)
                        .padding(8)
                        .into()
                    })
                    .collect();

                scrollable(column(items).spacing(6)).height(Fill).into()
            }
        } else {
            let items: Vec<Element<'_, Message>> = self
                .notes
                .iter()
                .map(|note| {
                    let is_active = self.current_note_id == Some(note.id);
                    let title = note.title();
                    let date = &note.updated_at;

                    button(column![text(title).size(14), text(date).size(11),].spacing(2))
                        .on_press(Message::SelectNote(note.id))
                        .width(Fill)
                        .padding(8)
                        .style(move |theme, status| {
                            let mut style = button::primary(theme, status);
                            if is_active {
                                style.border = Border {
                                    color: Color::from_rgb(0.4, 0.6, 1.0),
                                    width: 1.5,
                                    radius: 4.0.into(),
                                };
                            }
                            style
                        })
                        .into()
                })
                .collect();

            scrollable(column(items).spacing(4)).height(Fill).into()
        };

        container(
            column![title_row, search_box, list_content]
                .spacing(12)
                .width(Length::Fixed(280.0))
                .height(Fill),
        )
        .padding(12)
        .style(|theme: &Theme| {
            let palette = theme.palette();
            container::Style {
                background: Some(palette.background.into()),
                border: Border {
                    color: Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            }
        })
        .into()
    }

    fn view_editor(&self) -> Element<'_, Message> {
        let dirty_mark = if self.is_dirty() { "● " } else { "" };
        let note_header = match self.current_note_id {
            Some(id) => format!("{dirty_mark}Note #{id}"),
            None => format!("{dirty_mark}New Note (Unsaved)"),
        };

        let action_bar = row![
            text(note_header).size(16),
            space::horizontal(),
            button("Save (Ctrl+S)")
                .on_press_maybe(self.can_save().then_some(Message::SaveNote))
                .padding([6, 12]),
            button("Delete")
                .on_press_maybe(self.current_note_id.map(|_| Message::DeleteNote))
                .padding([6, 12]),
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        let editor = text_editor(&self.content)
            .placeholder("Type your markdown note here…")
            .on_action(Message::Edit)
            .key_binding(editor_bindings)
            .height(Fill);

        let model_label = match self.model_state {
            ModelState::Loading => "Model loading…",
            ModelState::Ready => "AI Search Ready",
            ModelState::Failed => "AI Search Unavailable",
        };

        let status_bar = row![
            text(&self.status).size(12),
            space::horizontal(),
            text(format!(
                "Lines: {} | Chars: {} | {}",
                self.content.line_count(),
                self.char_count,
                model_label
            ))
            .size(12),
        ]
        .align_y(Alignment::Center);

        column![action_bar, editor, status_bar]
            .spacing(12)
            .padding(Padding::new(16.0))
            .width(Fill)
            .height(Fill)
            .into()
    }
}

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
