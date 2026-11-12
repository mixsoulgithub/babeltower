use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, Gauge, List, ListItem, ListState, Paragraph, Row, Table, Wrap,
};

use crate::dict::{self, Entry};
use crate::store::{self, WordRow};
use crate::text::{display_width, truncate_width};
use crate::tui::{App, InsertTarget, LookupState, Mode, Overlay, ReviewMode, Tab};

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    if area.width < 60 || area.height < 16 {
        let paragraph = Paragraph::new("terminal too small")
            .alignment(Alignment::Center)
            .style(app.theme.error);
        frame.render_widget(paragraph, area);
        return;
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);
    draw_header(frame, app, chunks[0]);
    let body = chunks[1];
    frame.render_widget(Clear, body);
    let (nav_area, main_area) = if area.width >= 80 {
        let width = if area.width >= 100 { 20 } else { 5 };
        let split = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(width), Constraint::Min(0)])
            .split(body);
        (Some(split[0]), split[1])
    } else {
        (None, body)
    };
    if let Some(nav_area) = nav_area {
        draw_nav(frame, app, nav_area);
    }
    match app.tab {
        Tab::Lookup => draw_lookup(frame, app, main_area),
        Tab::Words => draw_words(frame, app, main_area),
        Tab::Review => draw_review(frame, app, main_area),
        Tab::Stats => draw_stats(frame, app, main_area),
    }
    draw_status(frame, app, chunks[2]);
    if matches!(
        app.mode,
        Mode::Insert(InsertTarget::EditTranslation { .. })
            | Mode::Insert(InsertTarget::AddWord)
            | Mode::Insert(InsertTarget::AddTranslation)
    ) {
        draw_edit_popup(frame, app, area);
    }
    if let Some(overlay) = &app.overlay {
        draw_overlay(frame, app, overlay, area);
    }
    if matches!(app.mode, Mode::Help) {
        draw_help(frame, app, area);
    }
    if let Mode::Confirm(action) = &app.mode {
        draw_confirm(frame, app, action);
    }
    set_cursor(frame, app, main_area, area);
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(38)])
        .split(area);
    let mut spans = vec![Span::styled("aitr ", app.theme.title)];
    for tab in Tab::all() {
        let style = if app.tab == tab {
            app.theme.selected.add_modifier(Modifier::BOLD)
        } else {
            app.theme.dim
        };
        let label = if area.width < 100 {
            tab.label()[..1].to_string()
        } else {
            tab.label().to_string()
        };
        spans.push(Span::styled(
            format!(" {} {} ", tab.index() + 1, label),
            style,
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), split[0]);
    frame.render_widget(
        Paragraph::new(app.info_line())
            .alignment(Alignment::Right)
            .style(app.theme.dim),
        split[1],
    );
}

fn draw_nav(frame: &mut Frame, app: &App, area: Rect) {
    let stats = app.stats.as_ref();
    let mut items = Vec::new();
    for tab in Tab::all() {
        let label = if area.width <= 6 {
            tab.label()[..1].to_string()
        } else {
            tab.label().to_string()
        };
        let extra = match tab {
            Tab::Words => format!(" {}", stats.map(|value| value.total).unwrap_or(0)),
            Tab::Stats => format!(" {}", stats.map(|value| value.due).unwrap_or(0)),
            _ => String::new(),
        };
        items.push(ListItem::new(format!("{label}{extra}")));
    }
    let selected = app.tab.index();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::RIGHT)
                .border_style(app.theme.border),
        )
        .highlight_style(app.theme.selected.add_modifier(Modifier::BOLD));
    let mut state = ListState::default().with_selected(Some(selected));
    frame.render_stateful_widget(list, area, &mut state);
    if area.width > 6 {
        let bottom = Rect {
            x: area.x,
            y: area.y + area.height.saturating_sub(2),
            width: area.width,
            height: 2,
        };
        let dict = if dict::dict_available() {
            "dict ok"
        } else {
            "dict missing"
        };
        frame.render_widget(Paragraph::new(dict).style(app.theme.dim), bottom);
    }
}

