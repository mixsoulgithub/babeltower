mod keymap;
mod render;
mod theme;

use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::dict::{self, Entry};
use crate::merge;
use crate::store::{self, Grade, Store, WordRow, WordUpdate};
use crate::text::{self, Classified, display_width};
use crate::translate::Translator;
use crate::tui::theme::Theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Lookup,
    Words,
    Review,
    Stats,
}

impl Tab {
    pub fn all() -> [Tab; 4] {
        [Tab::Lookup, Tab::Words, Tab::Review, Tab::Stats]
    }

    pub fn index(self) -> usize {
        Tab::all().iter().position(|tab| *tab == self).unwrap_or(0)
    }

    pub fn label(self) -> &'static str {
        match self {
            Tab::Lookup => "LOOKUP",
            Tab::Words => "WORDS",
            Tab::Review => "REVIEW",
            Tab::Stats => "STATS",
        }
    }

    pub fn parse(value: &str) -> Option<Tab> {
        match value {
            "lookup" | "l" => Some(Tab::Lookup),
            "words" | "w" => Some(Tab::Words),
            "review" | "r" => Some(Tab::Review),
            "stats" | "s" => Some(Tab::Stats),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum InsertTarget {
    Lookup,
    WordsFilter { original: String },
    EditTranslation { uuid: String },
    AddWord,
    AddTranslation,
    ReviewTyping,
}

#[derive(Clone, Debug)]
pub enum ConfirmAction {
    DeleteWord { uuid: String, word: String },
}

#[derive(Clone, Debug)]
pub enum Mode {
    Normal,
    Insert(InsertTarget),
    Command,
    Help,
    Confirm(ConfirmAction),
}

#[derive(Clone, Debug)]
pub enum Overlay {
    Dict(Entry),
    Word(WordRow),
}

#[derive(Clone, Debug)]
pub enum LookupState {
    Empty,
    Entries(Vec<Entry>),
    Sentence {
        source: String,
        translation: String,
        cached: bool,
    },
    NotFound,
    Loading {
        source: String,
    },
    Error(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Due,
    Hit,
    Last,
    Word,
    Box,
}

impl SortKey {
    pub fn label(self) -> &'static str {
        match self {
            SortKey::Due => "due",
            SortKey::Hit => "hit",
            SortKey::Last => "last",
            SortKey::Word => "word",
            SortKey::Box => "box",
        }
    }

    pub fn next(self) -> SortKey {
        match self {
            SortKey::Due => SortKey::Hit,
            SortKey::Hit => SortKey::Last,
            SortKey::Last => SortKey::Word,
            SortKey::Word => SortKey::Box,
            SortKey::Box => SortKey::Due,
        }
    }

    pub fn parse(value: &str) -> Option<SortKey> {
        match value {
            "due" => Some(SortKey::Due),
            "hit" => Some(SortKey::Hit),
            "last" => Some(SortKey::Last),
            "word" => Some(SortKey::Word),
            "box" => Some(SortKey::Box),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewMode {
    Flash,
    Type,
}

impl ReviewMode {
    pub fn label(self) -> &'static str {
        match self {
            ReviewMode::Flash => "flash",
            ReviewMode::Type => "type",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ReviewSession {
    pub queue: Vec<WordRow>,
    pub index: usize,
    pub revealed: bool,
    pub checked: bool,
    pub last_ok: Option<bool>,
    pub typed: String,
    pub mode: Option<ReviewMode>,
    pub reverse: bool,
    pub requeued: Vec<String>,
    pub done: usize,
    pub good: usize,
    pub again: usize,
    pub scroll: u16,
}

impl ReviewSession {
    pub fn current(&self) -> Option<&WordRow> {
        self.queue.get(self.index)
    }

    pub fn mode(&self) -> ReviewMode {
        self.mode.unwrap_or(ReviewMode::Flash)
    }

    pub fn prompt(&self, row: &WordRow) -> String {
        if self.reverse {
            row.translation.clone()
        } else {
            row.word.clone()
        }
    }

    pub fn answer(&self, row: &WordRow) -> String {
        if self.reverse {
            row.word.clone()
        } else {
            row.translation.clone()
        }
    }
}

#[derive(Clone, Debug)]
enum AiContext {
    Lookup,
    AddTranslation,
}

struct AiMessage {
    id: u64,
    source: String,
    context: AiContext,
    result: Result<(String, bool), String>,
}

pub struct App {
    pub(crate) store: Store,
    pub(crate) rows: Vec<WordRow>,
    pub(crate) tab: Tab,
    pub(crate) mode: Mode,
    pub(crate) theme: Theme,
    pub(crate) lookup_input: String,
    pub(crate) lookup_cursor: usize,
    pub(crate) lookup_history: Vec<String>,
    pub(crate) lookup_state: LookupState,
    pub(crate) lookup_selected: usize,
    pub(crate) lookup_scroll: u16,
    pub(crate) words_filter: String,
    pub(crate) words_sort: SortKey,
    pub(crate) words_selected: usize,
    pub(crate) words_scroll: u16,
    pub(crate) review: Option<ReviewSession>,
    pub(crate) stats: Option<store::Stats>,
    pub(crate) message: String,
    pub(crate) message_error: bool,
    pub(crate) message_at: Option<Instant>,
    pub(crate) overlay: Option<Overlay>,
    pub(crate) overlay_scroll: u16,
    pub(crate) edit_buffer: String,
    pub(crate) add_word: String,
    pub(crate) command: String,
    pub(crate) command_history: Vec<String>,
    pub(crate) command_pos: usize,
    pub(crate) help_scroll: u16,
    pub(crate) pending_ai: Option<(u64, String)>,
    pub(crate) next_request: u64,
    pub(crate) spinner: usize,
    pub(crate) pending_g: bool,
    pub(crate) pending_d: bool,
    pub(crate) should_quit: bool,
    ai_tx: Sender<AiMessage>,
    ai_rx: Receiver<AiMessage>,
    mtime: Option<SystemTime>,
}

impl App {
    pub fn new(store: Store) -> Self {
        let (ai_tx, ai_rx) = mpsc::channel();
        let rows = store.words().unwrap_or_default();
        let stats = store.stats().ok();
        let mtime = mtime_of(&store);
        Self {
            store,
            rows,
            tab: Tab::Lookup,
            mode: Mode::Normal,
            theme: Theme::load(),
            lookup_input: String::new(),
            lookup_cursor: 0,
            lookup_history: Vec::new(),
            lookup_state: LookupState::Empty,
            lookup_selected: 0,
            lookup_scroll: 0,
            words_filter: String::new(),
            words_sort: SortKey::Due,
            words_selected: 0,
            words_scroll: 0,
            review: None,
            stats,
            message: String::new(),
            message_error: false,
            message_at: None,
            overlay: None,
            overlay_scroll: 0,
            edit_buffer: String::new(),
            add_word: String::new(),
            command: String::new(),
            command_history: Vec::new(),
            command_pos: 0,
            help_scroll: 0,
            pending_ai: None,
            next_request: 0,
            spinner: 0,
            pending_g: false,
            pending_d: false,
            should_quit: false,
            ai_tx,
            ai_rx,
            mtime,
        }
    }

    pub fn info_line(&self) -> String {
        let stats = self.stats.as_ref();
        let due = stats.map(|value| value.due).unwrap_or(0);
        let new = stats.map(|value| value.new).unwrap_or(0);
        let total = stats.map(|value| value.total).unwrap_or(0);
        let dict = if dict::dict_available() {
            "dict ok"
        } else {
            "dict missing"
        };
        format!("due {due} new {new} total {total}  {dict}")
    }

    pub fn filtered_rows(&self) -> Vec<WordRow> {
        let filter = self.words_filter.to_lowercase();
        let mut rows: Vec<WordRow> = self
            .rows
            .iter()
            .filter(|row| row.deleted == 0)
            .filter(|row| {
                filter.is_empty()
                    || row.word.to_lowercase().contains(&filter)
                    || row.translation.to_lowercase().contains(&filter)
            })
            .cloned()
            .collect();
        match self.words_sort {
            SortKey::Due => rows.sort_by(|a, b| {
                (a.due.is_empty(), a.due.as_str(), a.word.as_str()).cmp(&(
                    b.due.is_empty(),
                    b.due.as_str(),
                    b.word.as_str(),
                ))
            }),
            SortKey::Hit => {
                rows.sort_by(|a, b| (b.hit, b.last.as_str()).cmp(&(a.hit, a.last.as_str())))
            }
            SortKey::Last => rows.sort_by(|a, b| b.last.cmp(&a.last)),
            SortKey::Word => rows.sort_by(|a, b| a.word.to_lowercase().cmp(&b.word.to_lowercase())),
            SortKey::Box => {
                rows.sort_by(|a, b| (a.box_, a.word.as_str()).cmp(&(b.box_, b.word.as_str())))
            }
        }
        rows
    }

    pub fn selected_word(&self) -> Option<WordRow> {
        let rows = self.filtered_rows();
        rows.get(self.words_selected).cloned()
    }

    pub fn set_message(&mut self, text: impl Into<String>, error: bool) {
        self.message = text.into();
        self.message_error = error;
        self.message_at = Some(Instant::now());
    }

    pub fn mode_badge(&self) -> &'static str {
        match self.mode {
            Mode::Normal => "NORMAL",
            Mode::Insert(_) => "INSERT",
            Mode::Command => "COMMAND",
            Mode::Help => "HELP",
            Mode::Confirm(_) => "CONFIRM",
        }
    }

    pub fn status_hint(&self) -> &'static str {
        match &self.mode {
            Mode::Insert(InsertTarget::Lookup) => "Enter lookup  Esc cancel  Up/Down history",
            Mode::Insert(InsertTarget::WordsFilter { .. }) => {
                "type filter  Enter keep  Esc restore"
            }
            Mode::Insert(InsertTarget::EditTranslation { .. }) => "Enter save  Esc cancel",
            Mode::Insert(InsertTarget::AddWord) => "Enter next  Esc cancel",
            Mode::Insert(InsertTarget::AddTranslation) => "Enter save  Esc cancel",
            Mode::Insert(InsertTarget::ReviewTyping) => "Enter check  Esc normal",
            Mode::Command => "Enter run  Esc cancel  Up/Down history",
            Mode::Confirm(_) => "y delete  any other key cancel",
            Mode::Help => "j/k scroll  any other key close",
            Mode::Normal => match self.tab {
                Tab::Lookup => "i input  Enter detail  s save  :ai  ? help",
                Tab::Words => "j/k move  Enter detail  e edit  dd delete  a add  s sort  r reload",
                Tab::Review => "Space reveal  1 again  2 good  t type  R reverse  n new  Esc close",
                Tab::Stats => "r reload  ? help",
            },
        }
    }

    pub fn switch_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.mode = Mode::Normal;
        self.overlay = None;
        match tab {
            Tab::Words => self.reload_rows(),
            Tab::Review => {
                if self.review.is_none() {
                    self.start_review();
                }
            }
            Tab::Stats => self.reload_stats(),
            Tab::Lookup => {}
        }
    }

    fn reload_rows(&mut self) {
        match self.store.words() {
            Ok(rows) => {
                self.rows = rows;
                self.mtime = mtime_of(&self.store);
                if self.words_selected >= self.filtered_rows().len() {
                    self.words_selected = self.filtered_rows().len().saturating_sub(1);
                }
            }
            Err(error) => self.set_message(format!("{error:#}"), true),
        }
        self.reload_stats();
    }

    fn reload_stats(&mut self) {
        match self.store.stats() {
            Ok(stats) => self.stats = Some(stats),
            Err(error) => self.set_message(format!("{error:#}"), true),
        }
    }

    fn tick(&mut self) {
        self.spinner = self.spinner.wrapping_add(1);
        if !self.message_error {
            if let Some(at) = self.message_at {
                if at.elapsed() > Duration::from_secs(3) {
                    self.message.clear();
                    self.message_at = None;
                }
            }
        }
        let current = mtime_of(&self.store);
        if current != self.mtime {
            self.reload_rows();
        }
    }

    fn poll_ai(&mut self) {
        while let Ok(message) = self.ai_rx.try_recv() {
            if self.pending_ai.as_ref().map(|(id, _)| *id) != Some(message.id) {
                continue;
            }
            self.pending_ai = None;
            match message.result {
                Ok((translation, cached)) => match message.context {
                    AiContext::Lookup => {
                        self.lookup_state = LookupState::Sentence {
                            source: message.source,
                            translation,
                            cached,
                        };
                        self.lookup_scroll = 0;
                    }
                    AiContext::AddTranslation => {
                        if matches!(self.mode, Mode::Insert(InsertTarget::AddTranslation)) {
                            self.edit_buffer = translation;
                        }
                    }
                },
                Err(error) => {
                    self.set_message(error, true);
                    if matches!(self.mode, Mode::Insert(InsertTarget::AddTranslation)) {
                        self.edit_buffer.clear();
                    } else if matches!(self.mode, Mode::Normal | Mode::Insert(InsertTarget::Lookup))
                    {
                        self.lookup_state = LookupState::Error("translate failed".to_string());
                    }
                }
            }
        }
    }

    fn spawn_ai(&mut self, source: String, context: AiContext) {
        let id = self.next_request;
        self.next_request += 1;
        self.pending_ai = Some((id, source.clone()));
        let tx = self.ai_tx.clone();
        std::thread::spawn(move || {
            let result = Translator::new()
                .and_then(|translator| translator.translate(&source))
                .map(|result| (result.translation, result.cached))
                .map_err(|error| format!("{error:#}"));
            let _ = tx.send(AiMessage {
                id,
                source,
                context,
                result,
            });
        });
    }

    fn submit_lookup(&mut self, force_ai: bool) {
        let text = self.lookup_input.trim().to_string();
        if text.is_empty() {
            return;
        }
        if self.lookup_history.last() != Some(&text) {
            self.lookup_history.push(text.clone());
        }
        self.lookup_selected = 0;
        self.lookup_scroll = 0;
        let classified = text::classify(&text);
        if force_ai || matches!(classified, Classified::Sentence) {
            self.lookup_state = LookupState::Loading {
                source: text.clone(),
            };
            self.spawn_ai(text, AiContext::Lookup);
            return;
        }
        match dict::lookup_word(&text, 20) {
            Ok(result) if result.entries.is_empty() => {
                self.lookup_state = LookupState::NotFound;
                if result.dict_missing {
                    self.set_message("dictionary missing: bbt dict fetch && bbt dict build", true);
                }
            }
            Ok(result) => {
                self.lookup_state = LookupState::Entries(result.entries);
            }
            Err(error) => {
                self.lookup_state = LookupState::Error(format!("{error:#}"));
            }
        }
    }

    fn save_lookup(&mut self) {
        match &self.lookup_state {
            LookupState::Entries(entries) => {
                let Some(entry) = entries.get(self.lookup_selected) else {
                    return;
                };
                let translation = dict::entry_translation(entry);
                match self.store.upsert(
                    &entry.head,
                    &translation,
                    Some(&entry.phonetic),
                    Some(&entry.tags),
                ) {
                    Ok(_) => {
                        self.set_message("saved", false);
                        self.reload_rows();
                    }
                    Err(error) => self.set_message(format!("{error:#}"), true),
                }
            }
            LookupState::Sentence {
                source,
                translation,
                ..
            } => match self.store.upsert(source, translation, None, None) {
                Ok(_) => {
                    self.set_message("saved", false);
                    self.reload_rows();
                }
                Err(error) => self.set_message(format!("{error:#}"), true),
            },
            _ => {}
        }
    }

    fn start_review(&mut self) {
        match self.store.queue(20, 10) {
            Ok(queue) => {
                let empty = queue.is_empty();
                self.review = Some(ReviewSession {
                    queue,
                    index: 0,
                    revealed: false,
                    checked: false,
                    last_ok: None,
                    typed: String::new(),
                    mode: Some(ReviewMode::Flash),
                    reverse: false,
                    requeued: Vec::new(),
                    done: 0,
                    good: 0,
                    again: 0,
                    scroll: 0,
                });
                if empty {
                    self.set_message("nothing due", false);
                }
            }
            Err(error) => self.set_message(format!("{error:#}"), true),
        }
    }

    fn grade_review(&mut self, grade: Grade, keep: bool) {
        let Some(session) = self.review.as_mut() else {
            return;
        };
        let Some(row) = session.current().cloned() else {
            return;
        };
        match self.store.review(&row.uuid, grade) {
            Ok(_) => {
                session.done += 1;
                match grade {
                    Grade::Good => session.good += 1,
                    Grade::Again => session.again += 1,
                }
                if grade == Grade::Again && !session.requeued.contains(&row.uuid) {
                    session.requeued.push(row.uuid.clone());
                    session.queue.push(row.clone());
                }
                if keep {
                    session.checked = true;
                    session.revealed = true;
                    session.last_ok = Some(grade == Grade::Good);
                } else {
                    session.index += 1;
                    session.revealed = false;
                    session.checked = false;
                    session.typed.clear();
                    session.last_ok = None;
                    session.scroll = 0;
                }
                self.reload_stats();
            }
            Err(error) => self.set_message(format!("{error:#}"), true),
        }
    }

    fn continue_review(&mut self) {
        let mut type_mode = false;
        if let Some(session) = self.review.as_mut() {
            session.index += 1;
            session.revealed = false;
            session.checked = false;
            session.typed.clear();
            session.last_ok = None;
            session.scroll = 0;
            type_mode = session.mode() == ReviewMode::Type && session.current().is_some();
        }
        if type_mode {
            self.mode = Mode::Insert(InsertTarget::ReviewTyping);
        }
    }

    fn check_typed(&mut self) {
        let Some(session) = self.review.as_ref() else {
            return;
        };
        let Some(row) = session.current().cloned() else {
            return;
        };
        let expected = session.answer(&row);
        let typed = session.typed.clone();
        let ok = text::answer_matches(&typed, &expected);
        self.grade_review(if ok { Grade::Good } else { Grade::Again }, true);
        if let Some(session) = self.review.as_mut() {
            session.last_ok = Some(ok);
        }
        self.mode = Mode::Normal;
    }

    fn save_edit_translation(&mut self, uuid: &str) {
        let value = self.edit_buffer.trim().to_string();
        match self.store.update(
            uuid,
            WordUpdate {
                translation: Some(value),
                ..WordUpdate::default()
            },
        ) {
            Ok(_) => {
                self.set_message("updated", false);
                self.mode = Mode::Normal;
                self.reload_rows();
            }
            Err(error) => self.set_message(format!("{error:#}"), true),
        }
    }

    fn finish_add_word(&mut self) {
        let word = self.edit_buffer.trim().to_string();
        if word.is_empty() {
            return;
        }
        self.add_word = word.clone();
        self.edit_buffer.clear();
        match text::classify(&word) {
            Classified::Word { .. } => match dict::lookup_word(&word, 20) {
                Ok(result) if !result.entries.is_empty() => {
                    self.edit_buffer = dict::entry_translation(&result.entries[0]);
                    self.mode = Mode::Insert(InsertTarget::AddTranslation);
                }
                _ => {
                    self.mode = Mode::Insert(InsertTarget::AddTranslation);
                }
            },
            Classified::Sentence => {
                self.mode = Mode::Insert(InsertTarget::AddTranslation);
                self.spawn_ai(word, AiContext::AddTranslation);
            }
        }
    }

    fn finish_add(&mut self) {
        let translation = self.edit_buffer.trim().to_string();
        match self.store.upsert(&self.add_word, &translation, None, None) {
            Ok(_) => {
                self.set_message("saved", false);
                self.mode = Mode::Normal;
                self.add_word.clear();
                self.edit_buffer.clear();
                self.reload_rows();
            }
            Err(error) => self.set_message(format!("{error:#}"), true),
        }
    }

    fn execute_command(&mut self) {
        let line = self.command.trim().to_string();
        if !line.is_empty() && self.command_history.last() != Some(&line) {
            self.command_history.push(line.clone());
        }
        self.command_pos = self.command_history.len();
        self.command.clear();
        self.mode = Mode::Normal;
        let mut parts = line.splitn(2, char::is_whitespace);
        let command = parts.next().unwrap_or("");
        let argument = parts.next().unwrap_or("").trim();
        match command {
            "" => {}
            "q" | "quit" => self.should_quit = true,
            "help" => {
                self.mode = Mode::Help;
                self.help_scroll = 0;
            }
            "ai" => {
                self.tab = Tab::Lookup;
                self.submit_lookup(true);
            }
            "dict" => match dict::info() {
                Ok(info) => self.set_message(
                    format!("dict {} en {} zh {}", info.total, info.en, info.zh),
                    false,
                ),
                Err(error) => self.set_message(format!("{error:#}"), true),
            },
            "merge" | "import" => {
                if argument.is_empty() {
                    self.set_message("usage: :merge <path>", true);
                    return;
                }
                let path = PathBuf::from(argument);
                let result = if command == "import" {
                    merge::import(&self.store, &path, false, None)
                } else {
                    merge::merge(&self.store, &path, false, None)
                };
                match result {
                    Ok(stats) => {
                        self.set_message(
                            format!(
                                "merge: +{} ~{} ={} !{}",
                                stats.added, stats.updated, stats.unchanged, stats.conflicts
                            ),
                            false,
                        );
                        self.reload_rows();
                    }
                    Err(error) => self.set_message(format!("{error:#}"), true),
                }
            }
            "gc" => {
                let days = argument.parse::<i64>().unwrap_or(90);
                match merge::gc(&self.store, days, false) {
                    Ok(count) => {
                        self.set_message(format!("gc: {count} tombstones, use CLI --force"), false)
                    }
                    Err(error) => self.set_message(format!("{error:#}"), true),
                }
            }
            "sort" => match SortKey::parse(argument) {
                Some(sort) => {
                    self.words_sort = sort;
                    self.tab = Tab::Words;
                    self.set_message(format!("sort: {}", sort.label()), false);
                }
                None => self.set_message("usage: :sort due|hit|last|word|box", true),
            },
            "tab" => match Tab::parse(argument) {
                Some(tab) => self.switch_tab(tab),
                None => self.set_message("usage: :tab lookup|words|review|stats", true),
            },
            "version" => self.set_message(format!("bbt {}", env!("CARGO_PKG_VERSION")), false),
            other => self.set_message(format!("unknown command: {other}"), true),
        }
    }

    fn handle_help(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.help_scroll = self.help_scroll.saturating_add(1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.help_scroll = self.help_scroll.saturating_sub(1)
            }
            _ => self.mode = Mode::Normal,
        }
    }

    fn handle_confirm(&mut self, key: KeyEvent) {
        let action = match &self.mode {
            Mode::Confirm(action) => action.clone(),
            _ => return,
        };
        self.mode = Mode::Normal;
        if key.code != KeyCode::Char('y') {
            return;
        }
        match action {
            ConfirmAction::DeleteWord { uuid, word } => match self.store.delete(&uuid) {
                Ok(_) => {
                    self.set_message(format!("deleted {word}"), false);
                    self.reload_rows();
                }
                Err(error) => self.set_message(format!("{error:#}"), true),
            },
        }
    }

    fn handle_command(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Enter => self.execute_command(),
            KeyCode::Backspace => {
                self.command.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.command.clear()
            }
            KeyCode::Up => {
                if !self.command_history.is_empty() {
                    self.command_pos = self.command_pos.saturating_sub(1);
                    self.command = self
                        .command_history
                        .get(self.command_pos)
                        .cloned()
                        .unwrap_or_default();
                }
            }
            KeyCode::Down => {
                if self.command_pos < self.command_history.len() {
                    self.command_pos += 1;
                    self.command = self
                        .command_history
                        .get(self.command_pos)
                        .cloned()
                        .unwrap_or_default();
                }
            }
            KeyCode::Char(ch) => self.command.push(ch),
            _ => {}
        }
    }

    fn lookup_chars(&self) -> Vec<char> {
        self.lookup_input.chars().collect()
    }

    fn lookup_end(&mut self) {
        self.lookup_cursor = self.lookup_input.chars().count();
    }

    fn lookup_insert_char(&mut self, ch: char) {
        let mut chars = self.lookup_chars();
        let cursor = self.lookup_cursor.min(chars.len());
        chars.insert(cursor, ch);
        self.lookup_input = chars.into_iter().collect();
        self.lookup_cursor = cursor + 1;
    }

    fn lookup_backspace(&mut self) {
        let mut chars = self.lookup_chars();
        if self.lookup_cursor > 0 && self.lookup_cursor <= chars.len() {
            chars.remove(self.lookup_cursor - 1);
            self.lookup_cursor -= 1;
            self.lookup_input = chars.into_iter().collect();
        }
    }

    fn lookup_delete(&mut self) {
        let mut chars = self.lookup_chars();
        if self.lookup_cursor < chars.len() {
            chars.remove(self.lookup_cursor);
            self.lookup_input = chars.into_iter().collect();
        }
    }

    fn lookup_left(&mut self) {
        if self.lookup_cursor > 0 {
            self.lookup_cursor -= 1;
        }
    }

    fn lookup_right(&mut self) {
        if self.lookup_cursor < self.lookup_input.chars().count() {
            self.lookup_cursor += 1;
        }
    }

    fn lookup_home(&mut self) {
        self.lookup_cursor = 0;
    }

    fn lookup_word_left(&mut self) {
        let chars = self.lookup_chars();
        let mut index = self.lookup_cursor.min(chars.len());
        while index > 0 && is_word_sep(chars[index - 1]) {
            index -= 1;
        }
        while index > 0 && !is_word_sep(chars[index - 1]) {
            index -= 1;
        }
        self.lookup_cursor = index;
    }

    fn lookup_word_right(&mut self) {
        let chars = self.lookup_chars();
        let mut index = self.lookup_cursor.min(chars.len());
        while index < chars.len() && is_word_sep(chars[index]) {
            index += 1;
        }
        while index < chars.len() && !is_word_sep(chars[index]) {
            index += 1;
        }
        self.lookup_cursor = index;
    }

    fn lookup_delete_word_back(&mut self) {
        let before = self.lookup_cursor;
        self.lookup_word_left();
        let start = self.lookup_cursor;
        if start >= before {
            return;
        }
        let mut chars = self.lookup_chars();
        chars.drain(start..before);
        self.lookup_input = chars.into_iter().collect();
        self.lookup_cursor = start;
    }

    pub(crate) fn lookup_display(&self, width: usize) -> (String, usize) {
        let chars = self.lookup_chars();
        let cursor = self.lookup_cursor.min(chars.len());
        let prefix: String = chars[..cursor].iter().collect();
        if display_width(&self.lookup_input) <= width {
            return (self.lookup_input.clone(), display_width(&prefix));
        }
        let reserve = width.saturating_sub(1);
        let mut start = cursor;
        let mut used = 0usize;
        while start > 0 {
            let char_width = display_width(&chars[start - 1].to_string());
            if used + char_width > reserve {
                break;
            }
            start -= 1;
            used += char_width;
        }
        let mut end = cursor;
        let mut after = 0usize;
        while end < chars.len() {
            let char_width = display_width(&chars[end].to_string());
            if used + after + char_width > reserve {
                break;
            }
            end += 1;
            after += char_width;
        }
        let visible: String = chars[start..end].iter().collect();
        let prefix: String = chars[start..cursor].iter().collect();
        (visible, display_width(&prefix))
    }

    fn handle_insert(&mut self, key: KeyEvent) {
        let target = match &self.mode {
            Mode::Insert(target) => target.clone(),
            _ => return,
        };
        if matches!(target, InsertTarget::Lookup) {
            let alt = key.modifiers.contains(KeyModifiers::ALT);
            let ctrl_w =
                key.code == KeyCode::Char('w') && key.modifiers.contains(KeyModifiers::CONTROL);
            if alt {
                match key.code {
                    KeyCode::Left => {
                        self.lookup_word_left();
                        return;
                    }
                    KeyCode::Right => {
                        self.lookup_word_right();
                        return;
                    }
                    KeyCode::Backspace => {
                        self.lookup_delete_word_back();
                        return;
                    }
                    _ => {}
                }
            }
            if ctrl_w {
                self.lookup_delete_word_back();
                return;
            }
            match key.code {
                KeyCode::Left => {
                    self.lookup_left();
                    return;
                }
                KeyCode::Right => {
                    self.lookup_right();
                    return;
                }
                KeyCode::Home => {
                    self.lookup_home();
                    return;
                }
                KeyCode::End => {
                    self.lookup_end();
                    return;
                }
                KeyCode::Delete => {
                    self.lookup_delete();
                    return;
                }
                _ => {}
            }
        }
        if key.code == KeyCode::Esc {
            if let InsertTarget::WordsFilter { original } = &target {
                self.words_filter = original.clone();
                self.words_selected = 0;
            }
            self.mode = Mode::Normal;
            return;
        }
        match &target {
            InsertTarget::WordsFilter { .. } => {
                if key.code == KeyCode::Enter {
                    self.mode = Mode::Normal;
                    return;
                }
                if key.code == KeyCode::Backspace {
                    self.edit_buffer.pop();
                } else if key.code == KeyCode::Char('u')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    self.edit_buffer.clear();
                } else if let Some(ch) = keymap::char_of(key) {
                    self.edit_buffer.push(ch);
                }
                self.words_filter = self.edit_buffer.clone();
                self.words_selected = 0;
                return;
            }
            InsertTarget::ReviewTyping => {
                let Some(session) = self.review.as_mut() else {
                    self.mode = Mode::Normal;
                    return;
                };
                match key.code {
                    KeyCode::Enter => {
                        self.check_typed();
                    }
                    KeyCode::Backspace => {
                        session.typed.pop();
                    }
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        session.typed.clear();
                    }
                    KeyCode::Char(ch) => session.typed.push(ch),
                    _ => {}
                }
                return;
            }
            _ => {}
        }
        match key.code {
            KeyCode::Enter => match target {
                InsertTarget::Lookup => {
                    self.submit_lookup(false);
                    self.mode = Mode::Normal;
                }
                InsertTarget::EditTranslation { uuid } => {
                    self.save_edit_translation(&uuid);
                }
                InsertTarget::AddWord => {
                    self.finish_add_word();
                }
                InsertTarget::AddTranslation => {
                    self.finish_add();
                }
                _ => {}
            },
            KeyCode::Backspace => {
                if matches!(target, InsertTarget::Lookup) {
                    self.lookup_backspace();
                } else {
                    self.edit_buffer.pop();
                }
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if matches!(target, InsertTarget::Lookup) {
                    self.lookup_input.clear();
                    self.lookup_cursor = 0;
                } else {
                    self.edit_buffer.clear();
                }
            }
            KeyCode::Up if matches!(target, InsertTarget::Lookup) => {
                if !self.lookup_history.is_empty() {
                    let current = self
                        .lookup_history
                        .iter()
                        .position(|value| *value == self.lookup_input);
                    let index = current
                        .map(|value| value.saturating_sub(1))
                        .unwrap_or(self.lookup_history.len() - 1);
                    self.lookup_input = self.lookup_history[index].clone();
                    self.lookup_end();
                }
            }
            KeyCode::Down if matches!(target, InsertTarget::Lookup) => {
                if !self.lookup_history.is_empty() {
                    if let Some(current) = self
                        .lookup_history
                        .iter()
                        .position(|value| *value == self.lookup_input)
                    {
                        if current + 1 < self.lookup_history.len() {
                            self.lookup_input = self.lookup_history[current + 1].clone();
                            self.lookup_end();
                        }
                    }
                }
            }
            KeyCode::Char(ch) => {
                if matches!(target, InsertTarget::Lookup) {
                    self.lookup_insert_char(ch);
                } else {
                    self.edit_buffer.push(ch);
                }
            }
            _ => {}
        }
    }

    fn handle_normal(&mut self, key: KeyEvent) {
        if self.overlay.is_some() {
            if key.code != KeyCode::Char('d') {
                self.pending_d = false;
            }
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.overlay = None;
                    self.overlay_scroll = 0;
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    self.overlay_scroll = self.overlay_scroll.saturating_add(1)
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.overlay_scroll = self.overlay_scroll.saturating_sub(1)
                }
                KeyCode::Char('e') => {
                    if let Some(Overlay::Word(row)) = self.overlay.clone() {
                        self.overlay = None;
                        self.edit_buffer = row.translation;
                        self.mode = Mode::Insert(InsertTarget::EditTranslation { uuid: row.uuid });
                    }
                }
                KeyCode::Char('s') => {
                    if matches!(self.overlay, Some(Overlay::Dict(_))) {
                        self.overlay = None;
                        self.save_lookup();
                    }
                }
                KeyCode::Char('d') => {
                    if let Some(Overlay::Word(row)) = self.overlay.clone() {
                        if self.pending_d {
                            self.overlay = None;
                            self.pending_d = false;
                            self.mode = Mode::Confirm(ConfirmAction::DeleteWord {
                                uuid: row.uuid,
                                word: row.word,
                            });
                        } else {
                            self.pending_d = true;
                            self.set_message("press d again to delete", false);
                        }
                    }
                }
                _ => {}
            }
            return;
        }
        if key.code == KeyCode::Char('?') {
            self.mode = Mode::Help;
            self.help_scroll = 0;
            return;
        }
        if key.code == KeyCode::Char(':') {
            self.mode = Mode::Command;
            self.command.clear();
            self.command_pos = self.command_history.len();
            return;
        }
        if key.code == KeyCode::Char('q') {
            self.should_quit = true;
            return;
        }
        if key.code == KeyCode::Tab {
            let index = (self.tab.index() + 1) % 4;
            self.switch_tab(Tab::all()[index]);
            return;
        }
        if key.code == KeyCode::Char('H') {
            let index = (self.tab.index() + 3) % 4;
            self.switch_tab(Tab::all()[index]);
            return;
        }
        if key.code == KeyCode::Char('L') {
            let index = (self.tab.index() + 1) % 4;
            self.switch_tab(Tab::all()[index]);
            return;
        }
        let digit = match key.code {
            KeyCode::Char(ch) => keymap::tab_by_number(ch),
            _ => None,
        };
        if let Some(index) = digit {
            if self.tab == Tab::Review {
                let flash_revealed = self
                    .review
                    .as_ref()
                    .map(|session| session.mode() == ReviewMode::Flash && session.revealed)
                    .unwrap_or(false);
                if flash_revealed && index == 0 {
                    self.grade_review(Grade::Again, false);
                } else if flash_revealed && index == 1 {
                    self.grade_review(Grade::Good, false);
                }
                return;
            }
            self.switch_tab(Tab::all()[index]);
            return;
        }
        if key.code != KeyCode::Char('d') {
            self.pending_d = false;
        }
        if key.code != KeyCode::Char('g') {
            self.pending_g = false;
        }
        match self.tab {
            Tab::Lookup => self.handle_lookup_normal(key),
            Tab::Words => self.handle_words_normal(key),
            Tab::Review => self.handle_review_normal(key),
            Tab::Stats => {
                if key.code == KeyCode::Char('r') {
                    self.reload_stats();
                    self.set_message("reloaded", false);
                }
            }
        }
    }

    fn handle_lookup_normal(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('i') | KeyCode::Char('a') | KeyCode::Char('/') => {
                if key.code == KeyCode::Char('/') {
                    self.lookup_input.clear();
                    self.lookup_cursor = 0;
                } else {
                    self.lookup_end();
                }
                self.mode = Mode::Insert(InsertTarget::Lookup);
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if let LookupState::Entries(entries) = &self.lookup_state {
                    self.lookup_selected = self
                        .lookup_selected
                        .saturating_add(1)
                        .min(entries.len().saturating_sub(1));
                } else {
                    self.lookup_scroll = self.lookup_scroll.saturating_add(1);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if matches!(self.lookup_state, LookupState::Entries(_)) {
                    self.lookup_selected = self.lookup_selected.saturating_sub(1);
                } else {
                    self.lookup_scroll = self.lookup_scroll.saturating_sub(1);
                }
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.lookup_scroll = self.lookup_scroll.saturating_add(5);
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.lookup_scroll = self.lookup_scroll.saturating_sub(5);
            }
            KeyCode::Enter => {
                if let LookupState::Entries(entries) = &self.lookup_state {
                    if let Some(entry) = entries.get(self.lookup_selected) {
                        self.overlay = Some(Overlay::Dict(entry.clone()));
                        self.overlay_scroll = 0;
                    }
                }
            }
            KeyCode::Char('s') => self.save_lookup(),
            KeyCode::Esc => {
                self.lookup_state = LookupState::Empty;
                self.lookup_scroll = 0;
                self.lookup_selected = 0;
            }
            _ => {}
        }
    }

    fn handle_words_normal(&mut self, key: KeyEvent) {
        let len = self.filtered_rows().len();
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.words_selected = self
                    .words_selected
                    .saturating_add(1)
                    .min(len.saturating_sub(1));
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.words_selected = self.words_selected.saturating_sub(1);
            }
            KeyCode::Char('g') => {
                if self.pending_g {
                    self.words_selected = 0;
                    self.pending_g = false;
                } else {
                    self.pending_g = true;
                    self.set_message("g again for top", false);
                }
            }
            KeyCode::Char('G') => {
                self.words_selected = len.saturating_sub(1);
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.words_selected = self
                    .words_selected
                    .saturating_add(10)
                    .min(len.saturating_sub(1));
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.words_selected = self.words_selected.saturating_sub(10);
            }
            KeyCode::Char('/') => {
                let original = self.words_filter.clone();
                self.edit_buffer = self.words_filter.clone();
                self.mode = Mode::Insert(InsertTarget::WordsFilter { original });
            }
            KeyCode::Char('s') => {
                self.words_sort = self.words_sort.next();
                self.set_message(format!("sort: {}", self.words_sort.label()), false);
            }
            KeyCode::Char('r') => {
                self.reload_rows();
                self.set_message("reloaded", false);
            }
            KeyCode::Char('n') => {
                self.words_selected = self
                    .words_selected
                    .saturating_add(1)
                    .min(len.saturating_sub(1));
            }
            KeyCode::Char('N') => {
                self.words_selected = self.words_selected.saturating_sub(1);
            }
            KeyCode::Enter => {
                if let Some(row) = self.selected_word() {
                    self.overlay = Some(Overlay::Word(row));
                    self.overlay_scroll = 0;
                }
            }
            KeyCode::Char('e') => {
                if let Some(row) = self.selected_word() {
                    self.edit_buffer = row.translation.clone();
                    self.mode = Mode::Insert(InsertTarget::EditTranslation { uuid: row.uuid });
                }
            }
            KeyCode::Char('a') => {
                self.add_word.clear();
                self.edit_buffer.clear();
                self.mode = Mode::Insert(InsertTarget::AddWord);
            }
            KeyCode::Char('d') => {
                if self.pending_d {
                    if let Some(row) = self.selected_word() {
                        self.mode = Mode::Confirm(ConfirmAction::DeleteWord {
                            uuid: row.uuid,
                            word: row.word,
                        });
                    }
                    self.pending_d = false;
                } else {
                    self.pending_d = true;
                    self.set_message("press d again to delete", false);
                }
            }
            _ => {}
        }
    }

    fn handle_review_normal(&mut self, key: KeyEvent) {
        if self.review.is_none() {
            if key.code == KeyCode::Char('n') {
                self.start_review();
            }
            return;
        }
        let (empty, mode, checked, revealed, reverse) = {
            let session = self.review.as_ref().unwrap();
            (
                session.current().is_none(),
                session.mode(),
                session.checked,
                session.revealed,
                session.reverse,
            )
        };
        if empty {
            match key.code {
                KeyCode::Char('n') => self.start_review(),
                KeyCode::Esc => self.review = None,
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Esc => {
                self.review = None;
                self.set_message("session closed", false);
            }
            KeyCode::Char('n') => self.start_review(),
            KeyCode::Char(' ') => {
                if mode == ReviewMode::Flash && !revealed {
                    if let Some(session) = self.review.as_mut() {
                        session.revealed = true;
                    }
                } else if mode == ReviewMode::Type && checked {
                    self.continue_review();
                } else if mode == ReviewMode::Type && !checked {
                    self.mode = Mode::Insert(InsertTarget::ReviewTyping);
                }
            }
            KeyCode::Enter if mode == ReviewMode::Type && checked => self.continue_review(),
            KeyCode::Char('i') if mode == ReviewMode::Type && !checked => {
                self.mode = Mode::Insert(InsertTarget::ReviewTyping);
            }
            KeyCode::Char('1') if mode == ReviewMode::Flash && revealed => {
                self.grade_review(Grade::Again, false)
            }
            KeyCode::Char('2') if mode == ReviewMode::Flash && revealed => {
                self.grade_review(Grade::Good, false)
            }
            KeyCode::Char('t') => {
                if let Some(session) = self.review.as_mut() {
                    session.mode = Some(match mode {
                        ReviewMode::Flash => ReviewMode::Type,
                        ReviewMode::Type => ReviewMode::Flash,
                    });
                    session.revealed = false;
                    session.checked = false;
                    session.typed.clear();
                    session.last_ok = None;
                }
                if mode == ReviewMode::Flash {
                    self.mode = Mode::Insert(InsertTarget::ReviewTyping);
                }
            }
            KeyCode::Char('R') => {
                if let Some(session) = self.review.as_mut() {
                    session.reverse = !reverse;
                }
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if let Some(session) = self.review.as_mut() {
                    session.scroll = session.scroll.saturating_add(1);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Some(session) = self.review.as_mut() {
                    session.scroll = session.scroll.saturating_sub(1);
                }
            }
            _ => {}
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        if keymap::is_ctrl_c(key) {
            self.should_quit = true;
            return;
        }
        if self.message_error {
            self.message.clear();
            self.message_error = false;
        }
        match self.mode.clone() {
            Mode::Help => self.handle_help(key),
            Mode::Confirm(_) => self.handle_confirm(key),
            Mode::Command => self.handle_command(key),
            Mode::Insert(_) => self.handle_insert(key),
            Mode::Normal => self.handle_normal(key),
        }
    }

    fn event_loop(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
        loop {
            terminal.draw(|frame| render::draw(frame, self))?;
            if event::poll(Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    self.handle_key(key);
                }
            }
            self.tick();
            self.poll_ai();
            if self.should_quit {
                return Ok(());
            }
        }
    }
}

fn is_word_sep(ch: char) -> bool {
    ch.is_whitespace() || ch.is_ascii_punctuation()
}

fn mtime_of(store: &Store) -> Option<SystemTime> {
    std::fs::metadata(&store.path)
        .and_then(|meta| meta.modified())
        .ok()
}

pub fn run(store: Store) -> Result<()> {
    enable_raw_mode().context("enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("create terminal")?;
    let mut app = App::new(store);
    let result = app.event_loop(&mut terminal);
    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();
    result
}
