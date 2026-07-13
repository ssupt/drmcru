use crate::edid::{
    CtaDtdSlot, DtdLocation, EdidError, EdidSlotSummary, LocatedStandardTiming,
    block_checksum_valid, cta_dtd_slots, delete_detailed_timing, delete_standard_timing,
    detailed_timing_locations, insert_cta_detailed_timing, insert_detailed_timing,
    insert_standard_timing, parse_edid, patch_detailed_timing, patch_standard_timing, slot_summary,
    standard_timing_locations,
};
use crate::models::{EdidData, StandardTiming, TimingDescriptor};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct EdidWorkspace {
    original_raw: Vec<u8>,
    working: EdidData,
    operations: Vec<WorkspaceOperation>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WorkspaceOperation {
    AddDtd {
        location: DtdLocation,
        timing: TimingDescriptor,
    },
    ReplaceDtd {
        location: DtdLocation,
        timing: TimingDescriptor,
    },
    DeleteDtd {
        location: DtdLocation,
    },
    DeleteStandardTiming {
        slot: usize,
    },
    AddStandardTiming {
        slot: usize,
        timing: StandardTiming,
    },
    ReplaceStandardTiming {
        slot: usize,
        timing: StandardTiming,
    },
    ReorderDtd {
        from: DtdLocation,
        to: DtdLocation,
    },
    Reset,
    ImportEdid {
        source: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    pub message: String,
}

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("failed to parse EDID: {0}")]
    Edid(#[from] EdidError),
}

impl EdidWorkspace {
    pub fn new(raw: Vec<u8>) -> Result<Self, WorkspaceError> {
        let working = parse_edid(raw.clone())?;
        Ok(Self {
            original_raw: raw,
            working,
            operations: Vec::new(),
        })
    }

    pub fn from_edid(edid: &EdidData) -> Result<Self, WorkspaceError> {
        Self::new(edid.raw.clone())
    }

    pub fn imported(raw: Vec<u8>, source: impl AsRef<Path>) -> Result<Self, WorkspaceError> {
        let working = parse_edid(raw)?;
        Ok(Self {
            original_raw: Vec::new(),
            working,
            operations: vec![WorkspaceOperation::ImportEdid {
                source: source.as_ref().display().to_string(),
            }],
        })
    }

    pub fn parsed(&self) -> &EdidData {
        &self.working
    }

    pub fn has_changes(&self) -> bool {
        self.original_raw != self.working.raw
    }

    pub fn add_dtd(&mut self, timing: TimingDescriptor) -> Result<DtdLocation, WorkspaceError> {
        let (raw, location) = insert_detailed_timing(&self.working.raw, &timing)?;
        self.replace_working_raw(raw)?;
        self.operations
            .push(WorkspaceOperation::AddDtd { location, timing });
        Ok(location)
    }

    pub fn add_cta_dtd(
        &mut self,
        extension_index: u8,
        timing: TimingDescriptor,
    ) -> Result<DtdLocation, WorkspaceError> {
        let (raw, location) =
            insert_cta_detailed_timing(&self.working.raw, extension_index, &timing)?;
        self.replace_working_raw(raw)?;
        self.operations
            .push(WorkspaceOperation::AddDtd { location, timing });
        Ok(location)
    }

    pub fn add_cta_dtd_at(
        &mut self,
        location: DtdLocation,
        timing: TimingDescriptor,
    ) -> Result<DtdLocation, WorkspaceError> {
        let (extension_index, slot) = match location {
            DtdLocation::Cta {
                extension_index,
                slot,
            } => (extension_index, slot),
            DtdLocation::Base { slot } => {
                return Err(WorkspaceError::Edid(EdidError::InvalidDetailedTimingSlot(
                    slot,
                )));
            }
        };

        let slots = cta_dtd_slots(&self.working.raw)?;
        let Some(row) = slots
            .iter()
            .find(|row| row.extension_index == extension_index && row.slot == slot)
        else {
            return Err(WorkspaceError::Edid(EdidError::InvalidDetailedTimingSlot(
                slot,
            )));
        };
        if row.timing.is_some() || row.occupied_unknown {
            return Err(WorkspaceError::Edid(
                EdidError::NoAvailableDetailedTimingSlot,
            ));
        }

        let raw = patch_detailed_timing(&self.working.raw, location, &timing)?;
        self.replace_working_raw(raw)?;
        self.operations
            .push(WorkspaceOperation::AddDtd { location, timing });
        Ok(location)
    }

    pub fn replace_dtd(
        &mut self,
        location: DtdLocation,
        timing: TimingDescriptor,
    ) -> Result<(), WorkspaceError> {
        let raw = patch_detailed_timing(&self.working.raw, location, &timing)?;
        self.replace_working_raw(raw)?;
        self.operations
            .push(WorkspaceOperation::ReplaceDtd { location, timing });
        Ok(())
    }

    pub fn delete_dtd(&mut self, location: DtdLocation) -> Result<(), WorkspaceError> {
        let raw = delete_detailed_timing(&self.working.raw, location)?;
        self.replace_working_raw(raw)?;
        self.operations
            .push(WorkspaceOperation::DeleteDtd { location });
        Ok(())
    }

    pub fn delete_standard_timing(&mut self, slot: usize) -> Result<(), WorkspaceError> {
        let raw = delete_standard_timing(&self.working.raw, slot)?;
        self.replace_working_raw(raw)?;
        self.operations
            .push(WorkspaceOperation::DeleteStandardTiming { slot });
        Ok(())
    }

    pub fn add_standard_timing(&mut self, timing: StandardTiming) -> Result<usize, WorkspaceError> {
        let (raw, slot) = insert_standard_timing(&self.working.raw, &timing)?;
        self.replace_working_raw(raw)?;
        self.operations
            .push(WorkspaceOperation::AddStandardTiming { slot, timing });
        Ok(slot)
    }

    pub fn replace_standard_timing(
        &mut self,
        slot: usize,
        timing: StandardTiming,
    ) -> Result<(), WorkspaceError> {
        let raw = patch_standard_timing(&self.working.raw, slot, &timing)?;
        self.replace_working_raw(raw)?;
        self.operations
            .push(WorkspaceOperation::ReplaceStandardTiming { slot, timing });
        Ok(())
    }

    pub fn move_dtd(
        &mut self,
        index: usize,
        direction: MoveDirection,
    ) -> Result<usize, WorkspaceError> {
        let dtds = self.dtds()?;
        if dtds.is_empty() {
            return Ok(index);
        }
        if index >= dtds.len() {
            return Ok(dtds.len() - 1);
        }

        let Some(target_index) = direction.target_index(index, dtds.len()) else {
            return Ok(index);
        };

        let current = &dtds[index];
        let target = &dtds[target_index];
        let raw = patch_detailed_timing(&self.working.raw, current.location, &target.timing)?;
        let raw = patch_detailed_timing(&raw, target.location, &current.timing)?;
        let from = current.location;
        let to = target.location;

        self.replace_working_raw(raw)?;
        self.operations
            .push(WorkspaceOperation::ReorderDtd { from, to });
        Ok(target_index)
    }

    pub fn reset(&mut self) -> Result<(), WorkspaceError> {
        self.working = parse_edid(self.original_raw.clone())?;
        self.operations.clear();
        self.operations.push(WorkspaceOperation::Reset);
        Ok(())
    }

    pub fn import_working_raw(
        &mut self,
        raw: Vec<u8>,
        source: impl AsRef<Path>,
    ) -> Result<(), WorkspaceError> {
        self.working = parse_edid(raw)?;
        self.operations.push(WorkspaceOperation::ImportEdid {
            source: source.as_ref().display().to_string(),
        });
        Ok(())
    }

    pub fn dtds(&self) -> Result<Vec<crate::edid::LocatedDetailedTiming>, WorkspaceError> {
        detailed_timing_locations(&self.working.raw).map_err(WorkspaceError::Edid)
    }

    pub fn standard_timings(&self) -> Result<Vec<LocatedStandardTiming>, WorkspaceError> {
        standard_timing_locations(&self.working.raw).map_err(WorkspaceError::Edid)
    }

    pub fn cta_dtd_slots(&self) -> Result<Vec<CtaDtdSlot>, WorkspaceError> {
        cta_dtd_slots(&self.working.raw).map_err(WorkspaceError::Edid)
    }

    pub fn slot_summary(&self) -> Result<EdidSlotSummary, WorkspaceError> {
        slot_summary(&self.working.raw).map_err(WorkspaceError::Edid)
    }

    pub fn validate(&self) -> Vec<ValidationIssue> {
        let mut issues = Vec::new();
        let expected_len = (usize::from(self.working.extension_blocks) + 1) * 128;
        if self.working.raw.len() != expected_len {
            issues.push(ValidationIssue {
                message: format!(
                    "EDID length is {} bytes but its extension count requires {expected_len}",
                    self.working.raw.len()
                ),
            });
        }

        for (index, block) in self.working.raw.chunks_exact(128).enumerate() {
            if !block_checksum_valid(block) {
                issues.push(ValidationIssue {
                    message: if index == 0 {
                        "base EDID checksum is invalid".to_string()
                    } else {
                        format!("EDID extension {index} checksum is invalid")
                    },
                });
            }
        }

        issues
    }

    pub fn diff_summary(&self) -> Vec<String> {
        if self.original_raw == self.working.raw {
            return vec!["No EDID byte changes.".to_string()];
        }

        let changed_bytes = self
            .original_raw
            .iter()
            .zip(self.working.raw.iter())
            .filter(|(original, working)| original != working)
            .count()
            + self.original_raw.len().abs_diff(self.working.raw.len());

        let mut summary = vec![format!("{changed_bytes} EDID bytes changed.")];
        for operation in &self.operations {
            summary.push(match operation {
                WorkspaceOperation::AddDtd { location, timing } => {
                    format!(
                        "Added {} at {}",
                        timing.hyprland_mode(),
                        format_location(*location)
                    )
                }
                WorkspaceOperation::ReplaceDtd { location, timing } => {
                    format!(
                        "Replaced {} with {}",
                        format_location(*location),
                        timing.hyprland_mode()
                    )
                }
                WorkspaceOperation::DeleteDtd { location } => {
                    format!("Deleted DTD at {}", format_location(*location))
                }
                WorkspaceOperation::DeleteStandardTiming { slot } => {
                    format!("Deleted standard timing slot {slot}")
                }
                WorkspaceOperation::AddStandardTiming { slot, timing } => {
                    format!(
                        "Added standard timing slot {slot}: {}x{} @ {} Hz",
                        timing.width, timing.height, timing.refresh_hz
                    )
                }
                WorkspaceOperation::ReplaceStandardTiming { slot, timing } => {
                    format!(
                        "Replaced standard timing slot {slot} with {}x{} @ {} Hz",
                        timing.width, timing.height, timing.refresh_hz
                    )
                }
                WorkspaceOperation::ReorderDtd { from, to } => {
                    format!(
                        "Moved DTD from {} to {}",
                        format_location(*from),
                        format_location(*to)
                    )
                }
                WorkspaceOperation::Reset => "Reset working EDID to original bytes".to_string(),
                WorkspaceOperation::ImportEdid { source } => {
                    format!("Imported EDID from {source}")
                }
            });
        }

        summary
    }

    pub fn export_bytes(&self) -> &[u8] {
        &self.working.raw
    }

    fn replace_working_raw(&mut self, raw: Vec<u8>) -> Result<(), WorkspaceError> {
        self.working = parse_edid(raw)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveDirection {
    Up,
    Down,
}

impl MoveDirection {
    fn target_index(self, index: usize, len: usize) -> Option<usize> {
        match self {
            Self::Up => index.checked_sub(1),
            Self::Down => (index + 1 < len).then_some(index + 1),
        }
    }
}

pub fn format_location(location: DtdLocation) -> String {
    match location {
        DtdLocation::Base { slot } => format!("base DTD slot {slot}"),
        DtdLocation::Cta {
            extension_index,
            slot,
        } => format!("CTA-861 extension {extension_index} DTD slot {slot}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edid::{block_checksum_valid, patch_base_detailed_timing};
    use crate::models::StandardTimingAspect;

    const HEADER: [u8; 8] = [0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00];

    fn sample_timing() -> TimingDescriptor {
        TimingDescriptor {
            pixel_clock_khz: 138_500,
            h_active: 1920,
            h_blanking: 160,
            h_front_porch: 48,
            h_sync_width: 32,
            h_back_porch: 80,
            v_active: 1080,
            v_blanking: 31,
            v_front_porch: 3,
            v_sync_width: 5,
            v_back_porch: 23,
            h_sync_positive: true,
            v_sync_positive: false,
            interlaced: false,
        }
    }

    fn alternate_timing() -> TimingDescriptor {
        TimingDescriptor {
            pixel_clock_khz: 241_500,
            h_active: 2560,
            h_blanking: 160,
            h_front_porch: 48,
            h_sync_width: 32,
            h_back_porch: 80,
            v_active: 1440,
            v_blanking: 41,
            v_front_porch: 3,
            v_sync_width: 5,
            v_back_porch: 33,
            h_sync_positive: true,
            v_sync_positive: false,
            interlaced: false,
        }
    }

    fn minimal_base_edid(extension_blocks: u8) -> Vec<u8> {
        let mut edid = vec![0u8; 128];
        edid[..8].copy_from_slice(&HEADER);
        edid[8] = 0x04;
        edid[9] = 0x6d;
        edid[38..54].fill(0x01);
        edid[126] = extension_blocks;
        repair_checksum(&mut edid);
        edid
    }

    fn cta_extension() -> Vec<u8> {
        let mut cta = vec![0u8; 128];
        cta[0] = 0x02;
        cta[1] = 3;
        cta[2] = 4;
        repair_checksum(&mut cta);
        cta
    }

    fn repair_checksum(block: &mut [u8]) {
        let last = block.len() - 1;
        block[last] = 0;
        let sum = block[..last]
            .iter()
            .fold(0u8, |acc, byte| acc.wrapping_add(*byte));
        block[last] = 0u8.wrapping_sub(sum);
    }

    #[test]
    fn add_dtd_tracks_location_and_diff() {
        let mut workspace = EdidWorkspace::new(minimal_base_edid(0)).expect("workspace");
        let location = workspace.add_dtd(sample_timing()).expect("add DTD");

        assert_eq!(location, DtdLocation::Base { slot: 0 });
        assert_eq!(workspace.operations.len(), 1);
        assert!(
            workspace
                .diff_summary()
                .iter()
                .any(|line| line.contains("Added"))
        );
        assert!(workspace.validate().is_empty());
    }

    #[test]
    fn unchanged_valid_workspace_has_no_validation_issues() {
        let workspace = EdidWorkspace::new(minimal_base_edid(0)).expect("workspace");

        assert!(workspace.validate().is_empty());
    }

    #[test]
    fn validation_checks_extension_count_and_every_block_checksum() {
        let mut truncated = minimal_base_edid(1);
        let workspace = EdidWorkspace::new(truncated.clone()).expect("workspace");
        assert!(
            workspace
                .validate()
                .iter()
                .any(|issue| issue.message.contains("length"))
        );

        truncated[126] = 0;
        repair_checksum(&mut truncated);
        let mut raw = truncated;
        let mut unknown = vec![0u8; 128];
        unknown[0] = 0x70;
        unknown[10] = 1;
        raw[126] = 1;
        repair_checksum(&mut raw[..128]);
        raw.extend(unknown);
        let workspace = EdidWorkspace::new(raw).expect("workspace");
        assert!(
            workspace
                .validate()
                .iter()
                .any(|issue| issue.message.contains("extension 1 checksum"))
        );
    }

    #[test]
    fn add_cta_dtd_targets_extension_slots() {
        let mut edid = minimal_base_edid(1);
        edid.extend(cta_extension());
        let mut workspace = EdidWorkspace::new(edid).expect("workspace");

        let location = workspace
            .add_cta_dtd(1, sample_timing())
            .expect("add CTA DTD");

        assert_eq!(
            location,
            DtdLocation::Cta {
                extension_index: 1,
                slot: 0
            }
        );
        assert!(block_checksum_valid(&workspace.export_bytes()[128..256]));
        let slots = workspace.cta_dtd_slots().expect("CTA DTD slots");
        assert!(slots[0].timing.is_some());
        assert!(
            workspace
                .diff_summary()
                .iter()
                .any(|line| line.contains("CTA-861 extension 1 DTD slot 0"))
        );
    }

    #[test]
    fn add_cta_dtd_at_targets_exact_free_slot() {
        let mut edid = minimal_base_edid(1);
        edid.extend(cta_extension());
        let mut workspace = EdidWorkspace::new(edid).expect("workspace");

        let location = workspace
            .add_cta_dtd_at(
                DtdLocation::Cta {
                    extension_index: 1,
                    slot: 2,
                },
                sample_timing(),
            )
            .expect("add exact CTA DTD");

        assert_eq!(
            location,
            DtdLocation::Cta {
                extension_index: 1,
                slot: 2
            }
        );
        let slots = workspace.cta_dtd_slots().expect("CTA DTD slots");
        assert!(slots[2].timing.is_some());
        assert!(slots[0].timing.is_none());
    }

    #[test]
    fn replace_and_delete_dtd_are_checksum_safe() {
        let timing = sample_timing();
        let mut edid = minimal_base_edid(0);
        edid = patch_base_detailed_timing(&edid, 0, &timing).expect("patch");

        let mut workspace = EdidWorkspace::new(edid).expect("workspace");
        workspace
            .replace_dtd(DtdLocation::Base { slot: 0 }, alternate_timing())
            .expect("replace");
        workspace
            .delete_dtd(DtdLocation::Base { slot: 0 })
            .expect("delete");

        assert!(block_checksum_valid(&workspace.export_bytes()[..128]));
        assert_eq!(workspace.dtds().expect("dtds").len(), 0);
        assert_eq!(workspace.operations.len(), 2);
    }

    #[test]
    fn move_dtd_swaps_adjacent_timing_payloads() {
        let first = sample_timing();
        let second = alternate_timing();
        let mut workspace = EdidWorkspace::new(minimal_base_edid(0)).expect("workspace");
        workspace.add_dtd(first.clone()).expect("add first");
        workspace.add_dtd(second.clone()).expect("add second");

        let selected = workspace.move_dtd(1, MoveDirection::Up).expect("move up");
        let dtds = workspace.dtds().expect("dtds");

        assert_eq!(selected, 0);
        assert_eq!(dtds[0].timing, second);
        assert_eq!(dtds[1].timing, first);
        assert!(block_checksum_valid(&workspace.export_bytes()[..128]));
    }

    #[test]
    fn reset_restores_original_bytes_and_clears_pending_changes() {
        let original = minimal_base_edid(0);
        let mut workspace = EdidWorkspace::new(original.clone()).expect("workspace");
        workspace.add_dtd(sample_timing()).expect("add");

        workspace.reset().expect("reset");

        assert_eq!(workspace.export_bytes(), original.as_slice());
        assert_eq!(workspace.operations, vec![WorkspaceOperation::Reset]);
        assert_eq!(workspace.diff_summary(), vec!["No EDID byte changes."]);
    }

    #[test]
    fn import_replaces_working_edid_and_preserves_original_diff() {
        let original = minimal_base_edid(0);
        let mut imported = minimal_base_edid(0);
        imported[16] = 1;
        repair_checksum(&mut imported);

        let mut workspace = EdidWorkspace::new(original).expect("workspace");
        workspace
            .import_working_raw(imported.clone(), "/tmp/imported.bin")
            .expect("import");

        assert_eq!(workspace.export_bytes(), imported.as_slice());
        assert!(workspace.has_changes());
        assert!(
            workspace
                .diff_summary()
                .iter()
                .any(|line| line.contains("Imported EDID"))
        );
    }

    #[test]
    fn delete_standard_timing_repairs_checksum() {
        let mut edid = minimal_base_edid(0);
        edid[38] = (1920u16 / 8 - 31) as u8;
        edid[39] = 0b11_000000;
        repair_checksum(&mut edid);

        let mut workspace = EdidWorkspace::new(edid).expect("workspace");
        assert_eq!(
            workspace
                .standard_timings()
                .expect("standard timings")
                .len(),
            1
        );

        workspace
            .delete_standard_timing(0)
            .expect("delete standard");

        assert!(block_checksum_valid(&workspace.export_bytes()[..128]));
        assert!(
            workspace
                .standard_timings()
                .expect("standard timings")
                .is_empty()
        );
        assert!(
            workspace
                .diff_summary()
                .iter()
                .any(|line| line.contains("Deleted standard timing"))
        );
    }

    #[test]
    fn add_and_replace_standard_timing_repairs_checksum() {
        let mut workspace = EdidWorkspace::new(minimal_base_edid(0)).expect("workspace");
        let slot = workspace
            .add_standard_timing(StandardTiming {
                slot: 0,
                width: 1920,
                height: 1080,
                refresh_hz: 60,
                aspect: StandardTimingAspect::SixteenNine,
            })
            .expect("add standard");

        assert_eq!(slot, 0);
        workspace
            .replace_standard_timing(
                slot,
                StandardTiming {
                    slot,
                    width: 1280,
                    height: 1024,
                    refresh_hz: 75,
                    aspect: StandardTimingAspect::FiveFour,
                },
            )
            .expect("replace standard");

        assert!(block_checksum_valid(&workspace.export_bytes()[..128]));
        let timings = workspace.standard_timings().expect("standard timings");
        assert_eq!(timings.len(), 1);
        assert_eq!(timings[0].slot, slot);
        assert_eq!(timings[0].timing.width, 1280);
        assert!(
            workspace
                .diff_summary()
                .iter()
                .any(|line| line.contains("Replaced standard timing"))
        );
    }
}