fn draw_lookup(frame: &mut Frame, app: &App, area: Rect) {
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);
    let input_style = if matches!(app.mode, Mode::Insert(InsertTarget::Lookup)) {
        app.theme.input_border
    } else {
        app.theme.border
    };
    let available = split[0].width.saturating_sub(4) as usize;
    let (visible, _) = app.lookup_display(available);
    let input = Paragraph::new(Line::from(vec![
        Span::styled("> ", app.theme.title),
        Span::raw(visible),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(input_style)
            .title("Lookup"),
    );
    frame.render_widget(input, split[0]);

    match &app.lookup_state {
        LookupState::Empty => draw_empty(
            frame,
            split[1],
            "offline dictionary + AI sentences",
            "i input   / clear   :ai force AI   ? help",
            app,
        ),
        LookupState::Loading { source } => {
            let spinner = ["|", "/", "-", "\\"][app.spinner % 4];
            let paragraph = Paragraph::new(format!("{spinner} translating...\n\n{source}"))
                .alignment(Alignment::Center)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(app.theme.border),
                )
                .style(app.theme.dim);
            frame.render_widget(paragraph, split[1]);
        }
        LookupState::NotFound => draw_empty(
            frame,
            split[1],
            "not found in offline dict",
            ":ai force AI   i edit",
            app,
        ),
        LookupState::Error(message) => {
            let paragraph = Paragraph::new(message.as_str())
                .alignment(Alignment::Center)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(app.theme.error),
                )
                .style(app.theme.error)
                .wrap(Wrap { trim: false });
            frame.render_widget(paragraph, split[1]);
        }
        LookupState::Sentence {
            source,
            translation,
            cached,
        } => {
            let split = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),
                    Constraint::Percentage(40),
                    Constraint::Min(0),
                ])
                .split(split[1]);
            let tag = if *cached { "AI cached" } else { "AI" };
            frame.render_widget(
                Paragraph::new(format!("{tag}   source -> translation")).style(app.theme.dim),
                split[0],
            );
            let source_block = Paragraph::new(source.as_str())
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(app.theme.border)
                        .title("SOURCE"),
                )
                .wrap(Wrap { trim: false })
                .scroll((app.lookup_scroll, 0));
            frame.render_widget(source_block, split[1]);
            let translation_block = Paragraph::new(translation.as_str())
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(app.theme.border)
                        .title("TRANSLATION"),
                )
                .style(app.theme.translation)
                .wrap(Wrap { trim: false })
                .scroll((app.lookup_scroll, 0));
            frame.render_widget(translation_block, split[2]);
        }
        LookupState::Entries(entries) => {
            let items: Vec<ListItem> = entries
                .iter()
                .enumerate()
                .map(|(index, entry)| ListItem::new(entry_text(entry, index, entries.len(), app)))
                .collect();
            let list = List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(app.theme.border),
                )
                .highlight_style(app.theme.selected)
                .style(app.theme.dim);
            let selected = app.lookup_selected.min(entries.len().saturating_sub(1));
            let mut state = ListState::default().with_selected(Some(selected));
            frame.render_stateful_widget(list, split[1], &mut state);
        }
    }
}

