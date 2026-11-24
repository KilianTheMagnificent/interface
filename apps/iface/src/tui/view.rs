//! Ratatui drawing and transcript formatting; no provider or store effects.
use super::approval::{ApprovalKind, answers_for_questions};
use super::{
    AgentStatus, Block, Borders, CellTone, Clear, Color, Composer, Constraint, DetailsLayout,
    Frame, HELP_TEXT, Layout, LayoutUnit, Line, MAX_ACTIVITY_CHARACTERS, Modifier, OverlayState,
    PaletteKind, PaletteState, Paragraph, PendingApproval, PickerItem, PlanEntryPriority,
    PlanEntryStatus, Rect, Span, Style, Transcript, TranscriptCell, TranscriptLayout,
    TranscriptScroll, TuiApp, TuiView, Wrap, effort_display_name, filtered_picker_indices,
    for_display, is_slash_query, slash_picker_indices,
};

pub(super) fn render_tui(frame: &mut Frame<'_>, app: &mut TuiApp) {
    let area = frame.area();
    let compact = area.width < 52 || area.height < 18;
    let view = app.controller.view();
    let approval = app.pending_approval();
    let slash_items = (!app.slash_dismissed && is_slash_query(app.input.text()))
        .then(|| app.slash_items())
        .filter(|items| !items.is_empty());
    let footer_height = footer_height(
        app,
        area,
        compact,
        approval.as_ref(),
        slash_items.as_deref(),
    );
    let [head, body, foot] = area.layout(&Layout::vertical([
        Constraint::Length(if compact { 2 } else { 4 }),
        Constraint::Min(1),
        Constraint::Length(footer_height),
    ]));
    render_header(frame, head, &view, compact);
    render_transcript(frame, body, app);
    render_footer(
        frame,
        foot,
        app,
        &view,
        compact,
        approval.as_ref(),
        slash_items.as_deref(),
    );
    match &mut app.overlay {
        Some(OverlayState::Help) => render_help(frame, overlay_rect(area)),
        Some(OverlayState::Picker(palette)) => {
            render_picker(frame, overlay_rect(area), palette, area);
        }
        Some(OverlayState::Details {
            title,
            lines,
            scroll,
            layout,
        }) => render_details(frame, overlay_rect(area), title, lines, scroll, layout),
        None => {}
    }
}

pub(super) fn footer_height(
    app: &TuiApp,
    area: Rect,
    compact: bool,
    approval: Option<&PendingApproval>,
    slash_items: Option<&[PickerItem]>,
) -> u16 {
    let base = if compact { 3 } else { 4 };
    if app.delivery_unknown.is_some() {
        return 5.min(area.height.saturating_sub(2).max(3));
    }
    if let Some(approval) = approval {
        let desired = match &approval.kind {
            ApprovalKind::Plain => 5,
            ApprovalKind::InvalidQuestions { .. } => 7,
            ApprovalKind::Questions {
                questions,
                active_question,
                ..
            } => questions
                .get((*active_question).min(questions.len().saturating_sub(1)))
                .map_or(6, |question| {
                    5 + u16::try_from(
                        question.options.len()
                            + question
                                .options
                                .iter()
                                .filter(|option| option.preview.is_some())
                                .count(),
                    )
                    .unwrap_or(8)
                }),
        };
        return desired.min(
            area.height
                .saturating_sub(if compact { 3 } else { 5 })
                .max(3),
        );
    }
    if slash_items.is_some() {
        return (base + picker_limit(area.width, area.height, 6) + 2).min(
            area.height
                .saturating_sub(if compact { 3 } else { 5 })
                .max(base),
        );
    }
    base
}

