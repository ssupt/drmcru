use super::state::{
    ApplyConfirmDialog, ApplyResultDialog, DetailedResolutionEditor, DetailsDialog, EditorMode,
    ExportConfirmDialog, ExportDialog, ImportDialog, StandardEditorMode, StandardResolutionEditor,
    SystemOperation,
};
use super::{
    App, ExtensionRow, FocusArea, ModeKey, ModeProvenance, PendingSystemAction, SystemExecution,
};
use crate::edid::DtdLocation;
use crate::export::{custom_edid_file_name, export_patched_edid, export_workspace_edid};
use crate::hyprland::{self, ModeRequest};
use crate::hyprland_config::{self, MonitorRuleInspection};
use crate::install::{self, InstallPlan, InstallPreview, UninstallPlan, UninstallPreview};
use crate::models::{CtaVideoDescriptor, StandardTiming, StandardTimingAspect};
use crate::validation::{TimingWarningSeverity, internal_panel_scaling_warning, validate_timing};
use crate::workspace::{EdidWorkspace, MoveDirection, format_location};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
struct SwitchModeCandidate {
    request: ModeRequest,
    source: &'static str,
}

impl App {
    pub(super) fn open_detailed_editor(&mut self, mode: EditorMode) {
        self.import_dialog = None;
        self.details_dialog = None;
        self.standard_editor = None;
        self.export_confirm_dialog = None;
        self.export_dialog = None;
        let timing = match mode {
            EditorMode::Add | EditorMode::AddCta { .. } => self.draft_timing.clone(),
            EditorMode::Edit { index, .. } => self
                .working_dtds()
                .get(index)
                .map(|row| row.timing.clone())
                .unwrap_or_else(|| self.draft_timing.clone()),
            EditorMode::EditCta { location, .. } => self
                .working_cta_dtd_slots()
                .into_iter()
                .find(|row| {
                    DtdLocation::Cta {
                        extension_index: row.extension_index,
                        slot: row.slot,
                    } == location
                })
                .and_then(|row| row.timing.clone())
                .unwrap_or_else(|| self.draft_timing.clone()),
        };
        self.detailed_editor = Some(DetailedResolutionEditor::from_timing(timing, mode));
        self.status = "Editing detailed resolution. Tab changes field, Enter applies, Esc cancels."
            .to_string();
    }

    pub(super) fn open_standard_editor(&mut self, mode: StandardEditorMode) {
        self.import_dialog = None;
        self.details_dialog = None;
        self.detailed_editor = None;
        self.export_confirm_dialog = None;
        self.export_dialog = None;
        let timing = match mode {
            StandardEditorMode::Add => self
                .working_standard_timings()
                .last()
                .map(|row| row.timing.clone())
                .unwrap_or_else(|| StandardTiming {
                    slot: 0,
                    width: 1920,
                    height: 1080,
                    refresh_hz: 60,
                    aspect: StandardTimingAspect::SixteenNine,
                }),
            StandardEditorMode::Edit { index, .. } => self
                .working_standard_timings()
                .get(index)
                .map(|row| row.timing.clone())
                .unwrap_or_else(|| StandardTiming {
                    slot: 0,
                    width: 1920,
                    height: 1080,
                    refresh_hz: 60,
                    aspect: StandardTimingAspect::SixteenNine,
                }),
        };
        self.standard_editor = Some(StandardResolutionEditor::from_timing(timing, mode));
        self.status =
            "Editing standard resolution. Values must fit EDID's compact standard-timing format."
                .to_string();
    }

    pub(super) fn open_import_dialog(&mut self) {
        self.detailed_editor = None;
        self.standard_editor = None;
        self.details_dialog = None;
        self.export_confirm_dialog = None;
        self.export_dialog = None;
        self.import_dialog = Some(ImportDialog::default());
        self.status = "Enter an EDID .bin path. Import validates and loads it into the workspace."
            .to_string();
    }

    pub(super) fn open_help_dialog(&mut self) {
        self.detailed_editor = None;
        self.standard_editor = None;
        self.import_dialog = None;
        self.export_confirm_dialog = None;
        self.export_dialog = None;
        self.apply_confirm_dialog = None;
        self.apply_result_dialog = None;
        self.details_dialog = Some(DetailsDialog::new("drmcru Help", help_lines()));
        self.status = "Help opened. Scroll with Up/Down, PageUp/PageDown, Home/End.".to_string();
    }

    pub(super) fn open_selected_details(&mut self) {
        let dialog = match self.focus {
            FocusArea::Monitor => self.monitor_details(),
            FocusArea::Established => self.established_details(),
            FocusArea::Detailed => self.detailed_details(),
            FocusArea::Standard => self.standard_details(),
            FocusArea::Extension => self.extension_details(),
            FocusArea::Global => None,
        };

        let Some(dialog) = dialog else {
            self.status = "Select a monitor or resolution row before opening details.".to_string();
            return;
        };

        self.detailed_editor = None;
        self.standard_editor = None;
        self.import_dialog = None;
        self.export_confirm_dialog = None;
        self.export_dialog = None;
        self.apply_confirm_dialog = None;
        self.apply_result_dialog = None;
        self.details_dialog = Some(dialog);
        self.status = "Viewing selected row details. Enter or Esc closes.".to_string();
    }

    fn monitor_details(&self) -> Option<DetailsDialog> {
        let monitor = self.selected_monitor()?;
        let mut lines = vec![
            format!("Connector: {}", monitor.connector),
            format!(
                "DRM path: {}",
                monitor
                    .drm_path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "(unknown)".to_string())
            ),
            format!("Status: {:?}", monitor.status),
        ];

        if let Some(hypr) = &monitor.hyprland {
            lines.extend([
                format!("Hyprland name: {}", hypr.name),
                format!("Description: {}", hypr.description),
                format!(
                    "Current mode: {}x{} @ {:.3} Hz",
                    hypr.active_width.unwrap_or_default(),
                    hypr.active_height.unwrap_or_default(),
                    hypr.refresh_hz.unwrap_or_default()
                ),
                format!("Available modes reported: {}", hypr.available_modes.len()),
            ]);
        }

        if let Some(edid) = &monitor.edid {
            lines.extend([
                format!(
                    "EDID monitor name: {}",
                    edid.monitor_name.as_deref().unwrap_or("(none)")
                ),
                format!(
                    "Manufacturer: {}",
                    edid.manufacturer_id.as_deref().unwrap_or("(unknown)")
                ),
                format!("Extension blocks: {}", edid.extension_blocks),
                format!("CTA blocks: {}", edid.cta_blocks.len()),
                format!("DisplayID blocks: {}", edid.displayid_blocks.len()),
                format!("Base checksum: {}", checksum_label(edid.checksum_valid)),
            ]);
        }

        if let Some(status) = self.selected_override_status() {
            append_override_status_lines(
                &mut lines,
                status,
                monitor.edid.as_ref().map(|edid| edid.raw.as_slice()),
            );
        }

        append_connector_config_lines(&mut lines, &monitor.connector);