fn entry_text(entry: &Entry, index: usize, total: usize, app: &App) -> Text<'static> {
    let mut lines = Vec::new();
    let mut header = vec![Span::styled(entry.head.clone(), app.theme.title)];
    if !entry.phonetic.is_empty() {
        header.push(Span::raw("  "));
        header.push(Span::styled(entry.phonetic.clone(), app.theme.dim));
    }
    if !entry.pinyin.is_empty() {
        header.push(Span::raw("  "));
        header.push(Span::styled(entry.pinyin.clone(), app.theme.dim));
    }
    if !entry.tags.is_empty() {
        header.push(Span::raw("  "));
        header.push(Span::styled(format!("[{}]", entry.tags), app.theme.dim));
    }
    lines.push(Line::from(header));
    let senses = dict::split_senses(&entry.translation);
    let mut body_lines = 0;
    for sense in &senses {
        let mut spans = Vec::new();
        if let Some(pos) = &sense.pos {
            spans.push(Span::styled(format!("{pos} "), app.theme.dim));
        }
        spans.push(Span::raw(sense.body.clone()));
        lines.push(Line::from(spans));
        body_lines += 1;
        if body_lines >= 10 {
            if senses.len() > body_lines {
                lines.push(Line::from(Span::styled(
                    format!("Enter detail ({} more)", senses.len() - body_lines),
                    app.theme.dim,
                )));
            }
            break;
        }
    }
    if senses.is_empty() && !entry.translation.is_empty() {
        lines.push(Line::from(entry.translation.clone()));
    }
    let mut footer = Vec::new();
    footer.push(Span::styled(
        format!(
            "bnc {}  frq {}  {}/{}",
            entry.bnc,
            entry.frq,
            index + 1,
            total
        ),
        app.theme.dim,
    ));
    lines.push(Line::from(footer));
    lines.push(Line::from(""));
    Text::from(lines)
}

fn draw_words(frame: &mut Frame, app: &App, area: Rect) {
    let rows = app.filtered_rows();
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(0)])
        .split(area);
    let filter = if matches!(app.mode, Mode::Insert(InsertTarget::WordsFilter { .. })) {
        format!("/{}", app.edit_buffer)
    } else if app.words_filter.is_empty() {
        String::new()
    } else {
        format!("/{}", app.words_filter)
    };
    let mut header_lines = vec![Line::from(vec![
        Span::styled(format!("Words {} entries", rows.len()), app.theme.title),
        Span::raw("   "),
        Span::styled(format!("filter:{filter}"), app.theme.dim),
        Span::raw("   "),
        Span::styled(format!("sort:{}", app.words_sort.label()), app.theme.dim),
    ])];
    if matches!(app.mode, Mode::Insert(InsertTarget::WordsFilter { .. })) {
        header_lines.push(Line::from(vec![
            Span::styled("> ", app.theme.title),
            Span::raw(app.edit_buffer.clone()),
        ]));
    }
    frame.render_widget(Paragraph::new(header_lines), split[0]);

    if rows.is_empty() {
        draw_empty(
            frame,
            split[1],
            "wordbook is empty",
            "a add   r reload   ? help",
            app,
        );
        return;
    }
    let selected = app.words_selected.min(rows.len() - 1);
    if area.width >= 100 {
        let panes = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Min(0)])
            .split(split[1]);
        draw_word_list(frame, app, &rows, selected, panes[0]);
        draw_word_preview(frame, app, &rows[selected], panes[1]);
    } else {
        draw_word_list(frame, app, &rows, selected, split[1]);
    }
}