pub(super) fn render_header(frame: &mut Frame<'_>, area: Rect, view: &TuiView, compact: bool) {
    let engine = view.engine.as_deref().unwrap_or("claude");
    let status = view.status.map_or("new", AgentStatus::as_str);
    let model = view.model.as_deref().unwrap_or("default");
    let title = if compact {
        format!(" iface | {engine} | {status} ")
    } else {
        format!(
            " Interface {} | {engine} | {model} | {status} ",
            crate::app::product_version()
        )
    };
    let body = if compact {
        String::new()
    } else {
        let settings = [
            view.effort.as_deref().map(effort_display_name),
            view.fast_mode.then(|| "Fast".into()),
            view.access.as_deref().map(for_display),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        format!(
            "{}{}\nEnter sends | /help commands | /quit exits",
            for_display(view.cwd.as_deref().unwrap_or("-")),
            if settings.is_empty() {
                String::new()
            } else {
                format!(" · {settings}")
            }
        )
    };
    frame.render_widget(
        Paragraph::new(body)
            .block(Block::default().borders(Borders::ALL).title(title))
            .wrap(Wrap { trim: true }),
        area,
    );
}

pub(super) fn render_transcript(frame: &mut Frame<'_>, area: Rect, app: &mut TuiApp) {
    let inner_width = area.width.saturating_sub(2).max(1);
    let layout = &mut app.transcript_layout;
    // Styled lines do not depend on width, so a resize re-measures the lines
    // already built instead of re-cloning and restyling every cell.
    if let Some(current) = layout
        .as_mut()
        .filter(|current| current.inner_width != inner_width && current.source.is_some())
    {
        current.row_starts = row_starts(&current.lines, inner_width);
        current.inner_width = inner_width;
    }
    let plan = app.controller.with_active_transcript(|transcript| {
        plan_transcript_layout(layout.as_ref(), transcript, inner_width)
    });
    apply_transcript_plan(layout, plan, inner_width);
    let layout = layout.as_mut().expect("transcript layout");
    let block = Block::default().borders(Borders::LEFT | Borders::RIGHT);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let visible = usize::from(inner.height);
    let total = layout.row_starts.last().copied().unwrap_or_default();
    let maximum = total.saturating_sub(visible);
    layout.maximum = maximum;
    let top = if let Some(top) = layout.held_top(app.scroll) {
        top.min(maximum)
    } else {
        // A view held on another transcript (a session switch, or a cleared
        // one showing the welcome text) follows the new one from its end.
        app.scroll = TranscriptScroll::Follow;
        maximum
    };
    render_wrapped_window(frame, inner, &layout.lines, &layout.row_starts, top);
}

impl TranscriptLayout {
    /// The row `scroll` holds at the top of this layout, unclamped; `None`
    /// when it follows output or holds a different transcript.
    pub(super) fn held_top(&self, scroll: TranscriptScroll) -> Option<usize> {
        let TranscriptScroll::Held {
            transcript,
            cell,
            rows,
        } = scroll
        else {
            return None;
        };
        if self.source.map(|(id, _)| id) != Some(transcript) {
            return None;
        }
        let mut line = 0;
        for unit in &self.units {
            let end = line + unit.lines;
            if cell < unit.first + unit.cells as u64 {
                let start = self.row_starts[line];
                // A cell dropped from the front leaves the oldest row on top.
                if cell < unit.first {
                    return Some(start);
                }
                let height = self.row_starts[end] - start;
                return Some(start + rows.min(height.saturating_sub(1)));
            }
            line = end;
        }
        Some(self.row_starts[line])
    }

    /// Holds wrapped row `top` at the top: the display unit it falls in and
    /// how many of that unit's rows lie above it.
    pub(super) fn hold_at(&self, top: usize) -> Option<TranscriptScroll> {
        let (transcript, _) = self.source?;
        let mut line = 0;
        for unit in &self.units {
            let end = line + unit.lines;
            if top < self.row_starts[end] {
                return Some(TranscriptScroll::Held {
                    transcript,
                    cell: unit.first,
                    rows: top - self.row_starts[line],
                });
            }
            line = end;
        }
        None
    }
}

/// What the transcript layout must do to match the active transcript,
/// decided under the state lock with owned copies of only the cells to
/// restyle.
pub(super) enum TranscriptPlan {
    Current,
    Welcome,
    /// Lay out `cells` from scratch; the first sits at absolute index `front`.
    Rebuild {
        source: (u64, u64),
        front: u64,
        cells: Vec<TranscriptCell>,
    },
    /// Drop the first `dropped` units (cells the transcript no longer holds),
    /// keep the next `kept`, and restyle `cells`, which start at the absolute
    /// index `resume`.
    Extend {
        source: (u64, u64),
        dropped: usize,
        kept: usize,
        resume: u64,
        cells: Vec<TranscriptCell>,
    },
}

pub(super) fn plan_transcript_layout(
    layout: Option<&TranscriptLayout>,
    transcript: Option<&Transcript>,
    inner_width: u16,
) -> TranscriptPlan {
    let same_width = layout.filter(|layout| layout.inner_width == inner_width);
    let Some(transcript) = transcript.filter(|transcript| !transcript.is_empty()) else {
        return if same_width.is_some_and(|layout| layout.source.is_none()) {
            TranscriptPlan::Current
        } else {
            TranscriptPlan::Welcome
        };
    };
    let source = (transcript.id(), transcript.version());
    let front = transcript.front();
    let rebuild = || TranscriptPlan::Rebuild {
        source,
        front,
        cells: transcript.to_vec(),
    };
    let Some(layout) = same_width else {
        return rebuild();
    };
    let Some((_, version)) = layout.source.filter(|(id, _)| *id == transcript.id()) else {
        return rebuild();
    };
    let Some(first_change) = transcript.first_change_since(version) else {
        return TranscriptPlan::Current;
    };
    // Units the transcript dropped from its front go; one it cut in half, or
    // a change the log no longer reaches, means starting over.
    let dropped = layout
        .units
        .iter()
        .take_while(|unit| unit.first < front)
        .count();
    if layout
        .units
        .get(dropped)
        .is_some_and(|unit| unit.first != front)
        || first_change < front
    {
        return rebuild();
    }
    let mut kept = layout.units[dropped..]
        .iter()
        .take_while(|unit| unit.first + unit.cells as u64 <= first_change)
        .count();
    // A command run can absorb the next cell, so it is restyled with it.
    if kept > 0 && layout.units[dropped + kept - 1].command_run {
        kept -= 1;
    }
    let resume = layout.units[dropped..dropped + kept]
        .last()
        .map_or(front, |unit| unit.first + unit.cells as u64);
    let start = usize::try_from(resume - front).unwrap_or(usize::MAX);
    TranscriptPlan::Extend {
        source,
        dropped,
        kept,
        resume,
        cells: transcript
            .get(start..)
            .map(<[TranscriptCell]>::to_vec)
            .unwrap_or_default(),
    }
}

pub(super) fn apply_transcript_plan(
    layout: &mut Option<TranscriptLayout>,
    plan: TranscriptPlan,
    inner_width: u16,
) {
    let (source, dropped, kept, resume, cells) = match plan {
        TranscriptPlan::Current => return,
        TranscriptPlan::Rebuild {
            source,
            front,
            cells,
        } => {
            *layout = Some(TranscriptLayout {
                source: Some(source),
                inner_width,
                units: Vec::new(),
                lines: Vec::new(),
                row_starts: vec![0],
                maximum: 0,
            });
            (source, 0, 0, front, cells)
        }
        TranscriptPlan::Welcome => {
            let welcome = Line::from(vec![
                Span::styled(
                    "Welcome to iface",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(" - start typing to create a session."),
            ]);
            *layout = Some(TranscriptLayout {
                source: None,
                inner_width,
                units: Vec::new(),
                row_starts: vec![0, wrapped_rows(&welcome, inner_width)],
                lines: vec![welcome],
                maximum: 0,
            });
            return;
        }
        TranscriptPlan::Extend {
            source,
            dropped,
            kept,
            resume,
            cells,
        } => (source, dropped, kept, resume, cells),
    };
    // Only a transcript layout (never the welcome text) reaches here.
    let Some(layout) = layout.as_mut() else {
        return;
    };
    if dropped > 0 {
        let lines = layout
            .units
            .drain(..dropped)
            .map(|unit| unit.lines)
            .sum::<usize>();
        layout.lines.drain(..lines);
        let removed = layout.row_starts[lines];
        layout.row_starts.drain(..lines);
        for start in &mut layout.row_starts {
            *start -= removed;
        }
    }
    layout.source = Some(source);
    layout.units.truncate(kept);
    let kept_lines = layout.units.iter().map(|unit| unit.lines).sum::<usize>();
    layout.lines.truncate(kept_lines);
    layout.row_starts.truncate(kept_lines + 1);
    let mut first = resume;
    for (count, command_run, lines) in transcript_styled_units(&cells) {
        let mut row = layout.row_starts.last().copied().unwrap_or_default();
        for line in &lines {
            row += wrapped_rows(line, inner_width);
            layout.row_starts.push(row);
        }
        layout.units.push(LayoutUnit {
            first,
            cells: count,
            command_run,
            lines: lines.len(),
        });
        layout.lines.extend(lines);
        first += count as u64;
    }
}

/// Renders the rows `top..` of `lines` that fit `area`, touching only the
/// lines in view. `row_starts[i]` is the wrapped row where `lines[i]` begins.
pub(super) fn render_wrapped_window(
    frame: &mut Frame<'_>,
    area: Rect,
    lines: &[Line<'_>],
    row_starts: &[usize],
    top: usize,
) {
    let (window, offset) = wrapped_window(row_starts, lines.len(), top, area.height);
    let Some(window) = lines.get(window) else {
        return;
    };
    render_window(
        frame,
        area,
        window.iter().map(borrowed_line).collect(),
        offset,
    );
}

/// The lines that cover rows `top..top + height`, and how many rows of the
/// first of them lie above `top`.
fn wrapped_window(
    row_starts: &[usize],
    lines: usize,
    top: usize,
    height: u16,
) -> (std::ops::Range<usize>, usize) {
    let starts = &row_starts[..lines];
    let first = starts
        .partition_point(|start| *start <= top)
        .saturating_sub(1);
    let end = starts.partition_point(|start| *start < top.saturating_add(usize::from(height)));
    let offset = top.saturating_sub(starts.get(first).copied().unwrap_or_default());
    (first..end.max(first), offset)
}

fn render_window(frame: &mut Frame<'_>, area: Rect, window: Vec<Line<'_>>, offset: usize) {
    // Only the first line can start above the view; a single line taller
    // than u16::MAX rows would clamp, which no bounded cell reaches in practice.
    frame.render_widget(
        Paragraph::new(window)
            .wrap(Wrap { trim: false })
            .scroll((u16::try_from(offset).unwrap_or(u16::MAX), 0)),
        area,
    );
}

fn borrowed_line<'a>(line: &'a Line<'_>) -> Line<'a> {
    Line {
        spans: line
            .spans
            .iter()
            .map(|span| Span::styled(span.content.as_ref(), span.style))
            .collect(),
        style: line.style,
        alignment: line.alignment,
    }
}

pub(super) fn render_footer(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &TuiApp,
    view: &TuiView,
    compact: bool,
    approval: Option<&PendingApproval>,
    slash_items: Option<&[PickerItem]>,
) {
    if let Some(approval) = approval {
        render_approval(frame, area, approval);
        return;
    }
    if let Some(pending) = &app.delivery_unknown {
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(
                    "Delivery outcome unknown",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Line::raw(for_display(&pending.message)),
                Line::styled("r resend · x discard", Style::default().fg(Color::DarkGray)),
            ])
            .block(Block::default().borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    let status = view.status.map_or("new", AgentStatus::as_str);
    let default_hint = if compact {
        "enter send | esc stop | /help"
    } else {
        "Enter send | Ctrl+T steer | Esc stop | /help"
    };
    let notice = app
        .effect_label()
        .or(app.notice.as_deref())
        .unwrap_or(default_hint);
    let mut lines = Vec::new();
    if let Some(items) = slash_items {
        let query = app.input.text().strip_prefix('/').unwrap_or_default();
        let matches = slash_picker_indices(items, query);
        let selected = app.slash_index.min(matches.len().saturating_sub(1));
        let limit = usize::from(picker_limit(frame.area().width, frame.area().height, 6));
        let (visible, start) = picker_window(&matches, selected, limit);
        if visible.is_empty() {
            lines.push(Line::styled(
                "No matching commands",
                Style::default().fg(Color::DarkGray),
            ));
        } else {
            lines.extend(
                visible
                    .into_iter()
                    .enumerate()
                    .filter_map(|(offset, index)| {
                        items
                            .get(index)
                            .map(|item| picker_item_line(item, start + offset == selected))
                    }),
            );
        }
        lines.push(Line::styled(
            "↑↓ move · enter insert · esc close",
            Style::default().fg(Color::DarkGray),
        ));
    }
    let status_color = match view.status {
        Some(AgentStatus::Working) => Color::Cyan,
        Some(AgentStatus::Blocked) => Color::Yellow,
        Some(AgentStatus::Failed) => Color::Red,
        Some(AgentStatus::NotLive) => Color::DarkGray,
        Some(AgentStatus::Idle) | None => Color::Green,
    };
    lines.extend([
        Line::from(
            std::iter::once(Span::styled(
                "❯ ",
                Style::default().fg(engine_color(view.engine.as_deref())),
            ))
            .chain(composer_spans(&app.input))
            .collect::<Vec<_>>(),
        ),
        Line::from(vec![
            Span::styled(format!("{status}  "), Style::default().fg(status_color)),
            Span::styled(for_display(notice), Style::default().fg(Color::DarkGray)),
        ]),
    ]);
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        area,
    );
}

pub(super) fn composer_spans(composer: &Composer) -> impl Iterator<Item = Span<'static>> {
    let (before, after) = composer.text().split_at(composer.cursor());
    let before = Span::raw(for_display(before));
    let (cursor, after) = after.chars().next().map_or_else(
        || (" ".to_owned(), String::new()),
        |character| {
            (
                for_display(&character.to_string()),
                for_display(&after[character.len_utf8()..]),
            )
        },
    );
    [
        before,
        Span::styled(cursor, Style::default().add_modifier(Modifier::REVERSED)),
        Span::raw(after),
    ]
    .into_iter()
}

pub(super) fn render_help(frame: &mut Frame<'_>, area: Rect) {
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(HELP_TEXT)
            .block(Block::default().borders(Borders::ALL).title(" iface help "))
            .wrap(Wrap { trim: false }),
        area,
    );
}

pub(super) fn render_picker(
    frame: &mut Frame<'_>,
    area: Rect,
    palette: &PaletteState,
    viewport: Rect,
) {
    frame.render_widget(Clear, area);
    let matches = filtered_picker_indices(&palette.items, &palette.query);
    let selected = palette.active_index.min(matches.len().saturating_sub(1));
    let limit = usize::from(picker_limit(viewport.width, viewport.height, 8));
    let (visible, start) = picker_window(&matches, selected, limit);
    let mut lines = vec![Line::styled(
        format!(
            "› {}",
            if palette.query.is_empty() {
                "type to filter"
            } else {
                &palette.query
            }
        ),
        Style::default().fg(Color::DarkGray),
    )];
    let visible_len = visible.len();
    if visible.is_empty() {
        lines.push(Line::styled(
            "No matches",
            Style::default().fg(Color::Yellow),
        ));
    } else {
        lines.extend(
            visible
                .into_iter()
                .enumerate()
                .filter_map(|(offset, index)| {
                    palette
                        .items
                        .get(index)
                        .map(|item| picker_item_line(item, start + offset == selected))
                }),
        );
    }
    if let Some(preview) = &palette.preview {
        lines.push(Line::default());
        lines.push(Line::styled(
            "preview",
            Style::default().fg(Color::DarkGray),
        ));
        lines.extend(preview.lines().map(|line| Line::raw(for_display(line))));
    }
    lines.push(Line::styled(
        format!(
            "↑↓ move · enter select{} · esc close{}",
            if palette.kind == PaletteKind::Sessions {
                " · space preview"
            } else {
                ""
            },
            if matches.len() > visible_len {
                format!(" · {}/{}", selected + 1, matches.len())
            } else {
                String::new()
            }
        ),
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {} ", palette.title)),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

/// Draws the rows of `lines` in view. The wrapped heights are measured once
/// per width into `layout`, and only the lines in view are built per frame.
pub(super) fn render_details(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    lines: &[String],
    scroll: &mut u16,
    layout: &mut Option<DetailsLayout>,
) {
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        // show_details sanitizes the title once when the overlay is stored.
        .title(format!(" {title} "));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = inner.width.max(1);
    let measured = match layout {
        Some(measured) if measured.width == width => measured,
        _ => layout.insert(DetailsLayout {
            width,
            row_starts: row_starts(
                &lines
                    .iter()
                    .map(|line| Line::raw(line.as_str()))
                    .collect::<Vec<_>>(),
                width,
            ),
        }),
    };
    let total = measured.row_starts.last().copied().unwrap_or_default();
    let maximum = total.saturating_sub(usize::from(inner.height));
    // End stores u16::MAX; it clamps to the last page here, and the clamped
    // offset is stored back so Up and PageUp move at once afterwards.
    let top = usize::from(*scroll).min(maximum);
    *scroll = u16::try_from(top).unwrap_or(u16::MAX);
    let (window, offset) = wrapped_window(&measured.row_starts, lines.len(), top, inner.height);
    let Some(window) = lines.get(window) else {
        return;
    };
    render_window(
        frame,
        inner,
        window.iter().map(|line| Line::raw(line.as_str())).collect(),
        offset,
    );
}

#[allow(clippy::too_many_lines)]
pub(super) fn render_approval(frame: &mut Frame<'_>, area: Rect, approval: &PendingApproval) {
    let mut lines = Vec::new();
    let (border, title) = match &approval.kind {
        ApprovalKind::Plain => {
            lines.push(Line::raw(for_display(&approval.summary)));
            lines.push(Line::styled(
                "a allow · d deny",
                Style::default().fg(Color::DarkGray),
            ));
            (Color::Yellow, format!(" Approval · {} ", approval.tool))
        }
        ApprovalKind::InvalidQuestions { reason } => {
            lines.push(Line::raw(for_display(&approval.summary)));
            lines.push(Line::styled(
                for_display(reason),
                Style::default().fg(Color::Red),
            ));
            lines.push(Line::styled("d deny", Style::default().fg(Color::DarkGray)));
            (Color::Red, format!(" Question · {} ", approval.tool))
        }
        ApprovalKind::Questions {
            questions,
            selections,
            active_question,
        } => {
            let active = (*active_question).min(questions.len().saturating_sub(1));
            let answered = questions
                .iter()
                .enumerate()
                .filter(|(index, question)| {
                    selections.get(index).is_some_and(|selected| {
                        !selected.is_empty()
                            && selected
                                .iter()
                                .all(|choice| *choice < question.options.len())
                            && (question.multi_select || selected.len() == 1)
                    })
                })
                .count();
            lines.push(Line::styled(
                format!(
                    "Question {}/{} · {answered}/{} answered",
                    active + 1,
                    questions.len(),
                    questions.len()
                ),
                Style::default().fg(Color::DarkGray),
            ));
            if let Some(question) = questions.get(active) {
                lines.push(Line::styled(
                    format!(
                        "› {}: {}",
                        for_display(&question.header),
                        for_display(&question.question)
                    ),
                    Style::default().add_modifier(Modifier::BOLD),
                ));
                for (option_index, option) in question.options.iter().enumerate() {
                    let selected = selections
                        .get(&active)
                        .is_some_and(|values| values.contains(&option_index));
                    let marker = if question.multi_select {
                        if selected { "[x]" } else { "[ ]" }
                    } else if selected {
                        "(●)"
                    } else {
                        "( )"
                    };
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!(
                                "{}. {marker} {}",
                                option_index + 1,
                                for_display(&option.label)
                            ),
                            if selected {
                                Style::default().fg(Color::Green)
                            } else {
                                Style::default()
                            },
                        ),
                        Span::styled(
                            format!("  {}", for_display(&option.description)),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]));
                    if let Some(preview) = &option.preview {
                        lines.push(Line::styled(
                            format!("  {}", for_display(preview)),
                            Style::default().fg(Color::DarkGray),
                        ));
                    }
                }
            }
            lines.push(Line::styled(
                format!(
                    "↑↓ question · 1-4 choose · d deny · {}",
                    if answers_for_questions(questions, selections).is_some() {
                        "enter submit"
                    } else {
                        "answer every question"
                    }
                ),
                Style::default().fg(Color::DarkGray),
            ));
            (Color::Yellow, format!(" Question · {} ", approval.tool))
        }
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(border))
                    .title(title),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

pub(super) fn picker_item_line(item: &PickerItem, selected: bool) -> Line<'static> {
    let text = format!(
        "{} {}{}{}",
        if selected { "›" } else { " " },
        item.badge
            .as_deref()
            .map(|badge| format!("{} · ", one_line(badge)))
            .unwrap_or_default(),
        one_line(&item.label),
        item.description
            .as_deref()
            .map(|description| format!(" · {}", one_line(description)))
            .unwrap_or_default()
    );
    Line::styled(
        text,
        if selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        },
    )
}

