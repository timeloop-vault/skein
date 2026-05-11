use std::path::Path;

use iced::widget::{button, column, container, row, scrollable, text};
use iced::{Element, Fill, Subscription, Task};
use skein_git::{Repo, StatusEntry, StatusKind};

const REPO_PATH: &str = ".";
const TERM_ID: u64 = 0;

fn main() -> iced::Result {
	iced::application(App::new, App::update, App::view)
		.title("Skein (proto-iced)")
		.subscription(App::subscription)
		.run()
}

struct App {
	entries: Vec<StatusEntry>,
	term: iced_term::Terminal,
}

#[derive(Debug, Clone)]
enum Message {
	Refresh,
	Terminal(iced_term::Event),
}

impl App {
	fn new() -> (Self, Task<Message>) {
		let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
		let settings = iced_term::settings::Settings {
			backend: iced_term::settings::BackendSettings {
				program: shell,
				args: vec!["-l".into(), "-i".into()],
				working_directory: Some(REPO_PATH.into()),
				..Default::default()
			},
			..Default::default()
		};
		let term = iced_term::Terminal::new(TERM_ID, settings)
			.expect("failed to create iced_term::Terminal");
		(
			Self {
				entries: refresh_status(),
				term,
			},
			Task::none(),
		)
	}

	fn update(&mut self, msg: Message) -> Task<Message> {
		match msg {
			Message::Refresh => self.entries = refresh_status(),
			Message::Terminal(iced_term::Event::BackendCall(_, cmd)) => {
				let _ = self.term.handle(iced_term::Command::ProxyToBackend(cmd));
			}
		}
		Task::none()
	}

	fn view(&self) -> Element<'_, Message> {
		let git_pane = column![
			row![
				text("git status").size(16),
				button("Refresh").on_press(Message::Refresh),
			]
			.spacing(12)
			.padding(12),
			scrollable(column(self.entries.iter().map(status_row)).spacing(2).padding(12)),
		];

		let term_pane =
			container(iced_term::TerminalView::show(&self.term).map(Message::Terminal))
				.width(Fill)
				.height(Fill);

		row![
			container(git_pane).width(Fill).height(Fill),
			container(term_pane).width(Fill).height(Fill),
		]
		.into()
	}

	fn subscription(&self) -> Subscription<Message> {
		self.term.subscription().map(Message::Terminal)
	}
}

fn status_row(entry: &StatusEntry) -> Element<'_, Message> {
	let glyph = status_kind_glyph(entry.kind);
	let staged = if entry.staged { "S" } else { " " };
	text(format!("{glyph} {staged}  {}", entry.path))
		.font(iced::Font::MONOSPACE)
		.into()
}

fn refresh_status() -> Vec<StatusEntry> {
	Repo::open(Path::new(REPO_PATH))
		.and_then(|r| r.status())
		.unwrap_or_default()
}

fn status_kind_glyph(k: StatusKind) -> &'static str {
	match k {
		StatusKind::Added => "A",
		StatusKind::Modified => "M",
		StatusKind::Deleted => "D",
		StatusKind::Renamed => "R",
		StatusKind::Untracked => "?",
		StatusKind::Conflicted => "!",
		StatusKind::Typechange => "T",
	}
}
