use super::state::{
    ApplyConfirmDialog, ApplyResultDialog, DetailedResolutionEditor, DetailsDialog, EditorField,
    ExportConfirmDialog, ExportDialog, ImportDialog, ModalButton, StandardResolutionEditor,
    SystemOperation,
};
use super::support::{center_label, centered_rect, focus_style, inner_rect, row_style};
use super::{
    App, ExtensionRow, FocusArea, GlobalAction, HitTarget, Hitbox, ModeKey, ResolutionSection,
    SectionAction, keep_selected_visible, scroll_title,
};
use crate::workspace::format_location;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

impl App {
    pub(super) fn draw(&mut self, frame: &mut Frame<'_>) {
        self.hitboxes.clear();

        let [top_bar, main_area, status_bar, bottom_bar] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(16),
            Constraint::Length(2),
            Constraint::Length(3),
        ])
        .areas(frame.area());

        self.draw_monitor_selector(frame, top_bar);

        let (established, detailed, standard, extension) = if main_area.width >= 100 {
            let [left_column, right_column] =
                Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)])
                    .areas(main_area);
            let [detailed, standard, extension] = Layout::vertical([
                Constraint::Ratio(1, 3),
                Constraint::Ratio(1, 3),
                Constraint::Ratio(1, 3),
            ])
            .areas(right_column);
            (left_column, detailed, standard, extension)
        } else {
            let [established, detailed, standard, extension] = Layout::vertical([
                Constraint::Ratio(1, 4),
                Constraint::Ratio(1, 4),
                Constraint::Ratio(1, 4),
                Constraint::Ratio(1, 4),
            ])
            .areas(main_area);
            (established, detailed, standard, extension)
        };
        self.draw_established_panel(frame, established);
        self.draw_resolution_section(frame, detailed, ResolutionSection::Detailed);
        self.draw_resolution_section(frame, standard, ResolutionSection::Standard);
        self.draw_resolution_section(frame, extension, ResolutionSection::Extension);

        frame.render_widget(
            Paragraph::new(self.status.as_str())
                .block(Block::default().borders(Borders::TOP))
                .wrap(Wrap { trim: true }),
            status_bar,
        );
        self.draw_global_buttons(frame, bottom_bar);

        if let Some(editor) = self.detailed_editor.clone() {
            self.draw_detailed_editor(frame, &editor);
        }
        if let Some(editor) = self.standard_editor.clone() {
            self.draw_standard_editor(frame, &editor);
        }
        if let Some(dialog) = self.import_dialog.clone() {
            self.draw_import_dialog(frame, &dialog);
        }
        if let Some(dialog) = self.details_dialog.clone() {
            self.draw_details_dialog(frame, &dialog);
        }
        if let Some(dialog) = self.export_confirm_dialog.clone() {
            self.draw_export_confirm_dialog(frame, &dialog);
        }
        if let Some(dialog) = self.export_dialog.clone() {
            self.draw_export_dialog(frame, &dialog);
        }
        if let Some(dialog) = self.apply_confirm_dialog.clone() {
            self.draw_apply_confirm_dialog(frame, &dialog);
        }
        if self.applying_in_progress {
            self.draw_apply_progress(frame);
        }
        if let Some(dialog) = self.apply_result_dialog.clone() {
            self.draw_apply_result_dialog(frame, &dialog);
        }
    }

    fn draw_monitor_selector(&mut self, frame: &mut Frame<'_>, area: Rect) {
        if area.width < 90 {
            let [selector_area, override_area] =
                Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)])
                    .areas(area);
            let label = self
                .selected_monitor()
                .map(|monitor| monitor.label())
                .unwrap_or_else(|| "No monitors".to_string());
            frame.render_widget(
                Paragraph::new(format!(" Monitor: {label}  ↕"))
                    .block(Block::default().borders(Borders::ALL))
                    .style(focus_style(self.focus == FocusArea::Monitor)),
                selector_area,
            );
            self.push_hitbox(selector_area, HitTarget::MonitorSelector, 0);
            let override_label = self
                .selected_override_status()
                .map(|status| status.short_label())
                .unwrap_or("not installed");
            frame.render_widget(
                Paragraph::new(format!(" EDID: {override_label}"))
                    .block(Block::default().borders(Borders::ALL)),
                override_area,
            );
            return;
        }

        let [label_area, selector_area, mode_area, override_area] = Layout::horizontal([
            Constraint::Length(12),
            Constraint::Min(20),
            Constraint::Length(28),
            Constraint::Length(31),
        ])
        .areas(area);
        frame.render_widget(Paragraph::new("Monitor:"), label_area);

        let label = self
            .selected_monitor()
            .map(|m| m.label())
            .unwrap_or_else(|| "No monitors".to_string());
        frame.render_widget(
            Paragraph::new(format!(" {}  v", label))
                .block(Block::default().borders(Borders::ALL))
                .style(focus_style(self.focus == FocusArea::Monitor)),
            selector_area,
        );
        self.push_hitbox(selector_area, HitTarget::MonitorSelector, 0);

        frame.render_widget(
            Paragraph::new(format!(" {}", self.selected_live_mode_label()))
                .block(Block::default().borders(Borders::ALL)),
            mode_area,
        );

        let override_label = self
            .selected_override_status()
            .map(|status| status.short_label())
            .unwrap_or("not installed");
        frame.render_widget(
            Paragraph::new(format!(" EDID override: {override_label}"))
                .block(Block::default().borders(Borders::ALL)),
            override_area,
        );
    }

    fn selected_live_mode_label(&self) -> String {
        let Some(hypr) = self
            .selected_monitor()
            .and_then(|monitor| monitor.hyprland.as_ref())
        else {
            return "Live: not reported".to_string();
        };

        let (Some(width), Some(height), Some(refresh)) =
            (hypr.active_width, hypr.active_height, hypr.refresh_hz)
        else {
            return "Live: unknown".to_string();
        };

        format!("Live: {width}x{height}@{refresh:.2}")
    }

    fn draw_established_panel(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let inner = inner_rect(area);
        let len = self
            .selected_edid()
            .map(|edid| edid.established_timings.len())
            .unwrap_or_default();
        keep_selected_visible(
            self.selected_established,
            &mut self.established_scroll,
            len,
            inner.height,
        );
        let title = scroll_title(
            "Established resolutions",
            self.established_scroll,
            len,
            inner.height,
        );
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(focus_style(self.focus == FocusArea::Established));
        frame.render_widget(block, area);

        if let Some(edid) = self.selected_edid() {
            let timings = edid.established_timings.clone();
            for (visible, (index, timing)) in timings
                .iter()
                .enumerate()
                .skip(self.established_scroll)
                .take(usize::from(inner.height))
                .enumerate()
            {
                let row = Rect {
                    x: inner.x,
                    y: inner.y + visible as u16,
                    width: inner.width,
                    height: 1,
                };
                let selected = self.selected_established == Some(index);
                let key = ModeKey::new(
                    timing.width,
                    timing.height,
                    f64::from(timing.refresh_hz),
                    false,
                );
                let text = format!(
                    "{} {}x{} @ {} Hz{}",
                    if selected { ">" } else { " " },
                    timing.width,
                    timing.height,
                    timing.refresh_hz,
                    self.provenance_suffix(key)
                );
                frame.render_widget(Paragraph::new(text).style(row_style(selected)), row);
                self.push_hitbox(row, HitTarget::EstablishedRow(index), 0);
            }
        }
    }

    fn draw_resolution_section(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        section: ResolutionSection,
    ) {
        let show_buttons = area.height >= 8 && area.width >= 58;
        let [list_area, buttons_area] = Layout::vertical([
            Constraint::Min(3),
            Constraint::Length(if show_buttons { 3 } else { 0 }),
        ])
        .areas(area);
        let inner = inner_rect(list_area);
        self.sync_section_scroll(section, inner.height);
        let title = self.section_title(section, inner.height);
        let focused = matches!(
            (self.focus, section),
            (FocusArea::Detailed, ResolutionSection::Detailed)
                | (FocusArea::Standard, ResolutionSection::Standard)
                | (FocusArea::Extension, ResolutionSection::Extension)
        );

        frame.render_widget(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(focus_style(focused)),
            list_area,
        );

        match section {
            ResolutionSection::Detailed => self.draw_detailed_rows(frame, inner),
            ResolutionSection::Standard => self.draw_standard_rows(frame, inner),
            ResolutionSection::Extension => {
                self.draw_extension_rows(frame, inner);
            }
        }

        if show_buttons {
            self.draw_section_buttons(frame, buttons_area, section);
        }
    }

    fn draw_detailed_rows(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let rows = self.working_dtds();
        if rows.is_empty() {
            frame.render_widget(Paragraph::new("No detailed timings."), area);
            return;
        }

        for (visible, (index, row)) in rows
            .iter()
            .enumerate()
            .skip(self.detailed_scroll)
            .take(usize::from(area.height))
            .enumerate()
        {
            let rect = Rect {
                x: area.x,
                y: area.y + visible as u16,
                width: area.width,
                height: 1,
            };
            let selected = self.selected_detailed == Some(index);
            let suffix = ModeKey::from_timing(&row.timing)
                .map(|key| self.provenance_suffix(key))
                .unwrap_or_default();
            let text = format!(
                "{} {:<18} {}{}",
                if selected { ">" } else { " " },
                row.timing.hyprland_mode(),
                format_location(row.location),
                suffix
            );
            frame.render_widget(Paragraph::new(text).style(row_style(selected)), rect);
            self.push_hitbox(rect, HitTarget::DetailedRow(index), 0);
        }
    }

    fn draw_standard_rows(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let rows = self.working_standard_timings();
        if rows.is_empty() {
            frame.render_widget(Paragraph::new("No standard timings."), area);
            return;
        }

        for (visible, (index, row)) in rows
            .iter()
            .enumerate()
            .skip(self.standard_scroll)
            .take(usize::from(area.height))
            .enumerate()
        {
            let rect = Rect {
                x: area.x,
                y: area.y + visible as u16,
                width: area.width,
                height: 1,
            };
            let selected = self.selected_standard == Some(index);
            let key = ModeKey::new(
                row.timing.width,
                row.timing.height,
                f64::from(row.timing.refresh_hz),
                false,
            );
            let text = format!(
                "{} slot {}  {}x{} @ {} Hz  {}{}",
                if selected { ">" } else { " " },
                row.slot,
                row.timing.width,
                row.timing.height,
                row.timing.refresh_hz,
                row.timing.aspect.label(),
                self.provenance_suffix(key)
            );
            frame.render_widget(Paragraph::new(text).style(row_style(selected)), rect);
            self.push_hitbox(rect, HitTarget::StandardRow(index), 0);
        }
    }

    fn draw_extension_rows(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let mut y = area.y;
        if let Some(edid) = self.selected_edid() {
            for cta in edid.cta_blocks.clone().iter() {
                if y >= area.y.saturating_add(area.height) {
                    return;
                }
                let rect = Rect {
                    x: area.x,
                    y,
                    width: area.width,
                    height: 1,
                };
                let text = format!(
                    "CTA ext {} rev {}  data {}  DTDs {} used, {} free  checksum {}",
                    cta.extension_index,
                    cta.revision,
                    cta_data_block_summary(cta),
                    cta.detailed_timings.len(),
                    cta.available_dtd_slots,
                    if cta.checksum_valid { "ok" } else { "bad" }
                );
                frame.render_widget(
                    Paragraph::new(text).style(Style::default().fg(Color::Cyan)),
                    rect,
                );
                y = y.saturating_add(1);
            }
            for displayid in edid.displayid_blocks.clone().iter() {
                if y >= area.y.saturating_add(area.height) {
                    return;
                }
                let rect = Rect {
                    x: area.x,
                    y,
                    width: area.width,
                    height: 1,
                };
                let text = format!(
                    "DisplayID ext {} v{}.{}  data {}  DTDs {}  checksum {}",
                    displayid.extension_index,
                    displayid.version_major,
                    displayid.version_minor,
                    displayid_data_block_summary(displayid),
                    displayid.detailed_timings.len(),
                    if displayid.checksum_valid {
                        "ok"
                    } else {
                        "bad"
                    }
                );
                frame.render_widget(
                    Paragraph::new(text).style(Style::default().fg(Color::Cyan)),
                    rect,
                );
                y = y.saturating_add(1);
            }
        }

        let rows = self.working_extension_rows();
        if rows.is_empty() {
            if y >= area.y.saturating_add(area.height) {
                return;
            }
            let rect = Rect {
                x: area.x,
                y,
                width: area.width,
                height: 1,
            };
            frame.render_widget(Paragraph::new("No editable CTA DTD slots parsed."), rect);
            y = y.saturating_add(1);
        } else {
            for (visible, (index, row)) in rows
                .iter()
                .enumerate()
                .skip(self.extension_scroll)
                .take(usize::from(
                    area.y.saturating_add(area.height).saturating_sub(y),
                ))
                .enumerate()
            {
                let rect = Rect {
                    x: area.x,
                    y: y + visible as u16,
                    width: area.width,
                    height: 1,
                };
                let selected = self.selected_extension == Some(index);
                let text = match row {
                    ExtensionRow::Video {
                        extension_index,
                        descriptor,
                    } => format!(
                        "{}   CTA ext {} video  {}{}",
                        if selected { ">" } else { " " },
                        extension_index,
                        descriptor.label(),
                        match descriptor {
                            crate::models::CtaVideoDescriptor::Known(mode) => self
                                .provenance_suffix(ModeKey::new(
                                    mode.width,
                                    mode.height,
                                    mode.refresh_hz(),
                                    mode.interlaced,
                                )),
                            crate::models::CtaVideoDescriptor::Unknown { .. } => String::new(),
                        }
                    ),
                    ExtensionRow::Dtd(row) => {
                        let (payload, suffix) = if let Some(timing) = &row.timing {
                            (
                                timing.hyprland_mode(),
                                ModeKey::from_timing(timing)
                                    .map(|key| self.provenance_suffix(key))
                                    .unwrap_or_default(),
                            )
                        } else if row.occupied_unknown {
                            ("[occupied unknown DTD]".to_string(), String::new())
                        } else {
                            ("[free DTD slot]".to_string(), String::new())
                        };
                        format!(
                            "{}   DTD slot {}.{}  {:<18} checksum {}{}",
                            if selected { ">" } else { " " },
                            row.extension_index,
                            row.slot,
                            payload,
                            if row.checksum_valid { "ok" } else { "bad" },
                            suffix
                        )
                    }
                    ExtensionRow::DisplayIdDtd(row) => {
                        let suffix = ModeKey::from_timing(&row.timing)
                            .map(|key| self.provenance_suffix(key))
                            .unwrap_or_default();
                        format!(
                            "{}   DisplayID ext {} DTD {}  {:<18}{}{}",
                            if selected { ">" } else { " " },
                            row.extension_index,
                            row.descriptor_index,
                            row.timing.hyprland_mode(),
                            if row.preferred { " preferred" } else { "" },
                            suffix
                        )
                    }
                };
                frame.render_widget(Paragraph::new(text).style(row_style(selected)), rect);
                self.push_hitbox(rect, HitTarget::ExtensionRow(index), 0);
            }
            y = y
                .saturating_add(rows.len().saturating_sub(self.extension_scroll) as u16)
                .min(area.y.saturating_add(area.height));
        }

        if let Some(workspace) = self.selected_workspace() {
            for issue in workspace.validate() {
                if y >= area.y.saturating_add(area.height) {
                    return;
                }
                let rect = Rect {
                    x: area.x,
                    y,
                    width: area.width,
                    height: 1,
                };
                frame.render_widget(Paragraph::new(format!("Error: {}", issue.message)), rect);
                y = y.saturating_add(1);
            }
            if workspace.has_changes() {
                for line in workspace.diff_summary().into_iter().take(3) {
                    if y >= area.y.saturating_add(area.height) {
                        return;
                    }
                    let rect = Rect {
                        x: area.x,
                        y,
                        width: area.width,
                        height: 1,
                    };
                    frame.render_widget(Paragraph::new(line), rect);
                    y = y.saturating_add(1);
                }
            }
        }
    }

    fn section_title(&self, section: ResolutionSection, viewport_height: u16) -> String {
        let Some(summary) = self
            .selected_workspace()
            .and_then(|workspace| workspace.slot_summary().ok())
        else {
            return match section {
                ResolutionSection::Detailed => scroll_title(
                    "Detailed resolutions",
                    self.detailed_scroll,
                    self.working_dtds().len(),
                    viewport_height,
                ),
                ResolutionSection::Standard => scroll_title(
                    "Standard resolutions",
                    self.standard_scroll,
                    self.working_standard_timings().len(),
                    viewport_height,
                ),
                ResolutionSection::Extension => scroll_title(
                    "Extension blocks",
                    self.extension_scroll,
                    self.working_extension_rows().len(),
                    viewport_height,
                ),
            };
        };

        match section {
            ResolutionSection::Detailed => format!(
                "{}  base {}/{} used, {} free",
                scroll_title(
                    "Detailed resolutions",
                    self.detailed_scroll,
                    self.working_dtds().len(),
                    viewport_height
                ),
                summary.base_dtd_used,
                summary.base_dtd_used + summary.base_dtd_free,
                summary.base_dtd_free
            ),
            ResolutionSection::Standard => format!(
                "{}  {}/8 used, {} free",
                scroll_title(
                    "Standard resolutions",
                    self.standard_scroll,
                    self.working_standard_timings().len(),
                    viewport_height
                ),
                summary.standard_used,
                summary.standard_free
            ),
            ResolutionSection::Extension => format!(
                "{}  CTA DTDs {} used, {} free",
                scroll_title(
                    "Extension blocks",
                    self.extension_scroll,
                    self.working_extension_rows().len(),
                    viewport_height
                ),
                summary.cta_dtd_used,
                summary.cta_dtd_free
            ),
        }
    }

    fn sync_section_scroll(&mut self, section: ResolutionSection, viewport_height: u16) {
        match section {
            ResolutionSection::Detailed => {
                let len = self.working_dtds().len();
                keep_selected_visible(
                    self.selected_detailed,
                    &mut self.detailed_scroll,
                    len,
                    viewport_height,
                );
            }
            ResolutionSection::Standard => {
                let len = self.working_standard_timings().len();
                keep_selected_visible(
                    self.selected_standard,
                    &mut self.standard_scroll,
                    len,
                    viewport_height,
                );
            }
            ResolutionSection::Extension => {
                let len = self.working_extension_rows().len();
                keep_selected_visible(
                    self.selected_extension,
                    &mut self.extension_scroll,
                    len,
                    viewport_height,
                );
            }
        }
    }

    fn draw_section_buttons(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        section: ResolutionSection,
    ) {
        let buttons: Vec<(&str, SectionAction)> = match section {
            ResolutionSection::Detailed => vec![
                ("Add", SectionAction::Add),
                ("Edit", SectionAction::Edit),
                ("Delete", SectionAction::Delete),
                ("Delete All", SectionAction::DeleteAll),
                ("Reset", SectionAction::Reset),
                ("Copy", SectionAction::Copy),
                ("Up", SectionAction::MoveUp),
                ("Down", SectionAction::MoveDown),
            ],
            ResolutionSection::Standard => vec![
                ("Add", SectionAction::Add),
                ("Edit", SectionAction::Edit),
                ("Delete", SectionAction::Delete),
                ("Delete All", SectionAction::DeleteAll),
                ("Reset", SectionAction::Reset),
            ],
            ResolutionSection::Extension => vec![
                ("Add", SectionAction::Add),
                ("Edit", SectionAction::Edit),
                ("Delete", SectionAction::Delete),
                ("Delete All", SectionAction::DeleteAll),
                ("Reset", SectionAction::Reset),
                ("Copy", SectionAction::Copy),
            ],
        };
        let constraints = vec![Constraint::Ratio(1, buttons.len() as u32); buttons.len()];
        let rects = Layout::horizontal(constraints).split(area);
        for (rect, (label, action)) in rects.iter().copied().zip(buttons) {
            self.draw_button(frame, rect, label);
            self.push_hitbox(rect, HitTarget::SectionButton(section, action), 0);
        }
    }

    fn draw_global_buttons(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let install_label = if self.selected_override_present() {
            "Update"
        } else {
            "Install"
        };
        let buttons = [
            ("Import", GlobalAction::Import),
            ("Export", GlobalAction::Export),
            ("Switch", GlobalAction::SwitchMode),
            ("Verify", GlobalAction::VerifyMode),
            (install_label, GlobalAction::Install),
            ("Uninstall", GlobalAction::Uninstall),
        ];
        let constraints = vec![Constraint::Ratio(1, buttons.len() as u32); buttons.len()];
        let rects = Layout::horizontal(constraints).split(area);
        for (rect, (label, action)) in rects.iter().copied().zip(buttons) {
            self.draw_button(frame, rect, label);
            self.push_hitbox(rect, HitTarget::GlobalButton(action), 0);
        }
    }

    fn draw_button(&self, frame: &mut Frame<'_>, area: Rect, label: &str) {
        frame.render_widget(
            Paragraph::new(center_label(label, area.width))
                .block(Block::default().borders(Borders::ALL)),
            area,
        );
    }

    fn draw_detailed_editor(&mut self, frame: &mut Frame<'_>, editor: &DetailedResolutionEditor) {
        let area = centered_rect(frame.area(), 88, 36);
        frame.render_widget(Clear, area);
        frame.render_widget(
            Block::default()
                .title("Detailed Resolution")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
            area,
        );

        let inner = Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(2),
        };
        let [intro, fields, derived, actions] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(20),
            Constraint::Min(9),
            Constraint::Length(3),
        ])
        .areas(inner);

        frame.render_widget(
            Paragraph::new(
                "Detailed Timing. Presets regenerate editable fields from active size and refresh.",
            ),
            intro,
        );

        for (field, rect, label) in editor.field_rows(fields) {
            let active = editor.active_field == field;
            let text = format!("{label:<10} [{}]", editor.render_field(field, active));
            frame.render_widget(Paragraph::new(text).style(focus_style(active)), rect);
            self.push_hitbox(rect, HitTarget::ModalField(field), 2);
        }

        frame.render_widget(
            Paragraph::new(editor.derived_text()).wrap(Wrap { trim: false }),
            derived,
        );

        let [ok, cancel, _spacer] = Layout::horizontal([
            Constraint::Length(10),
            Constraint::Length(12),
            Constraint::Min(0),
        ])
        .areas(actions);
        self.draw_button(frame, ok, "OK");
        self.draw_button(frame, cancel, "Cancel");
        self.push_hitbox(ok, HitTarget::ModalButton(ModalButton::Ok), 2);
        self.push_hitbox(cancel, HitTarget::ModalButton(ModalButton::Cancel), 2);
    }

    fn draw_standard_editor(&mut self, frame: &mut Frame<'_>, editor: &StandardResolutionEditor) {
        let area = centered_rect(frame.area(), 66, 17);
        frame.render_widget(Clear, area);
        frame.render_widget(
            Block::default()
                .title("Standard Resolution")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
            area,
        );

        let inner = Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(2),
        };
        let [intro, fields, derived, actions] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(5),
            Constraint::Min(4),
            Constraint::Length(3),
        ])
        .areas(inner);

        frame.render_widget(
            Paragraph::new("EDID standard timings store width, aspect ratio, and integer refresh."),
            intro,
        );

        let field_rects = editor.field_rects(fields);
        for (field, rect, label) in [
            (EditorField::Width, field_rects[0], "Width"),
            (EditorField::Height, field_rects[1], "Height"),
            (EditorField::Refresh, field_rects[2], "Refresh Hz"),
        ] {
            let active = editor.active_field == field;
            let text = format!("{label:<10} [{}]", editor.input(field).render(active));
            frame.render_widget(Paragraph::new(text).style(focus_style(active)), rect);
            self.push_hitbox(rect, HitTarget::ModalField(field), 2);
        }

        frame.render_widget(
            Paragraph::new(editor.derived_text()).wrap(Wrap { trim: false }),
            derived,
        );

        let [ok, cancel, _spacer] = Layout::horizontal([
            Constraint::Length(10),
            Constraint::Length(12),
            Constraint::Min(0),
        ])
        .areas(actions);
        self.draw_button(frame, ok, "OK");
        self.draw_button(frame, cancel, "Cancel");
        self.push_hitbox(ok, HitTarget::ModalButton(ModalButton::Ok), 2);
        self.push_hitbox(cancel, HitTarget::ModalButton(ModalButton::Cancel), 2);
    }

    fn draw_import_dialog(&mut self, frame: &mut Frame<'_>, dialog: &ImportDialog) {
        let area = centered_rect(frame.area(), 76, 9);
        frame.render_widget(Clear, area);
        frame.render_widget(
            Block::default()
                .title("Import EDID")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
            area,
        );

        let inner = Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(2),
        };
        let [help, field, actions] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(3),
        ])
        .areas(inner);

        frame.render_widget(
            Paragraph::new("Load a raw EDID binary into this monitor's workspace."),
            help,
        );
        frame.render_widget(
            Paragraph::new(format!("Path: [{}]", dialog.path.render(true)))
                .style(Style::default().fg(Color::Cyan)),
            field,
        );
        self.push_hitbox(field, HitTarget::ImportPathField, 2);

        let [ok, cancel, _spacer] = Layout::horizontal([
            Constraint::Length(10),
            Constraint::Length(12),
            Constraint::Min(0),
        ])
        .areas(actions);
        self.draw_button(frame, ok, "Import");
        self.draw_button(frame, cancel, "Cancel");
        self.push_hitbox(ok, HitTarget::ModalButton(ModalButton::Ok), 2);
        self.push_hitbox(cancel, HitTarget::ModalButton(ModalButton::Cancel), 2);
    }

    fn draw_details_dialog(&mut self, frame: &mut Frame<'_>, dialog: &DetailsDialog) {
        let area = centered_rect(frame.area(), 100, 28);
        frame.render_widget(Clear, area);

        let inner = Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(2),
        };
        let [body, actions] =
            Layout::vertical([Constraint::Min(12), Constraint::Length(3)]).areas(inner);
        let viewport = usize::from(body.height.max(1));
        let max_scroll = dialog.lines.len().saturating_sub(viewport);
        let scroll = dialog.scroll.min(max_scroll);
        let title = details_title(dialog, scroll, viewport);
        frame.render_widget(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
            area,
        );
        frame.render_widget(
            Paragraph::new(dialog.lines.join("\n"))
                .wrap(Wrap { trim: false })
                .scroll((scroll.min(usize::from(u16::MAX)) as u16, 0)),
            body,
        );

        let [close, _spacer] =
            Layout::horizontal([Constraint::Length(12), Constraint::Min(0)]).areas(actions);
        self.draw_button(frame, close, "Close");
        self.push_hitbox(close, HitTarget::ModalButton(ModalButton::Ok), 2);
    }

    fn draw_export_dialog(&mut self, frame: &mut Frame<'_>, dialog: &ExportDialog) {
        let area = centered_rect(frame.area(), 92, 15);
        frame.render_widget(Clear, area);
        frame.render_widget(
            Block::default()
                .title("Export EDID")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
            area,
        );

        let inner = Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(2),
        };
        let [body, actions] =
            Layout::vertical([Constraint::Min(8), Constraint::Length(3)]).areas(inner);
        let text = [
            format!("Wrote: {}", dialog.path),
            format!("Instructions: {}", dialog.instructions_path),
            format!("Source: {}", dialog.location),
            String::new(),
            "Kernel parameter:".to_string(),
            dialog.kernel_parameter.clone(),
            String::new(),
            "Hyprland monitor rule:".to_string(),
            dialog.hyprland_rule.clone(),
        ]
        .join("\n");

        frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), body);

        let [close, _spacer] =
            Layout::horizontal([Constraint::Length(12), Constraint::Min(0)]).areas(actions);
        self.draw_button(frame, close, "Close");
        self.push_hitbox(close, HitTarget::ModalButton(ModalButton::Ok), 2);
    }

    fn draw_export_confirm_dialog(&mut self, frame: &mut Frame<'_>, dialog: &ExportConfirmDialog) {
        let area = centered_rect(frame.area(), 88, 17);
        frame.render_widget(Clear, area);
        frame.render_widget(
            Block::default()
                .title("Export Validation")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
            area,
        );

        let inner = Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(2),
        };
        let [body, actions] =
            Layout::vertical([Constraint::Min(10), Constraint::Length(3)]).areas(inner);
        let text = std::iter::once("Review these warnings before writing the EDID:".to_string())
            .chain(std::iter::once(String::new()))
            .chain(
                dialog
                    .issues
                    .iter()
                    .take(8)
                    .map(|issue| format!("- {issue}")),
            )
            .chain((dialog.issues.len() > 8).then(|| {
                format!(
                    "- ... {} more warning(s) not shown",
                    dialog.issues.len().saturating_sub(8)
                )
            }))
            .collect::<Vec<_>>()
            .join("\n");

        frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), body);

        let [cont, cancel, _spacer] = Layout::horizontal([
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Min(0),
        ])
        .areas(actions);
        self.draw_button(frame, cont, "Continue");
        self.draw_button(frame, cancel, "Cancel");
        self.push_hitbox(cont, HitTarget::ModalButton(ModalButton::Ok), 2);
        self.push_hitbox(cancel, HitTarget::ModalButton(ModalButton::Cancel), 2);
    }

    fn draw_apply_progress(&mut self, frame: &mut Frame<'_>) {
        let operation = self
            .applying_operation
            .or_else(|| {
                self.apply_confirm_dialog
                    .as_ref()
                    .map(|dialog| dialog.operation)
            })
            .or_else(|| {
                self.apply_result_dialog
                    .as_ref()
                    .map(|dialog| dialog.operation)
            })
            .unwrap_or(SystemOperation::Install);
        let (title, verb) = match operation {
            SystemOperation::Install => ("Install EDID", "Installing custom EDID"),
            SystemOperation::Update => ("Update EDID", "Updating custom EDID"),
            SystemOperation::Uninstall => ("Uninstall EDID", "Uninstalling custom EDID"),
        };
        let area = centered_rect(frame.area(), 60, 7);
        frame.render_widget(Clear, area);
        frame.render_widget(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
            area,
        );

        let inner = Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(2),
        };

        // Simple spinner using elapsed time
        let spinner_frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        let tick = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            / 100) as usize;
        let spinner = spinner_frames[tick % spinner_frames.len()];

        let text = format!("{spinner} {verb}...\n\n  Authenticate in the popup to continue.");
        frame.render_widget(
            Paragraph::new(text)
                .style(Style::default().fg(Color::Cyan))
                .wrap(Wrap { trim: false }),
            inner,
        );
    }

    fn draw_apply_confirm_dialog(&mut self, frame: &mut Frame<'_>, dialog: &ApplyConfirmDialog) {
        let (title, confirm_label, reboot_text) = match dialog.operation {
            SystemOperation::Install => (
                "Install EDID",
                "Install",
                "A reboot is required after install.",
            ),
            SystemOperation::Update => (
                "Update EDID",
                "Update",
                "A reboot is required after update.",
            ),
            SystemOperation::Uninstall => (
                "Uninstall EDID",
                "Uninstall",
                "A reboot is required after uninstall.",
            ),
        };
        let area = centered_rect(frame.area(), 92, 15);
        frame.render_widget(Clear, area);
        frame.render_widget(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Green)),
            area,
        );

        let inner = Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(2),
        };
        let [body, actions] =
            Layout::vertical([Constraint::Min(8), Constraint::Length(3)]).areas(inner);

        let text =
            std::iter::once("The following system changes will be made as root:".to_string())
                .chain(std::iter::once(String::new()))
                .chain(dialog.summary_lines.iter().map(|line| format!("  {line}")))
                .chain(std::iter::once(String::new()))
                .chain(std::iter::once(reboot_text.to_string()))
                .collect::<Vec<_>>()
                .join("\n");

        frame.render_widget(
            Paragraph::new(text)
                .scroll((dialog.scroll.min(u16::MAX as usize) as u16, 0))
                .wrap(Wrap { trim: false }),
            body,
        );

        let [apply_btn, cancel_btn, _spacer] = Layout::horizontal([
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Min(0),
        ])
        .areas(actions);
        self.draw_button(frame, apply_btn, confirm_label);
        self.draw_button(frame, cancel_btn, "Cancel");
        self.push_hitbox(apply_btn, HitTarget::ModalButton(ModalButton::Ok), 2);
        self.push_hitbox(cancel_btn, HitTarget::ModalButton(ModalButton::Cancel), 2);
    }

    fn draw_apply_result_dialog(&mut self, frame: &mut Frame<'_>, dialog: &ApplyResultDialog) {
        let area = centered_rect(frame.area(), 92, 18);
        frame.render_widget(Clear, area);

        let border_color = if dialog.success {
            Color::Green
        } else {
            Color::Red
        };
        let title = match (dialog.operation, dialog.success) {
            (SystemOperation::Install, true) => "Install EDID — Success",
            (SystemOperation::Install, false) => "Install EDID — Failed",
            (SystemOperation::Update, true) => "Update EDID — Success",
            (SystemOperation::Update, false) => "Update EDID — Failed",
            (SystemOperation::Uninstall, true) => "Uninstall EDID — Success",
            (SystemOperation::Uninstall, false) => "Uninstall EDID — Failed",
        };

        frame.render_widget(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color)),
            area,
        );

        let inner = Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(2),
        };
        let [body, actions] =
            Layout::vertical([Constraint::Min(10), Constraint::Length(3)]).areas(inner);

        let mut lines: Vec<String> = Vec::new();
        if dialog.success {
            match dialog.operation {
                SystemOperation::Install => {
                    lines.push("Custom EDID installed successfully!".to_string());
                    lines.push(String::new());
                    lines.push("Reboot to activate the custom resolution.".to_string());
                    lines.push(
                        "After reboot, set your monitor rule in Hyprland config.".to_string(),
                    );
                }
                SystemOperation::Update => {
                    lines.push("Custom EDID updated successfully.".to_string());
                    lines.push(String::new());
                    lines.push("Reboot to activate the updated EDID.".to_string());
                }
                SystemOperation::Uninstall => {
                    lines.push("Custom EDID override removed successfully.".to_string());
                    lines.push(String::new());
                    lines.push("Reboot to return to the monitor's normal EDID.".to_string());
                }
            }
        } else {
            lines.push(match dialog.operation {
                SystemOperation::Install => "Installation failed. Details:".to_string(),
                SystemOperation::Update => "Update failed. Details:".to_string(),
                SystemOperation::Uninstall => "Uninstall failed. Details:".to_string(),
            });
        }
        lines.push(String::new());
        for line in dialog.output.lines() {
            lines.push(line.to_string());
        }
        let text = lines.join("\n");

        frame.render_widget(
            Paragraph::new(text)
                .scroll((dialog.scroll.min(u16::MAX as usize) as u16, 0))
                .wrap(Wrap { trim: false }),
            body,
        );

        let [close_btn, _spacer] =
            Layout::horizontal([Constraint::Length(12), Constraint::Min(0)]).areas(actions);
        self.draw_button(frame, close_btn, "Close");
        self.push_hitbox(close_btn, HitTarget::ModalButton(ModalButton::Ok), 2);
    }

    pub(super) fn push_hitbox(&mut self, rect: Rect, target: HitTarget, z: u8) {
        if rect.width > 0 && rect.height > 0 {
            self.hitboxes.push(Hitbox { rect, target, z });
        }
    }

    pub(super) fn hit_test(&self, x: u16, y: u16) -> Option<Hitbox> {
        self.hitboxes
            .iter()
            .rev()
            .filter(|hitbox| super::support::rect_contains(hitbox.rect, x, y))
            .max_by_key(|hitbox| hitbox.z)
            .copied()
    }
}

