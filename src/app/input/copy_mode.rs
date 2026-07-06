use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
use unicode_width::UnicodeWidthChar;

use crate::{
    app::{
        state::{CopyModeFindKind, CopyModeSelection, CopyModeState, CopyModeSubState},
        App, AppState, Mode,
    },
    input::TerminalKey,
    selection::Selection,
    terminal::TerminalRuntimeRegistry,
};

impl App {
    pub(crate) fn handle_copy_mode_key(&mut self, key: TerminalKey) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        self.state.update_dismissed = true;
        if self.state.is_prefix_key(key) {
            self.state.mode = Mode::Prefix;
            return;
        }
        self.state
            .handle_copy_mode_key(&self.terminal_runtimes, key);
        if let Some(content) = self.state.request_clipboard_write.take() {
            if self
                .event_tx
                .try_send(crate::events::AppEvent::ClipboardWrite { content })
                .is_err()
            {
                tracing::warn!("failed to queue clipboard write event");
            }
        }
    }
}

impl AppState {
    pub(crate) fn enter_copy_mode(&mut self, terminal_runtimes: &TerminalRuntimeRegistry) {
        let Some(ws_idx) = self.active else {
            return;
        };
        let Some(pane_id) = self
            .workspaces
            .get(ws_idx)
            .and_then(|ws| ws.focused_pane_id())
        else {
            return;
        };
        let Some(info) = self.pane_info_by_id(pane_id).cloned() else {
            return;
        };
        if info.inner_rect.width == 0 || info.inner_rect.height == 0 {
            return;
        }

        let cursor = self
            .runtime_for_pane_in_workspace(terminal_runtimes, ws_idx, pane_id)
            .and_then(|rt| rt.cursor_state(info.inner_rect, true))
            .filter(|cursor| cursor.visible)
            .map(|cursor| {
                (
                    cursor.y.saturating_sub(info.inner_rect.y),
                    cursor.x.saturating_sub(info.inner_rect.x),
                )
            })
            .unwrap_or_else(|| (info.inner_rect.height.saturating_sub(1), 0));
        let entry_offset_from_bottom = self
            .pane_scroll_metrics(terminal_runtimes, pane_id)
            .map_or(0, |metrics| metrics.offset_from_bottom);

        self.clear_selection();
        self.copy_mode = Some(CopyModeState {
            pane_id,
            cursor_row: cursor.0.min(info.inner_rect.height.saturating_sub(1)),
            cursor_col: cursor.1.min(info.inner_rect.width.saturating_sub(1)),
            entry_offset_from_bottom,
            selection: None,
            substate: CopyModeSubState::default(),
            find: None,
        });
        self.mode = Mode::Copy;
    }