        Some(DetailsDialog::new("Monitor Details", lines))
    }

    fn established_details(&self) -> Option<DetailsDialog> {
        let index = self.selected_established?;
        let timing = self.selected_edid()?.established_timings.get(index)?;
        Some(DetailsDialog::new(
            "Established Resolution Details",
            with_provenance_lines(
                vec![
                    "Source: Base EDID established timing bitmap".to_string(),
                    format!("Row: {}", index + 1),
                    format!(
                        "Mode: {}x{} @ {} Hz",
                        timing.width, timing.height, timing.refresh_hz
                    ),
                    "Editable: no; established timings are fixed EDID bit flags.".to_string(),
                ],
                self.mode_provenance(ModeKey::new(
                    timing.width,
                    timing.height,
                    f64::from(timing.refresh_hz),
                    false,
                )),
            ),
        ))
    }

    fn standard_details(&self) -> Option<DetailsDialog> {
        let index = self.selected_standard?;
        let row = self.working_standard_timings().get(index)?.clone();
        Some(DetailsDialog::new(
            "Standard Resolution Details",
            with_provenance_lines(
                vec![
                    "Source: Base EDID standard timing identifier".to_string(),
                    format!("Slot: {}", row.slot),
                    format!(
                        "Mode: {}x{} @ {} Hz",
                        row.timing.width, row.timing.height, row.timing.refresh_hz
                    ),
                    format!("Aspect encoding: {}", row.timing.aspect.label()),
                    "Limit: standard timings only encode 16:10, 4:3, 5:4, and 16:9 shapes."
                        .to_string(),
                ],
                self.mode_provenance(ModeKey::new(
                    row.timing.width,
                    row.timing.height,
                    f64::from(row.timing.refresh_hz),
                    false,
                )),
            ),
        ))
    }

    fn detailed_details(&self) -> Option<DetailsDialog> {
        let index = self.selected_detailed?;
        let row = self.working_dtds().get(index)?.clone();
        let mut lines = timing_detail_lines(
            "Source: Detailed Timing Descriptor",
            Some(format_location(row.location)),
            &row.timing,
        );
        if let Some(key) = ModeKey::from_timing(&row.timing) {
            lines = with_provenance_lines(lines, self.mode_provenance(key));
        }
        Some(DetailsDialog::new("Detailed Resolution Details", lines))
    }

    fn extension_details(&self) -> Option<DetailsDialog> {
        let index = self.selected_extension?;
        match self.working_extension_rows().get(index)?.clone() {
            ExtensionRow::Video {
                extension_index,
                descriptor,
            } => {
                let mut lines = vec![
                    "Source: CTA-861 Video Data Block short video descriptor".to_string(),
                    format!("CTA extension: {extension_index}"),
                    format!("Descriptor: {}", descriptor.label()),
                ];
                match descriptor {
                    CtaVideoDescriptor::Known(mode) => {
                        lines.extend([
                            "Mapped: yes".to_string(),
                            format!("VIC: {}", mode.vic),
                            format!("Native flag: {}", yes_no(mode.native)),
                            format!("Resolution: {}x{}", mode.width, mode.height),
                            format!("Refresh: {:.3} Hz", mode.refresh_hz()),
                            format!("Interlaced: {}", yes_no(mode.interlaced)),
                            "Editable: no; this is a CTA video capability row.".to_string(),
                        ]);
                        lines = with_provenance_lines(
                            lines,
                            self.mode_provenance(ModeKey::new(
                                mode.width,
                                mode.height,
                                mode.refresh_hz(),
                                mode.interlaced,
                            )),
                        );
                    }
                    CtaVideoDescriptor::Unknown { vic, native } => {
                        lines.extend([
                            "Mapped: no".to_string(),
                            format!("VIC: {vic}"),
                            format!("Native flag: {}", yes_no(native)),
                            "Switch: unavailable until this VIC is mapped.".to_string(),
                        ]);
                    }
                }
                Some(DetailsDialog::new("CTA Video Mode Details", lines))
            }
            ExtensionRow::Dtd(row) => {
                let location = format_location(crate::edid::DtdLocation::Cta {
                    extension_index: row.extension_index,
                    slot: row.slot,
                });
                let lines = if let Some(timing) = row.timing {
                    let mut lines = timing_detail_lines(
                        "Source: CTA-861 Detailed Timing Descriptor",
                        Some(location),
                        &timing,
                    );
                    if let Some(key) = ModeKey::from_timing(&timing) {
                        lines = with_provenance_lines(lines, self.mode_provenance(key));
                    }
                    lines
                } else {
                    vec![
                        "Source: CTA-861 Detailed Timing Descriptor slot".to_string(),
                        format!("Location: {location}"),
                        format!(
                            "State: {}",
                            if row.occupied_unknown {
                                "occupied unknown payload"
                            } else {
                                "free"
                            }
                        ),
                        format!("CTA revision: {}", row.revision),
                        format!("DTD offset: {}", row.dtd_offset),
                        format!("Checksum: {}", checksum_label(row.checksum_valid)),
                    ]
                };
                Some(DetailsDialog::new("CTA DTD Slot Details", lines))
            }
            ExtensionRow::DisplayIdDtd(row) => {
                let mut lines = timing_detail_lines(
                    "Source: DisplayID Type I Detailed Timing",
                    Some(format!(
                        "DisplayID extension {} data block {} DTD {}",
                        row.extension_index, row.data_block_index, row.descriptor_index
                    )),
                    &row.timing,
                );
                lines.extend([
                    format!("Preferred: {}", yes_no(row.preferred)),
                    format!("Raw flags: 0x{:02x}", row.raw_flags),
                    "Editable: no; copy it into a new detailed timing to edit.".to_string(),
                ]);
                if let Some(key) = ModeKey::from_timing(&row.timing) {
                    lines = with_provenance_lines(lines, self.mode_provenance(key));
                }
                Some(DetailsDialog::new("DisplayID DTD Details", lines))
            }
        }
    }

    pub(super) fn edit_selected_detailed(&mut self) {
        let rows = self.working_dtds();
        let Some(index) = self.selected_detailed else {
            self.status = "Select a detailed timing before Edit.".to_string();
            return;
        };
        let Some(row) = rows.get(index) else {
            self.status = "Selected detailed timing no longer exists.".to_string();
            return;
        };
        self.open_detailed_editor(EditorMode::Edit {
            index,
            location: row.location,
        });
    }

    pub(super) fn copy_selected_detailed(&mut self) {
        let rows = self.working_dtds();
        let Some(index) = self.selected_detailed else {
            self.status = "Select a detailed timing before Copy.".to_string();
            return;
        };
        let Some(row) = rows.get(index) else {
            self.status = "Selected detailed timing no longer exists.".to_string();
            return;
        };

        self.detailed_clipboard = Some(row.timing.clone());
        self.draft_timing = row.timing.clone();
        self.status = format!(
            "Copied {}. Press p or Add to paste it into a new detailed timing.",
            row.timing.hyprland_mode()
        );
    }

    pub(super) fn paste_detailed_as_new(&mut self) {
        let Some(timing) = self.detailed_clipboard.clone() else {
            self.status = "No copied detailed timing. Select one and press c first.".to_string();
            return;
        };

        self.draft_timing = timing;
        if self.focus == FocusArea::Extension {
            self.open_extension_add_editor();
            if self.detailed_editor.is_some() {
                self.status =
                    "Pasting copied detailed timing into the selected CTA slot.".to_string();
            }
        } else {
            self.open_detailed_editor(EditorMode::Add);
            self.status = "Pasting copied detailed timing into a new editable slot.".to_string();
        }
    }

    pub(super) fn activate_extension_default(&mut self) {
        match self.selected_extension_row() {
            Some(ExtensionRow::Video { descriptor, .. }) => {
                self.status = format!(
                    "{} is a read-only CTA video mode. Press s to switch mapped modes if Hyprland exposes them.",
                    descriptor.label()
                );
            }
            Some(ExtensionRow::Dtd(row)) if row.timing.is_some() => {
                self.edit_selected_extension_dtd();
            }
            Some(ExtensionRow::DisplayIdDtd(row)) => {
                self.status = format!(
                    "{} is a read-only DisplayID detailed timing. Press s to switch if Hyprland exposes it, or c to copy it.",
                    row.timing.hyprland_mode()
                );
            }
            _ => self.open_extension_add_editor(),
        }
    }

    pub(super) fn open_extension_add_editor(&mut self) {
        let Some((extension_index, slot)) = self.selected_or_first_free_cta_slot() else {
            self.status = "No free CTA extension DTD slot is available.".to_string();
            return;
        };

        self.open_detailed_editor(EditorMode::AddCta {
            extension_index,
            slot: Some(slot),
        });
        self.status =
            format!("Adding detailed timing to CTA-861 extension {extension_index} slot {slot}.");
    }

    pub(super) fn edit_selected_extension_dtd(&mut self) {
        let Some(index) = self.selected_extension else {
            self.status = "Select a CTA DTD slot before Edit.".to_string();
            return;
        };
        let Some(row) = self.selected_extension_slot() else {
            self.status = "Selected CTA DTD slot no longer exists.".to_string();
            return;
        };
        if row.timing.is_none() {
            self.status = "Selected CTA DTD slot is empty; use Add instead.".to_string();
            return;
        }

        self.open_detailed_editor(EditorMode::EditCta {
            extension_row: index,
            location: DtdLocation::Cta {
                extension_index: row.extension_index,
                slot: row.slot,
            },
        });
    }

    pub(super) fn copy_selected_extension_dtd(&mut self) {
        match self.selected_extension_row() {
            Some(ExtensionRow::Dtd(row)) => {
                let Some(timing) = row.timing else {
                    self.status = "Selected CTA DTD slot is empty.".to_string();
                    return;
                };
                self.detailed_clipboard = Some(timing.clone());
                self.draft_timing = timing.clone();
                self.status = format!(
                    "Copied CTA extension {} slot {}: {}.",
                    row.extension_index,
                    row.slot,
                    timing.hyprland_mode()
                );
            }
            Some(ExtensionRow::DisplayIdDtd(row)) => {
                self.detailed_clipboard = Some(row.timing.clone());
                self.draft_timing = row.timing.clone();
                self.status = format!(
                    "Copied DisplayID extension {} DTD {}: {}.",
                    row.extension_index,
                    row.descriptor_index,
                    row.timing.hyprland_mode()
                );
            }
            _ => {
                self.status = "Select a detailed timing row before Copy.".to_string();
            }
        }
    }

    pub(super) fn delete_selected_detailed(&mut self) {
        let rows = self.working_dtds();
        let Some(index) = self.selected_detailed else {
            self.status = "Select a detailed timing before Delete.".to_string();
            return;
        };
        let Some(row) = rows.get(index) else {
            self.status = "Selected detailed timing no longer exists.".to_string();
            return;
        };
        let location = row.location;
        match self
            .selected_workspace_mut()
            .and_then(|workspace| workspace.delete_dtd(location).ok())
        {
            Some(()) => {
                let remaining = self.working_dtds().len();
                self.selected_detailed = (remaining > 0).then_some(index.min(remaining - 1));
                self.status = format!("Deleted DTD at {}.", format_location(location));
            }
            None => {
                self.status =
                    "Cannot delete: selected monitor has no editable workspace.".to_string();
            }
        }
    }

    pub(super) fn delete_selected_extension_dtd(&mut self) {
        let Some(index) = self.selected_extension else {
            self.status = "Select a CTA DTD slot before Delete.".to_string();
            return;
        };
        let Some(row) = self.selected_extension_slot() else {
            self.status = "Selected CTA DTD slot no longer exists.".to_string();
            return;
        };
        if row.timing.is_none() {
            self.status = "Selected CTA DTD slot is already empty.".to_string();
            return;
        }

        let location = DtdLocation::Cta {
            extension_index: row.extension_index,
            slot: row.slot,
        };
        match self
            .selected_workspace_mut()
            .and_then(|workspace| workspace.delete_dtd(location).ok())
        {
            Some(()) => {
                let remaining = self.working_cta_dtd_slots().len();
                self.selected_extension = (remaining > 0).then_some(index.min(remaining - 1));
                self.status = format!("Deleted DTD at {}.", format_location(location));
            }
            None => {
                self.status =
                    "Cannot delete: selected monitor has no editable workspace.".to_string();
            }
        }
    }

    pub(super) fn delete_all_detailed(&mut self) {
        let locations = self
            .working_dtds()
            .into_iter()
            .map(|row| row.location)
            .collect::<Vec<_>>();
        if locations.is_empty() {
            self.status = "No detailed timings to delete.".to_string();
            return;
        }
        for location in locations {
            if let Some(workspace) = self.selected_workspace_mut() {
                let _ = workspace.delete_dtd(location);
            }
        }
        self.selected_detailed = None;
        self.status = "Deleted all detailed timings from the working EDID.".to_string();
    }

    pub(super) fn delete_all_extension_dtds(&mut self) {
        let locations = self
            .working_cta_dtd_slots()
            .into_iter()
            .filter(|row| row.timing.is_some())
            .map(|row| DtdLocation::Cta {
                extension_index: row.extension_index,
                slot: row.slot,
            })
            .collect::<Vec<_>>();
        if locations.is_empty() {
            self.status = "No CTA detailed timings to delete.".to_string();
            return;
        }

        let mut deleted = 0usize;
        for location in locations {
            if let Some(workspace) = self.selected_workspace_mut() {
                if workspace.delete_dtd(location).is_ok() {
                    deleted += 1;
                }
            }
        }
        self.selected_extension = None;
        self.status = format!("Deleted {deleted} CTA detailed timing(s) from the working EDID.");
    }

    pub(super) fn delete_selected_standard(&mut self) {
        let rows = self.working_standard_timings();
        let Some(index) = self.selected_standard else {
            self.status = "Select a standard timing before Delete.".to_string();
            return;
        };
        let Some(row) = rows.get(index) else {
            self.status = "Selected standard timing no longer exists.".to_string();
            return;
        };
        let slot = row.slot;
        match self
            .selected_workspace_mut()
            .and_then(|workspace| workspace.delete_standard_timing(slot).ok())
        {
            Some(()) => {
                let remaining = self.working_standard_timings().len();
                self.selected_standard = (remaining > 0).then_some(index.min(remaining - 1));
                self.status = format!("Deleted standard timing slot {slot}.");
            }
            None => {
                self.status =
                    "Cannot delete: selected monitor has no editable workspace.".to_string();
            }
        }
    }

    pub(super) fn edit_selected_standard(&mut self) {
        let rows = self.working_standard_timings();
        let Some(index) = self.selected_standard else {
            self.status = "Select a standard timing before Edit.".to_string();
            return;
        };
        let Some(row) = rows.get(index) else {
            self.status = "Selected standard timing no longer exists.".to_string();
            return;
        };
        self.open_standard_editor(StandardEditorMode::Edit {
            index,
            slot: row.slot,
        });
    }

    pub(super) fn delete_all_standard(&mut self) {
        let slots = self
            .working_standard_timings()
            .into_iter()
            .map(|row| row.slot)
            .collect::<Vec<_>>();
        if slots.is_empty() {
            self.status = "No standard timings to delete.".to_string();
            return;
        }
        for slot in slots {
            if let Some(workspace) = self.selected_workspace_mut() {
                let _ = workspace.delete_standard_timing(slot);
            }
        }
        self.selected_standard = None;
        self.status = "Deleted all standard timings from the working EDID.".to_string();
    }

    pub(super) fn move_selected_detailed(&mut self, direction: MoveDirection) {
        let Some(index) = self.selected_detailed else {
            self.status = "Select a detailed timing before moving it.".to_string();
            return;
        };
        let Some(workspace) = self.selected_workspace_mut() else {
            self.status = "Cannot move: selected monitor has no editable workspace.".to_string();
            return;
        };

        match workspace.move_dtd(index, direction) {
            Ok(new_index) => {
                self.selected_detailed = Some(new_index);
                self.status = format!("Moved detailed timing to row {}.", new_index + 1);
            }
            Err(error) => {
                self.status = format!("Cannot move detailed timing: {error}");
            }
        }
    }

    pub(super) fn reset_workspace(&mut self) {
        let Some(workspace) = self.selected_workspace_mut() else {
            self.status = "Cannot reset: selected monitor has no editable workspace.".to_string();
            return;
        };

        match workspace.reset() {
            Ok(()) => {
                self.selected_detailed = None;
                self.selected_extension = None;
                self.status = "Reset working EDID to the original monitor EDID.".to_string();
            }
            Err(error) => {
                self.status = format!("Cannot reset workspace: {error}");
            }
        }
    }

    pub(super) fn apply_detailed_editor(&mut self) {
        let Some(editor) = &self.detailed_editor else {
            return;
        };
        let mode = editor.mode;
        let Ok(timing) = editor.timing() else {
            self.status =
                "Cannot apply: width, height, and refresh must be valid numbers.".to_string();
            return;
        };

        let result = match mode {
            EditorMode::Add => {
                let Some(workspace) = self.selected_workspace_mut() else {
                    self.status =
                        "Cannot apply: selected monitor has no readable EDID.".to_string();
                    return;
                };
                workspace.add_dtd(timing.clone())
            }
            EditorMode::AddCta {
                extension_index,
                slot,
            } => {
                let Some(workspace) = self.selected_workspace_mut() else {
                    self.status =
                        "Cannot apply: selected monitor has no readable EDID.".to_string();
                    return;
                };
                if let Some(slot) = slot {
                    workspace.add_cta_dtd_at(
                        DtdLocation::Cta {
                            extension_index,
                            slot,
                        },
                        timing.clone(),
                    )
                } else {
                    workspace.add_cta_dtd(extension_index, timing.clone())
                }
            }
            EditorMode::Edit { location, index } => {
                let Some(workspace) = self.selected_workspace_mut() else {
                    self.status =
                        "Cannot apply: selected monitor has no readable EDID.".to_string();
                    return;
                };
                let result = workspace
                    .replace_dtd(location, timing.clone())
                    .map(|()| location);
                self.selected_detailed = Some(index);
                result
            }
            EditorMode::EditCta {
                location,
                extension_row,
            } => {
                let Some(workspace) = self.selected_workspace_mut() else {
                    self.status =
                        "Cannot apply: selected monitor has no readable EDID.".to_string();
                    return;
                };
                let result = workspace
                    .replace_dtd(location, timing.clone())
                    .map(|()| location);
                self.selected_extension = Some(extension_row);
                result
            }
        };

        match result {
            Ok(location) => {
                let warning_count = validate_timing(&timing).len();
                self.draft_timing = timing;
                self.detailed_editor = None;
                if matches!(mode, EditorMode::Add) {
                    self.selected_detailed = self.working_dtds().len().checked_sub(1);
                } else if matches!(mode, EditorMode::AddCta { .. }) {
                    self.selected_extension = self.extension_row_index_for_location(location);
                }
                self.status = format!(
                    "Applied {} at {}. Export writes the workspace EDID.{}",
                    self.draft_timing.hyprland_mode(),
                    format_location(location),
                    if warning_count > 0 {
                        format!(" {warning_count} timing warning(s).")
                    } else {
                        String::new()
                    }
                );
            }
            Err(error) => {
                self.status = format!("Cannot apply detailed timing: {error}");
            }
        }
    }

    pub(super) fn apply_standard_editor(&mut self) {
        let Some(editor) = &self.standard_editor else {
            return;
        };
        let mode = editor.mode;
        let timing = match editor.timing() {
            Ok(timing) => timing,
            Err(reason) => {
                self.status = format!("Cannot apply standard timing: {reason}.");
                return;
            }
        };

        let result = match mode {
            StandardEditorMode::Add => {
                let Some(workspace) = self.selected_workspace_mut() else {
                    self.status =
                        "Cannot apply: selected monitor has no readable EDID.".to_string();
                    return;
                };
                workspace.add_standard_timing(timing.clone())
            }
            StandardEditorMode::Edit { slot, index } => {
                let Some(workspace) = self.selected_workspace_mut() else {
                    self.status =
                        "Cannot apply: selected monitor has no readable EDID.".to_string();
                    return;
                };
                let result = workspace
                    .replace_standard_timing(slot, timing.clone())
                    .map(|()| slot);
                self.selected_standard = Some(index);
                result
            }
        };

        match result {
            Ok(slot) => {
                self.standard_editor = None;
                if matches!(mode, StandardEditorMode::Add) {
                    self.selected_standard = self
                        .working_standard_timings()
                        .iter()
                        .position(|row| row.slot == slot);
                }
                self.status = format!(
                    "Applied standard timing slot {slot}: {}x{} @ {} Hz.",
                    timing.width, timing.height, timing.refresh_hz
                );
            }
            Err(error) => {
                self.status = format!("Cannot apply standard timing: {error}");
            }
        }
    }

    pub(super) fn apply_import_dialog(&mut self) {
        let Some(dialog) = &self.import_dialog else {
            return;
        };
        let path = normalize_import_path(
            dialog.path.buffer.trim(),
            std::env::var_os("HOME").as_deref().map(Path::new),
        );
        if path.as_os_str().is_empty() {
            self.status = "Import path is empty.".to_string();
            return;
        }

        let raw = match fs::read(&path) {
            Ok(raw) => raw,
            Err(error) => {
                self.status = format!("Import failed: could not read {}: {error}", path.display());
                return;
            }
        };

        let result = if let Some(workspace) = self.selected_workspace_mut() {
            workspace.import_working_raw(raw, &path)
        } else {
            EdidWorkspace::imported(raw, &path).map(|workspace| {
                if let Some(slot) = self.workspaces.get_mut(self.selected_monitor) {
                    *slot = Some(workspace);
                }
            })
        };

        match result {
            Ok(()) => {
                self.selected_detailed = None;
                self.selected_established = None;
                self.selected_extension = None;
                self.import_dialog = None;
                self.details_dialog = None;
                self.status = format!("Imported EDID from {} into workspace.", path.display());
            }
            Err(error) => {
                self.status = format!("Import failed: {error}");
            }
        }
    }

    pub(super) fn export_selected_monitor(&mut self) {
        let issues = self.export_validation_issues();
        let blocking = issues
            .iter()
            .filter(|issue| issue.starts_with("Error:"))
            .cloned()
            .collect::<Vec<_>>();
        if !blocking.is_empty() {
            self.detailed_editor = None;
            self.standard_editor = None;
            self.import_dialog = None;
            self.export_confirm_dialog = None;
            self.export_dialog = None;
            self.details_dialog = Some(DetailsDialog::new(
                "Export Blocked",
                std::iter::once(
                    "The EDID was not written because these errors must be fixed:".to_string(),
                )
                .chain(std::iter::once(String::new()))
                .chain(blocking)
                .collect(),
            ));
            self.status = "Export blocked by EDID validation errors.".to_string();
            return;
        }

        if !issues.is_empty() {
            self.detailed_editor = None;
            self.standard_editor = None;
            self.import_dialog = None;
            self.details_dialog = None;
            self.export_dialog = None;
            self.export_confirm_dialog = Some(ExportConfirmDialog::new(issues));
            self.status = "Review export validation warnings.".to_string();
            return;
        }

        self.export_selected_monitor_unchecked();
    }

    pub(super) fn export_selected_monitor_unchecked(&mut self) {
        let Some(monitor) = self.selected_monitor() else {
            self.status = "No monitor selected.".to_string();
            return;
        };

        let Ok(output_dir) = std::env::current_dir() else {
            self.status = "Could not determine current output directory.".to_string();
            return;
        };

        let result = match self.selected_workspace() {
            Some(workspace) if workspace.has_changes() => {
                let Some(mode) = self.workspace_target_mode() else {
                    self.status =
                        "Export needs at least one resolution to generate a Hyprland rule."
                            .to_string();
                    return;
                };
                export_workspace_edid(monitor, workspace, &mode, &output_dir)
            }
            _ => export_patched_edid(monitor, &self.draft_timing, &output_dir),
        };

        match result {
            Ok(result) => {
                let dialog = ExportDialog::from_result(&result);
                self.detailed_editor = None;
                self.standard_editor = None;
                self.import_dialog = None;
                self.details_dialog = None;
                self.export_confirm_dialog = None;
                self.status = format!(
                    "Wrote {}. Review export instructions.",
                    result.path.display()
                );
                self.export_dialog = Some(dialog);
            }
            Err(error) => {
                self.status = format!("Export failed: {error}");
            }
        }
    }

    fn export_validation_issues(&self) -> Vec<String> {
        let mut issues = Vec::new();
        let Some(monitor) = self.selected_monitor() else {
            issues.push("Error: No monitor is selected.".to_string());
            return issues;
        };

        let exporting_workspace = self
            .selected_workspace()
            .map(EdidWorkspace::has_changes)
            .unwrap_or(false);

        if exporting_workspace {
            if let Some(workspace) = self.selected_workspace() {
                issues.extend(self.workspace_validation_issues(workspace));
            }
        } else {
            let Some(edid) = monitor.edid.as_ref() else {
                issues.push("Error: Selected monitor has no readable EDID to patch.".to_string());
                return issues;
            };

            if !edid.checksum_valid {
                issues.push("Error: Base EDID checksum is invalid.".to_string());
            }
            for cta in &edid.cta_blocks {
                if !cta.checksum_valid {
                    issues.push(format!(
                        "Error: CTA-861 extension {} checksum is invalid.",
                        cta.extension_index
                    ));
                }
            }
            let free_dtd_slots = self
                .selected_workspace()
                .and_then(|workspace| workspace.slot_summary().ok())
                .map(|summary| summary.base_dtd_free + summary.cta_dtd_free)
                .unwrap_or(0);
            if free_dtd_slots == 0 {
                issues.push(
                    "Error: No free base or CTA detailed timing slot is visible for draft insertion."
                        .to_string(),
                );
            }
            issues.push(
                "Warning: No workspace EDID changes are pending; export will insert the current draft detailed timing."
                    .to_string(),
            );
        }

        if !exporting_workspace {
            issues.extend(
                validate_timing(&self.draft_timing)
                    .into_iter()
                    .map(|warning| {
                        format!("{}: Draft timing: {}", warning.label(), warning.message)
                    }),
            );
            if let Some(warning) = self.internal_panel_timing_warning() {
                issues.push(format!("Warning: Internal panel: {}", warning.message));
            }

            if let Some(key) = ModeKey::from_timing(&self.draft_timing) {
                let provenance = self.mode_provenance(key);
                if provenance.sources.len() > 1 {
                    issues.push(format!(
                        "Warning: Draft timing duplicates an existing mode in {} source(s): {}.",
                        provenance.sources.len(),
                        provenance.sources.join(", ")
                    ));
                }
            }
        }

        issues
    }

    fn workspace_validation_issues(&self, workspace: &EdidWorkspace) -> Vec<String> {
        let mut issues = workspace
            .validate()
            .into_iter()
            .map(|issue| format!("Error: {}", issue.message))
            .collect::<Vec<_>>();

        if let Ok(rows) = workspace.dtds() {
            for row in rows {
                for warning in validate_timing(&row.timing) {
                    let prefix = match warning.severity {
                        TimingWarningSeverity::Error => "Error",
                        TimingWarningSeverity::Warning => "Warning",
                    };
                    issues.push(format!(
                        "{prefix}: {}: {}",
                        format_location(row.location),
                        warning.message
                    ));
                }
            }
        }

        if let Some(warning) = self.internal_panel_timing_warning() {
            issues.push(format!("Warning: {}", warning.message));
        }

        issues.sort();
        issues.dedup();
        issues
    }

    fn workspace_target_mode(&self) -> Option<String> {
        self.selected_switch_mode()
            .map(|candidate| candidate.request.label())
            .or_else(|| {
                self.working_dtds()
                    .first()
                    .map(|row| row.timing.hyprland_mode())
            })
            .or_else(|| {
                self.working_standard_timings().first().map(|row| {
                    format!(
                        "{}x{}@{}",
                        row.timing.width, row.timing.height, row.timing.refresh_hz
                    )
                })
            })
            .or_else(|| {
                self.selected_edid()
                    .and_then(|edid| edid.established_timings.first())
                    .map(|timing| {
                        format!("{}x{}@{}", timing.width, timing.height, timing.refresh_hz)
                    })
            })
    }

    fn extension_row_index_for_location(&self, location: DtdLocation) -> Option<usize> {
        self.working_extension_rows()
            .iter()
            .position(|row| match row {
                ExtensionRow::Dtd(row) => {
                    DtdLocation::Cta {
                        extension_index: row.extension_index,
                        slot: row.slot,
                    } == location
                }
                _ => false,
            })
    }

    fn internal_panel_timing_warning(&self) -> Option<crate::validation::TimingWarning> {
        let monitor = self.selected_monitor()?;
        let native = monitor.edid.as_ref()?.detailed_timings.first()?;
        let timings = match self.selected_workspace() {
            Some(workspace) if workspace.has_changes() => workspace
                .dtds()
                .ok()?
                .into_iter()
                .map(|row| row.timing)
                .collect::<Vec<_>>(),
            _ => vec![self.draft_timing.clone()],
        };

        internal_panel_scaling_warning(&monitor.connector, native, &timings)
    }

    fn selected_extension_slot(&self) -> Option<crate::edid::CtaDtdSlot> {
        match self.selected_extension_row()? {
            ExtensionRow::Dtd(row) => Some(row),
            ExtensionRow::Video { .. } => None,
            ExtensionRow::DisplayIdDtd(_) => None,
        }
    }

    fn selected_extension_row(&self) -> Option<ExtensionRow> {
        let index = self.selected_extension?;
        self.working_extension_rows().get(index).cloned()
    }

    fn selected_or_first_free_cta_slot(&self) -> Option<(u8, usize)> {
        if let Some(row) = self.selected_extension_slot() {
            if row.timing.is_none() && !row.occupied_unknown {
                return Some((row.extension_index, row.slot));
            }
        }

        self.working_extension_rows()
            .into_iter()
            .find_map(|row| match row {
                ExtensionRow::Dtd(row) if row.timing.is_none() && !row.occupied_unknown => {
                    Some((row.extension_index, row.slot))
                }
                _ => None,
            })
    }

    fn selected_switch_mode(&self) -> Option<SwitchModeCandidate> {
        match self.focus {
            FocusArea::Established => self.selected_established_mode(),
            FocusArea::Standard => self.selected_standard_mode(),
            FocusArea::Detailed => self.selected_detailed_mode(),
            FocusArea::Extension => self.selected_extension_mode(),
            _ => self
                .selected_detailed_mode()
                .or_else(|| self.selected_extension_mode())
                .or_else(|| self.selected_standard_mode())
                .or_else(|| self.selected_established_mode()),
        }
    }

    fn selected_established_mode(&self) -> Option<SwitchModeCandidate> {
        let index = self.selected_established?;
        let timing = self.selected_edid()?.established_timings.get(index)?;
        Some(SwitchModeCandidate {
            request: ModeRequest::new(
                u32::from(timing.width),
                u32::from(timing.height),
                f64::from(timing.refresh_hz),
            ),
            source: "Established resolutions",
        })
    }

    fn selected_standard_mode(&self) -> Option<SwitchModeCandidate> {
        let index = self.selected_standard?;
        let row = self.working_standard_timings().get(index)?.clone();
        Some(SwitchModeCandidate {
            request: ModeRequest::new(
                u32::from(row.timing.width),
                u32::from(row.timing.height),
                f64::from(row.timing.refresh_hz),
            ),
            source: "Standard resolutions",
        })
    }

    fn selected_detailed_mode(&self) -> Option<SwitchModeCandidate> {
        let index = self.selected_detailed?;
        let row = self.working_dtds().get(index)?.clone();
        Some(SwitchModeCandidate {
            request: ModeRequest::new(
                u32::from(row.timing.h_active),
                u32::from(row.timing.v_active),
                row.timing.refresh_hz()?,
            ),
            source: "Detailed resolutions",
        })
    }

    fn selected_extension_mode(&self) -> Option<SwitchModeCandidate> {
        let index = self.selected_extension?;
        match self.working_extension_rows().get(index)?.clone() {
            ExtensionRow::Video { descriptor, .. } => match descriptor {
                CtaVideoDescriptor::Known(mode) => Some(SwitchModeCandidate {
                    request: ModeRequest::new(
                        u32::from(mode.width),
                        u32::from(mode.height),
                        mode.refresh_hz(),
                    ),
                    source: "CTA video data block",
                }),
                CtaVideoDescriptor::Unknown { .. } => None,
            },
            ExtensionRow::Dtd(row) => {
                let timing = row.timing?;
                Some(SwitchModeCandidate {
                    request: ModeRequest::new(
                        u32::from(timing.h_active),
                        u32::from(timing.v_active),
                        timing.refresh_hz()?,
                    ),
                    source: "CTA detailed timing",
                })
            }
            ExtensionRow::DisplayIdDtd(row) => Some(SwitchModeCandidate {
                request: ModeRequest::new(
                    u32::from(row.timing.h_active),
                    u32::from(row.timing.v_active),
                    row.timing.refresh_hz()?,
                ),
                source: "DisplayID detailed timing",
            }),
        }
    }

    pub(super) fn switch_selected_monitor_mode(&mut self) {
        let Some(monitor) = self.selected_monitor() else {
            self.status = "No monitor selected.".to_string();
            return;
        };
        let connector = monitor.connector.clone();
        let Some(candidate) = self.selected_switch_mode() else {
            self.status =
                "Select a mapped established, standard, detailed, or CTA video mode before switching."
                    .to_string();
            return;
        };
        let requested = candidate.request.label();

        match hyprland::switch_to_available_mode(&connector, &candidate.request) {
            Ok(report) => {
                let response = if report.output.is_empty() {
                    "ok".to_string()
                } else {
                    report.output
                };
                self.status = if report.matched {
                    if report.already_active {
                        format!(
                            "{requested} is already active on {connector}. To survive reload, persist {}.",
                            report.monitor_rule
                        )
                    } else {
                        format!(
                            "Switched {connector} to {requested} from {} ({response}). Runtime only; persist with {}.",
                            candidate.source, report.monitor_rule
                        )
                    }
                } else if let Some(actual) = report.actual {
                    let restore_note = if report.restored_previous_mode {
                        " Restored the previous mode."
                    } else {
                        " Could not restore the previous mode automatically."
                    };
                    format!(
                        "Switch requested {requested}, but Hyprland reports {}.",
                        actual.label(),
                    ) + restore_note
                } else if report.restored_previous_mode {
                    format!(
                        "Switch requested {requested}, but Hyprland's active mode could not be verified. Restored the previous mode."
                    )
                } else {
                    format!(
                        "Switch requested {requested}, but Hyprland's active mode could not be verified ({response})."
                    )
                };
            }
            Err(hyprland::HyprlandError::ModeUnavailable { .. }) => {
                let override_status = self.selected_override_status();
                self.status = unavailable_mode_message(
                    &connector,
                    &requested,
                    override_status,
                    monitor.edid.as_ref().map(|edid| edid.raw.as_slice()),
                );
            }
            Err(error) => {
                self.status = format!("Switch failed: {error}");
            }
        }
    }

    pub(super) fn verify_selected_monitor_mode(&mut self) {
        let Some(monitor) = self.selected_monitor() else {
            self.status = "No monitor selected.".to_string();
            return;
        };
        let connector = monitor.connector.clone();
        let Some(candidate) = self.selected_switch_mode() else {
            self.status =
                "Select a mapped established, standard, detailed, or CTA video mode before verifying."
                    .to_string();
            return;
        };
        let override_status = self.selected_override_status().cloned();
        let requested = candidate.request.label();
        let mut lines = vec![
            format!("Connector: {connector}"),
            format!("Selected:  {requested}"),
            format!("Source:    {}", candidate.source),
        ];

        if let Some(status) = override_status.as_ref() {
            append_override_status_lines(
                &mut lines,
                status,
                monitor.edid.as_ref().map(|edid| edid.raw.as_slice()),
            );
        }

        lines.push(String::new());
        match hyprland::inspect_mode(&connector, &candidate.request) {
            Ok(report) => {
                lines.push(format!(
                    "Hyprland active: {}",
                    report
                        .active
                        .as_ref()
                        .map(|active| active.label())
                        .unwrap_or_else(|| "unknown".to_string())
                ));
                lines.push(format!(
                    "Selected mode exposed: {}",
                    yes_no(report.is_available())
                ));
                if let Some(mode) = &report.available_mode {
                    lines.push(format!("Exact Hyprland mode: {mode}"));
                }
                lines.push(format!(
                    "Selected mode active: {}",
                    yes_no(report.active_matches())
                ));
                if let Some(rule) = &report.monitor_rule {
                    lines.push(String::new());
                    lines.push("Persist this exact Hyprland rule for reloads:".to_string());
                    lines.push(rule.clone());

                    let config_report = hyprland_config::inspect_monitor_rule(&connector, rule);
                    append_config_inspection_lines(&mut lines, &config_report);
                }

                let recommendation = if report.active_matches() {
                    "Result: selected mode is active. If reload changes it, fix the Hyprland rule."
                        .to_string()
                } else if report.is_available() {
                    "Result: mode is available. Press Switch to use it now, then persist the rule."
                        .to_string()
                } else {
                    unavailable_mode_message(
                        &connector,
                        &requested,
                        override_status.as_ref(),
                        monitor.edid.as_ref().map(|edid| edid.raw.as_slice()),
                    )
                };
                lines.push(String::new());
                lines.push(recommendation);
            }
            Err(error) => {
                lines.push(format!("Hyprland inspection failed: {error}"));
                lines.push(String::new());
                lines.push(
                    "Result: cannot verify runtime mode state until Hyprland is reachable."
                        .to_string(),
                );
            }
        }

        if let Some(status) = override_status {
            if !status.read_warnings.is_empty() {
                lines.push(String::new());
                lines.push("Read warnings".to_string());
                lines.extend(
                    status
                        .read_warnings
                        .into_iter()
                        .map(|warning| format!("- {warning}")),
                );
            }
        }

        self.detailed_editor = None;
        self.standard_editor = None;
        self.import_dialog = None;
        self.export_confirm_dialog = None;
        self.export_dialog = None;
        self.apply_confirm_dialog = None;
        self.apply_result_dialog = None;
        self.details_dialog = Some(DetailsDialog::new("Mode Verification", lines));
        self.status = "Mode verification summary opened.".to_string();
    }

    pub(super) fn apply_selected_monitor(&mut self) {
        let Some(monitor) = self.selected_monitor() else {
            self.status = "No monitor selected.".to_string();
            return;
        };
        let monitor = monitor.clone();
        let override_status = self.selected_override_status().cloned();
        let operation = if override_status
            .as_ref()
            .is_some_and(|status| status.has_any_override())
        {
            SystemOperation::Update
        } else {
            SystemOperation::Install
        };

        let Ok(output_dir) = std::env::current_dir() else {
            self.status = "Could not determine current output directory.".to_string();
            return;
        };
        let Some(workspace) = self
            .selected_workspace()
            .filter(|workspace| workspace.has_changes())
        else {
            self.status =
                "No workspace changes are pending. Add, edit, delete, or import a timing first."
                    .to_string();
            return;
        };
        let workspace = workspace.clone();
        let Some(hyprland_mode) = self.workspace_target_mode() else {
            self.status = "Apply needs at least one resolution in the working EDID.".to_string();
            return;
        };
        let validation_issues = self.workspace_validation_issues(&workspace);
        let validation_errors = validation_issues
            .iter()
            .filter(|issue| issue.starts_with("Error:"))
            .cloned()
            .collect::<Vec<_>>();
        if !validation_errors.is_empty() {
            self.apply_result_dialog = Some(ApplyResultDialog {
                operation,
                success: false,
                output: validation_errors.join("\n"),
                scroll: 0,
            });
            self.status =
                "Apply blocked because the working EDID has validation errors.".to_string();
            return;
        }

        let connector = monitor.connector.clone();
        let edid_file_name = custom_edid_file_name(&connector);
        let plan = InstallPlan {
            connector: connector.clone(),
            edid_source: output_dir.join(&edid_file_name),
            kernel_parameter: format!("drm.edid_firmware={connector}:edid/{edid_file_name}"),
            edid_file_name,
        };
        let preview = InstallPreview::from_plan(&plan);
        let mut summary_lines = preview.summary_lines();
        if let Some(status) = override_status {
            summary_lines.insert(0, format!("Current:    {}", status.short_label()));
            for warning in status.read_warnings {
                summary_lines.push(format!("Warning:    {warning}"));
            }
        }
        summary_lines.push(format!("Hyprland:  {hyprland_mode}"));
        summary_lines.extend(
            validation_issues
                .into_iter()
                .map(|issue| format!("Validation: {issue}")),
        );
        let support = install::inspect_system_support();
        if !support.is_supported() {
            self.detailed_editor = None;
            self.standard_editor = None;
            self.import_dialog = None;
            self.details_dialog = None;
            self.export_confirm_dialog = None;
            self.export_dialog = None;
            self.apply_confirm_dialog = None;
            self.pending_system_action = None;
            self.apply_result_dialog = Some(ApplyResultDialog {
                operation,
                success: false,
                output: support.report_text(),
                scroll: 0,
            });
            self.status =
                "Automatic Apply is unsupported on this system. Review the details.".to_string();
            return;
        }
        summary_lines.extend(support.summary_lines());

        self.detailed_editor = None;
        self.standard_editor = None;
        self.import_dialog = None;
        self.details_dialog = None;
        self.export_confirm_dialog = None;
        self.export_dialog = None;
        self.apply_result_dialog = None;

        self.pending_system_action = Some(PendingSystemAction::Install {
            monitor: Box::new(monitor),
            workspace: Box::new(workspace),
            hyprland_mode,
            output_dir,
            operation,
        });
        self.apply_confirm_dialog = Some(ApplyConfirmDialog::new(operation, summary_lines));
        self.status = match operation {
            SystemOperation::Install => {
                "Review persistent EDID install changes. Enter to install, Esc to cancel."
            }
            SystemOperation::Update => {
                "Review persistent EDID update changes. Enter to update, Esc to cancel."
            }
            SystemOperation::Uninstall => unreachable!("install action cannot uninstall"),
        }
        .to_string();
    }

    pub(super) fn uninstall_selected_monitor(&mut self) {
        let Some(monitor) = self.selected_monitor() else {
            self.status = "No monitor selected.".to_string();
            return;
        };
        let connector = monitor.connector.clone();
        let override_status = self.selected_override_status().cloned();
        if override_status
            .as_ref()
            .is_some_and(|status| !status.has_any_override() && status.read_warnings.is_empty())
        {
            self.status = format!("No drmcru EDID override detected for {connector}.");
            return;
        }

        let edid_file_name = custom_edid_file_name(&connector);
        let plan = UninstallPlan {
            connector: connector.clone(),
            kernel_parameter: format!("drm.edid_firmware={connector}:edid/{edid_file_name}"),
            edid_file_name,
        };
        let preview = UninstallPreview::from_plan(&plan);
        let mut summary_lines = preview.summary_lines();
        if let Some(status) = override_status {
            summary_lines.insert(0, format!("Current:    {}", status.short_label()));
            for warning in status.read_warnings {
                summary_lines.push(format!("Warning:    {warning}"));
            }
        }
        let support = install::inspect_system_support();
        if !support.is_supported() {
            self.detailed_editor = None;
            self.standard_editor = None;
            self.import_dialog = None;
            self.details_dialog = None;
            self.export_confirm_dialog = None;
            self.export_dialog = None;
            self.apply_confirm_dialog = None;
            self.pending_system_action = None;
            self.apply_result_dialog = Some(ApplyResultDialog {
                operation: SystemOperation::Uninstall,
                success: false,
                output: support.report_text(),
                scroll: 0,
            });
            self.status = "Automatic uninstall is unsupported on this system. Review the details."
                .to_string();
            return;
        }
        summary_lines.extend(support.summary_lines());

        self.detailed_editor = None;
        self.standard_editor = None;
        self.import_dialog = None;
        self.details_dialog = None;
        self.export_confirm_dialog = None;
        self.export_dialog = None;
        self.apply_result_dialog = None;

        self.pending_system_action = Some(PendingSystemAction::Uninstall(plan));
        self.apply_confirm_dialog = Some(ApplyConfirmDialog::new(
            SystemOperation::Uninstall,
            summary_lines,
        ));
        self.status =
            "Review EDID uninstall changes. Enter to uninstall, Esc to cancel.".to_string();
    }

    pub(super) fn confirm_apply(&mut self) {
        let Some(action) = self.pending_system_action.take() else {
            self.status = "No pending system action.".to_string();
            return;
        };
        let operation = match &action {
            PendingSystemAction::Install { operation, .. } => *operation,
            PendingSystemAction::Uninstall(_) => SystemOperation::Uninstall,
        };

        let execution = match action {
            PendingSystemAction::Install {
                monitor,
                workspace,
                hyprland_mode,
                output_dir,
                ..
            } => {
                let export_result = match export_workspace_edid(
                    &monitor,
                    &workspace,
                    &hyprland_mode,
                    &output_dir,
                ) {
                    Ok(result) => result,
                    Err(error) => {
                        self.apply_confirm_dialog = None;
                        self.status = format!("Apply failed during export: {error}");
                        return;
                    }
                };
                SystemExecution::Install(InstallPlan {
                    connector: monitor.connector,
                    edid_source: export_result.path,
                    edid_file_name: export_result.plan.edid_file_name.clone(),
                    kernel_parameter: export_result.plan.drm_kernel_parameter(),
                })
            }
            PendingSystemAction::Uninstall(plan) => SystemExecution::Uninstall(plan),
        };
        self.apply_confirm_dialog = None;
        self.applying_in_progress = true;
        self.applying_operation = Some(operation);
        self.status = match operation {
            SystemOperation::Install => "Installing EDID... (waiting for authentication)",
            SystemOperation::Update => "Updating EDID... (waiting for authentication)",
            SystemOperation::Uninstall => "Uninstalling EDID... (waiting for authentication)",
        }
        .to_string();

        // Spawn the install in a background thread so the TUI keeps rendering
        let (tx, rx) = std::sync::mpsc::channel();
        self.install_receiver = Some(rx);

        std::thread::spawn(move || {
            let result = match execution {
                SystemExecution::Install(plan) => install::install(&plan),
                SystemExecution::Uninstall(plan) => install::uninstall(&plan),
            };
            let _ = tx.send((operation, result));
        });
    }

    pub(super) fn poll_install_result(&mut self) {
        let Some(rx) = self.install_receiver.as_ref() else {
            return;
        };

        match rx.try_recv() {
            Ok((operation, Ok(report))) => {
                self.applying_in_progress = false;
                self.applying_operation = None;
                self.install_receiver = None;
                self.refresh_override_statuses();
                self.apply_result_dialog = Some(ApplyResultDialog {
                    operation,
                    success: report.success,
                    output: report.output,
                    scroll: 0,
                });
                self.status = match operation {
                    SystemOperation::Install => {
                        "Install complete. Reboot to activate the custom EDID."
                    }
                    SystemOperation::Update => {
                        "Update complete. Reboot to activate the updated EDID."
                    }
                    SystemOperation::Uninstall => {
                        "Uninstall complete. Reboot to return to the monitor EDID."
                    }
                }
                .to_string();
            }
            Ok((operation, Err(install::InstallError::Cancelled))) => {
                self.applying_in_progress = false;
                self.applying_operation = None;
                self.install_receiver = None;
                self.status = match operation {
                    SystemOperation::Install => "Install cancelled — authentication was dismissed.",
                    SystemOperation::Update => "Update cancelled — authentication was dismissed.",
                    SystemOperation::Uninstall => {
                        "Uninstall cancelled — authentication was dismissed."
                    }
                }
                .to_string();
            }
            Ok((operation, Err(error))) => {
                self.applying_in_progress = false;
                self.applying_operation = None;
                self.install_receiver = None;
                self.refresh_override_statuses();
                self.apply_result_dialog = Some(ApplyResultDialog {
                    operation,
                    success: false,
                    output: error.to_string(),
                    scroll: 0,
                });
                self.status = match operation {
                    SystemOperation::Install => format!("Install failed: {error}"),
                    SystemOperation::Update => format!("Update failed: {error}"),
                    SystemOperation::Uninstall => format!("Uninstall failed: {error}"),
                };
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                // Still running — keep the progress dialog visible
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                // Thread died unexpectedly
                self.applying_in_progress = false;
                self.applying_operation = None;
                self.install_receiver = None;
                self.refresh_override_statuses();
                self.status = "System action thread disconnected unexpectedly.".to_string();
            }
        }
    }

    pub(super) fn dismiss_apply_result(&mut self) {
        self.apply_result_dialog = None;
        self.status = "System action dialog closed.".to_string();
    }

    pub(super) fn cancel_apply(&mut self) {
        self.apply_confirm_dialog = None;
        self.pending_system_action = None;
        self.status = "System action cancelled.".to_string();
    }
}