fn cta_data_block_summary(cta: &crate::models::Cta861Block) -> String {
    if cta.data_blocks.is_empty() {
        return "none".to_string();
    }

    let mut labels = cta
        .data_blocks
        .iter()
        .take(4)
        .map(|block| {
            if block.video_modes.is_empty() {
                format!("{}({})", block.label(), block.payload_len)
            } else {
                format!("{}({} VIC)", block.label(), block.video_modes.len())
            }
        })
        .collect::<Vec<_>>();
    if cta.data_blocks.len() > labels.len() {
        labels.push(format!("+{}", cta.data_blocks.len() - labels.len()));
    }
    labels.join(", ")
}

fn displayid_data_block_summary(displayid: &crate::models::DisplayIdBlock) -> String {
    if displayid.data_blocks.is_empty() {
        return "none".to_string();
    }

    let mut labels = displayid
        .data_blocks
        .iter()
        .take(4)
        .map(|block| format!("{}({})", block.label(), block.payload_len))
        .collect::<Vec<_>>();
    if displayid.data_blocks.len() > labels.len() {
        labels.push(format!("+{}", displayid.data_blocks.len() - labels.len()));
    }
    labels.join(", ")
}

fn details_title(dialog: &DetailsDialog, scroll: usize, viewport: usize) -> String {
    let len = dialog.lines.len();
    if len <= viewport {
        return dialog.title.clone();
    }

    let start = scroll.saturating_add(1).min(len);
    let end = (scroll + viewport).min(len);
    let up = if scroll > 0 { "↑" } else { " " };
    let down = if end < len { "↓" } else { " " };
    format!("{} {up}{start}-{end}/{len}{down}", dialog.title)
}