    pub(crate) fn handle_copy_mode_key(
        &mut self,
        terminal_runtimes: &TerminalRuntimeRegistry,
        key: TerminalKey,
    ) {
        match key.code {
            KeyCode::Esc => {
                // In Selecting state: clear selection, stay in copy mode.
                // In Navigate state: exit copy mode.
                if let Some(copy_mode) = &self.copy_mode {
                    if copy_mode.substate == CopyModeSubState::Selecting {
                        self.clear_selection();
                        if let Some(cm) = self.copy_mode.as_mut() {
                            cm.substate = CopyModeSubState::Navigate;
                        }
                        return;
                    }
                }
                self.exit_copy_mode(terminal_runtimes, false);
                return;
            }
            KeyCode::Enter => {
                self.exit_copy_mode(terminal_runtimes, true);
                return;
            }
            KeyCode::Left => {
                self.move_copy_cursor(terminal_runtimes, 0, -1);
                return;
            }
            KeyCode::Down => {
                self.move_copy_cursor(terminal_runtimes, 1, 0);
                return;
            }
            KeyCode::Up => {
                self.move_copy_cursor(terminal_runtimes, -1, 0);
                return;
            }
            KeyCode::Right => {
                self.move_copy_cursor(terminal_runtimes, 0, 1);
                return;
            }
            KeyCode::PageUp => {
                self.scroll_copy_mode_page(terminal_runtimes, -1, false);
                return;
            }
            KeyCode::PageDown => {
                self.scroll_copy_mode_page(terminal_runtimes, 1, false);
                return;
            }
            KeyCode::Home => {
                self.copy_mode_line_edge(terminal_runtimes, false);
                return;
            }
            KeyCode::End => {
                self.copy_mode_line_edge(terminal_runtimes, true);
                return;
            }
            _ => {}
        }

        match (key.code, key.modifiers) {
            (KeyCode::Char('b'), mods) if mods.contains(KeyModifiers::CONTROL) => {
                self.scroll_copy_mode_page(terminal_runtimes, -1, false)
            }
            (KeyCode::Char('f'), mods) if mods.contains(KeyModifiers::CONTROL) => {
                self.scroll_copy_mode_page(terminal_runtimes, 1, false)
            }
            (KeyCode::Char('u'), mods) if mods.contains(KeyModifiers::CONTROL) => {
                self.scroll_copy_mode_page(terminal_runtimes, -1, true)
            }
            (KeyCode::Char('d'), mods) if mods.contains(KeyModifiers::CONTROL) => {
                self.scroll_copy_mode_page(terminal_runtimes, 1, true)
            }
            _ => {}
        }

        let Some(ch) = copy_mode_command_char(key) else {
            return;
        };

        // If a find-char (f/F/t/T) operation is pending, consume the next
        // character as the target.
        if self.copy_mode.map_or(false, |cm| cm.find.is_some()) {
            self.copy_mode_find_char(terminal_runtimes, ch);
            return;
        }

        match ch {
            'q' => self.exit_copy_mode(terminal_runtimes, false),
            'y' => self.exit_copy_mode(terminal_runtimes, true),
            'v' | ' ' => {
                // PORT: enter Selecting substate, then begin selection.
                if let Some(cm) = self.copy_mode.as_mut() {
                    cm.substate = CopyModeSubState::Selecting;
                }
                self.begin_copy_mode_selection(terminal_runtimes);
            }
            'V' => self.select_copy_mode_line(terminal_runtimes),
            'h' => self.move_copy_cursor(terminal_runtimes, 0, -1),
            'j' => self.move_copy_cursor(terminal_runtimes, 1, 0),
            'k' => self.move_copy_cursor(terminal_runtimes, -1, 0),
            'l' => self.move_copy_cursor(terminal_runtimes, 0, 1),
            'g' => self.copy_mode_history_top(terminal_runtimes),
            'G' => self.copy_mode_history_bottom(terminal_runtimes),
            '0' => self.copy_mode_line_edge(terminal_runtimes, false),
            '$' => self.copy_mode_line_edge(terminal_runtimes, true),
            '^' => self.copy_mode_first_non_blank(terminal_runtimes),
            'w' => self.copy_mode_word_motion(terminal_runtimes, WordMotion::NextStart),
            'b' => self.copy_mode_word_motion(terminal_runtimes, WordMotion::PreviousStart),
            'e' => self.copy_mode_word_motion(terminal_runtimes, WordMotion::NextEnd),
            'W' => self.copy_mode_word_motion(terminal_runtimes, WordMotion::NextStart),
            'E' => self.copy_mode_word_motion(terminal_runtimes, WordMotion::NextEnd),
            'B' => self.copy_mode_word_motion(terminal_runtimes, WordMotion::PreviousStart),
            'f' => self.copy_mode_find(terminal_runtimes, CopyModeFindKind::Forward),
            'F' => self.copy_mode_find(terminal_runtimes, CopyModeFindKind::Backward),
            't' => self.copy_mode_find(terminal_runtimes, CopyModeFindKind::TillForward),
            'T' => self.copy_mode_find(terminal_runtimes, CopyModeFindKind::TillBackward),
            'A' => self.exit_copy_mode(terminal_runtimes, false),
            '{' => self.copy_mode_paragraph(terminal_runtimes, -1),
            '}' => self.copy_mode_paragraph(terminal_runtimes, 1),
            _ => {}
        }
    }

    pub(crate) fn cancel_copy_mode(&mut self, terminal_runtimes: &TerminalRuntimeRegistry) {
        self.exit_copy_mode(terminal_runtimes, false);
    }

    fn exit_copy_mode(&mut self, terminal_runtimes: &TerminalRuntimeRegistry, copy: bool) {
        let restore_scroll = self
            .copy_mode
            .map(|copy_mode| (copy_mode.pane_id, copy_mode.entry_offset_from_bottom));
        if copy {
            self.copy_selection(terminal_runtimes);
        } else {
            self.clear_selection();
        }
        if let Some((pane_id, offset_from_bottom)) = restore_scroll {
            self.set_pane_scroll_offset(terminal_runtimes, pane_id, offset_from_bottom);
        }
        self.copy_mode = None;
        self.mode = if self.active.is_some() {
            Mode::Terminal
        } else {
            Mode::Navigate
        };
    }

    fn begin_copy_mode_selection(&mut self, terminal_runtimes: &TerminalRuntimeRegistry) {
        let Some(copy_mode) = self.copy_mode else {
            return;
        };
        let Some(info) = self.pane_info_by_id(copy_mode.pane_id).cloned() else {
            return;
        };
        if copy_mode.cursor_row >= info.inner_rect.height
            || copy_mode.cursor_col >= info.inner_rect.width
        {
            return;
        }

        let metrics = self.pane_scroll_metrics(terminal_runtimes, copy_mode.pane_id);
        self.selection = Some(Selection::anchor(
            copy_mode.pane_id,
            copy_mode.cursor_row,
            copy_mode.cursor_col,
            metrics,
        ));
        if let Some(copy_mode) = self.copy_mode.as_mut() {
            copy_mode.selection = Some(CopyModeSelection::Character);
        }
    }

