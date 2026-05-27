use super::support::wrap_index;
use crate::edid::DtdLocation;
use crate::export::ExportResult;
use crate::models::{StandardTiming, StandardTimingAspect, TimingDescriptor};
use crate::timings::{CvtRequest, TimingPreset, timing_for_preset};
use crate::validation::validate_timing;
use crate::workspace::format_location;
use ratatui::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ModalButton {
    Ok,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SystemOperation {
    Install,
    Update,
    Uninstall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EditorMode {
    Add,
    AddCta {
        extension_index: u8,
        slot: Option<usize>,
    },
    Edit {
        index: usize,
        location: DtdLocation,
    },
    EditCta {
        extension_row: usize,
        location: DtdLocation,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StandardEditorMode {
    Add,
    Edit { index: usize, slot: usize },
}

#[derive(Debug, Clone)]
pub(super) struct ImportDialog {
    pub(super) path: TextInput,
}

#[derive(Debug, Clone)]
pub(super) struct ExportDialog {
    pub(super) path: String,
    pub(super) instructions_path: String,
    pub(super) location: String,
    pub(super) kernel_parameter: String,
    pub(super) hyprland_rule: String,
}

#[derive(Debug, Clone)]
pub(super) struct ExportConfirmDialog {
    pub(super) issues: Vec<String>,
}

impl ExportConfirmDialog {
    pub(super) fn new(issues: Vec<String>) -> Self {
        Self { issues }
    }
}

#[derive(Debug, Clone)]
pub(super) struct ApplyConfirmDialog {
    pub(super) operation: SystemOperation,
    pub(super) summary_lines: Vec<String>,
}

impl ApplyConfirmDialog {
    pub(super) fn new(operation: SystemOperation, summary_lines: Vec<String>) -> Self {
        Self {
            operation,
            summary_lines,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct ApplyResultDialog {
    pub(super) operation: SystemOperation,
    pub(super) success: bool,
    pub(super) output: String,
}

#[derive(Debug, Clone)]
pub(super) struct DetailsDialog {
    pub(super) title: String,
    pub(super) lines: Vec<String>,
    pub(super) scroll: usize,
}

impl DetailsDialog {
    pub(super) fn new(title: impl Into<String>, lines: Vec<String>) -> Self {
        Self {
            title: title.into(),
            lines,
            scroll: 0,
        }
    }

    pub(super) fn scroll_by(&mut self, delta: isize) {
        if delta < 0 {
            self.scroll = self.scroll.saturating_sub(delta.unsigned_abs());
        } else {
            self.scroll = self
                .scroll
                .saturating_add(delta as usize)
                .min(self.lines.len().saturating_sub(1));
        }
    }

    pub(super) fn scroll_page(&mut self, delta: isize) {
        self.scroll_by(delta.saturating_mul(10));
    }

    pub(super) fn scroll_to_top(&mut self) {
        self.scroll = 0;
    }

    pub(super) fn scroll_to_bottom(&mut self) {
        self.scroll = self.lines.len().saturating_sub(1);
    }
}

impl ExportDialog {
    pub(super) fn from_result(result: &ExportResult) -> Self {
        Self {
            path: result.path.display().to_string(),
            instructions_path: result.instructions_path.display().to_string(),
            location: result
                .insert_location
                .map(format_location)
                .unwrap_or_else(|| "workspace EDID".to_string()),
            kernel_parameter: result.plan.drm_kernel_parameter(),
            hyprland_rule: result.plan.hyprland_monitor_rule(),
        }
    }
}

impl Default for ImportDialog {
    fn default() -> Self {
        Self {
            path: TextInput::new(String::new()),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct DetailedResolutionEditor {
    h_active: TextInput,
    v_active: TextInput,
    h_front: TextInput,
    h_sync: TextInput,
    h_back: TextInput,
    h_blanking: TextInput,
    h_total: TextInput,
    v_front: TextInput,
    v_sync: TextInput,
    v_back: TextInput,
    v_blanking: TextInput,
    v_total: TextInput,
    pixel_clock: TextInput,
    h_rate: TextInput,
    refresh_hint: TextInput,
    h_sync_positive: bool,
    v_sync_positive: bool,
    interlaced: bool,
    preset: TimingPreset,
    pub(super) active_field: EditorField,
    pub(super) mode: EditorMode,
}

impl DetailedResolutionEditor {
    pub(super) fn from_timing(timing: TimingDescriptor, mode: EditorMode) -> Self {
        Self {
            h_active: TextInput::new(timing.h_active.to_string()),
            v_active: TextInput::new(timing.v_active.to_string()),
            h_front: TextInput::new(timing.h_front_porch.to_string()),
            h_sync: TextInput::new(timing.h_sync_width.to_string()),
            h_back: TextInput::new(timing.h_back_porch.to_string()),
            h_blanking: TextInput::new(timing.h_blanking.to_string()),
            h_total: TextInput::new(timing.h_total().to_string()),
            v_front: TextInput::new(timing.v_front_porch.to_string()),
            v_sync: TextInput::new(timing.v_sync_width.to_string()),
            v_back: TextInput::new(timing.v_back_porch.to_string()),
            v_blanking: TextInput::new(timing.v_blanking.to_string()),
            v_total: TextInput::new(timing.v_total().to_string()),
            pixel_clock: TextInput::new(timing.pixel_clock_khz.to_string()),
            h_rate: TextInput::new(format!(
                "{:.3}",
                timing.horizontal_rate_khz().unwrap_or_default()
            )),
            refresh_hint: TextInput::new(format!("{:.2}", timing.refresh_hz().unwrap_or(60.0))),
            h_sync_positive: timing.h_sync_positive,
            v_sync_positive: timing.v_sync_positive,
            interlaced: timing.interlaced,
            preset: TimingPreset::Manual,
            active_field: EditorField::Width,
            mode,
        }
    }

    pub(super) fn timing(&self) -> Result<TimingDescriptor, &'static str> {
        let manual = self.manual_timing()?;
        if self.preset == TimingPreset::Manual {
            return Ok(manual);
        }

        let request = self.preset_request()?;
        Ok(timing_for_preset(self.preset, request, Some(&manual)))
    }

    pub(super) fn manual_timing(&self) -> Result<TimingDescriptor, &'static str> {
        let h_active = self.parse_u16(EditorField::Width, "invalid horizontal active pixels")?;
        let v_active = self.parse_u16(EditorField::Height, "invalid vertical active pixels")?;
        let h_front = self.parse_u16(EditorField::HFront, "invalid horizontal front porch")?;
        let h_sync = self.parse_u16(EditorField::HSync, "invalid horizontal sync width")?;
        let h_back = self.parse_u16(EditorField::HBack, "invalid horizontal back porch")?;
        let v_front = self.parse_u16(EditorField::VFront, "invalid vertical front porch")?;
        let v_sync = self.parse_u16(EditorField::VSync, "invalid vertical sync width")?;
        let v_back = self.parse_u16(EditorField::VBack, "invalid vertical back porch")?;
        let pixel_clock_khz = self
            .pixel_clock
            .buffer
            .parse::<u32>()
            .map_err(|_| "invalid pixel clock")?;

        if h_active == 0 || v_active == 0 || pixel_clock_khz == 0 {
            return Err("active pixels and pixel clock must be greater than zero");
        }

        let h_blanking = h_front
            .checked_add(h_sync)
            .and_then(|value| value.checked_add(h_back))
            .ok_or("horizontal blanking exceeds EDID limits")?;
        let v_blanking = v_front
            .checked_add(v_sync)
            .and_then(|value| value.checked_add(v_back))
            .ok_or("vertical blanking exceeds EDID limits")?;

        Ok(TimingDescriptor {
            pixel_clock_khz,
            h_active,
            h_blanking,
            h_front_porch: h_front,
            h_sync_width: h_sync,
            h_back_porch: h_back,
            v_active,
            v_blanking,
            v_front_porch: v_front,
            v_sync_width: v_sync,
            v_back_porch: v_back,
            h_sync_positive: self.h_sync_positive,
            v_sync_positive: self.v_sync_positive,
            interlaced: self.interlaced,
        })
    }

    pub(super) fn preset_request(&self) -> Result<CvtRequest, &'static str> {
        let width = self.parse_u16(EditorField::Width, "invalid horizontal active pixels")?;
        let height = self.parse_u16(EditorField::Height, "invalid vertical active pixels")?;
        let refresh_hz = self
            .refresh_hint
            .buffer
            .parse::<f64>()
            .map_err(|_| "invalid refresh")?;
        if width == 0 || height == 0 || !refresh_hz.is_finite() || refresh_hz <= 0.0 {
            return Err("active pixels and refresh must be greater than zero");
        }

        Ok(CvtRequest {
            width,
            height,
            refresh_hz,
        })
    }

    pub(super) fn derived_text(&self) -> String {
        match self.timing() {
            Ok(timing) => {
                let mut lines = vec![
                    "Detailed Timing Summary".to_string(),
                    format!("Mode: {}", timing.hyprland_mode()),
                    format!(
                        "Pixel clock: {} kHz   Horizontal rate: {:.3} kHz   Refresh: {:.3} Hz",
                        timing.pixel_clock_khz,
                        timing.horizontal_rate_khz().unwrap_or_default(),
                        timing.refresh_hz().unwrap_or_default()
                    ),
                    format!(
                        "Horizontal blanking {} total {}   Vertical blanking {} total {}",
                        timing.h_blanking,
                        timing.h_total(),
                        timing.v_blanking,
                        timing.v_total()
                    ),
                    format!(
                        "Sync polarity: H{} V{}   Interlaced: {}",
                        if timing.h_sync_positive { "+" } else { "-" },
                        if timing.v_sync_positive { "+" } else { "-" },
                        if timing.interlaced { "yes" } else { "no" }
                    ),
                    format!("Preset: {}", self.preset.label()),
                ];
                let warnings = validate_timing(&timing);
                if !warnings.is_empty() {
                    lines.push(String::new());
                    lines.push("Timing warnings".to_string());
                    lines.extend(
                        warnings
                            .iter()
                            .take(5)
                            .map(|warning| format!("- {}: {}", warning.label(), warning.message)),
                    );
                    if warnings.len() > 5 {
                        lines.push(format!("- ... {} more warning(s)", warnings.len() - 5));
                    }
                }
                lines.join("\n")
            }
            Err(reason) => format!("Invalid detailed timing: {reason}."),
        }
    }

    pub(super) fn field_rows(&self, area: Rect) -> Vec<(EditorField, Rect, &'static str)> {
        let rows = Layout::vertical([Constraint::Length(1); 19]).split(area);
        detailed_field_order()
            .into_iter()
            .zip(rows.iter().copied())
            .map(|((field, label), rect)| (field, rect, label))
            .collect()
    }

    pub(super) fn input(&self, field: EditorField) -> &TextInput {
        match field {
            EditorField::Width => &self.h_active,
            EditorField::Height => &self.v_active,
            EditorField::HFront => &self.h_front,
            EditorField::HSync => &self.h_sync,
            EditorField::HBack => &self.h_back,
            EditorField::HBlanking => &self.h_blanking,
            EditorField::HTotal => &self.h_total,
            EditorField::VFront => &self.v_front,
            EditorField::VSync => &self.v_sync,
            EditorField::VBack => &self.v_back,
            EditorField::VBlanking => &self.v_blanking,
            EditorField::VTotal => &self.v_total,
            EditorField::HRate => &self.h_rate,
            EditorField::PixelClock => &self.pixel_clock,
            EditorField::Refresh => &self.refresh_hint,
            EditorField::Preset
            | EditorField::HPolarity
            | EditorField::VPolarity
            | EditorField::Interlaced => &self.refresh_hint,
        }
    }

    pub(super) fn render_field(&self, field: EditorField, active: bool) -> String {
        match field {
            EditorField::Preset => self.preset.label().to_string(),
            EditorField::HPolarity => {
                format!("H{}", if self.h_sync_positive { "+" } else { "-" })
            }
            EditorField::VPolarity => {
                format!("V{}", if self.v_sync_positive { "+" } else { "-" })
            }
            EditorField::Interlaced => {
                if self.interlaced {
                    "yes".to_string()
                } else {
                    "no".to_string()
                }
            }
            _ => self.input(field).render(active),
        }
    }

    pub(super) fn active_input_mut(&mut self) -> &mut TextInput {
        match self.active_field {
            EditorField::Width => &mut self.h_active,
            EditorField::Height => &mut self.v_active,
            EditorField::HFront => &mut self.h_front,
            EditorField::HSync => &mut self.h_sync,
            EditorField::HBack => &mut self.h_back,
            EditorField::HBlanking => &mut self.h_blanking,
            EditorField::HTotal => &mut self.h_total,
            EditorField::VFront => &mut self.v_front,
            EditorField::VSync => &mut self.v_sync,
            EditorField::VBack => &mut self.v_back,
            EditorField::VBlanking => &mut self.v_blanking,
            EditorField::VTotal => &mut self.v_total,
            EditorField::HRate => &mut self.h_rate,
            EditorField::PixelClock => &mut self.pixel_clock,
            EditorField::Refresh => &mut self.refresh_hint,
            EditorField::Preset
            | EditorField::HPolarity
            | EditorField::VPolarity
            | EditorField::Interlaced => &mut self.refresh_hint,
        }
    }

    pub(super) fn next_field(&mut self) {
        self.active_field = cycle_detailed_field(self.active_field, 1);
    }

    pub(super) fn previous_field(&mut self) {
        self.active_field = cycle_detailed_field(self.active_field, -1);
    }

    pub(super) fn set_cursor_from_column(&mut self, field: EditorField, column: u16, rect: Rect) {
        let left_padding = 13usize;
        self.active_field = field;
        if self.toggle_field(field) {
            return;
        }
        let input = self.input_mut(field);
        let clicked = usize::from(column.saturating_sub(rect.x)).saturating_sub(left_padding);
        input.cursor = clicked.min(input.buffer.len());
    }

    pub(super) fn move_cursor_left(&mut self) {
        if self.active_field == EditorField::Preset {
            self.cycle_preset(-1);
            return;
        }
        if !self.active_field.is_text_field() {
            return;
        }
        self.active_input_mut().move_left();
    }

    pub(super) fn move_cursor_right(&mut self) {
        if self.active_field == EditorField::Preset {
            self.cycle_preset(1);
            return;
        }
        if !self.active_field.is_text_field() {
            return;
        }
        self.active_input_mut().move_right();
    }

    pub(super) fn move_cursor_home(&mut self) {
        if !self.active_field.is_text_field() {
            return;
        }
        self.active_input_mut().cursor = 0;
    }

    pub(super) fn move_cursor_end(&mut self) {
        if !self.active_field.is_text_field() {
            return;
        }
        let input = self.active_input_mut();
        input.cursor = input.buffer.len();
    }

    pub(super) fn backspace(&mut self) {
        if !self.active_field.is_text_field() {
            return;
        }
        let field = self.active_field;
        self.active_input_mut().backspace();
        self.after_text_edit(field);
    }

    pub(super) fn delete(&mut self) {
        if !self.active_field.is_text_field() {
            return;
        }
        let field = self.active_field;
        self.active_input_mut().delete();
        self.after_text_edit(field);
    }

    pub(super) fn insert_char(&mut self, value: char) {
        let field = self.active_field;
        if value == ' ' {
            let _ = self.toggle_field(field);
        } else if field.allows(value, self.input(field)) {
            self.active_input_mut().insert(value);
            self.after_text_edit(field);
        }
    }

    pub(super) fn input_mut(&mut self, field: EditorField) -> &mut TextInput {
        match field {
            EditorField::Width => &mut self.h_active,
            EditorField::Height => &mut self.v_active,
            EditorField::HFront => &mut self.h_front,
            EditorField::HSync => &mut self.h_sync,
            EditorField::HBack => &mut self.h_back,
            EditorField::HBlanking => &mut self.h_blanking,
            EditorField::HTotal => &mut self.h_total,
            EditorField::VFront => &mut self.v_front,
            EditorField::VSync => &mut self.v_sync,
            EditorField::VBack => &mut self.v_back,
            EditorField::VBlanking => &mut self.v_blanking,
            EditorField::VTotal => &mut self.v_total,
            EditorField::HRate => &mut self.h_rate,
            EditorField::PixelClock => &mut self.pixel_clock,
            EditorField::Refresh => &mut self.refresh_hint,
            EditorField::Preset
            | EditorField::HPolarity
            | EditorField::VPolarity
            | EditorField::Interlaced => &mut self.refresh_hint,
        }
    }

    pub(super) fn parse_u16(
        &self,
        field: EditorField,
        error: &'static str,
    ) -> Result<u16, &'static str> {
        self.input(field).buffer.parse::<u16>().map_err(|_| error)
    }

    pub(super) fn toggle_field(&mut self, field: EditorField) -> bool {
        match field {
            EditorField::Preset => self.cycle_preset(1),
            EditorField::HPolarity => {
                self.preset = TimingPreset::Manual;
                self.h_sync_positive = !self.h_sync_positive;
            }
            EditorField::VPolarity => {
                self.preset = TimingPreset::Manual;
                self.v_sync_positive = !self.v_sync_positive;
            }
            EditorField::Interlaced => {
                self.preset = TimingPreset::Manual;
                self.interlaced = !self.interlaced;
            }
            _ => return false,
        }
        true
    }

    pub(super) fn cycle_preset(&mut self, delta: isize) {
        self.preset = self.preset.cycle(delta);
        if self.preset != TimingPreset::Manual {
            self.apply_preset_to_fields();
        }
    }

    pub(super) fn after_text_edit(&mut self, field: EditorField) {
        if self.preset != TimingPreset::Manual
            && matches!(
                field,
                EditorField::Width | EditorField::Height | EditorField::Refresh
            )
        {
            self.apply_preset_to_fields();
            return;
        }

        if field != EditorField::Preset {
            self.preset = TimingPreset::Manual;
        }

        match field {
            EditorField::Refresh => self.set_pixel_clock_from_refresh(),
            EditorField::HRate => self.set_pixel_clock_from_h_rate(),
            EditorField::HBlanking => {
                self.set_h_back_from_blanking();
                self.sync_horizontal_totals();
                self.update_rate_fields_from_pixel_clock(field);
            }
            EditorField::HTotal => {
                self.set_h_back_from_total();
                self.sync_horizontal_totals();
                self.update_rate_fields_from_pixel_clock(field);
            }
            EditorField::VBlanking => {
                self.set_v_back_from_blanking();
                self.sync_vertical_totals();
                self.update_rate_fields_from_pixel_clock(field);
            }
            EditorField::VTotal => {
                self.set_v_back_from_total();
                self.sync_vertical_totals();
                self.update_rate_fields_from_pixel_clock(field);
            }
            EditorField::Width | EditorField::Height => {
                self.sync_aggregate_fields();
                // Keep the refresh rate, recalculate pixel clock (like Windows CRU)
                self.set_pixel_clock_from_refresh();
            }
            EditorField::HFront
            | EditorField::HSync
            | EditorField::HBack
            | EditorField::VFront
            | EditorField::VSync
            | EditorField::VBack
            | EditorField::PixelClock => {
                self.sync_aggregate_fields();
                self.update_rate_fields_from_pixel_clock(field);
            }
            _ => {}
        }
    }

    pub(super) fn apply_preset_to_fields(&mut self) {
        let Ok(request) = self.preset_request() else {
            return;
        };
        let current = self.manual_timing().ok();
        let timing = timing_for_preset(self.preset, request, current.as_ref());
        self.set_timing_fields(&timing);
        self.refresh_hint.set(format!("{:.2}", request.refresh_hz));
    }

    pub(super) fn set_timing_fields(&mut self, timing: &TimingDescriptor) {
        self.h_active.set(timing.h_active.to_string());
        self.v_active.set(timing.v_active.to_string());
        self.h_front.set(timing.h_front_porch.to_string());
        self.h_sync.set(timing.h_sync_width.to_string());
        self.h_back.set(timing.h_back_porch.to_string());
        self.h_blanking.set(timing.h_blanking.to_string());
        self.h_total.set(timing.h_total().to_string());
        self.v_front.set(timing.v_front_porch.to_string());
        self.v_sync.set(timing.v_sync_width.to_string());
        self.v_back.set(timing.v_back_porch.to_string());
        self.v_blanking.set(timing.v_blanking.to_string());
        self.v_total.set(timing.v_total().to_string());
        self.pixel_clock.set(timing.pixel_clock_khz.to_string());
        self.h_rate.set(format!(
            "{:.3}",
            timing.horizontal_rate_khz().unwrap_or_default()
        ));
        self.h_sync_positive = timing.h_sync_positive;
        self.v_sync_positive = timing.v_sync_positive;
        self.interlaced = timing.interlaced;
    }

    pub(super) fn set_pixel_clock_from_refresh(&mut self) {
        let Ok(refresh_hz) = self.refresh_hint.buffer.parse::<f64>() else {
            return;
        };
        let Some((h_total, v_total)) = self.totals_from_fields() else {
            return;
        };
        if !refresh_hz.is_finite() || refresh_hz <= 0.0 {
            return;
        }

        let pixel_clock_khz = (f64::from(h_total * v_total) * refresh_hz / 1000.0).round() as u32;
        self.pixel_clock.set(pixel_clock_khz.to_string());
        self.h_rate.set(format!(
            "{:.3}",
            f64::from(pixel_clock_khz) / f64::from(h_total)
        ));
    }

    pub(super) fn sync_aggregate_fields(&mut self) {
        self.sync_horizontal_totals();
        self.sync_vertical_totals();
    }

    pub(super) fn sync_horizontal_totals(&mut self) {
        let Some((h_active, h_front, h_sync, h_back)) = self.horizontal_parts() else {
            return;
        };
        let h_blanking = h_front + h_sync + h_back;
        self.h_blanking.set(h_blanking.to_string());
        self.h_total.set((h_active + h_blanking).to_string());
    }

    pub(super) fn sync_vertical_totals(&mut self) {
        let Some((v_active, v_front, v_sync, v_back)) = self.vertical_parts() else {
            return;
        };
        let v_blanking = v_front + v_sync + v_back;
        self.v_blanking.set(v_blanking.to_string());
        self.v_total.set((v_active + v_blanking).to_string());
    }

    pub(super) fn set_h_back_from_blanking(&mut self) {
        let Ok(blanking) = self.h_blanking.buffer.parse::<u32>() else {
            return;
        };
        let Some((_, front, sync, _)) = self.horizontal_parts() else {
            return;
        };
        let back = blanking.saturating_sub(front + sync);
        self.h_back.set(back.min(u32::from(u16::MAX)).to_string());
    }

    pub(super) fn set_h_back_from_total(&mut self) {
        let Ok(total) = self.h_total.buffer.parse::<u32>() else {
            return;
        };
        let Some((active, front, sync, _)) = self.horizontal_parts() else {
            return;
        };
        let back = total.saturating_sub(active + front + sync);
        self.h_back.set(back.min(u32::from(u16::MAX)).to_string());
    }

    pub(super) fn set_v_back_from_blanking(&mut self) {
        let Ok(blanking) = self.v_blanking.buffer.parse::<u32>() else {
            return;
        };
        let Some((_, front, sync, _)) = self.vertical_parts() else {
            return;
        };
        let back = blanking.saturating_sub(front + sync);
        self.v_back.set(back.min(u32::from(u16::MAX)).to_string());
    }

    pub(super) fn set_v_back_from_total(&mut self) {
        let Ok(total) = self.v_total.buffer.parse::<u32>() else {
            return;
        };
        let Some((active, front, sync, _)) = self.vertical_parts() else {
            return;
        };
        let back = total.saturating_sub(active + front + sync);
        self.v_back.set(back.min(u32::from(u16::MAX)).to_string());
    }

    pub(super) fn set_pixel_clock_from_h_rate(&mut self) {
        let Ok(horizontal_rate_khz) = self.h_rate.buffer.parse::<f64>() else {
            return;
        };
        let Some((h_total, v_total)) = self.totals_from_fields() else {
            return;
        };
        if !horizontal_rate_khz.is_finite() || horizontal_rate_khz <= 0.0 {
            return;
        }

        let pixel_clock_khz = (horizontal_rate_khz * f64::from(h_total)).round() as u32;
        self.pixel_clock.set(pixel_clock_khz.to_string());
        self.refresh_hint.set(format!(
            "{:.3}",
            f64::from(pixel_clock_khz) * 1000.0 / f64::from(h_total * v_total)
        ));
    }

    pub(super) fn update_rate_fields_from_pixel_clock(&mut self, edited_field: EditorField) {
        let Ok(pixel_clock_khz) = self.pixel_clock.buffer.parse::<u32>() else {
            return;
        };
        let Some((h_total, v_total)) = self.totals_from_fields() else {
            return;
        };
        if pixel_clock_khz == 0 {
            return;
        }

        if edited_field != EditorField::HRate {
            self.h_rate.set(format!(
                "{:.3}",
                f64::from(pixel_clock_khz) / f64::from(h_total)
            ));
        }
        if edited_field != EditorField::Refresh {
            self.refresh_hint.set(format!(
                "{:.3}",
                f64::from(pixel_clock_khz) * 1000.0 / f64::from(h_total * v_total)
            ));
        }
    }

    pub(super) fn totals_from_fields(&self) -> Option<(u32, u32)> {
        let (h_active, h_front, h_sync, h_back) = self.horizontal_parts()?;
        let (v_active, v_front, v_sync, v_back) = self.vertical_parts()?;
        let h_total = h_active
            .checked_add(h_front)?
            .checked_add(h_sync)?
            .checked_add(h_back)?;
        let v_total = v_active
            .checked_add(v_front)?
            .checked_add(v_sync)?
            .checked_add(v_back)?;

        (h_total > 0 && v_total > 0).then_some((h_total, v_total))
    }

    pub(super) fn horizontal_parts(&self) -> Option<(u32, u32, u32, u32)> {
        Some((
            u32::from(self.h_active.buffer.parse::<u16>().ok()?),
            u32::from(self.h_front.buffer.parse::<u16>().ok()?),
            u32::from(self.h_sync.buffer.parse::<u16>().ok()?),
            u32::from(self.h_back.buffer.parse::<u16>().ok()?),
        ))
    }

    pub(super) fn vertical_parts(&self) -> Option<(u32, u32, u32, u32)> {
        Some((
            u32::from(self.v_active.buffer.parse::<u16>().ok()?),
            u32::from(self.v_front.buffer.parse::<u16>().ok()?),
            u32::from(self.v_sync.buffer.parse::<u16>().ok()?),
            u32::from(self.v_back.buffer.parse::<u16>().ok()?),
        ))
    }
}

#[derive(Debug, Clone)]
pub(super) struct StandardResolutionEditor {
    width: TextInput,
    height: TextInput,
    refresh: TextInput,
    pub(super) active_field: EditorField,
    pub(super) mode: StandardEditorMode,
}

impl StandardResolutionEditor {
    pub(super) fn from_timing(timing: StandardTiming, mode: StandardEditorMode) -> Self {
        Self {
            width: TextInput::new(timing.width.to_string()),
            height: TextInput::new(timing.height.to_string()),
            refresh: TextInput::new(timing.refresh_hz.to_string()),
            active_field: EditorField::Width,
            mode,
        }
    }

    pub(super) fn timing(&self) -> Result<StandardTiming, &'static str> {
        let width = self
            .width
            .buffer
            .parse::<u16>()
            .map_err(|_| "invalid width")?;
        let height = self
            .height
            .buffer
            .parse::<u16>()
            .map_err(|_| "invalid height")?;
        let refresh_hz = self
            .refresh
            .buffer
            .parse::<u16>()
            .map_err(|_| "refresh must be an integer")?;

        if !(256..=2288).contains(&width) {
            return Err("width must be 256..2288");
        }
        if !width.is_multiple_of(8) {
            return Err("width must be divisible by 8");
        }
        if !(60..=123).contains(&refresh_hz) {
            return Err("refresh must be 60..123 Hz");
        }

        let Some(aspect) = StandardTimingAspect::from_dimensions(width, height) else {
            return Err("height must match 16:10, 4:3, 5:4, or 16:9 exactly");
        };

        Ok(StandardTiming {
            slot: 0,
            width,
            height,
            refresh_hz,
            aspect,
        })
    }

    pub(super) fn derived_text(&self) -> String {
        match self.timing() {
            Ok(timing) => [
                "Encodable EDID Standard Timing".to_string(),
                format!(
                    "Mode: {}x{} @ {} Hz",
                    timing.width, timing.height, timing.refresh_hz
                ),
                format!("Aspect: {}", timing.aspect.label()),
                "This writes one base-block standard timing slot.".to_string(),
            ]
            .join("\n"),
            Err(reason) => format!(
                "Not encodable: {reason}\nStandard timings cannot store arbitrary modelines; use Detailed resolutions for custom porch/pixel-clock control."
            ),
        }
    }

    pub(super) fn field_rects(&self, area: Rect) -> [Rect; 3] {
        let [width, height, refresh, _rest] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(area);
        [width, height, refresh]
    }

    pub(super) fn input(&self, field: EditorField) -> &TextInput {
        match field {
            EditorField::Width => &self.width,
            EditorField::Height => &self.height,
            EditorField::Refresh => &self.refresh,
            _ => &self.width,
        }
    }

    pub(super) fn active_input_mut(&mut self) -> &mut TextInput {
        match self.active_field {
            EditorField::Width => &mut self.width,
            EditorField::Height => &mut self.height,
            EditorField::Refresh => &mut self.refresh,
            _ => &mut self.width,
        }
    }

    pub(super) fn next_field(&mut self) {
        self.active_field = match self.active_field {
            EditorField::Width => EditorField::Height,
            EditorField::Height => EditorField::Refresh,
            EditorField::Refresh => EditorField::Width,
            _ => EditorField::Width,
        };
    }

    pub(super) fn previous_field(&mut self) {
        self.active_field = match self.active_field {
            EditorField::Width => EditorField::Refresh,
            EditorField::Height => EditorField::Width,
            EditorField::Refresh => EditorField::Height,
            _ => EditorField::Width,
        };
    }

    pub(super) fn set_cursor_from_column(&mut self, field: EditorField, column: u16, rect: Rect) {
        let left_padding = 13usize;
        let input = match field {
            EditorField::Width => &mut self.width,
            EditorField::Height => &mut self.height,
            EditorField::Refresh => &mut self.refresh,
            _ => &mut self.width,
        };
        let clicked = usize::from(column.saturating_sub(rect.x)).saturating_sub(left_padding);
        input.cursor = clicked.min(input.buffer.len());
    }

    pub(super) fn move_cursor_left(&mut self) {
        self.active_input_mut().move_left();
    }

    pub(super) fn move_cursor_right(&mut self) {
        self.active_input_mut().move_right();
    }

    pub(super) fn move_cursor_home(&mut self) {
        self.active_input_mut().cursor = 0;
    }

    pub(super) fn move_cursor_end(&mut self) {
        let input = self.active_input_mut();
        input.cursor = input.buffer.len();
    }

    pub(super) fn backspace(&mut self) {
        self.active_input_mut().backspace();
    }

    pub(super) fn delete(&mut self) {
        self.active_input_mut().delete();
    }

    pub(super) fn insert_char(&mut self, value: char) {
        if value.is_ascii_digit() {
            self.active_input_mut().insert(value);
        }
    }
}

fn detailed_field_order() -> [(EditorField, &'static str); 19] {
    [
        (EditorField::Preset, "Timing"),
        (EditorField::Width, "H Active"),
        (EditorField::HFront, "H Front"),
        (EditorField::HSync, "H Sync"),
        (EditorField::HBack, "H Back"),
        (EditorField::HBlanking, "H Blank"),
        (EditorField::HTotal, "H Total"),
        (EditorField::Height, "V Active"),
        (EditorField::VFront, "V Front"),
        (EditorField::VSync, "V Sync"),
        (EditorField::VBack, "V Back"),
        (EditorField::VBlanking, "V Blank"),
        (EditorField::VTotal, "V Total"),
        (EditorField::HRate, "H Rate"),
        (EditorField::Refresh, "Refresh"),
        (EditorField::PixelClock, "Pixel kHz"),
        (EditorField::HPolarity, "H Pol"),
        (EditorField::VPolarity, "V Pol"),
        (EditorField::Interlaced, "Interlace"),
    ]
}

fn cycle_detailed_field(current: EditorField, delta: isize) -> EditorField {
    let order = detailed_field_order();
    let current = order
        .iter()
        .position(|(field, _)| *field == current)
        .unwrap_or(0);
    order[wrap_index(current, order.len(), delta)].0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EditorField {
    Width,
    Height,
    Refresh,
    HFront,
    HSync,
    HBack,
    HBlanking,
    HTotal,
    VFront,
    VSync,
    VBack,
    VBlanking,
    VTotal,
    HRate,
    PixelClock,
    Preset,
    HPolarity,
    VPolarity,
    Interlaced,
}

impl EditorField {
    pub(super) fn is_text_field(self) -> bool {
        !matches!(
            self,
            Self::Preset | Self::HPolarity | Self::VPolarity | Self::Interlaced
        )
    }

    pub(super) fn allows(self, value: char, input: &TextInput) -> bool {
        match self {
            Self::Width
            | Self::Height
            | Self::HFront
            | Self::HSync
            | Self::HBack
            | Self::HBlanking
            | Self::HTotal
            | Self::VFront
            | Self::VSync
            | Self::VBack
            | Self::VBlanking
            | Self::VTotal
            | Self::PixelClock => value.is_ascii_digit(),
            Self::HRate | Self::Refresh => {
                value.is_ascii_digit()
                    || ((value == '.' || value == ',') && !input.buffer.contains('.'))
            }
            Self::Preset | Self::HPolarity | Self::VPolarity | Self::Interlaced => false,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct TextInput {
    pub(super) buffer: String,
    pub(super) cursor: usize,
}

impl TextInput {
    pub(super) fn new(buffer: String) -> Self {
        let cursor = buffer.len();
        Self { buffer, cursor }
    }

    pub(super) fn set(&mut self, buffer: String) {
        self.buffer = buffer;
        self.cursor = self.buffer.len();
    }

    pub(super) fn render(&self, active: bool) -> String {
        if !active {
            return self.buffer.clone();
        }
        let mut rendered = self.buffer.clone();
        rendered.insert(self.cursor, '|');
        rendered
    }

    pub(super) fn insert(&mut self, value: char) {
        let value = if value == ',' { '.' } else { value };
        self.buffer.insert(self.cursor, value);
        self.cursor += value.len_utf8();
    }

    pub(super) fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.buffer.remove(self.cursor);
        }
    }

    pub(super) fn delete(&mut self) {
        if self.cursor < self.buffer.len() {
            self.buffer.remove(self.cursor);
        }
    }

    pub(super) fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub(super) fn move_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.buffer.len());
    }

    pub(super) fn move_end(&mut self) {
        self.cursor = self.buffer.len();
    }

    pub(super) fn set_cursor_from_column(&mut self, column: u16, rect: Rect, left_padding: usize) {
        let clicked = usize::from(column.saturating_sub(rect.x)).saturating_sub(left_padding);
        self.cursor = clicked.min(self.buffer.len());
    }
}