fn draw_word_list(frame: &mut Frame, app: &App, rows: &[WordRow], selected: usize, area: Rect) {
    let today = store::today();
    let items: Vec<ListItem> = rows
        .iter()
        .map(|row| {
            let due = if row.due.is_empty() {
                "new".to_string()
            } else if row.due == today {
                "today".to_string()
            } else {
                row.due.get(5..).unwrap_or(&row.due).to_string()
            };
            let due_style = if row.due.is_empty() {
                app.theme.warn
            } else if row.due == today {
                app.theme.translation
            } else if row.due < today {
                app.theme.error
            } else {
                app.theme.dim
            };
            let translation = truncate_width(&row.translation.replace('\n', " "), 40);
            let line = Line::from(vec![
                Span::styled(format!("{:>3} ", row.box_), app.theme.dim),
                Span::styled(format!("{due:<10} "), due_style),
                Span::styled(format!("{:>4} ", row.hit), app.theme.dim),
                Span::styled(row.word.clone(), app.theme.title),
                Span::raw("  "),
                Span::styled(translation, app.theme.dim),
            ]);
            ListItem::new(line)
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(app.theme.border),
        )
        .highlight_style(app.theme.selected)
        .highlight_symbol("> ");
    let mut state = ListState::default().with_selected(Some(selected));
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_word_preview(frame: &mut Frame, app: &App, row: &WordRow, area: Rect) {
    let mut lines = vec![
        Line::from(Span::styled(row.word.clone(), app.theme.title)),
        Line::from(Span::styled(
            row.translation.replace("\\n", "\n"),
            app.theme.translation,
        )),
    ];
    if !row.phonetic.is_empty() {
        lines.push(Line::from(Span::styled(
            row.phonetic.clone(),
            app.theme.dim,
        )));
    }
    if !row.tags.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("[{}]", row.tags),
            app.theme.dim,
        )));
    }
    lines.push(Line::from(Span::styled(
        format!(
            "box {}   due {}   hit {}",
            row.box_,
            if row.due.is_empty() { "new" } else { &row.due },
            row.hit
        ),
        app.theme.dim,
    )));
    lines.push(Line::from(Span::styled(
        format!(
            "reviews {}   last {}   reviewed {}",
            row.reviews,
            if row.last.is_empty() { "-" } else { &row.last },
            if row.reviewed.is_empty() {
                "-"
            } else {
                &row.reviewed
            },
        ),
        app.theme.dim,
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "e edit   dd delete   Enter detail",
        app.theme.dim,
    )));
    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(app.theme.border)
                .title("Preview"),
        )
        .wrap(Wrap { trim: false })
        .scroll((app.words_scroll, 0));
    frame.render_widget(paragraph, area);
}

fn draw_review(frame: &mut Frame, app: &App, area: Rect) {
    let Some(session) = app.review.as_ref() else {
        draw_empty(
            frame,
            area,
            "no review session",
            "n new session   Esc close",
            app,
        );
        return;
    };
    if session.queue.is_empty() {
        draw_empty(frame, area, "nothing due", "n new session   Esc close", app);
        return;
    }
    if session.index >= session.queue.len() {
        let text = vec![
            Line::from(Span::styled("Session done", app.theme.translation)),
            Line::from(""),
            Line::from(format!(
                "reviewed {}   good {}   again {}",
                session.done, session.good, session.again
            )),
            Line::from(""),
            Line::from(Span::styled("n new session   Esc close", app.theme.dim)),
        ];
        let paragraph = Paragraph::new(text).alignment(Alignment::Center).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(app.theme.border),
        );
        let card = centered(area, 78, 12);
        frame.render_widget(Clear, card);
        frame.render_widget(paragraph, card);
        return;
    }
    let row = session.current().unwrap();
    let card = centered(area, 78, 20);
    frame.render_widget(Clear, card);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(app.theme.border)
        .title(format!(
            " Review {}/{} ",
            session.index + 1,
            session.queue.len()
        ));
    let inner = block.inner(card);
    frame.render_widget(block, card);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(inner);
    let ratio = session.index as f64 / session.queue.len().max(1) as f64;
    let gauge_style = if ratio < 0.34 {
        app.theme.error
    } else if ratio < 0.67 {
        app.theme.warn
    } else {
        app.theme.translation
    };
    let gauge = Gauge::default()
        .ratio(ratio.clamp(0.0, 1.0))
        .label(format!("{}%", (ratio * 100.0) as u32))
        .gauge_style(gauge_style);
    frame.render_widget(gauge, chunks[0]);
    frame.render_widget(Paragraph::new(""), chunks[1]);

    let prompt = session.prompt(row);
    let answer = session.answer(row);
    let mut body = vec![Line::from(Span::styled(
        prompt.clone(),
        app.theme.title.add_modifier(Modifier::BOLD),
    ))];
    if !row.phonetic.is_empty() {
        body.push(Line::from(Span::styled(
            row.phonetic.clone(),
            app.theme.dim,
        )));
    }
    if !row.tags.is_empty() {
        body.push(Line::from(Span::styled(
            format!("[{}]", row.tags),
            app.theme.dim,
        )));
    }
    body.push(Line::from(""));
    if session.mode() == ReviewMode::Flash {
        if !session.revealed {
            body.push(Line::from(Span::styled("Space reveal", app.theme.warn)));
        } else {
            body.push(Line::from(Span::styled(
                answer.clone(),
                app.theme.translation,
            )));
        }
    } else if !session.checked {
        body.push(Line::from(vec![
            Span::styled("> ", app.theme.title),
            Span::raw(session.typed.clone()),
        ]));
    } else {
        let ok = session.last_ok.unwrap_or(false);
        body.push(Line::from(Span::styled(
            if ok { "Correct" } else { "Wrong" },
            if ok {
                app.theme.translation
            } else {
                app.theme.error
            },
        )));
        body.push(Line::from(Span::styled(
            format!("expected: {answer}"),
            app.theme.dim,
        )));
        body.push(Line::from(Span::styled(
            "Space/Enter continue",
            app.theme.warn,
        )));
    }
    let paragraph = Paragraph::new(body)
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false })
        .scroll((session.scroll, 0));
    frame.render_widget(paragraph, chunks[2]);
    let chip = format!(
        "{}  {}  box {}",
        session.mode().label(),
        if session.reverse {
            "reverse"
        } else {
            "forward"
        },
        row.box_
    );
    frame.render_widget(
        Paragraph::new(chip)
            .alignment(Alignment::Center)
            .style(app.theme.dim),
        chunks[3],
    );
}