    fn select_copy_mode_line(&mut self, terminal_runtimes: &TerminalRuntimeRegistry) {
        let Some(mut copy_mode) = self.copy_mode else {
            return;
        };
        let Some(info) = self.pane_info_by_id(copy_mode.pane_id) else {
            return;
        };
        let end_col = info.inner_rect.width.saturating_sub(1);
        let metrics = self.pane_scroll_metrics(terminal_runtimes, copy_mode.pane_id);
        let anchor_row = Selection::absolute_row_for_viewport(copy_mode.cursor_row, metrics);
        self.selection = Some(Selection::line_range(
            copy_mode.pane_id,
            anchor_row,
            anchor_row,
            end_col,
        ));
        copy_mode.selection = Some(CopyModeSelection::Linewise { anchor_row });
        self.copy_mode = Some(copy_mode);
    }

    fn move_copy_cursor(
        &mut self,
        terminal_runtimes: &TerminalRuntimeRegistry,
        row_delta: i16,
        col_delta: i16,
    ) {
        let Some(mut copy_mode) = self.copy_mode else {
            return;
        };
        let Some(info) = self.pane_info_by_id(copy_mode.pane_id).cloned() else {
            self.exit_copy_mode(terminal_runtimes, false);
            return;
        };

        if col_delta < 0 {
            copy_mode.cursor_col = copy_mode
                .cursor_col
                .saturating_sub(col_delta.unsigned_abs());
        } else if col_delta > 0 {
            copy_mode.cursor_col = copy_mode
                .cursor_col
                .saturating_add(col_delta as u16)
                .min(info.inner_rect.width.saturating_sub(1));
        }

        if row_delta < 0 {
            let delta = row_delta.unsigned_abs();
            if copy_mode.cursor_row >= delta {
                copy_mode.cursor_row -= delta;
            } else {
                self.scroll_pane_up(terminal_runtimes, copy_mode.pane_id, usize::from(delta));
                copy_mode.cursor_row = 0;
            }
        } else if row_delta > 0 {
            let delta = row_delta as u16;
            let bottom = info.inner_rect.height.saturating_sub(1);
            if copy_mode.cursor_row.saturating_add(delta) <= bottom {
                copy_mode.cursor_row += delta;
            } else {
                self.scroll_pane_down(terminal_runtimes, copy_mode.pane_id, usize::from(delta));
                copy_mode.cursor_row = bottom;
            }
        }

        self.copy_mode = Some(copy_mode);
        self.sync_copy_mode_selection(terminal_runtimes);
    }

    fn scroll_copy_mode_page(
        &mut self,
        terminal_runtimes: &TerminalRuntimeRegistry,
        direction: i16,
        half_page: bool,
    ) {
        let Some(mut copy_mode) = self.copy_mode else {
            return;
        };
        let Some(info) = self.pane_info_by_id(copy_mode.pane_id).cloned() else {
            self.exit_copy_mode(terminal_runtimes, false);
            return;
        };
        let lines = copy_mode_page_lines(info.inner_rect.height, half_page);
        if let Some(metrics) = self.pane_scroll_metrics(terminal_runtimes, copy_mode.pane_id) {
            if direction < 0 {
                let next_offset = metrics.offset_from_bottom.saturating_add(lines);
                if next_offset > metrics.max_offset_from_bottom {
                    let scrolled_lines = metrics
                        .max_offset_from_bottom
                        .saturating_sub(metrics.offset_from_bottom);
                    let cursor_lines = lines.saturating_sub(scrolled_lines);
                    self.set_pane_scroll_offset(
                        terminal_runtimes,
                        copy_mode.pane_id,
                        metrics.max_offset_from_bottom,
                    );
                    copy_mode.cursor_row = copy_mode
                        .cursor_row
                        .saturating_sub(cursor_lines.min(u16::MAX as usize) as u16);
                } else {
                    self.set_pane_scroll_offset(terminal_runtimes, copy_mode.pane_id, next_offset);
                }
            } else if metrics.offset_from_bottom < lines {
                let cursor_lines = lines.saturating_sub(metrics.offset_from_bottom);
                self.set_pane_scroll_offset(terminal_runtimes, copy_mode.pane_id, 0);
                copy_mode.cursor_row = copy_mode
                    .cursor_row
                    .saturating_add(cursor_lines.min(u16::MAX as usize) as u16)
                    .min(info.inner_rect.height.saturating_sub(1));
            } else {
                self.set_pane_scroll_offset(
                    terminal_runtimes,
                    copy_mode.pane_id,
                    metrics.offset_from_bottom - lines,
                );
            }
        } else if direction < 0 {
            self.scroll_pane_up(terminal_runtimes, copy_mode.pane_id, lines);
        } else {
            self.scroll_pane_down(terminal_runtimes, copy_mode.pane_id, lines);
        }
        self.copy_mode = Some(copy_mode);
        self.sync_copy_mode_selection(terminal_runtimes);
    }

