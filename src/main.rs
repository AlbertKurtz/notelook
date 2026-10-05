use iced::widget::{button, column, row, space, text, text_editor};
use iced::{Element, Fill, Length, Padding, Size, Theme};

pub fn main() -> iced::Result {
    iced::application(App::boot, App::update, App::view)
        .title("Notelook")
        .theme(theme)
        .window_size(Size::new(900.0, 600.0))
        .run()
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