fn draw_stats(frame: &mut Frame, app: &App, area: Rect) {
    let Some(stats) = app.stats.as_ref() else {
        draw_empty(frame, area, "stats unavailable", "r reload", app);
        return;
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(7),
            Constraint::Min(0),
        ])
        .split(area);
    let cards = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Ratio(1, 5),
            Constraint::Ratio(1, 5),
            Constraint::Ratio(1, 5),
            Constraint::Ratio(1, 5),
            Constraint::Ratio(1, 5),
        ])
        .split(chunks[0]);
    let values = [
        ("total", stats.total),
        ("new", stats.new),
        ("due", stats.due),
        ("today", stats.reviewed_today),
        ("hits", stats.hits),
    ];
    for (index, (label, value)) in values.iter().enumerate() {
        let paragraph = Paragraph::new(vec![
            Line::from(Span::styled(
                value.to_string(),
                app.theme.title.add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(*label, app.theme.dim)),
        ])
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(app.theme.border),
        );
        frame.render_widget(paragraph, cards[index]);
    }
    let max = stats.by_box.iter().copied().max().unwrap_or(1).max(1);
    let boxes_block = Block::default()
        .borders(Borders::ALL)
        .border_style(app.theme.border)
        .title("Boxes");
    let boxes_inner = boxes_block.inner(chunks[1]);
    frame.render_widget(boxes_block, chunks[1]);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Ratio(1, 5),
            Constraint::Ratio(1, 5),
            Constraint::Ratio(1, 5),
            Constraint::Ratio(1, 5),
            Constraint::Ratio(1, 5),
        ])
        .split(boxes_inner);
    for (index, count) in stats.by_box.iter().enumerate() {
        let ratio = *count as f64 / max as f64;
        let style = if ratio < 0.34 {
            app.theme.error
        } else if ratio < 0.67 {
            app.theme.warn
        } else {
            app.theme.translation
        };
        let gauge = Gauge::default()
            .ratio(ratio.clamp(0.0, 1.0))
            .label(format!("box {}   {}", index + 1, count))
            .gauge_style(style);
        frame.render_widget(gauge, rows[index]);
    }
    let top_rows: Vec<Row> = stats
        .top
        .iter()
        .map(|row| {
            Row::new(vec![
                Cell::from(row.hit.to_string()),
                Cell::from(row.word.clone()),
                Cell::from(truncate_width(&row.translation.replace('\n', " "), 60)),
            ])
        })
        .collect();
    let table = Table::new(
        top_rows,
        [
            Constraint::Length(5),
            Constraint::Length(20),
            Constraint::Min(10),
        ],
    )
    .header(Row::new(vec!["hit", "word", "translation"]).style(app.theme.dim))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(app.theme.border)
            .title("Top hits"),
    )
    .column_spacing(1);
    frame.render_widget(table, chunks[2]);
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    if matches!(app.mode, Mode::Command) {
        let line = Line::from(vec![
            Span::styled(":", app.theme.command),
            Span::raw(app.command.clone()),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }
    let split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(32)])
        .split(area);
    let line = Line::from(vec![
        Span::styled(
            format!(" {} ", app.mode_badge()),
            app.theme.mode_style(&app.mode),
        ),
        Span::raw(" "),
        Span::styled(app.status_hint(), app.theme.dim),
    ]);
    frame.render_widget(Paragraph::new(line), split[0]);
    let style = if app.message_error {
        app.theme.error
    } else {
        app.theme.warn
    };
    frame.render_widget(
        Paragraph::new(app.message.clone())
            .alignment(Alignment::Right)
            .style(style),
        split[1],
    );
}