    fn copy_mode_history_top(&mut self, terminal_runtimes: &TerminalRuntimeRegistry) {
        let Some(mut copy_mode) = self.copy_mode else {
            return;
        };
        let Some(metrics) = self.pane_scroll_metrics(terminal_runtimes, copy_mode.pane_id) else {
            return;
        };
        self.set_pane_scroll_offset(
            terminal_runtimes,
            copy_mode.pane_id,
            metrics.max_offset_from_bottom,
        );
        copy_mode.cursor_row = 0;
        self.copy_mode = Some(copy_mode);
        self.sync_copy_mode_selection(terminal_runtimes);
    }

    fn copy_mode_history_bottom(&mut self, terminal_runtimes: &TerminalRuntimeRegistry) {
        let Some(mut copy_mode) = self.copy_mode else {
            return;
        };
        let Some(info) = self.pane_info_by_id(copy_mode.pane_id) else {
            self.exit_copy_mode(terminal_runtimes, false);
            return;
        };
        self.set_pane_scroll_offset(terminal_runtimes, copy_mode.pane_id, 0);
        copy_mode.cursor_row = info.inner_rect.height.saturating_sub(1);
        self.copy_mode = Some(copy_mode);
        self.sync_copy_mode_selection(terminal_runtimes);
    }

    fn copy_mode_line_edge(&mut self, terminal_runtimes: &TerminalRuntimeRegistry, end: bool) {
        let Some(mut copy_mode) = self.copy_mode else {
            return;
        };
        let Some(info) = self.pane_info_by_id(copy_mode.pane_id) else {
            self.exit_copy_mode(terminal_runtimes, false);
            return;
        };
        copy_mode.cursor_col = if end {
            info.inner_rect.width.saturating_sub(1)
        } else {
            0
        };
        self.copy_mode = Some(copy_mode);
        self.sync_copy_mode_selection(terminal_runtimes);
    }