fn timing_detail_lines(
    source: &str,
    location: Option<String>,
    timing: &crate::models::TimingDescriptor,
) -> Vec<String> {
    let mut lines = vec![
        source.to_string(),
        format!("Mode: {}", timing.hyprland_mode()),
    ];
    if let Some(location) = location {
        lines.push(format!("Location: {location}"));
    }
    lines.extend([
        format!("Pixel clock: {} kHz", timing.pixel_clock_khz),
        format!(
            "Horizontal: active {}  front {}  sync {}  back {}  blanking {}  total {}",
            timing.h_active,
            timing.h_front_porch,
            timing.h_sync_width,
            timing.h_back_porch,
            timing.h_blanking,
            timing.h_total()
        ),
        format!(
            "Vertical:   active {}  front {}  sync {}  back {}  blanking {}  total {}",
            timing.v_active,
            timing.v_front_porch,
            timing.v_sync_width,
            timing.v_back_porch,
            timing.v_blanking,
            timing.v_total()
        ),
        format!("Refresh: {:.3} Hz", timing.refresh_hz().unwrap_or_default()),
        format!(
            "Horizontal rate: {:.3} kHz",
            timing.horizontal_rate_khz().unwrap_or_default()
        ),
        format!(
            "Sync polarity: H {}  V {}",
            polarity_label(timing.h_sync_positive),
            polarity_label(timing.v_sync_positive)
        ),
        format!("Interlaced: {}", yes_no(timing.interlaced)),
    ]);
    lines
}