fn draw_overlay(frame: &mut Frame, app: &App, overlay: &Overlay, area: Rect) {
    let rect = centered_percent(area, 90, 85);
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(app.theme.border)
        .title(match overlay {
            Overlay::Dict(_) => " Dictionary ",
            Overlay::Word(_) => " Word ",
        });
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let lines = match overlay {
        Overlay::Dict(entry) => dict_detail_lines(entry, app),
        Overlay::Word(row) => word_detail_lines(row, app),
    };
    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((app.overlay_scroll, 0));
    frame.render_widget(paragraph, inner);
}

fn dict_detail_lines(entry: &Entry, app: &App) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(
        entry.head.clone(),
        app.theme.title,
    ))];
    if !entry.phonetic.is_empty() {
        lines.push(Line::from(Span::styled(
            entry.phonetic.clone(),
            app.theme.dim,
        )));
    }
    if !entry.pinyin.is_empty() {
        lines.push(Line::from(Span::styled(
            entry.pinyin.clone(),
            app.theme.dim,
        )));
    }
    if !entry.trad.is_empty() && entry.trad != entry.head {
        lines.push(Line::from(Span::styled(
            format!("trad: {}", entry.trad),
            app.theme.dim,
        )));
    }
    if !entry.tags.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("tags: {}", entry.tags),
            app.theme.dim,
        )));
    }
    lines.push(Line::from(Span::styled(
        format!(
            "{}   bnc {}   frq {}   collins {}   oxford {}",
            entry.lang, entry.bnc, entry.frq, entry.collins, entry.oxford
        ),
        app.theme.dim,
    )));
    lines.push(Line::from(""));
    for sense in dict::split_senses(&entry.translation) {
        let mut spans = Vec::new();
        if let Some(pos) = sense.pos {
            spans.push(Span::styled(format!("{pos} "), app.theme.dim));
        }
        spans.push(Span::raw(sense.body));
        lines.push(Line::from(spans));
    }
    if !entry.definition.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("definition", app.theme.dim)));
        for line in entry.definition.lines() {
            lines.push(Line::from(line.to_string()));
        }
    }
    if !entry.exchange.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("exchange: {}", entry.exchange),
            app.theme.dim,
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "s save   Esc close",
        app.theme.dim,
    )));
    lines
}