pub(super) fn picker_window(
    indices: &[usize],
    selected: usize,
    limit: usize,
) -> (Vec<usize>, usize) {
    let limit = limit.max(1);
    if indices.len() <= limit {
        return (indices.to_vec(), 0);
    }
    let selected = selected.min(indices.len() - 1);
    let start = selected
        .saturating_sub(limit - 1)
        .min(indices.len().saturating_sub(limit));
    (indices[start..start + limit].to_vec(), start)
}

const fn picker_limit(columns: u16, rows: u16, wide: u16) -> u16 {
    if columns < 52 || rows <= 12 {
        2
    } else if columns < 72 || rows <= 18 {
        4
    } else {
        wide
    }
}

pub(super) fn overlay_rect(area: Rect) -> Rect {
    if area.width < 36 || area.height < 12 {
        area
    } else {
        area.centered(Constraint::Percentage(90), Constraint::Percentage(82))
    }
}

pub(super) fn transcript_styled_lines(cells: &[TranscriptCell]) -> Vec<Line<'static>> {
    transcript_styled_units(cells)
        .into_iter()
        .flat_map(|(_, _, lines)| lines)
        .collect()
}

/// The transcript's display units in order as `(cells, command run, lines)`:
/// one cell, or a run of consecutive command-activity tool cells shown as one
/// summary. Units are independent, so a suffix restyles on its own.
pub(super) fn transcript_styled_units(
    cells: &[TranscriptCell],
) -> Vec<(usize, bool, Vec<Line<'static>>)> {
    let command = |cell: Option<&TranscriptCell>| {
        matches!(
            cell,
            Some(TranscriptCell::Tool {
                command_activity: true,
                ..
            })
        )
    };
    let mut units = Vec::new();
    let mut index = 0;
    while let Some(cell) = cells.get(index) {
        if command(Some(cell)) {
            let start = index;
            while command(cells.get(index)) {
                index += 1;
            }
            units.push((
                index - start,
                true,
                command_run_styled_lines(&cells[start..index]),
            ));
            continue;
        }
        units.push((1, false, cell_styled_lines(cell)));
        index += 1;
    }
    units
}

