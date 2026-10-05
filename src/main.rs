use iced::widget::{button, column, row, space, text, text_editor};
use iced::{Element, Fill, Length, Padding, Size, Theme, window};

pub fn main() -> iced::Result {
    let icon = load_window_icon();
    iced::application(App::boot, App::update, App::view)
        .title("Notelook")
        .theme(theme)
        .window_size(Size::new(900.0, 600.0))
        .window(window::Settings {
            icon,
            ..Default::default()
        })
        .run()
}

// fn load_window_icon() -> Option<window::Icon> {
//     const ICON_BYTES: &[u8] = include_bytes!("../assets/icon.rgba");
//     window::icon::from_rgba(ICON_BYTES.to_vec(), 32, 32).ok()
// }

fn load_window_icon() -> Option<window::Icon> {
    // Embed the original logo.png directly into the binary
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

#[derive(Default)]
struct App {
    content: text_editor::Content,
    status_msg: String,
    current_note_id: Option<i64>,
}

#[derive(Debug, Clone)]
enum Message {
    Edit(text_editor::Action),
    NewNote,
    SaveNote,
    Quit,
}

impl App {
    fn boot() -> Self {
        Self {
            content: text_editor::Content::new(),
            status_msg: String::from("Ready"),
            current_note_id: None,
        }
    }

    fn update(&mut self, _message: Message) {
        match _message {
            Message::Edit(action) => {
                self.content.perform(action);
            }
            Message::NewNote => {
                self.content = text_editor::Content::new();
                self.current_note_id = None;
                self.status_msg = String::from("New note created");
            }
            Message::SaveNote => {
                let note_text = self.content.text();
                if note_text.trim().is_empty() {
                    self.status_msg = String::from("Cannot save empty note");
                } else {
                    self.status_msg = String::from("Saving...");
                }
            }
            Message::Quit => {
                std::process::exit(0);
            }
        }
    }

    fn view(&self) -> Element<Message> {
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
