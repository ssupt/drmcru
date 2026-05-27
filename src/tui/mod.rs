use crate::edid::{CtaDtdSlot, LocatedDetailedTiming, LocatedStandardTiming};
use crate::export::custom_edid_file_name;
use crate::install::{
    self, InstallError, InstallPlan, InstallReport, InstalledOverrideStatus, UninstallPlan,
};
use crate::models::{CtaVideoDescriptor, EdidData, Monitor, TimingDescriptor};
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
    Import,
    Export,
    SwitchMode,
    VerifyMode,
    Install,
    Uninstall,
    Ok,
    Cancel,
}

#[derive(Debug, Clone)]
enum PendingSystemAction {
    Install {
        plan: InstallPlan,
        operation: SystemOperation,
    },
    Uninstall(UninstallPlan),
}

#[derive(Debug, Clone, PartialEq)]
enum ExtensionRow {
    Video {
        extension_index: u8,
        descriptor: CtaVideoDescriptor,
    },
    Dtd(CtaDtdSlot),
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
    EstablishedCheckbox(usize),
    DetailedRow(usize),
    StandardRow(usize),
    ExtensionRow(usize),
    SectionButton(ResolutionSection, SectionAction),
    GlobalButton(GlobalAction),
    ModalField(state::EditorField),
    ImportPathField,
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
        let workspaces = monitors
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
            status:
                "Add/Edit timings, switch exposed modes, or install EDID overrides. Press ? for help."
                    .to_string(),
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

        video_rows.chain(dtd_rows).collect()
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
                ExtensionRow::Dtd(row) => {
                    if let Some(timing) = row
                        .timing
                        .and_then(|timing| ModeKey::from_timing(&timing).map(|key| (key, timing)))
                    {
                        let (key, _) = timing;
                        push_mode_source(
                            &mut map,
                            key,
                            format!("CTA ext {} DTD slot {}", row.extension_index, row.slot),
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
        if len == 0 {
            return;
        }
        let current = self.selected_established.unwrap_or(0);
        self.selected_established = Some(wrap_index(current, len, delta));
    }

    fn move_detailed(&mut self, delta: isize) {
        let len = self.working_dtds().len();
        if len == 0 {
            self.selected_detailed = None;
            return;
        }
        let current = self.selected_detailed.unwrap_or(0);
        self.selected_detailed = Some(wrap_index(current, len, delta));
    }

    fn move_standard(&mut self, delta: isize) {
        let len = self.working_standard_timings().len();
        if len == 0 {
            self.selected_standard = None;
            return;
        }
        let current = self.selected_standard.unwrap_or(0);
        self.selected_standard = Some(wrap_index(current, len, delta));
    }

    fn move_extension(&mut self, delta: isize) {
        let len = self.working_extension_rows().len();
        if len == 0 {
            self.selected_extension = None;
            return;
        }
        let current = self.selected_extension.unwrap_or(0);
        self.selected_extension = Some(wrap_index(current, len, delta));
    }

    fn activate_focused(&mut self) {
        match self.focus {
            FocusArea::Detailed => self.open_detailed_editor(state::EditorMode::Add),
            FocusArea::Standard => self.open_standard_editor(state::StandardEditorMode::Add),
            FocusArea::Extension => self.activate_extension_default(),
            FocusArea::Global => self.export_selected_monitor(),
            _ => {}
        }
    }
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
