use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub fn is_ctrl_c(key: KeyEvent) -> bool {
    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
}

pub fn char_of(key: KeyEvent) -> Option<char> {
    if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) {
        return None;
    }
    match key.code {
        KeyCode::Char(ch) => Some(ch),
        _ => None,
    }
}

pub fn tab_by_number(ch: char) -> Option<usize> {
    match ch {
        '1' => Some(0),
        '2' => Some(1),
        '3' => Some(2),
        '4' => Some(3),
        _ => None,
    }
}
