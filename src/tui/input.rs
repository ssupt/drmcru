use super::state::{EditorMode, ModalButton, StandardEditorMode};
use super::{App, FocusArea, GlobalAction, HitTarget, Hitbox, ResolutionSection, SectionAction};
use crate::workspace::MoveDirection;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};

impl App {
    pub(super) fn handle_main_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return true,
            KeyCode::Tab => self.next_focus(),
            KeyCode::BackTab => self.previous_focus(),
            KeyCode::Char('?') | KeyCode::Char('h') | KeyCode::F(1) => self.open_help_dialog(),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Enter => self.activate_focused(),
            KeyCode::Char('a') => match self.focus {
                FocusArea::Standard => self.open_standard_editor(StandardEditorMode::Add),
                FocusArea::Extension => self.open_extension_add_editor(),
                _ => self.open_detailed_editor(EditorMode::Add),
            },
            KeyCode::Char('e') => self.export_selected_monitor(),
            KeyCode::Char('s') => self.switch_selected_monitor_mode(),
            KeyCode::Char('v') => self.verify_selected_monitor_mode(),
            KeyCode::Char('A') => self.apply_selected_monitor(),
            KeyCode::Char('u') => self.uninstall_selected_monitor(),
            KeyCode::Char('i') => self.open_selected_details(),
            KeyCode::Char('c') => match self.focus {
                FocusArea::Extension => self.copy_selected_extension_dtd(),
                _ => self.copy_selected_detailed(),
            },
            KeyCode::Char('p') => self.paste_detailed_as_new(),
            KeyCode::Delete | KeyCode::Char('d') => match self.focus {
                FocusArea::Standard => self.delete_selected_standard(),
                FocusArea::Extension => self.delete_selected_extension_dtd(),
                _ => self.delete_selected_detailed(),
            },
            _ => {}
        }
        false
    }

    pub(super) fn handle_mouse(&mut self, mouse: MouseEvent) {
        if self.details_dialog.is_some() {
            match mouse.kind {
                MouseEventKind::ScrollDown => {
                    if let Some(dialog) = self.details_dialog.as_mut() {
                        dialog.scroll_by(3);
                    }
                    return;
                }
                MouseEventKind::ScrollUp => {
                    if let Some(dialog) = self.details_dialog.as_mut() {
                        dialog.scroll_by(-3);
                    }
                    return;
                }
                _ => {}
            }
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(hitbox) = self.hit_test(mouse.column, mouse.row) {
                    self.activate_hit(hitbox, mouse.column);
                }
            }
            MouseEventKind::Down(MouseButton::Right) => {
                if let Some(hitbox) = self.hit_test(mouse.column, mouse.row) {
                    let target = hitbox.target;
                    self.activate_hit(hitbox, mouse.column);
                    if matches!(
                        target,
                        HitTarget::EstablishedRow(_)
                            | HitTarget::EstablishedCheckbox(_)
                            | HitTarget::DetailedRow(_)
                            | HitTarget::StandardRow(_)
                            | HitTarget::ExtensionRow(_)
                    ) {
                        self.open_selected_details();
                    }
                }
            }
            MouseEventKind::ScrollDown => self.move_selection(1),
            MouseEventKind::ScrollUp => self.move_selection(-1),
            _ => {}
        }
    }

    fn activate_hit(&mut self, hitbox: Hitbox, mouse_column: u16) {
        match hitbox.target {
            HitTarget::MonitorSelector => {
                self.focus = FocusArea::Monitor;
                self.move_monitor(1);
            }
            HitTarget::EstablishedRow(index) | HitTarget::EstablishedCheckbox(index) => {
                self.selected_established = Some(index);
                self.focus = FocusArea::Established;
            }
            HitTarget::DetailedRow(index) => {
                self.selected_detailed = Some(index);
                self.focus = FocusArea::Detailed;
            }
            HitTarget::StandardRow(index) => {
                self.selected_standard = Some(index);
                self.focus = FocusArea::Standard;
            }
            HitTarget::ExtensionRow(index) => {
                self.selected_extension = Some(index);
                self.focus = FocusArea::Extension;
            }
            HitTarget::SectionButton(section, action) => {
                self.activate_section_action(section, action)
            }
            HitTarget::GlobalButton(action) => self.activate_global_action(action),
            HitTarget::ModalField(field) => {
                if let Some(editor) = self.detailed_editor.as_mut() {
                    editor.active_field = field;
                    editor.set_cursor_from_column(field, mouse_column, hitbox.rect);
                } else if let Some(editor) = self.standard_editor.as_mut() {
                    editor.active_field = field;
                    editor.set_cursor_from_column(field, mouse_column, hitbox.rect);
                }
            }
            HitTarget::ImportPathField => {
                if let Some(dialog) = self.import_dialog.as_mut() {
                    dialog
                        .path
                        .set_cursor_from_column(mouse_column, hitbox.rect, 7);
                }
            }
            HitTarget::ModalButton(button) => self.activate_modal_button(button),
        }
    }

    fn activate_section_action(&mut self, section: ResolutionSection, action: SectionAction) {
        match (section, action) {
            (ResolutionSection::Detailed, SectionAction::Add) => {
                self.open_detailed_editor(EditorMode::Add)
            }
            (ResolutionSection::Detailed, SectionAction::Edit) => self.edit_selected_detailed(),
            (ResolutionSection::Detailed, SectionAction::Delete) => self.delete_selected_detailed(),
            (ResolutionSection::Detailed, SectionAction::DeleteAll) => self.delete_all_detailed(),
            (ResolutionSection::Detailed, SectionAction::Reset) => {
                self.reset_workspace();
            }
            (ResolutionSection::Detailed, SectionAction::Copy) => self.copy_selected_detailed(),
            (ResolutionSection::Detailed, SectionAction::MoveUp) => {
                self.move_selected_detailed(MoveDirection::Up);
            }
            (ResolutionSection::Detailed, SectionAction::MoveDown) => {
                self.move_selected_detailed(MoveDirection::Down);
            }
            (ResolutionSection::Standard, SectionAction::Delete) => {
                self.delete_selected_standard();
            }
            (ResolutionSection::Standard, SectionAction::DeleteAll) => {
                self.delete_all_standard();
            }
            (ResolutionSection::Standard, SectionAction::Reset) => {
                self.reset_workspace();
            }
            (ResolutionSection::Standard, SectionAction::Add) => {
                self.open_standard_editor(StandardEditorMode::Add);
            }
            (ResolutionSection::Standard, SectionAction::Edit) => {
                self.edit_selected_standard();
            }
            (ResolutionSection::Standard, SectionAction::Copy) => {
                self.status =
                    "Copy currently targets detailed timings; use detailed rows for full timing clones."
                        .to_string();
            }
            (ResolutionSection::Standard, SectionAction::MoveUp | SectionAction::MoveDown) => {
                self.status =
                    "Standard timing reordering is not needed; slots stay fixed.".to_string();
            }
            (ResolutionSection::Extension, SectionAction::Add) => {
                self.open_extension_add_editor();
            }
            (ResolutionSection::Extension, SectionAction::Edit) => {
                self.edit_selected_extension_dtd();
            }
            (ResolutionSection::Extension, SectionAction::Delete) => {
                self.delete_selected_extension_dtd();
            }
            (ResolutionSection::Extension, SectionAction::DeleteAll) => {
                self.delete_all_extension_dtds();
            }
            (ResolutionSection::Extension, SectionAction::Reset) => {
                self.reset_workspace();
            }
            (ResolutionSection::Extension, SectionAction::Copy) => {
                self.copy_selected_extension_dtd();
            }
            (ResolutionSection::Extension, SectionAction::MoveUp | SectionAction::MoveDown) => {
                self.status =
                    "CTA DTD slots are fixed by EDID location; use Detailed rows for global reordering."
                        .to_string();
            }
        }
    }

    fn activate_global_action(&mut self, action: GlobalAction) {
        match action {
            GlobalAction::Import => self.open_import_dialog(),
            GlobalAction::Export | GlobalAction::Ok => self.export_selected_monitor(),
            GlobalAction::SwitchMode => self.switch_selected_monitor_mode(),
            GlobalAction::VerifyMode => self.verify_selected_monitor_mode(),
            GlobalAction::Install => self.apply_selected_monitor(),
            GlobalAction::Uninstall => self.uninstall_selected_monitor(),
            GlobalAction::Cancel => {
                self.status = "Cancel requested. Press q or Esc to quit.".to_string();
            }
        }
    }

    pub(super) fn handle_detailed_editor_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.detailed_editor = None;
                self.status = "Detailed resolution edit cancelled.".to_string();
            }
            KeyCode::Enter => self.apply_detailed_editor(),
            KeyCode::Tab | KeyCode::Down => {
                if let Some(editor) = self.detailed_editor.as_mut() {
                    editor.next_field();
                }
            }
            KeyCode::BackTab | KeyCode::Up => {
                if let Some(editor) = self.detailed_editor.as_mut() {
                    editor.previous_field();
                }
            }
            KeyCode::Left => {
                if let Some(editor) = self.detailed_editor.as_mut() {
                    editor.move_cursor_left();
                }
            }
            KeyCode::Right => {
                if let Some(editor) = self.detailed_editor.as_mut() {
                    editor.move_cursor_right();
                }
            }
            KeyCode::Home => {
                if let Some(editor) = self.detailed_editor.as_mut() {
                    editor.move_cursor_home();
                }
            }
            KeyCode::End => {
                if let Some(editor) = self.detailed_editor.as_mut() {
                    editor.move_cursor_end();
                }
            }
            KeyCode::Backspace => {
                if let Some(editor) = self.detailed_editor.as_mut() {
                    editor.backspace();
                }
            }
            KeyCode::Delete => {
                if let Some(editor) = self.detailed_editor.as_mut() {
                    editor.delete();
                }
            }
            KeyCode::Char(value) => {
                if let Some(editor) = self.detailed_editor.as_mut() {
                    editor.insert_char(value);
                }
            }
            _ => {}
        }
    }

    pub(super) fn handle_standard_editor_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.standard_editor = None;
                self.status = "Standard resolution edit cancelled.".to_string();
            }
            KeyCode::Enter => self.apply_standard_editor(),
            KeyCode::Tab | KeyCode::Down => {
                if let Some(editor) = self.standard_editor.as_mut() {
                    editor.next_field();
                }
            }
            KeyCode::BackTab | KeyCode::Up => {
                if let Some(editor) = self.standard_editor.as_mut() {
                    editor.previous_field();
                }
            }
            KeyCode::Left => {
                if let Some(editor) = self.standard_editor.as_mut() {
                    editor.move_cursor_left();
                }
            }
            KeyCode::Right => {
                if let Some(editor) = self.standard_editor.as_mut() {
                    editor.move_cursor_right();
                }
            }
            KeyCode::Home => {
                if let Some(editor) = self.standard_editor.as_mut() {
                    editor.move_cursor_home();
                }
            }
            KeyCode::End => {
                if let Some(editor) = self.standard_editor.as_mut() {
                    editor.move_cursor_end();
                }
            }
            KeyCode::Backspace => {
                if let Some(editor) = self.standard_editor.as_mut() {
                    editor.backspace();
                }
            }
            KeyCode::Delete => {
                if let Some(editor) = self.standard_editor.as_mut() {
                    editor.delete();
                }
            }
            KeyCode::Char(value) => {
                if let Some(editor) = self.standard_editor.as_mut() {
                    editor.insert_char(value);
                }
            }
            _ => {}
        }
    }

    pub(super) fn handle_import_dialog_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.import_dialog = None;
                self.status = "Import cancelled.".to_string();
            }
            KeyCode::Enter => self.apply_import_dialog(),
            KeyCode::Left => {
                if let Some(dialog) = self.import_dialog.as_mut() {
                    dialog.path.move_left();
                }
            }
            KeyCode::Right => {
                if let Some(dialog) = self.import_dialog.as_mut() {
                    dialog.path.move_right();
                }
            }
            KeyCode::Home => {
                if let Some(dialog) = self.import_dialog.as_mut() {
                    dialog.path.cursor = 0;
                }
            }
            KeyCode::End => {
                if let Some(dialog) = self.import_dialog.as_mut() {
                    dialog.path.move_end();
                }
            }
            KeyCode::Backspace => {
                if let Some(dialog) = self.import_dialog.as_mut() {
                    dialog.path.backspace();
                }
            }
            KeyCode::Delete => {
                if let Some(dialog) = self.import_dialog.as_mut() {
                    dialog.path.delete();
                }
            }
            KeyCode::Char(value) => {
                if let Some(dialog) = self.import_dialog.as_mut() {
                    dialog.path.insert(value);
                }
            }
            _ => {}
        }
    }

    pub(super) fn handle_export_dialog_key(&mut self, key: KeyEvent) {
        if matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
            self.export_dialog = None;
            self.status = "Export summary closed.".to_string();
        }
    }

    pub(super) fn handle_details_dialog_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                self.details_dialog = None;
                self.status = "Details closed.".to_string();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(dialog) = self.details_dialog.as_mut() {
                    dialog.scroll_by(1);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(dialog) = self.details_dialog.as_mut() {
                    dialog.scroll_by(-1);
                }
            }
            KeyCode::PageDown => {
                if let Some(dialog) = self.details_dialog.as_mut() {
                    dialog.scroll_page(1);
                }
            }
            KeyCode::PageUp => {
                if let Some(dialog) = self.details_dialog.as_mut() {
                    dialog.scroll_page(-1);
                }
            }
            KeyCode::Home => {
                if let Some(dialog) = self.details_dialog.as_mut() {
                    dialog.scroll_to_top();
                }
            }
            KeyCode::End => {
                if let Some(dialog) = self.details_dialog.as_mut() {
                    dialog.scroll_to_bottom();
                }
            }
            _ => {}
        }
    }

    pub(super) fn handle_export_confirm_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                self.export_confirm_dialog = None;
                self.export_selected_monitor_unchecked();
            }
            KeyCode::Esc => {
                self.export_confirm_dialog = None;
                self.status = "Export cancelled after validation warnings.".to_string();
            }
            _ => {}
        }
    }

    pub(super) fn handle_apply_confirm_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.confirm_apply(),
            KeyCode::Esc => self.cancel_apply(),
            _ => {}
        }
    }

    pub(super) fn handle_apply_result_key(&mut self, key: KeyEvent) {
        if matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
            self.dismiss_apply_result();
        }
    }

    fn activate_modal_button(&mut self, button: ModalButton) {
        if self.import_dialog.is_some() {
            match button {
                ModalButton::Ok => self.apply_import_dialog(),
                ModalButton::Cancel => {
                    self.import_dialog = None;
                    self.status = "Import cancelled.".to_string();
                }
            }
            return;
        }

        if self.standard_editor.is_some() {
            match button {
                ModalButton::Ok => self.apply_standard_editor(),
                ModalButton::Cancel => {
                    self.standard_editor = None;
                    self.status = "Standard resolution edit cancelled.".to_string();
                }
            }
            return;
        }

        if self.export_confirm_dialog.is_some() {
            match button {
                ModalButton::Ok => {
                    self.export_confirm_dialog = None;
                    self.export_selected_monitor_unchecked();
                }
                ModalButton::Cancel => {
                    self.export_confirm_dialog = None;
                    self.status = "Export cancelled after validation warnings.".to_string();
                }
            }
            return;
        }

        if self.export_dialog.is_some() {
            let _ = button;
            self.export_dialog = None;
            self.status = "Export summary closed.".to_string();
            return;
        }

        if self.details_dialog.is_some() {
            let _ = button;
            self.details_dialog = None;
            self.status = "Details closed.".to_string();
            return;
        }

        if self.apply_confirm_dialog.is_some() {
            match button {
                ModalButton::Ok => self.confirm_apply(),
                ModalButton::Cancel => self.cancel_apply(),
            }
            return;
        }

        if self.apply_result_dialog.is_some() {
            let _ = button;
            self.dismiss_apply_result();
            return;
        }

        match button {
            ModalButton::Ok => self.apply_detailed_editor(),
            ModalButton::Cancel => {
                self.detailed_editor = None;
                self.status = "Detailed resolution edit cancelled.".to_string();
            }
        }
    }
}
