use ratatui::style::{Color, Modifier, Style};

#[derive(Clone, Debug)]
pub struct Theme {
    pub title: Style,
    pub translation: Style,
    pub warn: Style,
    pub error: Style,
    pub dim: Style,
    pub selected: Style,
    pub border: Style,
    pub input_border: Style,
    pub normal: Style,
    pub insert: Style,
    pub command: Style,
    pub confirm: Style,
}

impl Theme {
    pub fn load() -> Self {
        let mono = std::env::var_os("NO_COLOR").is_some()
            || std::env::var("BBT_THEME")
                .map(|value| value == "mono")
                .unwrap_or(false);
        if mono {
            return Self {
                title: Style::default().add_modifier(Modifier::BOLD),
                translation: Style::default().add_modifier(Modifier::BOLD),
                warn: Style::default().add_modifier(Modifier::BOLD),
                error: Style::default().add_modifier(Modifier::BOLD),
                dim: Style::default(),
                selected: Style::default().add_modifier(Modifier::REVERSED),
                border: Style::default(),
                input_border: Style::default().add_modifier(Modifier::BOLD),
                normal: Style::default(),
                insert: Style::default().add_modifier(Modifier::BOLD),
                command: Style::default().add_modifier(Modifier::BOLD),
                confirm: Style::default().add_modifier(Modifier::BOLD),
            };
        }
        Self {
            title: Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
            translation: Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
            warn: Style::default().fg(Color::Yellow),
            error: Style::default().fg(Color::Red),
            dim: Style::default().fg(Color::DarkGray),
            selected: Style::default().add_modifier(Modifier::REVERSED),
            border: Style::default().fg(Color::DarkGray),
            input_border: Style::default().fg(Color::Cyan),
            normal: Style::default().bg(Color::Cyan).fg(Color::Black),
            insert: Style::default().bg(Color::Green).fg(Color::Black),
            command: Style::default().bg(Color::Yellow).fg(Color::Black),
            confirm: Style::default().bg(Color::Red).fg(Color::Black),
        }
    }

    pub fn mode_style(&self, mode: &super::Mode) -> Style {
        match mode {
            super::Mode::Normal => self.normal,
            super::Mode::Insert(_) => self.insert,
            super::Mode::Command => self.command,
            super::Mode::Help => self.dim,
            super::Mode::Confirm(_) => self.confirm,
        }
    }
}