fn with_provenance_lines(mut lines: Vec<String>, provenance: ModeProvenance) -> Vec<String> {
    lines.push(String::new());
    lines.push(format!(
        "Mode key: {}x{} @ {:.3} Hz{}",
        provenance.key.width,
        provenance.key.height,
        provenance.key.refresh_hz(),
        if provenance.key.interlaced {
            " interlaced"
        } else {
            ""
        }
    ));
    lines.push(format!("Sources: {}", provenance.sources.len()));
    for source in provenance.sources {
        lines.push(format!("  - {source}"));
    }
    lines
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn help_lines() -> Vec<String> {
    [
        "Core workflow",
        "1. Select a monitor in the top pane.",
        "2. Add, edit, copy, paste, delete, or reorder detailed timings in the working EDID.",
        "3. Export writes a patched EDID and instruction file without touching system config.",
        "4. Install/Update writes the EDID override for the next boot on supported Limine systems.",
        "5. Reboot, then Verify or run doctor to confirm the override is active and matching.",
        "6. Switch selects a mode only after DRM/Hyprland already exposes it.",
        "7. Persist the generated monitor=... rule manually in Hyprland config.",
        "",
        "Global keys",
        "Tab / Shift+Tab  Move between panes",
        "Up/Down or j/k    Move the focused selection",
        "Enter             Open details, edit a selected row, or add when no row is selected",
        "? / h / F1        Open this help",
        "i                 Details for the focused monitor or row",
        "e                 Export patched EDID",
        "Shift+E           Edit the selected detailed, standard, or CTA DTD row",
        "A                 Install/Update EDID override",
        "u                 Uninstall drmcru EDID override",
        "s                 Switch to selected exposed mode",
        "v                 Verify selected mode and persistent config",
        "q / Esc           Quit from the main screen",
        "",
        "Detailed and CTA timing keys",
        "a                 Add timing in the focused timing pane",
        "Shift+E / Edit    Edit the selected row",
        "c                 Copy selected detailed or CTA DTD timing",
        "p                 Paste copied detailed timing into a new slot",
        "d / Delete        Delete selected timing",
        "Reset button      Restore the original monitor EDID workspace",
        "Up/Down buttons   Reorder base detailed timings",
        "",
        "Editor keys",
        "Tab / Shift+Tab  Move between fields",
        "Up/Down          Move between fields",
        "Left/Right       Move cursor, or cycle timing preset on the Timing field",
        "Home/End         Move to start/end of text fields",
        "Space            Toggle sync polarity/interlace fields",
        "Enter            Apply editor changes to the in-memory EDID",
        "Esc              Cancel the editor",
        "",
        "Mouse",
        "Left click        Select rows, fields, and buttons",
        "Right click       Open details for monitor and resolution rows",
        "Wheel             Scroll lists or long dialogs",
        "",
        "Status labels",
        "not installed    No drmcru EDID override detected for this connector",
        "partial install  Some override files exist, but the boot path is incomplete",
        "reboot pending   Boot config is ready, but this kernel has not loaded it yet",
        "active            The current kernel command line references the override",
        "matching          Live connector EDID bytes match the installed firmware file",
        "",
        "Important model",
        "Hyprland cannot make new DRM modes appear by itself. New custom timings must be in the kernel-exposed EDID first.",
        "Switch changes only modes that DRM/Hyprland already exposes.",
        "Install/Update requires a reboot because the kernel reads EDID firmware at boot.",
        "drmcru does not auto-edit Hyprland config; Verify reports the exact rule to persist.",
        "",
        "Diagnostics",
        "Run `drmcru doctor` outside the TUI for diagnostics.",
        "Monitor Details shows override state, live EDID match, and the winning Hyprland monitor rule.",
        "Verify shows whether the selected mode is exposed, active, and reload-persistent.",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn append_override_status_lines(
    lines: &mut Vec<String>,
    status: &install::InstalledOverrideStatus,
    live_edid: Option<&[u8]>,
) {
    let firmware_comparison =
        install::compare_firmware_to_live_edid(&status.firmware_target, live_edid);
    lines.push(String::new());
    lines.push("EDID override".to_string());
    lines.push(format!("Status:             {}", status.short_label()));
    lines.push(format!(
        "Firmware file:      {}",
        yes_no(status.firmware_present)
    ));
    lines.push(format!(
        "mkinitcpio FILES:   {}",
        yes_no(status.mkinitcpio_references_firmware)
    ));
    lines.push(format!(
        "Bootloader config:  {} ({}/{} cmdline entries)",
        yes_no(status.bootloader_references_kernel_parameter),
        status.bootloader_cmdline_entries_with_kernel_parameter,
        status.bootloader_cmdline_entries
    ));
    lines.push(format!(
        "Limine entry-tool:  {}",
        yes_no(status.limine_entry_tool_references_kernel_parameter)
    ));
    lines.push(format!(
        "Active kernel arg:  {}",
        yes_no(status.active_kernel_references_kernel_parameter)
    ));
    lines.push(format!(
        "Live EDID match:    {}",
        firmware_comparison.short_label()
    ));
    lines.push(format!(
        "Match detail:       {}",
        firmware_comparison.detail()
    ));

    let result = if status.is_active()
        && matches!(
            firmware_comparison,
            install::FirmwareEdidComparison::Matches { .. }
        ) {
        "Result: installed EDID override is active on this boot."
    } else if status.is_configured_for_next_boot() {
        "Result: override is configured for next boot, but the active kernel does not use it yet."
    } else if status.has_any_override() {
        "Result: override install is partial; run Install/Update before rebooting."
    } else {
        "Result: no drmcru EDID override is installed for this connector."
    };
    lines.push(result.to_string());

    if !status.read_warnings.is_empty() {
        lines.push("Override warnings".to_string());
        lines.extend(
            status
                .read_warnings
                .iter()
                .map(|warning| format!("- {warning}")),
        );
    }
}

fn unavailable_mode_message(
    connector: &str,
    requested: &str,
    status: Option<&install::InstalledOverrideStatus>,
    live_edid: Option<&[u8]>,
) -> String {
    let prefix = format!("{requested} is not exposed by DRM/Hyprland for {connector}.");
    match status {
        Some(status) if status.is_active() => {
            let comparison =
                install::compare_firmware_to_live_edid(&status.firmware_target, live_edid);
            if matches!(comparison, install::FirmwareEdidComparison::Matches { .. }) {
                format!(
                    "{prefix} The EDID override is active, so this mode is not in the active EDID payload. Update the EDID override with the current workspace and reboot."
                )
            } else {
                format!(
                    "{prefix} The kernel EDID argument is active, but live EDID does not match the installed firmware file. Run Install/Update again and reboot."
                )
            }
        }
        Some(status) if status.is_configured_for_next_boot() => format!(
            "{prefix} The EDID override is installed for next boot, but this boot is still using the old EDID. Reboot and verify again."
        ),
        Some(status) if status.has_any_override() => format!(
            "{prefix} The EDID override install is partial. Run Install/Update, reboot, then verify again."
        ),
        _ => format!("{prefix} Install the EDID override and reboot first."),
    }
}

fn append_connector_config_lines(lines: &mut Vec<String>, connector: &str) {
    let report = hyprland_config::inspect_connector_rules(connector);
    lines.push(String::new());
    lines.push("Hyprland connector config".to_string());
    lines.push(format!(
        "Root: {}",
        hyprland_config::human_path(&report.root_path)
    ));
    lines.push(format!("Files read: {}", report.files_read));
    lines.push(format!(
        "Literal rules for connector: {}",
        report.connector_rules.len()
    ));
    if let Some(rule) = &report.last_connector_rule {
        lines.push(format!("Last literal rule: {}", rule.location()));
        lines.push(format!("Last literal mode: {}", rule.normalized_rule()));
    } else {
        lines.push("Last literal rule: none".to_string());
    }

    if !report.read_warnings.is_empty() {
        lines.push("Config warnings".to_string());
        lines.extend(
            report
                .read_warnings
                .iter()
                .take(3)
                .map(|warning| format!("- {warning}")),
        );
        if report.read_warnings.len() > 3 {
            lines.push(format!(
                "- ... {} more warning(s)",
                report.read_warnings.len() - 3
            ));
        }
    }
}

fn append_config_inspection_lines(lines: &mut Vec<String>, report: &MonitorRuleInspection) {
    lines.push(String::new());
    lines.push("Hyprland config check".to_string());
    lines.push(format!(
        "Root: {}",
        hyprland_config::human_path(&report.root_path)
    ));
    lines.push(format!("Files read: {}", report.files_read));
    lines.push(format!(
        "Literal rules for connector: {}",
        report.connector_rules.len()
    ));
    lines.push(format!(
        "Exact rule present: {}",
        yes_no(report.exact_match.is_some())
    ));

    if let Some(rule) = &report.exact_match {
        lines.push(format!("Exact rule location: {}", rule.location()));
    }
    if let Some(rule) = &report.last_connector_rule {
        lines.push(format!("Last connector rule: {}", rule.location()));
        lines.push(format!("Last connector mode: {}", rule.normalized_rule()));
    }

    if report.exact_match_is_effective() {
        lines.push(
            "Config result: the selected mode is the last literal rule found; dynamic Lua may still override it."
                .to_string(),
        );
    } else if report.exact_match.is_some() {
        lines.push(
            "Config result: a later literal connector rule may override the exact rule."
                .to_string(),
        );
    } else if report.connector_rules.is_empty() {
        lines.push(
            "Config result: no literal monitor rule for this connector was found.".to_string(),
        );
    } else {
        lines.push(
            "Config result: the last literal connector rule differs from the selected mode."
                .to_string(),
        );
    }

    if !report.read_warnings.is_empty() {
        lines.push("Config warnings".to_string());
        lines.extend(
            report
                .read_warnings
                .iter()
                .take(3)
                .map(|warning| format!("- {warning}")),
        );
        if report.read_warnings.len() > 3 {
            lines.push(format!(
                "- ... {} more warning(s)",
                report.read_warnings.len() - 3
            ));
        }
    }
}

pub(super) fn normalize_import_path(value: &str, home: Option<&Path>) -> PathBuf {
    let value = value.trim();
    let value = if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        &value[1..value.len() - 1]
    } else {
        value
    };

    if value == "~" {
        return home
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(value));
    }
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = home {
            return home.join(rest);
        }
    }
    if let Some(rest) = value.strip_prefix("$HOME/") {
        if let Some(home) = home {
            return home.join(rest);
        }
    }

    PathBuf::from(value)
}

fn polarity_label(positive: bool) -> &'static str {
    if positive { "+" } else { "-" }
}

fn checksum_label(valid: bool) -> &'static str {
    if valid { "ok" } else { "bad" }
}