    fn copy_mode_first_non_blank(&mut self, terminal_runtimes: &TerminalRuntimeRegistry) {
        let Some(mut copy_mode) =

... [OUTPUT TRUNCATED - 23892 chars omitted out of 73892 total] ...

r('a');
        app.state.prefix_mods = KeyModifiers::CONTROL;
        app.state.enter_copy_mode(&app.terminal_runtimes);
        let height = app.state.copy_mode.expect("copy mode").cursor_row + 1;
        let expected_lines = copy_mode_page_lines(height, false);

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('b'), KeyModifiers::CONTROL));

        assert_eq!(app.state.mode, Mode::Copy);
        assert_eq!(copy_mode_offset_from_bottom(&app, pane_id), expected_lines);
    }

    #[tokio::test]
    async fn copy_mode_prefix_takes_priority_over_ctrl_b_page_up() {
        let bytes = numbered_lines_bytes(64);
        let (mut app, pane_id) = app_with_copy_scrollback(&bytes);
        app.state.enter_copy_mode(&app.terminal_runtimes);

        app.handle_key(TerminalKey::new(
            app.state.prefix_code,
            app.state.prefix_mods,
        ))
        .await;

        assert_eq!(app.state.mode, Mode::Prefix);
        assert_eq!(copy_mode_offset_from_bottom(&app, pane_id), 0);
        assert!(app.state.copy_mode.is_some());
    }

    #[tokio::test]
    async fn copy_mode_prefix_escape_returns_to_copy_mode() {
        let (mut app, _) = app_with_copy_screen(b"alpha\nbeta\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        let copy_mode = app.state.copy_mode.expect("copy mode");

        app.handle_key(TerminalKey::new(
            app.state.prefix_code,
            app.state.prefix_mods,
        ))
        .await;
        app.handle_key(TerminalKey::new(KeyCode::Esc, KeyModifiers::empty()))
            .await;

        assert_eq!(app.state.mode, Mode::Copy);
        assert_eq!(app.state.copy_mode, Some(copy_mode));
    }

    #[tokio::test]
    async fn copy_mode_prefix_focus_keeps_copy_mode_on_source_pane() {
        let (mut app, first_pane, second_pane) = app_with_split_copy_screen(b"alpha\nbeta\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        let copy_mode = app.state.copy_mode.expect("copy mode");
        assert_eq!(copy_mode.pane_id, first_pane);

        app.handle_key(TerminalKey::new(
            app.state.prefix_code,
            app.state.prefix_mods,
        ))
        .await;
        app.handle_key(TerminalKey::new(KeyCode::Char('l'), KeyModifiers::empty()))
            .await;

        assert_eq!(app.state.mode, Mode::Terminal);
        assert_eq!(app.state.copy_mode, Some(copy_mode));
        assert_eq!(
            app.state.workspaces[0].tabs[0].layout.focused(),
            second_pane
        );

        refresh_split_pane_infos(&mut app);
        app.handle_key(TerminalKey::new(
            app.state.prefix_code,
            app.state.prefix_mods,
        ))
        .await;
        app.handle_key(TerminalKey::new(KeyCode::Char('h'), KeyModifiers::empty()))
            .await;

        assert_eq!(app.state.mode, Mode::Copy);
        assert_eq!(app.state.copy_mode, Some(copy_mode));
        assert_eq!(app.state.workspaces[0].tabs[0].layout.focused(), first_pane);
    }

    #[tokio::test]
    async fn copy_mode_focus_away_preserves_scrollback_position() {
        let bytes = numbered_lines_bytes(64);
        let (mut app, first_pane, second_pane) = app_with_split_copy_scrollback(&bytes);
        app.state.enter_copy_mode(&app.terminal_runtimes);
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::PageUp, KeyModifiers::empty()));
        let scrolled_offset = copy_mode_offset_from_bottom(&app, first_pane);
        assert!(scrolled_offset > 0);

        app.handle_key(TerminalKey::new(
            app.state.prefix_code,
            app.state.prefix_mods,
        ))
        .await;
        app.handle_key(TerminalKey::new(KeyCode::Char('l'), KeyModifiers::empty()))
            .await;

        assert_eq!(app.state.mode, Mode::Terminal);
        assert_eq!(
            app.state.workspaces[0].tabs[0].layout.focused(),
            second_pane
        );
        assert_eq!(
            copy_mode_offset_from_bottom(&app, first_pane),
            scrolled_offset
        );
    }

    #[tokio::test]
    async fn copy_mode_cancel_restores_scroll_after_workspace_switch() {
        let bytes = numbered_lines_bytes(64);
        let (mut app, pane_id) = app_with_copy_scrollback(&bytes);
        app.state
            .workspaces
            .push(crate::workspace::Workspace::test_new("other"));
        app.state.enter_copy_mode(&app.terminal_runtimes);
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::PageUp, KeyModifiers::empty()));
        assert!(copy_mode_offset_from_bottom(&app, pane_id) > 0);

        app.state.switch_workspace(1);
        app.state.cancel_copy_mode(&app.terminal_runtimes);

        assert_eq!(copy_mode_offset_from_bottom(&app, pane_id), 0);
        assert!(app.state.copy_mode.is_none());
        assert_eq!(app.state.mode, Mode::Terminal);
    }

    #[tokio::test]
    async fn copy_mode_clears_when_source_tab_closes_after_focus_away() {
        let (mut app, first_pane, _) = app_with_split_copy_screen(b"alpha\nbeta\n");
        let survivor_tab = app.state.workspaces[0].test_add_tab(Some("survivor"));
        let survivor_pane = app.state.workspaces[0].tabs[survivor_tab].root_pane;
        let survivor_terminal = app.state.workspaces[0].tabs[survivor_tab].panes[&survivor_pane]
            .attached_terminal_id
            .clone();
        app.state.terminals.insert(
            survivor_terminal.clone(),
            crate::terminal::TerminalState::new(survivor_terminal, "/tmp".into()),
        );
        app.state.enter_copy_mode(&app.terminal_runtimes);
        assert_eq!(app.state.copy_mode.expect("copy mode").pane_id, first_pane);

        app.handle_key(TerminalKey::new(
            app.state.prefix_code,
            app.state.prefix_mods,
        ))
        .await;
        app.handle_key(TerminalKey::new(KeyCode::Char('l'), KeyModifiers::empty()))
            .await;
        assert_eq!(app.state.mode, Mode::Terminal);
        assert!(app.state.copy_mode.is_some());

        assert!(!app.state.close_tab());

        assert_eq!(app.state.mode, Mode::Terminal);
        assert!(app.state.copy_mode.is_none());
        app.state.assert_invariants_for_test();
    }

    #[tokio::test]
    async fn copy_mode_ctrl_f_uses_page_down() {
        let bytes = numbered_lines_bytes(64);
        let (mut app, pane_id) = app_with_copy_scrollback(&bytes);
        app.state.enter_copy_mode(&app.terminal_runtimes);
        let height = app.state.copy_mode.expect("copy mode").cursor_row + 1;
        let page_lines = copy_mode_page_lines(height, false);
        app.state
            .set_pane_scroll_offset(&app.terminal_runtimes, pane_id, page_lines);

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('f'), KeyModifiers::CONTROL));

        assert_eq!(app.state.mode, Mode::Copy);
        assert_eq!(copy_mode_offset_from_bottom(&app, pane_id), 0);
    }

    #[tokio::test]
    async fn copy_mode_word_motions_use_visible_row_words() {
        let (mut app, _) = app_with_copy_screen(b"foo bar baz\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = 0;
            copy_mode.cursor_col = 0;
        }

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('w'), KeyModifiers::empty()));
        assert_eq!(app.state.copy_mode.expect("copy mode").cursor_col, 4);

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('e'), KeyModifiers::empty()));
        assert_eq!(app.state.copy_mode.expect("copy mode").cursor_col, 6);

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('b'), KeyModifiers::empty()));
        assert_eq!(app.state.copy_mode.expect("copy mode").cursor_col, 4);
    }

    #[tokio::test]
    async fn copy_mode_shift_v_y_copies_visible_line() {
        let (mut app, _) = app_with_copy_screen(b"alpha\r\nbeta\r\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = 1;
            copy_mode.cursor_col = 2;
        }

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('v'), KeyModifiers::SHIFT));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('y'), KeyModifiers::empty()));

        assert_eq!(copy_mode_clipboard_text(&mut app), "beta");
        assert_eq!(app.state.mode, Mode::Terminal);
    }

    #[tokio::test]
    async fn copy_mode_shift_v_extends_linewise_down() {
        let (mut app, _) = app_with_copy_screen(b"alpha\r\nbeta\r\ngamma\r\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = 0;
            copy_mode.cursor_col = 2;
        }

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('v'), KeyModifiers::SHIFT));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('j'), KeyModifiers::empty()));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('y'), KeyModifiers::empty()));

        assert_eq!(copy_mode_clipboard_text(&mut app), "alpha\nbeta");
    }

    #[tokio::test]
    async fn copy_mode_shift_v_extends_linewise_up() {
        let (mut app, _) = app_with_copy_screen(b"alpha\r\nbeta\r\ngamma\r\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = 1;
            copy_mode.cursor_col = 2;
        }

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('v'), KeyModifiers::SHIFT));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('k'), KeyModifiers::empty()));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('y'), KeyModifiers::empty()));

        assert_eq!(copy_mode_clipboard_text(&mut app), "alpha\nbeta");
    }

    #[tokio::test]
    async fn copy_mode_shift_v_reverses_without_character_tail() {
        let (mut app, _) = app_with_copy_screen(b"alpha\r\nbeta\r\ngamma\r\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = 1;
            copy_mode.cursor_col = 2;
        }

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('v'), KeyModifiers::SHIFT));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('j'), KeyModifiers::empty()));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('k'), KeyModifiers::empty()));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('k'), KeyModifiers::empty()));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('y'), KeyModifiers::empty()));

        assert_eq!(copy_mode_clipboard_text(&mut app), "alpha\nbeta");
    }

    #[tokio::test]
    async fn copy_mode_shift_v_horizontal_motion_keeps_linewise_selection() {
        let (mut app, _) = app_with_copy_screen(b"alpha\r\nbeta\r\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = 1;
            copy_mode.cursor_col = 2;
        }

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('v'), KeyModifiers::SHIFT));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('h'), KeyModifiers::empty()));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('l'), KeyModifiers::empty()));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('y'), KeyModifiers::empty()));

        assert_eq!(copy_mode_clipboard_text(&mut app), "beta");
    }

    #[tokio::test]
    async fn copy_mode_shift_v_page_up_keeps_linewise_scrollback_selection() {
        let bytes = numbered_lines_bytes(64);
        let (mut app, pane_id) = app_with_copy_scrollback(&bytes);
        app.state.enter_copy_mode(&app.terminal_runtimes);
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = 0;
            copy_mode.cursor_col = 2;
        }

        let anchor_row = copy_mode_viewport_top_row(&app, pane_id);
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('v'), KeyModifiers::SHIFT));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::PageUp, KeyModifiers::empty()));
        let cursor_row = copy_mode_viewport_top_row(&app, pane_id);
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('y'), KeyModifiers::empty()));

        assert!(cursor_row < anchor_row);
        let expected = (cursor_row..=anchor_row)
            .map(|row| format!("{row:06}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(copy_mode_clipboard_text(&mut app), expected);
        assert_eq!(copy_mode_offset_from_bottom(&app, pane_id), 0);
    }

    #[tokio::test]
    async fn copy_mode_page_up_uses_tmux_page_size() {
        let bytes = numbered_lines_bytes(64);
        let (mut app, pane_id) = app_with_copy_scrollback(&bytes);
        app.state.enter_copy_mode(&app.terminal_runtimes);
        let height = app.state.copy_mode.expect("copy mode").cursor_row + 1;
        let expected_lines = copy_mode_page_lines(height, false);

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::PageUp, KeyModifiers::empty()));

        assert_eq!(copy_mode_offset_from_bottom(&app, pane_id), expected_lines);
    }

    #[tokio::test]
    async fn copy_mode_ctrl_u_moves_cursor_when_history_top_clamps() {
        let bytes = numbered_lines_bytes(64);
        let (mut app, pane_id) = app_with_copy_scrollback(&bytes);
        app.state.enter_copy_mode(&app.terminal_runtimes);
        let bottom = app.state.copy_mode.expect("copy mode").cursor_row;
        let lines = copy_mode_page_lines(bottom + 1, true);
        let metrics = copy_mode_scroll_metrics(&app, pane_id);
        assert!(metrics.max_offset_from_bottom >= lines);
        app.state.set_pane_scroll_offset(
            &app.terminal_runtimes,
            pane_id,
            metrics.max_offset_from_bottom - lines + 1,
        );
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = bottom;
        }

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('u'), KeyModifiers::CONTROL));

        let copy_mode = app.state.copy_mode.expect("copy mode");
        let expected_cursor_delta = 1;
        assert_eq!(
            copy_mode_offset_from_bottom(&app, pane_id),
            metrics.max_offset_from_bottom
        );
        assert_eq!(
            copy_mode.cursor_row,
            bottom.saturating_sub(expected_cursor_delta as u16)
        );
    }

    #[tokio::test]
    async fn copy_mode_ctrl_d_moves_cursor_when_live_bottom_clamps() {
        let bytes = numbered_lines_bytes(64);
        let (mut app, pane_id) = app_with_copy_scrollback(&bytes);
        app.state.enter_copy_mode(&app.terminal_runtimes);
        let bottom = app.state.copy_mode.expect("copy mode").cursor_row;
        let lines = copy_mode_page_lines(bottom + 1, true);
        assert!(lines > 1);
        app.state
            .set_pane_scroll_offset(&app.terminal_runtimes, pane_id, lines - 1);
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = 0;
        }

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('d'), KeyModifiers::CONTROL));

        let copy_mode = app.state.copy_mode.expect("copy mode");
        assert_eq!(copy_mode_offset_from_bottom(&app, pane_id), 0);
        assert_eq!(copy_mode.cursor_row, 1);
    }

    #[tokio::test]
    async fn copy_mode_q_exits_and_returns_to_bottom_after_scrollback() {
        let bytes = numbered_lines_bytes(64);
        let (mut app, pane_id) = app_with_copy_scrollback(&bytes);
        app.state.enter_copy_mode(&app.terminal_runtimes);

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::PageUp, KeyModifiers::empty()));
        assert!(copy_mode_offset_from_bottom(&app, pane_id) > 0);

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('q'), KeyModifiers::empty()));

        assert_eq!(app.state.mode, Mode::Terminal);
        assert!(app.state.copy_mode.is_none());
        assert_eq!(copy_mode_offset_from_bottom(&app, pane_id), 0);
    }

    #[tokio::test]
    async fn copy_mode_q_restores_entry_scrollback_offset() {
        let bytes = numbered_lines_bytes(64);
        let (mut app, pane_id) = app_with_copy_scrollback(&bytes);
        let entry_offset = 3;
        app.state
            .set_pane_scroll_offset(&app.terminal_runtimes, pane_id, entry_offset);
        assert_eq!(copy_mode_offset_from_bottom(&app, pane_id), entry_offset);

        app.state.enter_copy_mode(&app.terminal_runtimes);
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::PageUp, KeyModifiers::empty()));
        assert!(copy_mode_offset_from_bottom(&app, pane_id) > entry_offset);

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('q'), KeyModifiers::empty()));

        assert_eq!(app.state.mode, Mode::Terminal);
        assert!(app.state.copy_mode.is_none());
        assert_eq!(copy_mode_offset_from_bottom(&app, pane_id), entry_offset);
    }

    #[tokio::test]
    async fn shifted_punctuation_keys_work_with_enhanced_key_reporting() {
        let (mut app, _) = app_with_copy_screen(b"foo\r\n\r\nbar\r\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = 2;
            copy_mode.cursor_col = 2;
        }

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('6'), KeyModifiers::SHIFT));
        assert_eq!(app.state.copy_mode.expect("copy mode").cursor_col, 0);

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char(']'), KeyModifiers::SHIFT));
        assert_eq!(app.state.copy_mode.expect("copy mode").cursor_row, 3);

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('['), KeyModifiers::SHIFT));
        assert_eq!(app.state.copy_mode.expect("copy mode").cursor_row, 1);

        app.handle_copy_mode_key(
            TerminalKey::new(KeyCode::Char(']'), KeyModifiers::SHIFT)
                .with_shifted_codepoint('}' as u32),
        );
        assert_eq!(app.state.copy_mode.expect("copy mode").cursor_row, 3);
    }

    #[tokio::test]
    async fn copy_mode_v_y_copies_selection_and_exits() {
        let (mut app, _) = app_with_copy_screen(b"alpha\nbeta\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = 0;
            copy_mode.cursor_col = 0;
        }
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('v'), KeyModifiers::empty()));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('l'), KeyModifiers::empty()));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('l'), KeyModifiers::empty()));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('y'), KeyModifiers::empty()));

        match app.event_rx.try_recv().expect("clipboard event") {
            AppEvent::ClipboardWrite { content } => assert_eq!(content, b"alp"),
            other => panic!("unexpected event: {other:?}"),
        }
        assert_eq!(app.state.mode, Mode::Terminal);
        assert!(app.state.copy_mode.is_none());
    }
