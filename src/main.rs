use iced::widget::{button, column, row, space, text, text_editor};
use iced::{Element, Fill, Length, Padding, Size, Task, Theme, window};
mod db;
use std::fmt;
mod embed;

use embed::Embedder;
use std::io;
use std::path::Path;
use tokio::sync::watch::error;

use db::{add_chunks, add_label, add_note, init_db, last_notes, search_chunks};
use std::fs;
use turso::{Builder, Connection};

use std::sync::Arc;

pub fn main() -> iced::Result {
    let icon = load_window_icon();
    iced::application(App::boot, App::update, App::view)
        .title("Notelook")
        .theme(theme)
        .window(window::Settings {
            size: Size::new(900.0, 600.0),
            icon,
            ..Default::default()
        })
        .run()
}

fn load_window_icon() -> Option<window::Icon> {
    const PNG_BYTES: &[u8] = include_bytes!("../assets/logo.png");

    let img = image::load_from_memory(PNG_BYTES)
        .map_err(|e| eprintln!("Failed to decode PNG: {e}"))
        .ok()?
        .to_rgba8();

    let (width, height) = img.dimensions();

    window::icon::from_rgba(img.into_raw(), width, height)
        .map_err(|e| eprintln!("Failed to build window icon: {e}"))
        .ok()
}

fn theme(_state: &App) -> Theme {
    Theme::TokyoNightStorm
}

#[derive(Clone)]
pub struct AppContext {
    pub conn: Arc<Connection>,
    pub embedder: Arc<Embedder>,
}
impl fmt::Debug for AppContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppContext").finish_non_exhaustive()
    }
}

#[derive(Default)]
struct App {
    content: text_editor::Content,
    status_msg: String,
    current_note_id: Option<i64>,
    ctx: Option<AppContext>,
}

#[derive(Debug, Clone)]
enum Message {
    InitResult(Result<AppContext, String>),
    Edit(text_editor::Action),
    NewNote,
    SaveNote,
    NoteSaved(Result<i64, String>),
    Quit,
}

impl App {
    fn boot() -> (Self, Task<Message>) {
        let app = Self {
            content: text_editor::Content::new(),
            status_msg: String::from("Initializing database & model..."),
            current_note_id: None,
            ctx: None,
        };

        // Run heavy async initialization in the background
        let init_task = Task::perform(
            async {
                // 1. Setup DB
                let db = Builder::new_local("app.db")
                    .build()
                    .await
                    .map_err(|e| format!("DB build error: {e}"))?;
                let conn = db.connect().map_err(|e| format!("DB connect error: {e}"))?;
                conn.execute("PRAGMA foreign_keys = ON;", ())
                    .await
                    .map_err(|e| format!("Pragma error: {e}"))?;
                init_db(&conn)
                    .await
                    .map_err(|e| format!("Init DB error: {e}"))?;

                // 2. Load Embedder (Candle weights) on blocking pool
                let embedder = tokio::task::spawn_blocking(Embedder::try_new)
                    .await
                    .map_err(|e| format!("Embedder thread join error: {e}"))?
                    .map_err(|e| format!("Model load error: {e}"))?;

                Ok(AppContext {
                    conn: Arc::new(conn),
                    embedder: Arc::new(embedder),
                })
            },
            Message::InitResult,
        );

        (app, init_task)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::InitResult(Ok(ctx)) => {
                self.ctx = Some(ctx);
                self.status_msg = String::from("Ready");
                Task::none()
            }
            Message::InitResult(Err(err)) => {
                self.status_msg = format!("Failed to init: {err}");
                eprintln!("{err}");
                Task::none()
            }
            Message::Edit(action) => {
                self.content.perform(action);
                Task::none()
            }
            Message::NewNote => {
                self.content = text_editor::Content::new();
                self.current_note_id = None;
                self.status_msg = String::from("New note created");
                Task::none()
            }
            Message::SaveNote => {
                let note_text = self.content.text();
                if note_text.trim().is_empty() {
                    self.status_msg = String::from("Cannot save empty note");
                    return Task::none();
                }

                let Some(ctx) = self.ctx.clone() else {
                    self.status_msg = String::from("System not ready yet");
                    return Task::none();
                };

                self.status_msg = String::from("Saving & embedding...");

                Task::perform(
                    async move {
                        let note_id = add_note(note_text.clone(), &ctx.conn)
                            .await
                            .map_err(|e| format!("Failed to insert note: {e}"))?;

                        add_chunks(note_id, &note_text, &ctx.embedder, &ctx.conn)
                            .await
                            .map_err(|e| format!("Failed to chunk/embed note: {e}"))?;

                        Ok(note_id)
                    },
                    Message::NoteSaved,
                )
            }
            Message::NoteSaved(Ok(note_id)) => {
                self.current_note_id = Some(note_id);
                self.status_msg = format!("Note {note_id} saved & indexed");
                Task::none()
            }
            Message::NoteSaved(Err(e)) => {
                self.status_msg = format!("Save error: {e}");
                Task::none()
            }
            Message::Quit => {
                std::process::exit(0);
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let text_size = 13;
        let line_count = self.content.line_count();
        let char_count = self.content.text().chars().count();
        let stats_info = format!("Lines: {line_count} | Chars: {char_count}");
        let save_status = if self.status_msg.is_empty() {
            "Ready"
        } else {
            &self.status_msg
        };

        let action_bar = row![
            button("New").on_press(Message::NewNote),
            button("Save").on_press(Message::SaveNote),
            space::horizontal(),
            text(save_status).size(text_size),
            text(" . ").size(text_size),
            text(stats_info).size(text_size)
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);

        let editor = text_editor(&self.content)
            .placeholder("Type your note here...")
            .on_action(Message::Edit)
            .height(Fill);

        column![action_bar, editor]
            .spacing(12)
            .padding(Padding::new(16.0))
            .width(Fill)
            .height(Fill)
            .into()
    }
}
