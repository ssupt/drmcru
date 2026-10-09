use crate::edid::{CtaDtdSlot, LocatedDetailedTiming, LocatedStandardTiming};
use crate::export::custom_edid_file_name;
use crate::install::{
    self, InstallError, InstallPlan, InstallReport, InstalledOverrideStatus, UninstallPlan,
};
use crate::models::{
    CtaVideoDescriptor, DisplayIdDetailedTiming, EdidData, Monitor, TimingDescriptor,
};
use crate::timings::{CvtRequest, cvt_reduced_blanking};
use crate::workspace::{EdidWorkspace, format_location};
mod actions;
mod input;
mod render;
mod state;
mod support;

use anyhow::Result;
use crossterm::event::{self, Event};
use ratatui::prelude::*;
use state::{
    ApplyConfirmDialog, ApplyResultDialog, DetailedResolutionEditor, DetailsDialog,
    ExportConfirmDialog, ExportDialog, ImportDialog, StandardResolutionEditor, SystemOperation,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;
use support::{TerminalSession, wrap_index};

#[derive(Debug)]
pub struct App {
    monitors: Vec<Monitor>,
    workspaces: Vec<Option<EdidWorkspace>>,
    override_statuses: Vec<InstalledOverrideStatus>,
    selected_monitor: usize,
    selected_detailed: Option<usize>,
    selected_standard: Option<usize>,
    selected_established: Option<usize>,
    selected_extension: Option<usize>,
    established_scroll: usize,
    detailed_scroll: usize,
    standard_scroll: usize,
    extension_scroll: usize,
    focus: FocusArea,
    draft_timing: TimingDescriptor,
    detailed_clipboard: Option<TimingDescriptor>,
    detailed_editor: Option<DetailedResolutionEditor>,
    standard_editor: Option<StandardResolutionEditor>,
    vrr_editor: Option<state::VrrRangeEditor>,
    import_dialog: Option<ImportDialog>,
    details_dialog: Option<DetailsDialog>,
    export_confirm_dialog: Option<ExportConfirmDialog>,
    export_dialog: Option<ExportDialog>,
    apply_confirm_dialog: Option<ApplyConfirmDialog>,
    apply_result_dialog: Option<ApplyResultDialog>,
    applying_in_progress: bool,
    applying_operation: Option<SystemOperation>,
    pending_system_action: Option<PendingSystemAction>,
    install_receiver:
        Option<mpsc::Receiver<(SystemOperation, Result<InstallReport, InstallError>)>>,
    hitboxes: Vec<Hitbox>,
    status: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusArea {
    Monitor,
    Established,
    Detailed,
    Standard,
    Extension,
    Global,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolutionSection {
    Detailed,
    Standard,
    Extension,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SectionAction {
    Add,
    Edit,
    Delete,
    DeleteAll,
    Reset,
    Copy,
    MoveUp,
    MoveDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GlobalAction {
    VrrRange,
    Import,
    Export,
    SwitchMode,
    VerifyMode,
    Install,
    Uninstall,
}

#[derive(Debug, Clone)]
enum PendingSystemAction {
    Install {
        monitor: Box<Monitor>,
        workspace: Box<EdidWorkspace>,
        hyprland_mode: String,
        output_dir: PathBuf,
        operation: SystemOperation,
    },
    Uninstall(UninstallPlan),
}

#[derive(Debug, Clone)]
enum SystemExecution {
    Install(InstallPlan),
    Uninstall(UninstallPlan),
}

#[derive(Debug, Clone, PartialEq)]
enum ExtensionRow {
    Video {
        extension_index: u8,
        descriptor: CtaVideoDescriptor,
    },
    Dtd(CtaDtdSlot),
    DisplayIdDtd(DisplayIdDetailedTiming),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ModeKey {
    width: u16,
    height: u16,
    refresh_millihz: u32,
    interlaced: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModeProvenance {
    key: ModeKey,
    sources: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HitTarget {
    MonitorSelector,
    EstablishedRow(usize),
    DetailedRow(usize),
    StandardRow(usize),
    ExtensionRow(usize),
    SectionButton(ResolutionSection, SectionAction),
    GlobalButton(GlobalAction),
    ModalField(state::EditorField),
    ImportPathField,
    VrrField(usize),
    ModalButton(state::ModalButton),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Hitbox {
    rect: Rect,
    target: HitTarget,
    z: u8,
}

impl App {
    pub fn new(monitors: Vec<Monitor>) -> Self {
        let workspaces: Vec<Option<EdidWorkspace>> = monitors
            .iter()
            .map(|monitor| {
                monitor
                    .edid
                    .as_ref()
                    .and_then(|edid| EdidWorkspace::from_edid(edid).ok())
            })
            .collect();
        let override_statuses = monitors
            .iter()
            .map(|monitor| inspect_connector_override(&monitor.connector))
            .collect();

        let status = if monitors.is_empty() {
            "No DRM connectors found under /sys/class/drm. Run drmcru doctor for details."
                .to_string()
        } else if workspaces.iter().all(Option::is_none) {
            "No readable monitor EDID found. Select a connector or run drmcru doctor for details."
                .to_string()
        } else {
            "Add/Edit timings, switch exposed modes, or install EDID overrides. Press ? for help."
                .to_string()
        };

        Self {
            monitors,
            workspaces,
            override_statuses,
            selected_monitor: 0,
            selected_detailed: None,
            selected_standard: None,
            selected_established: None,
            selected_extension: None,
            established_scroll: 0,
            detailed_scroll: 0,
            standard_scroll: 0,
            extension_scroll: 0,
            focus: FocusArea::Detailed,
            draft_timing: cvt_reduced_blanking(CvtRequest {
                width: 2560,
                height: 1440,
                refresh_hz: 144.0,
            }),
            detailed_clipboard: None,
            detailed_editor: None,
            standard_editor: None,
            vrr_editor: None,
            import_dialog: None,
            details_dialog: None,
            export_confirm_dialog: None,
            export_dialog: None,
            apply_confirm_dialog: None,
            apply_result_dialog: None,
            applying_in_progress: false,
            applying_operation: None,
            pending_system_action: None,
            install_receiver: None,
            hitboxes: Vec::new(),
            status,
        }
    }

    pub fn run(&mut self) -> Result<()> {
        let mut terminal = TerminalSession::enter()?;

        loop {
            // Poll for async install completion
            self.poll_install_result();

            terminal.draw(|frame| self.draw(frame))?;
            if event::poll(Duration::from_millis(200))? {
                match event::read()? {
                    Event::Key(key) => {
                        if self.applying_in_progress {
                            // Swallow all keys while install is running
                        } else if self.vrr_editor.is_some() {
                            self.handle_vrr_editor_key(key);
                        } else if self.detailed_editor.is_some() {
                            self.handle_detailed_editor_key(key);
                        } else if self.standard_editor.is_some() {
                            self.handle_standard_editor_key(key);
                        } else if self.import_dialog.is_some() {
                            self.handle_import_dialog_key(key);
                        } else if self.details_dialog.is_some() {
                            self.handle_details_dialog_key(key);
                        } else if self.export_confirm_dialog.is_some() {
                            self.handle_export_confirm_key(key);
                        } else if self.export_dialog.is_some() {
                            self.handle_export_dialog_key(key);
                        } else if self.apply_confirm_dialog.is_some() {
                            self.handle_apply_confirm_key(key);
                        } else if self.apply_result_dialog.is_some() {
                            self.handle_apply_result_key(key);
                        } else if self.handle_main_key(key) {
                            break;
                        }
                    }
                    Event::Mouse(mouse) if !self.applying_in_progress => {
                        self.handle_mouse(mouse);
                    }
                    _ => {}
                }
            }
        }

        Ok(())
    }

    fn selected_monitor(&self) -> Option<&Monitor> {
        self.monitors.get(self.selected_monitor)
    }

    fn modal_open(&self) -> bool {
        self.vrr_editor.is_some()
            || self.detailed_editor.is_some()
            || self.standard_editor.is_some()
            || self.import_dialog.is_some()
            || self.details_dialog.is_some()
            || self.export_confirm_dialog.is_some()
            || self.export_dialog.is_some()
            || self.apply_confirm_dialog.is_some()
            || self.apply_result_dialog.is_some()
            || self.applying_in_progress
    }

    fn selected_override_status(&self) -> Option<&InstalledOverrideStatus> {
        self.override_statuses.get(self.selected_monitor)
    }

    fn selected_override_present(&self) -> bool {
        self.selected_override_status()
            .is_some_and(InstalledOverrideStatus::has_any_override)
    }

    fn refresh_override_statuses(&mut self) {
        self.override_statuses = self
            .monitors
            .iter()
            .map(|monitor| inspect_connector_override(&monitor.connector))
            .collect();
    }

    fn selected_edid(&self) -> Option<&EdidData> {
        self.selected_workspace()
            .map(EdidWorkspace::parsed)
            .or_else(|| {
                self.selected_monitor()
                    .and_then(|monitor| monitor.edid.as_ref())
            })
    }

    fn selected_workspace(&self) -> Option<&EdidWorkspace> {
        self.workspaces
            .get(self.selected_monitor)
            .and_then(Option::as_ref)
    }

    fn selected_workspace_mut(&mut self) -> Option<&mut EdidWorkspace> {
        self.workspaces
            .get_mut(self.selected_monitor)
            .and_then(Option::as_mut)
    }

    fn working_dtds(&self) -> Vec<LocatedDetailedTiming> {
        self.selected_workspace()
            .and_then(|workspace| workspace.dtds().ok())
            .unwrap_or_default()
    }

    fn working_standard_timings(&self) -> Vec<LocatedStandardTiming> {
        self.selected_workspace()
            .and_then(|workspace| workspace.standard_timings().ok())
            .unwrap_or_default()
    }

    fn working_cta_dtd_slots(&self) -> Vec<CtaDtdSlot> {
        self.selected_workspace()
            .and_then(|workspace| workspace.cta_dtd_slots().ok())
            .unwrap_or_default()
    }

    fn working_extension_rows(&self) -> Vec<ExtensionRow> {
        let video_rows = self
            .selected_edid()
            .into_iter()
            .flat_map(|edid| edid.cta_blocks.iter())
            .flat_map(|cta| {
                cta.data_blocks
                    .iter()
                    .flat_map(|block| block.video_modes.iter())
                    .cloned()
                    .map(|descriptor| ExtensionRow::Video {
                        extension_index: cta.extension_index,
                        descriptor,
                    })
            });
        let dtd_rows = self
            .working_cta_dtd_slots()
            .into_iter()
            .map(ExtensionRow::Dtd);
        let displayid_rows = self
            .selected_edid()
            .into_iter()
            .flat_map(|edid| edid.displayid_blocks.iter())
            .flat_map(|block| block.detailed_timings.iter())
            .cloned()
            .map(ExtensionRow::DisplayIdDtd);

        video_rows.chain(dtd_rows).chain(displayid_rows).collect()
    }

    fn mode_provenance_map(&self) -> BTreeMap<ModeKey, Vec<String>> {
        let mut map: BTreeMap<ModeKey, Vec<String>> = BTreeMap::new();

        if let Some(edid) = self.selected_edid() {
            for (index, timing) in edid.established_timings.iter().enumerate() {
                push_mode_source(
                    &mut map,
                    ModeKey::new(
                        timing.width,
                        timing.height,
                        f64::from(timing.refresh_hz),
                        false,
                    ),
                    format!("Established row {}", index + 1),
                );
            }
        }

        for row in self.working_standard_timings() {
            push_mode_source(
                &mut map,
                ModeKey::new(
                    row.timing.width,
                    row.timing.height,
                    f64::from(row.timing.refresh_hz),
                    false,
                ),
                format!("Standard slot {}", row.slot),
            );
        }

        for row in self.working_dtds() {
            if let Some(key) = ModeKey::from_timing(&row.timing) {
                push_mode_source(&mut map, key, format_location(row.location));
            }
        }

        for row in self.working_extension_rows() {
            match row {
                ExtensionRow::Video {
                    extension_index,
                    descriptor: CtaVideoDescriptor::Known(mode),
                } => push_mode_source(
                    &mut map,
                    ModeKey::new(mode.width, mode.height, mode.refresh_hz(), mode.interlaced),
                    format!("CTA ext {extension_index} VIC {}", mode.vic),
                ),
                ExtensionRow::Dtd(_) => {}
                ExtensionRow::DisplayIdDtd(row) => {
                    if let Some(key) = ModeKey::from_timing(&row.timing) {
                        push_mode_source(
                            &mut map,
                            key,
                            format!(
                                "DisplayID ext {} Type I DTD {}",
                                row.extension_index, row.descriptor_index
                            ),
                        );
                    }
                }
                _ => {}
            }
        }

        map
    }

    fn mode_provenance(&self, key: ModeKey) -> ModeProvenance {
        let sources = self
            .mode_provenance_map()
            .remove(&key)
            .unwrap_or_else(|| vec!["Selected row".to_string()]);
        ModeProvenance { key, sources }
    }

    fn provenance_suffix(&self, key: ModeKey) -> String {
        let sources = self.mode_provenance_map().remove(&key).unwrap_or_default();
        if sources.len() > 1 {
            format!("  [{} sources]", sources.len())
        } else {
            String::new()
        }
    }

    fn next_focus(&mut self) {
        self.focus = match self.focus {
            FocusArea::Monitor => FocusArea::Established,
            FocusArea::Established => FocusArea::Detailed,
            FocusArea::Detailed => FocusArea::Standard,
            FocusArea::Standard => FocusArea::Extension,
            FocusArea::Extension => FocusArea::Global,
            FocusArea::Global => FocusArea::Monitor,
        };
    }

    fn previous_focus(&mut self) {
        self.focus = match self.focus {
            FocusArea::Monitor => FocusArea::Global,
            FocusArea::Established => FocusArea::Monitor,
            FocusArea::Detailed => FocusArea::Established,
            FocusArea::Standard => FocusArea::Detailed,
            FocusArea::Extension => FocusArea::Standard,
            FocusArea::Global => FocusArea::Extension,
        };
    }

    fn move_selection(&mut self, delta: isize) {
        match self.focus {
            FocusArea::Monitor => self.move_monitor(delta),
            FocusArea::Established => self.move_established(delta),
            FocusArea::Detailed => self.move_detailed(delta),
            FocusArea::Standard => self.move_standard(delta),
            FocusArea::Extension => self.move_extension(delta),
            _ => {}
        }
    }

    fn move_monitor(&mut self, delta: isize) {
        if self.monitors.is_empty() {
            return;
        }
        self.selected_monitor = wrap_index(self.selected_monitor, self.monitors.len(), delta);
        self.selected_detailed = None;
        self.selected_standard = None;
        self.selected_established = None;
        self.selected_extension = None;
        self.established_scroll = 0;
        self.detailed_scroll = 0;
        self.standard_scroll = 0;
        self.extension_scroll = 0;
    }

    fn move_established(&mut self, delta: isize) {
        let len = self
            .selected_edid()
            .map(|edid| edid.established_timings.len())
            .unwrap_or_default();
        self.selected_established = next_list_selection(self.selected_established, len, delta);
    }

    fn move_detailed(&mut self, delta: isize) {
        let len = self.working_dtds().len();
        self.selected_detailed = next_list_selection(self.selected_detailed, len, delta);
    }

    fn move_standard(&mut self, delta: isize) {
        let len = self.working_standard_timings().len();
        self.selected_standard = next_list_selection(self.selected_standard, len, delta);
    }

    fn move_extension(&mut self, delta: isize) {
        let len = self.working_extension_rows().len();
        self.selected_extension = next_list_selection(self.selected_extension, len, delta);
    }

    fn activate_focused(&mut self) {
        match self.focus {
            FocusArea::Monitor => self.move_monitor(1),
            FocusArea::Established => self.open_selected_details(),
            FocusArea::Detailed if self.selected_detailed.is_some() => {
                self.edit_selected_detailed()
            }
            FocusArea::Detailed => self.open_detailed_editor(state::EditorMode::Add),
            FocusArea::Standard if self.selected_standard.is_some() => {
                self.edit_selected_standard()
            }
            FocusArea::Standard => self.open_standard_editor(state::StandardEditorMode::Add),
            FocusArea::Extension => self.activate_extension_default(),
            FocusArea::Global => self.export_selected_monitor(),
        }
    }
}

fn next_list_selection(current: Option<usize>, len: usize, delta: isize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(match current {
        Some(current) => wrap_index(current, len, delta),
        None if delta < 0 => len - 1,
        None => 0,
    })
}

fn keep_selected_visible(
    selected: Option<usize>,
    scroll: &mut usize,
    len: usize,
    viewport_height: u16,
) {
    let viewport = usize::from(viewport_height).max(1);
    if len <= viewport {
        *scroll = 0;
        return;
    }

    let max_scroll = len.saturating_sub(viewport);
    *scroll = (*scroll).min(max_scroll);

    let Some(selected) = selected else {
        return;
    };
    if selected < *scroll {
        *scroll = selected;
    } else if selected >= *scroll + viewport {
        *scroll = selected.saturating_add(1).saturating_sub(viewport);
    }
}

fn scroll_title(label: &str, scroll: usize, len: usize, viewport_height: u16) -> String {
    let viewport = usize::from(viewport_height).max(1);
    if len <= viewport || len == 0 {
        return label.to_string();
    }

    let start = scroll.saturating_add(1).min(len);
    let end = (scroll + viewport).min(len);
    let up = if scroll > 0 { "↑" } else { " " };
    let down = if end < len { "↓" } else { " " };
    format!("{label} {up}{down} {start}-{end}/{len}")
}

fn inspect_connector_override(connector: &str) -> InstalledOverrideStatus {
    let edid_file_name = custom_edid_file_name(connector);
    let plan = UninstallPlan {
        connector: connector.to_string(),
        kernel_parameter: format!("drm.edid_firmware={connector}:edid/{edid_file_name}"),
        edid_file_name,
    };
    install::inspect_installed_override(&plan)
}

impl ModeKey {
    fn new(width: u16, height: u16, refresh_hz: f64, interlaced: bool) -> Self {
        Self {
            width,
            height,
            refresh_millihz: (refresh_hz * 1000.0)
                .round()
                .clamp(0.0, f64::from(u32::MAX)) as u32,
            interlaced,
        }
    }

    fn from_timing(timing: &TimingDescriptor) -> Option<Self> {
        Some(Self::new(
            timing.h_active,
            timing.v_active,
            timing.refresh_hz()?,
            timing.interlaced,
        ))
    }

    fn refresh_hz(self) -> f64 {
        f64::from(self.refresh_millihz) / 1000.0
    }
}

fn push_mode_source(map: &mut BTreeMap<ModeKey, Vec<String>>, key: ModeKey, source: String) {
    let sources = map.entry(key).or_default();
    if !sources.iter().any(|existing| existing == &source) {
        sources.push(source);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edid::{DtdLocation, encode_detailed_timing, parse_edid};
    use crate::models::ConnectorStatus;

    fn timing() -> TimingDescriptor {
        TimingDescriptor {
            pixel_clock_khz: 167_000,
            h_active: 1600,
            h_blanking: 160,
            h_front_porch: 48,
            h_sync_width: 32,
            h_back_porch: 80,
            v_active: 900,
            v_blanking: 50,
            v_front_porch: 3,
            v_sync_width: 5,
            v_back_porch: 42,
            h_sync_positive: true,
            v_sync_positive: false,
            interlaced: false,
        }
    }

    fn repair_checksum(block: &mut [u8]) {
        let last = block.len() - 1;
        block[last] = 0;
        let sum = block[..last]
            .iter()
            .fold(0u8, |acc, byte| acc.wrapping_add(*byte));
        block[last] = 0u8.wrapping_sub(sum);
    }

    fn monitor_with_cta_dtd() -> Monitor {
        let mut raw = vec![0u8; 256];
        raw[..8].copy_from_slice(&[0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00]);
        raw[38..54].fill(0x01);
        raw[126] = 1;
        repair_checksum(&mut raw[..128]);

        let cta = &mut raw[128..];
        cta[0] = 0x02;
        cta[1] = 3;
        cta[2] = 6;
        cta[4] = (2 << 5) | 1;
        cta[5] = 16;
        cta[6..24].copy_from_slice(&encode_detailed_timing(&timing()).unwrap());
        repair_checksum(cta);

        Monitor {
            connector: "DP-TEST".to_string(),
            drm_path: None,
            status: ConnectorStatus::Connected,
            hyprland: None,
            edid: Some(parse_edid(raw).unwrap()),
        }
    }

    #[test]
    fn extension_edit_resolves_cta_location_after_video_rows() {
        let expected = timing();
        let mut app = App::new(vec![monitor_with_cta_dtd()]);
        app.selected_extension = app
            .working_extension_rows()
            .iter()
            .position(|row| matches!(row, ExtensionRow::Dtd(slot) if slot.timing.is_some()));

        app.edit_selected_extension_dtd();

        let editor = app.detailed_editor.as_ref().expect("DTD editor");
        assert_eq!(editor.timing().unwrap(), expected);
        assert!(matches!(
            editor.mode,
            state::EditorMode::EditCta {
                location: DtdLocation::Cta {
                    extension_index: 1,
                    slot: 0
                },
                ..
            }
        ));
    }

    #[test]
    fn cta_dtd_has_one_provenance_source() {
        let app = App::new(vec![monitor_with_cta_dtd()]);
        let key = ModeKey::from_timing(&timing()).unwrap();

        assert_eq!(app.mode_provenance(key).sources.len(), 1);
    }

    #[test]
    fn modal_mouse_events_cannot_change_background_selection() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let mut app = App::new(vec![monitor_with_cta_dtd(), monitor_with_cta_dtd()]);
        app.open_detailed_editor(state::EditorMode::Add);
        app.push_hitbox(Rect::new(0, 0, 20, 3), HitTarget::MonitorSelector, 0);
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 1,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.selected_monitor, 0);
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 1,
            row: 1,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.selected_detailed, None);
    }

    #[test]
    fn compact_terminal_keeps_active_editor_and_extension_rows_visible() {
        use ratatui::backend::TestBackend;
        let mut app = App::new(vec![monitor_with_cta_dtd()]);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        app.open_detailed_editor(state::EditorMode::Add);
        for _ in 0..19 {
            let active = app.detailed_editor.as_ref().unwrap().active_field;
            terminal.draw(|frame| app.draw(frame)).unwrap();
            assert!(
                app.hitboxes
                    .iter()
                    .any(|hitbox| hitbox.target == HitTarget::ModalField(active)
                        && hitbox.rect.height > 0)
            );
            app.detailed_editor.as_mut().unwrap().next_field();
        }
        assert!(app.hitboxes.iter().any(|hitbox| hitbox.target
            == HitTarget::ModalButton(state::ModalButton::Ok)
            && hitbox.rect.height > 0));

        app.detailed_editor = None;
        app.selected_extension = Some(3);
        terminal.draw(|frame| app.draw(frame)).unwrap();
        assert!(app.hitboxes.iter().any(|hitbox| hitbox.target == HitTarget::ExtensionRow(3) && hitbox.rect.height > 0));
    }

    #[test]
    fn deleting_last_detailed_and_standard_rows_clears_selection() {
        let mut app = App::new(vec![monitor_with_cta_dtd()]);
        app.selected_detailed = Some(0);
        app.delete_selected_detailed();
        assert!(app.working_dtds().is_empty());
        assert_eq!(app.selected_detailed, None);

        let standard = crate::models::StandardTiming {
            slot: 0,
            width: 1920,
            height: 1080,
            refresh_hz: 60,
            aspect: crate::models::StandardTimingAspect::SixteenNine,
        };
        app.selected_workspace_mut()
            .unwrap()
            .add_standard_timing(standard)
            .unwrap();
        app.selected_standard = Some(0);
        app.delete_selected_standard();
        assert!(app.working_standard_timings().is_empty());
        assert_eq!(app.selected_standard, None);
    }

    #[test]
    fn cta_delete_keeps_selection_after_video_rows() {
        let mut app = App::new(vec![monitor_with_cta_dtd()]);
        let location = DtdLocation::Cta {
            extension_index: 1,
            slot: 5,
        };
        app.selected_workspace_mut()
            .unwrap()
            .add_cta_dtd_at(location, timing())
            .unwrap();
        app.selected_extension = Some(6);
        app.delete_selected_extension_dtd();
        assert_eq!(app.selected_extension, Some(6));
        assert!(
            matches!(&app.working_extension_rows()[6], ExtensionRow::Dtd(row) if row.slot == 5 && row.timing.is_none())
        );
    }

    #[test]
    fn workspace_export_checks_unknown_extension_checksums_and_length() {
        for truncate in [false, true] {
            let mut monitor = monitor_with_cta_dtd();
            let mut raw = monitor.edid.take().unwrap().raw;
            raw[128] = 0x40;
            repair_checksum(&mut raw[128..]);
            if truncate {
                raw.pop();
            } else {
                raw[129] ^= 1;
            }
            monitor.edid = Some(parse_edid(raw).unwrap());
            let mut app = App::new(vec![monitor]);
            app.selected_workspace_mut()
                .unwrap()
                .add_dtd(timing())
                .unwrap();
            app.export_selected_monitor();
            assert!(
                app.details_dialog
                    .as_ref()
                    .is_some_and(|dialog| dialog.title == "Export Blocked")
            );
            assert!(app.export_confirm_dialog.is_none());
        }
    }

    #[test]
    fn adding_a_timing_explicitly_can_append_a_cta_when_existing_slots_are_full() {
        let mut monitor = monitor_with_cta_dtd();
        let mut raw = monitor.edid.take().unwrap().raw;
        for slot in 0..4 {
            raw = crate::edid::patch_detailed_timing(&raw, DtdLocation::Base { slot }, &timing())
                .unwrap();
        }
        for slot in 0..6 {
            raw = crate::edid::patch_detailed_timing(
                &raw,
                DtdLocation::Cta {
                    extension_index: 1,
                    slot,
                },
                &timing(),
            )
            .unwrap();
        }
        monitor.edid = Some(parse_edid(raw).unwrap());
        let mut app = App::new(vec![monitor]);
        app.open_detailed_editor(state::EditorMode::Add);
        app.apply_detailed_editor();

        assert!(app.detailed_editor.is_none());
        let workspace = app.selected_workspace().unwrap();
        assert!(workspace.has_changes());
        assert_eq!(workspace.parsed().raw.len(), 384);
        assert!(workspace.validate().is_empty());
    }

    #[test]
    fn export_without_a_monitor_or_edid_has_an_actionable_error() {
        let mut app = App::new(Vec::new());
        app.export_selected_monitor();
        assert_eq!(app.status, "No monitor selected.");

        let mut monitor = monitor_with_cta_dtd();
        monitor.edid = None;
        let mut app = App::new(vec![monitor]);
        app.export_selected_monitor();
        assert_eq!(
            app.status,
            "Selected monitor has no readable EDID to export."
        );
        assert!(app.export_confirm_dialog.is_none());
        assert!(app.export_dialog.is_none());
    }

    #[test]
    fn export_refuses_unchanged_non_stereo_workspace() {
        for flags in [0x1a, 0x19, 0x1b] {
            let mut monitor = monitor_with_cta_dtd();
            let mut raw = monitor.edid.take().unwrap().raw;
            raw[151] = flags;
            repair_checksum(&mut raw[128..]);
            monitor.edid = Some(parse_edid(raw.clone()).unwrap());
            let mut app = App::new(vec![monitor]);

            app.export_selected_monitor();

            assert!(app.export_confirm_dialog.is_none());
            assert!(app.export_dialog.is_none());
            assert!(app.details_dialog.is_none());
            assert!(app.status.contains("No workspace changes"));
            assert_eq!(app.selected_workspace().unwrap().export_bytes(), raw);
        }
    }

    #[test]
    fn stereo_only_export_uses_workspace_validation_instead_of_the_draft() {
        let mut monitor = monitor_with_cta_dtd();
        let mut raw = monitor.edid.take().unwrap().raw;
        raw[151] |= 0xe1;
        repair_checksum(&mut raw[128..]);
        monitor.edid = Some(parse_edid(raw.clone()).unwrap());
        let mut app = App::new(vec![monitor]);
        app.draft_timing.pixel_clock_khz = 0;

        app.export_selected_monitor();

        assert!(app.selected_workspace().unwrap().has_changes());
        assert!(app.export_confirm_dialog.is_some());
        assert!(app.details_dialog.is_none());
        assert_eq!(app.selected_workspace().unwrap().parsed().raw, raw);
    }

    #[test]
    fn apply_refuses_unchanged_workspace() {
        let mut app = App::new(vec![monitor_with_cta_dtd()]);

        app.apply_selected_monitor();

        assert!(app.pending_system_action.is_none());
        assert!(app.apply_confirm_dialog.is_none());
        assert!(app.status.contains("No workspace changes"));
    }

    #[test]
    fn vrr_dialog_rejects_invalid_input_and_saves_a_valid_range() {
        let mut monitor = monitor_with_cta_dtd();
        let mut raw = monitor.edid.as_ref().unwrap().raw.clone();
        raw[18] = 1;
        raw[19] = 4;
        raw[24] |= 1;
        raw[108..126].copy_from_slice(&[
            0, 0, 0, 0xfd, 0, 24, 120, 30, 200, 60, 1, 10, 0, 0, 0, 0, 0, 0,
        ]);
        repair_checksum(&mut raw[..128]);
        monitor.edid = Some(parse_edid(raw).unwrap());
        let mut app = App::new(vec![monitor]);
        app.open_help_dialog();
        app.open_vrr_editor();
        assert!(app.vrr_editor.is_none());
        assert!(app.details_dialog.is_some());
        app.details_dialog = None;
        app.handle_main_key(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Char('R'),
        ));
        app.vrr_editor.as_mut().unwrap().inputs[0].set("121".to_string());
        app.apply_vrr_editor();
        assert!(!app.vrr_editor.as_ref().unwrap().error.is_empty());
        assert!(!app.selected_workspace().unwrap().has_changes());
        app.vrr_editor.as_mut().unwrap().inputs[0].set("40".to_string());
        app.apply_vrr_editor();
        assert!(app.vrr_editor.is_none());
        assert!(app.selected_workspace().unwrap().has_changes());
        app.open_vrr_editor();
        assert_eq!(app.vrr_editor.as_ref().unwrap().inputs[0].buffer, "40");
        assert!(app.modal_open());
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let contents = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(contents.contains("VRR range limits"));
        assert!(contents.contains("Minimum Hz: [40|]"));
        app.handle_vrr_editor_key(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Esc,
        ));
        assert!(app.vrr_editor.is_none());
    }

    #[test]
    fn empty_discovery_has_actionable_first_run_status() {
        let app = App::new(Vec::new());

        assert!(app.status.contains("No DRM connectors"));
        assert!(app.status.contains("drmcru doctor"));
    }

    #[test]
    fn enter_edits_a_selected_detailed_timing() {
        let mut app = App::new(vec![monitor_with_cta_dtd()]);
        app.focus = FocusArea::Detailed;
        app.selected_detailed = Some(0);

        app.activate_focused();

        assert!(app.detailed_editor.is_some());
        assert!(matches!(
            app.detailed_editor.as_ref().map(|editor| editor.mode),
            Some(state::EditorMode::Edit { .. })
        ));
    }

    #[test]
    fn first_keyboard_movement_selects_the_nearest_row() {
        assert_eq!(next_list_selection(None, 4, 1), Some(0));
        assert_eq!(next_list_selection(None, 4, -1), Some(3));
        assert_eq!(next_list_selection(None, 0, 1), None);
    }

    #[test]
    fn import_paths_expand_home_and_terminal_quotes() {
        let home = PathBuf::from("/home/example");

        assert_eq!(
            actions::normalize_import_path("~/Downloads/display.bin", Some(&home)),
            home.join("Downloads/display.bin")
        );
        assert_eq!(
            actions::normalize_import_path("'/tmp/display file.bin'", Some(&home)),
            PathBuf::from("/tmp/display file.bin")
        );
    }

    #[test]
    fn workspace_export_validation_errors_cannot_be_bypassed() {
        let mut app = App::new(vec![monitor_with_cta_dtd()]);
        let mut invalid_timing = timing();
        invalid_timing.h_blanking = 16;
        app.selected_workspace_mut()
            .unwrap()
            .add_dtd(invalid_timing)
            .unwrap();

        app.export_selected_monitor();

        assert!(app.export_confirm_dialog.is_none());
        assert!(
            app.details_dialog
                .as_ref()
                .is_some_and(|dialog| dialog.title == "Export Blocked")
        );
        assert!(app.status.contains("blocked"));
    }
}