fn word_detail_lines(row: &WordRow, app: &App) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(row.word.clone(), app.theme.title)),
        Line::from(Span::styled(
            row.translation.replace("\\n", "\n"),
            app.theme.translation,
        )),
    ];
    if !row.phonetic.is_empty() {
        lines.push(Line::from(Span::styled(
            row.phonetic.clone(),
            app.theme.dim,
        )));
    }
    if !row.tags.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("tags: {}", row.tags),
            app.theme.dim,
        )));
    }
    lines.push(Line::from(Span::styled(
        format!(
            "box {}   due {}   hit {}   reviews {}",
            row.box_,
            if row.due.is_empty() { "new" } else { &row.due },
            row.hit,
            row.reviews
        ),
        app.theme.dim,
    )));
    lines.push(Line::from(Span::styled(
        format!(
            "last {}   reviewed {}   uuid {}",
            if row.last.is_empty() { "-" } else { &row.last },
            if row.reviewed.is_empty() {
                "-"
            } else {
                &row.reviewed
            },
            row.uuid
        ),
        app.theme.dim,
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "e edit   Esc close",
        app.theme.dim,
    )));
    lines
}

fn draw_help(frame: &mut Frame, app: &App, area: Rect) {
    let rect = centered_percent(area, 70, 70);
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(app.theme.border)
        .title(" Help ");
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let mut lines = vec![
        Line::from(Span::styled("Global", app.theme.title)),
        Line::from("q quit   Ctrl-c quit   ? help   1..4/Tab/H/L tabs   : command"),
        Line::from(""),
        Line::from(Span::styled("Lookup", app.theme.title)),
        Line::from("i/a input   / clear input   j/k select   Ctrl-d/u scroll"),
        Line::from("Enter detail   s save   :ai force AI   Esc clear"),
        Line::from(""),
        Line::from(Span::styled("Words", app.theme.title)),
        Line::from("j/k move   gg/G top/bottom   / filter   s sort   r reload"),
        Line::from("Enter detail   e edit   dd delete   a add"),
        Line::from(""),
        Line::from(Span::styled("Review", app.theme.title)),
        Line::from("Space reveal  1 again  2 good  t type  R reverse  n new  Esc close"),
        Line::from("type mode: i edit, Enter check, Space/Enter continue"),
        Line::from(""),
        Line::from(Span::styled("Commands", app.theme.title)),
        Line::from(":q :help :ai :dict :merge <path> :import <path> :gc [days]"),
        Line::from(":sort <due|hit|last|word|box>   :tab <lookup|words|review|stats>   :version"),
        Line::from(""),
        Line::from(Span::styled(
            "j/k scroll, any other key closes",
            app.theme.dim,
        )),
    ];
    let paragraph = Paragraph::new(std::mem::take(&mut lines))
        .wrap(Wrap { trim: false })
        .scroll((app.help_scroll, 0));
    frame.render_widget(paragraph, inner);
}

fn draw_confirm(frame: &mut Frame, app: &App, action: &crate::tui::ConfirmAction) {
    let area = frame.area();
    let rect = centered(area, 50, 7);
    frame.render_widget(Clear, rect);
    let (word, label) = match action {
        crate::tui::ConfirmAction::DeleteWord { word, .. } => (word.as_str(), "Delete word"),
    };
    let text = vec![
        Line::from(Span::styled(label, app.theme.error)),
        Line::from(""),
        Line::from(word.to_string()),
        Line::from(""),
        Line::from(Span::styled(
            "y confirm   any other key cancel",
            app.theme.dim,
        )),
    ];
    let paragraph = Paragraph::new(text).alignment(Alignment::Center).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(app.theme.error),
    );
    frame.render_widget(paragraph, rect);
}

fn draw_empty(frame: &mut Frame, area: Rect, title: &str, hint: &str, app: &App) {
    let paragraph = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(title.to_string(), app.theme.title)),
        Line::from(""),
        Line::from(Span::styled(hint.to_string(), app.theme.dim)),
    ])
    .alignment(Alignment::Center)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(app.theme.border),
    );
    frame.render_widget(paragraph, area);
}