pub(super) fn command_run_styled_lines(cells: &[TranscriptCell]) -> Vec<Line<'static>> {
    let results = cells.iter().filter_map(|cell| match cell {
        TranscriptCell::Tool { result, .. } => Some(result),
        _ => None,
    });
    let running = results.clone().any(Option::is_none);
    let failed = results
        .filter(|result| result.as_ref().is_some_and(|result| !result.ok))
        .count();
    let count = cells.len();
    let amount = if count == 1 {
        "a command".to_owned()
    } else {
        format!("{count} commands")
    };
    let failure = match (count, failed) {
        (_, 0) => String::new(),
        (1, _) => " · failed".into(),
        (_, failed) => format!(" · {failed} failed"),
    };
    vec![
        Line::default(),
        Line::styled(
            format!(
                "{} {} {amount}{failure}",
                if failed > 0 { "✗" } else { "•" },
                if running { "Running" } else { "Ran" }
            ),
            if failed > 0 {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ),
    ]
}

pub(super) fn transcript_plain_lines(cells: &[TranscriptCell]) -> Vec<String> {
    transcript_styled_lines(cells)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

/// Rows `line` occupies when word-wrapped (`Wrap { trim: false }`) at
/// `width`: ratatui's own `WordWrapper` count, so it is exactly what renders.
pub(super) fn wrapped_rows(line: &Line<'_>, width: u16) -> usize {
    Paragraph::new(borrowed_line(line))
        .wrap(Wrap { trim: false })
        .line_count(width.max(1))
}

/// Wrapped row where each line begins, plus the total as the last entry.
pub(super) fn row_starts(lines: &[Line<'_>], width: u16) -> Vec<usize> {
    let mut starts = Vec::with_capacity(lines.len() + 1);
    let mut row = 0;
    starts.push(row);
    for line in lines {
        row += wrapped_rows(line, width);
        starts.push(row);
    }
    starts
}

#[allow(clippy::too_many_lines)]
pub(super) fn cell_styled_lines(cell: &TranscriptCell) -> Vec<Line<'static>> {
    let mut lines = vec![Line::default()];
    match cell {
        TranscriptCell::Message {
            role: "user",
            markdown,
        } => {
            let mut body = markdown.trim().lines();
            if let Some(first) = body.next() {
                lines.push(Line::from(vec![
                    Span::styled(
                        "› ",
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        for_display(first),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                ]));
                lines.extend(body.map(|line| {
                    Line::from(vec![
                        Span::raw("  "),
                        Span::styled(
                            for_display(line),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                    ])
                }));
            }
        }
        TranscriptCell::Message { markdown, .. } => lines.extend(markdown_styled_lines(markdown)),
        TranscriptCell::ImageAttachments { count } => lines.push(Line::styled(
            image_attachment_summary(*count),
            Style::default().fg(Color::Blue),
        )),
        TranscriptCell::Peer {
            source_engine,
            objective,
            markdown,
        } => {
            lines.push(Line::styled(
                format!("• Consulted {} · untrusted", for_display(source_engine)),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            lines.push(Line::styled(
                format!("  └ {}", for_display(objective)),
                Style::default().fg(Color::DarkGray),
            ));
            lines.extend(markdown_styled_lines(markdown).into_iter().map(|line| {
                let mut spans = vec![Span::raw("  ")];
                spans.extend(line.spans);
                Line::from(spans)
            }));
        }
        TranscriptCell::Tool {
            tool,
            summary,
            result,
            ..
        } => {
            let text = {
                let tool = one_line(tool);
                let summary = one_line(summary);
                let glyph = if result.as_ref().is_some_and(|result| !result.ok) {
                    "✗"
                } else {
                    "•"
                };
                let detail = if summary == tool { "" } else { &summary };
                truncate_end(
                    &format!(
                        "{glyph} {tool}{}{}",
                        if detail.is_empty() { "" } else { " · " },
                        if result.as_ref().is_some_and(|result| !result.ok) {
                            if detail.is_empty() { "failed" } else { detail }
                        } else {
                            detail
                        }
                    ),
                    MAX_ACTIVITY_CHARACTERS,
                )
            };
            lines.push(Line::styled(
                text,
                if result.as_ref().is_some_and(|result| !result.ok) {
                    Style::default().fg(Color::Red)
                } else {
                    Style::default().fg(Color::DarkGray)
                },
            ));
        }
        TranscriptCell::Diff { path, withheld, .. } => lines.push(Line::styled(
            format!(
                "• Edited {}{}",
                one_line(path),
                if *withheld {
                    " · content withheld"
                } else {
                    ""
                }
            ),
            if *withheld {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::Magenta)
            },
        )),
        TranscriptCell::Notice { message, tone } => lines.extend(message.lines().map(|line| {
            Line::styled(
                for_display(line),
                match tone {
                    CellTone::Info => Style::default().fg(Color::DarkGray),
                    CellTone::Warning => Style::default().fg(Color::Yellow),
                    CellTone::Error => Style::default().fg(Color::Red),
                },
            )
        })),
        TranscriptCell::Compact {
            trigger,
            before_tokens,
            after_tokens,
            ok,
        } => lines.push(Line::styled(
            format!(
                "─ context compacted · {trigger}{}{} ─",
                before_tokens.map_or_else(String::new, |before| format!(
                    " · {before} → {}",
                    after_tokens.map_or_else(|| "?".into(), |after| after.to_string())
                )),
                if *ok == Some(false) { " · failed" } else { "" }
            ),
            Style::default().fg(Color::DarkGray),
        )),
        TranscriptCell::Retry {
            message,
            attempt,
            max_attempts,
            delay_ms,
        } => lines.push(Line::styled(
            format!(
                "retry {attempt}/{max_attempts}{}: {}",
                delay_ms.map_or_else(String::new, |delay| format!(
                    " in {}s",
                    delay.div_ceil(1_000)
                )),
                for_display(message)
            ),
            Style::default().fg(Color::Yellow),
        )),
        TranscriptCell::Plan { entries } => {
            lines.push(Line::styled("plan", Style::default().fg(Color::DarkGray)));
            lines.extend(entries.iter().map(|entry| {
                let (glyph, style) = match entry.status {
                    PlanEntryStatus::Completed => ("✓", Style::default().fg(Color::Green)),
                    PlanEntryStatus::InProgress => ("•", Style::default().fg(Color::Yellow)),
                    PlanEntryStatus::Pending => ("○", Style::default().fg(Color::DarkGray)),
                };
                Line::styled(
                    format!(
                        "{glyph} {} [{}]",
                        for_display(&entry.content),
                        plan_priority_text(entry.priority)
                    ),
                    style,
                )
            }));
        }
        TranscriptCell::Subagent {
            label,
            description,
            model,
            result,
            ..
        } => {
            let failed = result.as_ref().is_some_and(|result| !result.ok);
            lines.push(Line::styled(
                format!(
                    "{} subagent {}{}: {}",
                    if failed { "✗" } else { "•" },
                    for_display(label),
                    model
                        .as_deref()
                        .map(|model| format!(" · {}", for_display(model)))
                        .unwrap_or_default(),
                    for_display(description)
                ),
                if failed {
                    Style::default().fg(Color::Red)
                } else {
                    Style::default().fg(Color::DarkGray)
                },
            ));
            if let Some(result) = result {
                lines.extend(result.summary.lines().map(|line| {
                    Line::styled(
                        format!("  {}", for_display(line)),
                        Style::default().fg(Color::DarkGray),
                    )
                }));
            }
        }
    }
    lines
}

pub(super) fn image_attachment_summary(count: usize) -> String {
    if count <= 1 {
        "[image 1]".into()
    } else {
        format!("[image 1] · +{} more", count - 1)
    }
}

const fn plan_priority_text(priority: PlanEntryPriority) -> &'static str {
    match priority {
        PlanEntryPriority::High => "high",
        PlanEntryPriority::Medium => "medium",
        PlanEntryPriority::Low => "low",
    }
}

pub(super) fn markdown_styled_lines(source: &str) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut in_code = false;
    for raw in source.lines() {
        let trimmed = raw.trim();
        if trimmed.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            lines.push(Line::styled(
                format!("  {}", for_display(raw)),
                Style::default().fg(Color::Cyan),
            ));
            continue;
        }
        if trimmed.is_empty() {
            lines.push(Line::default());
            continue;
        }
        if is_table_separator(trimmed) {
            continue;
        }
        if trimmed.starts_with('|') && trimmed.ends_with('|') {
            let row = trimmed
                .trim_matches('|')
                .split('|')
                .map(str::trim)
                .map(for_display)
                .collect::<Vec<_>>()
                .join(" │ ");
            lines.push(Line::raw(row));
            continue;
        }
        if trimmed
            .chars()
            .all(|character| matches!(character, '-' | '_' | '*'))
            && trimmed.len() >= 3
        {
            lines.push(Line::styled(
                "─".repeat(24),
                Style::default().fg(Color::DarkGray),
            ));
            continue;
        }
        if let Some(heading) = trimmed.strip_prefix('#') {
            let heading = heading.trim_start_matches('#').trim();
            lines.push(Line::styled(
                for_display(heading),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            continue;
        }
        if let Some(quote) = trimmed.strip_prefix('>') {
            let mut spans = vec![Span::styled("│ ", Style::default().fg(Color::DarkGray))];
            spans.extend(inline_spans(quote.trim_start()));
            lines.push(Line::from(spans));
            continue;
        }
        if let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("+ "))
        {
            let mut spans = vec![Span::styled("• ", Style::default().fg(Color::DarkGray))];
            spans.extend(inline_spans(item));
            lines.push(Line::from(spans));
            continue;
        }
        lines.push(Line::from(inline_spans(raw)));
    }
    if lines.is_empty() {
        lines.push(Line::default());
    }
    lines
}

pub(super) fn inline_spans(source: &str) -> Vec<Span<'static>> {
    let safe = for_display(source);
    let mut spans = Vec::new();
    let mut rest = safe.as_str();
    while !rest.is_empty() {
        if let Some(image) = rest.strip_prefix("![") {
            if let Some(end) = image.find("](") {
                if let Some(close) = image[end + 2..].find(')') {
                    spans.push(Span::raw(format!("[image: {}]", &image[..end])));
                    rest = &image[end + 3 + close..];
                    continue;
                }
            }
        }
        if let Some(link) = rest.strip_prefix('[') {
            if let Some(end) = link.find("](") {
                if let Some(close) = link[end + 2..].find(')') {
                    spans.push(Span::styled(
                        link[..end].to_owned(),
                        Style::default()
                            .fg(Color::Blue)
                            .add_modifier(Modifier::UNDERLINED),
                    ));
                    rest = &link[end + 3 + close..];
                    continue;
                }
            }
        }
        let matched = [
            ("**", "**", Modifier::BOLD, None),
            ("__", "__", Modifier::BOLD, None),
            ("~~", "~~", Modifier::CROSSED_OUT, None),
            ("`", "`", Modifier::empty(), Some(Color::Cyan)),
            ("*", "*", Modifier::ITALIC, None),
            ("_", "_", Modifier::ITALIC, None),
        ]
        .into_iter()
        .find_map(|(open, close, modifier, color)| {
            let body = rest.strip_prefix(open)?;
            let end = body.find(close)?;
            Some((open, close, modifier, color, body, end))
        });
        if let Some((_open, close, modifier, color, body, end)) = matched {
            let mut style = Style::default().add_modifier(modifier);
            if let Some(color) = color {
                style = style.fg(color);
            }
            spans.push(Span::styled(body[..end].to_owned(), style));
            rest = &body[end + close.len()..];
            continue;
        }
        let next = rest
            .char_indices()
            .skip(1)
            .find(|(_, character)| matches!(character, '!' | '[' | '*' | '_' | '~' | '`'))
            .map_or(rest.len(), |(index, _)| index);
        spans.push(Span::raw(rest[..next].to_owned()));
        rest = &rest[next..];
    }
    spans
}

pub(super) fn is_table_separator(value: &str) -> bool {
    value.starts_with('|')
        && value.ends_with('|')
        && value.trim_matches('|').split('|').all(|cell| {
            cell.trim()
                .trim_matches(':')
                .chars()
                .all(|character| character == '-')
        })
}

pub(super) fn engine_color(engine: Option<&str>) -> Color {
    match engine.unwrap_or_default() {
        "claude" => Color::Magenta,
        "codex" => Color::Green,
        "acp:grok" => Color::Cyan,
        "acp:qwen" => Color::Blue,
        _ => Color::White,
    }
}

pub(super) fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn truncate_end(value: &str, maximum: usize) -> String {
    if value.chars().nth(maximum).is_none() {
        return value.to_owned();
    }
    let end = value
        .char_indices()
        .nth(maximum.saturating_sub(1))
        .map_or(value.len(), |(index, _)| index);
    let mut out = value[..end].to_owned();
    out.push('…');
    out
}