\\\\\\\        to: molxoysr 5b4450c9 "docs: remove star history embed" (rebase destination)

    #[tokio::test]
    async fn copy_mode_same_tab_switch_preserves_selection() {
        let (mut app, _) = app_with_copy_screen(b"alpha\nbeta\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        if let Some(copy_mode) = app.state.copy_mode.as_mut() {
            copy_mode.cursor_row = 0;
            copy_mode.cursor_col = 0;
        }
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('v'), KeyModifiers::empty()));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('l'), KeyModifiers::empty()));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('l'), KeyModifiers::empty()));

        assert!(app.state.switch_workspace_tab(0, 0));

        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('y'), KeyModifiers::empty()));
        assert_eq!(copy_mode_clipboard_text(&mut app), "alp");
        assert_eq!(app.state.mode, Mode::Terminal);
        assert!(app.state.copy_mode.is_none());
    }


    // -- find-char helper unit tests --

    #[test]
    fn forward_find_col_finds_after_cursor() {
        assert_eq!(forward_find_col("hello world", 0, 'h'), None); // cursor on 'h', skip self
        assert_eq!(forward_find_col("hello world", 0, 'e'), Some(1));
        assert_eq!(forward_find_col("hello world", 2, 'l'), Some(3)); // third l
        assert_eq!(forward_find_col("hello world", 0, 'w'), Some(6));
    }

    #[test]
    fn forward_find_col_returns_none_when_not_found() {
        assert_eq!(forward_find_col("hello world", 0, 'z'), None);
        assert_eq!(forward_find_col("hello world", 0, 'h'), None); // cursor on it
    }

    #[test]
    fn forward_find_col_handles_multibyte() {
        // "héllo" — é is 1 cell wide in unicode-width
        assert_eq!(forward_find_col("héllo", 0, 'l'), Some(2));
    }

    #[test]
    fn backward_find_col_finds_before_cursor() {
        assert_eq!(backward_find_col("hello world", 6, 'o'), Some(4));
        assert_eq!(backward_find_col("hello world", 5, 'l'), Some(3));
        assert_eq!(backward_find_col("hello world", 1, 'h'), Some(0));
    }

    #[test]
    fn backward_find_col_returns_none_when_not_found() {
        assert_eq!(backward_find_col("hello world", 6, 'z'), None);
        assert_eq!(backward_find_col("hello world", 6, 'w'), None); // cursor past 'w'
    }

    #[test]
    fn backward_find_col_finds_closest_before_cursor() {
        // Multiple 'l's before cursor 5 (positions 2, 3), closest is 3
        assert_eq!(backward_find_col("hello world", 5, 'l'), Some(3));
    }

    // -- copy mode find-char integration tests --

    fn set_cursor(app: &mut App, row: u16, col: u16) {
        if let Some(cm) = app.state.copy_mode.as_mut() {
            cm.cursor_row = row;
            cm.cursor_col = col;
        }
    }

    #[tokio::test]
    async fn copy_mode_f_finds_char_forward() {
        let (mut app, _) = app_with_copy_screen(b"hello world\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        set_cursor(&mut app, 0, 0);

        // Press f then 'w'
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('f'), KeyModifiers::empty()));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('w'), KeyModifiers::empty()));

        assert_eq!(app.state.copy_mode.expect("copy mode").cursor_col, 6);
    }

    #[tokio::test]
    async fn copy_mode_f_no_match_stays_put() {
        let (mut app, _) = app_with_copy_screen(b"hello world\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        set_cursor(&mut app, 0, 0);

        // Press f then 'z' (not present)
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('f'), KeyModifiers::empty()));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('z'), KeyModifiers::empty()));

        assert_eq!(app.state.copy_mode.expect("copy mode").cursor_col, 0);
    }

    #[tokio::test]
    async fn copy_mode_t_till_char_places_before_target() {
        let (mut app, _) = app_with_copy_screen(b"hello world\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        set_cursor(&mut app, 0, 0);

        // Press t then 'w' — should land on space (col 5)
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('t'), KeyModifiers::empty()));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('w'), KeyModifiers::empty()));

        assert_eq!(app.state.copy_mode.expect("copy mode").cursor_col, 5);
    }

    #[tokio::test]
    async fn copy_mode_F_finds_char_backward() {
        let (mut app, _) = app_with_copy_screen(b"hello world\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        set_cursor(&mut app, 0, 6);

        // Press F then 'h'
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('F'), KeyModifiers::SHIFT));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('h'), KeyModifiers::empty()));

        assert_eq!(app.state.copy_mode.expect("copy mode").cursor_col, 0);
    }

    #[tokio::test]
    async fn copy_mode_T_places_after_target() {
        let (mut app, _) = app_with_copy_screen(b"hello world\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);
        set_cursor(&mut app, 0, 6);

        // Press T then 'h' — should land on col 1 (after h)
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('T'), KeyModifiers::SHIFT));
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('h'), KeyModifiers::empty()));

        assert_eq!(app.state.copy_mode.expect("copy mode").cursor_col, 1);
    }

    #[tokio::test]
    async fn copy_mode_y_exits_and_returns_to_terminal() {
        let (mut app, _) = app_with_copy_screen(b"hello\n");
        app.state.enter_copy_mode(&app.terminal_runtimes);

        // Press y — simplified: exits without selecting
        app.handle_copy_mode_key(TerminalKey::new(KeyCode::Char('y'), KeyModifiers::empty()));

        assert_eq!(app.state.mode, Mode::Terminal);
        assert!(app.state.copy_mode.is_none());
    }
}