fn draw_edit_popup(frame: &mut Frame, app: &App, area: Rect) {
    let rect = centered(area, 64, 7);
    frame.render_widget(Clear, rect);
    let (title, lines) = match &app.mode {
        Mode::Insert(InsertTarget::EditTranslation { .. }) => (
            " Edit translation ",
            vec![Line::from(vec![
                Span::styled("translation> ", app.theme.title),
                Span::raw(app.edit_buffer.clone()),
            ])],
        ),
        Mode::Insert(InsertTarget::AddWord) => (
            " Add word ",
            vec![Line::from(vec![
                Span::styled("word> ", app.theme.title),
                Span::raw(app.edit_buffer.clone()),
            ])],
        ),
        Mode::Insert(InsertTarget::AddTranslation) => (
            " Add translation ",
            vec![
                Line::from(vec![
                    Span::styled("word: ", app.theme.dim),
                    Span::raw(app.add_word.clone()),
                ]),
                Line::from(vec![
                    Span::styled("translation> ", app.theme.title),
                    Span::raw(app.edit_buffer.clone()),
                ]),
            ],
        ),
        _ => return,
    };
    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(app.theme.input_border)
                .title(title),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, rect);
}

fn set_cursor(frame: &mut Frame, app: &App, main: Rect, area: Rect) {
    match &app.mode {
        Mode::Insert(InsertTarget::Lookup) => {
            let available = main.width.saturating_sub(4) as usize;
            let (_, cursor_col) = app.lookup_display(available);
            let x = main.x + 3 + cursor_col as u16;
            let y = main.y + 1;
            set_cursor_clamped(frame, x, y, area);
        }
        Mode::Insert(InsertTarget::WordsFilter { .. }) => {
            let x = main.x + 2 + display_width(&app.edit_buffer) as u16;
            let y = main.y + 1;
            set_cursor_clamped(frame, x, y, area);
        }
        Mode::Insert(InsertTarget::EditTranslation { .. })
        | Mode::Insert(InsertTarget::AddWord)
        | Mode::Insert(InsertTarget::AddTranslation) => {
            let rect = centered(area, 64, 7);
            let (label, line) = match &app.mode {
                Mode::Insert(InsertTarget::AddTranslation) => ("translation> ", 2u16),
                Mode::Insert(InsertTarget::AddWord) => ("word> ", 1u16),
                _ => ("translation> ", 1u16),
            };
            let x =
                rect.x + 1 + display_width(label) as u16 + display_width(&app.edit_buffer) as u16;
            let y = rect.y + line;
            set_cursor_clamped(frame, x, y, area);
        }
        Mode::Insert(InsertTarget::ReviewTyping) => {
            let card = centered(main, 78, 20);
            let x = card.x
                + 2
                + 2
                + app
                    .review
                    .as_ref()
                    .map(|s| display_width(&s.typed))
                    .unwrap_or(0) as u16;
            let y = card.y + 8;
            set_cursor_clamped(frame, x, y, area);
        }
        Mode::Command => {
            let x = area.x + 1 + display_width(&app.command) as u16;
            set_cursor_clamped(frame, x, area.y + area.height.saturating_sub(1), area);
        }
        _ => {}
    }
}

fn set_cursor_clamped(frame: &mut Frame, x: u16, y: u16, area: Rect) {
    let max_x = area.x + area.width.saturating_sub(1);
    let max_y = area.y + area.height.saturating_sub(1);
    frame.set_cursor_position((x.min(max_x), y.min(max_y)));
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

fn centered_percent(area: Rect, width: u16, height: u16) -> Rect {
    let width = area.width * width / 100;
    let height = area.height * height / 100;
    centered(
        area,
        width.max(20).min(area.width),
        height.max(5).min(area.height),
    )
}
