use crate::models::{
    Cta861Block, CtaDataBlock, CtaVideoDescriptor, CtaVideoMode, DisplayIdBlock,
    DisplayIdDataBlock, DisplayIdDetailedTiming, EdidData, EstablishedTiming, StandardTiming,
    StandardTimingAspect, TimingDescriptor,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EdidError {
    #[error("EDID must be at least 128 bytes, got {0}")]
    TooShort(usize),
    #[error("invalid EDID header")]
    InvalidHeader,
    #[error("detailed timing slot must be 0..=3, got {0}")]
    InvalidDetailedTimingSlot(usize),
    #[error("standard timing slot must be 0..=7, got {0}")]
    InvalidStandardTimingSlot(usize),
    #[error("standard timing is invalid: {0}")]
    InvalidStandardTiming(&'static str),
    #[error("timing field {field}={value} exceeds EDID's detailed timing limit")]
    TimingFieldTooLarge { field: &'static str, value: u32 },
    #[error("pixel clock {0} kHz exceeds EDID's detailed timing limit")]
    PixelClockTooLarge(u32),
    #[error("no empty base or CTA-861 detailed timing slot is available")]
    NoAvailableDetailedTimingSlot,
    #[error("no empty standard timing slot is available")]
    NoAvailableStandardTimingSlot,
}

const HEADER: [u8; 8] = [0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00];
const BASE_BLOCK_LEN: usize = 128;
const EDID_BLOCK_LEN: usize = 128;
const DTD_START: usize = 54;
const DTD_LEN: usize = 18;
const DTD_SLOTS: usize = 4;
const STANDARD_TIMING_START: usize = 38;
const STANDARD_TIMING_LEN: usize = 2;
const STANDARD_TIMING_SLOTS: usize = 8;
const CTA_TAG: u8 = 0x02;
const DISPLAYID_TAG: u8 = 0x70;
const CTA_HEADER_LEN: usize = 4;
const DISPLAYID_HEADER_LEN: usize = 5;
const DISPLAYID_DATA_BLOCK_HEADER_LEN: usize = 3;
const DISPLAYID_TYPE_I_DTD_LEN: usize = 20;
const BLOCK_CHECKSUM_INDEX: usize = 127;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DtdLocation {
    Base { slot: usize },
    Cta { extension_index: u8, slot: usize },
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocatedDetailedTiming {
    pub location: DtdLocation,
    pub timing: TimingDescriptor,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CtaDtdSlot {
    pub extension_index: u8,
    pub revision: u8,
    pub dtd_offset: u8,
    pub slot: usize,
    pub timing: Option<TimingDescriptor>,
    pub occupied_unknown: bool,
    pub checksum_valid: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocatedStandardTiming {
    pub slot: usize,
    pub timing: StandardTiming,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdidSlotSummary {
    pub base_dtd_used: usize,
    pub base_dtd_free: usize,
    pub standard_used: usize,
    pub standard_free: usize,
    pub cta_dtd_used: usize,
    pub cta_dtd_free: usize,
}

pub fn parse_edid(raw: Vec<u8>) -> Result<EdidData, EdidError> {
    if raw.len() < BASE_BLOCK_LEN {
        return Err(EdidError::TooShort(raw.len()));
    }

    let base = &raw[..BASE_BLOCK_LEN];
    if base[..8] != HEADER {
        return Err(EdidError::InvalidHeader);
    }

    let manufacturer_id = parse_manufacturer_id(base[8], base[9]);
    let product_code = Some(u16::from_le_bytes([base[10], base[11]]));
    let serial_number = Some(u32::from_le_bytes([base[12], base[13], base[14], base[15]]));
    let extension_blocks = base[126];
    let checksum_valid = block_checksum_valid(base);
    let established_timings = parse_established_timings(base);
    let standard_timings = parse_standard_timings(base);

    let mut monitor_name = None;
    let mut descriptor_text = Vec::new();
    let mut detailed_timings = Vec::new();

    for descriptor in base[DTD_START..126].chunks_exact(DTD_LEN) {
        if descriptor[0] == 0 && descriptor[1] == 0 {
            match descriptor[3] {
                0xfc => monitor_name = parse_descriptor_text(descriptor),
                0xfe | 0xff => {
                    if let Some(text) = parse_descriptor_text(descriptor) {
                        descriptor_text.push(text);
                    }
                }
                _ => {}
            }
        } else if let Some(timing) = parse_detailed_timing(descriptor) {
            detailed_timings.push(timing);
        }
    }
    let cta_blocks = parse_cta_blocks(&raw, extension_blocks);
    let displayid_blocks = parse_displayid_blocks(&raw, extension_blocks);

    Ok(EdidData {
        raw,
        manufacturer_id,
        product_code,
        serial_number,
        monitor_name,
        descriptor_text,
        established_timings,
        standard_timings,
        detailed_timings,
        cta_blocks,
        displayid_blocks,
        extension_blocks,
        checksum_valid,
    })
}

pub fn slot_summary(raw: &[u8]) -> Result<EdidSlotSummary, EdidError> {
    if raw.len() < BASE_BLOCK_LEN {
        return Err(EdidError::TooShort(raw.len()));
    }
    if raw[..8] != HEADER {
        return Err(EdidError::InvalidHeader);
    }

    let base_descriptors = raw[DTD_START..126].chunks_exact(DTD_LEN);
    let base_dtd_used = base_descriptors
        .clone()
        .filter(|descriptor| parse_detailed_timing(descriptor).is_some())
        .count();
    let base_dtd_free = base_descriptors
        .filter(|descriptor| descriptor.iter().all(|byte| *byte == 0))
        .count();
    let standard_used = parse_standard_timings(&raw[..BASE_BLOCK_LEN]).len();
    let standard_free = raw[STANDARD_TIMING_START
        ..STANDARD_TIMING_START + STANDARD_TIMING_SLOTS * STANDARD_TIMING_LEN]
        .chunks_exact(STANDARD_TIMING_LEN)
        .filter(|timing| *timing == [0x01, 0x01])
        .count();

    let mut cta_dtd_used = 0;
    let mut cta_dtd_free = 0;
    let extension_blocks = raw[126];
    for extension_index in 1..=extension_blocks {
        let block_start = usize::from(extension_index) * EDID_BLOCK_LEN;
        let block_end = block_start + EDID_BLOCK_LEN;
        if block_end > raw.len() {
            break;
        }
        let block = &raw[block_start..block_end];
        if block[0] != CTA_TAG {
            continue;
        }
        let Some(dtd_range) = cta_dtd_range(block) else {
            continue;
        };
        for descriptor in block[dtd_range].chunks_exact(DTD_LEN) {
            if descriptor.iter().all(|byte| *byte == 0) {
                cta_dtd_free += 1;
            } else if parse_detailed_timing(descriptor).is_some() {
                cta_dtd_used += 1;
            }
        }
    }

    Ok(EdidSlotSummary {
        base_dtd_used,
        base_dtd_free,
        standard_used,
        standard_free,
        cta_dtd_used,
        cta_dtd_free,
    })
}

pub fn delete_standard_timing(raw: &[u8], slot: usize) -> Result<Vec<u8>, EdidError> {
    let mut patched = raw.to_vec();
    let offset = standard_timing_offset(raw, slot)?;
    patched[offset] = 0x01;
    patched[offset + 1] = 0x01;
    repair_block_checksum(&mut patched[..BASE_BLOCK_LEN]);
    Ok(patched)
}

pub fn insert_standard_timing(
    raw: &[u8],
    timing: &StandardTiming,
) -> Result<(Vec<u8>, usize), EdidError> {
    if raw.len() < BASE_BLOCK_LEN {
        return Err(EdidError::TooShort(raw.len()));
    }
    if raw[..8] != HEADER {
        return Err(EdidError::InvalidHeader);
    }

    let Some(slot) = empty_standard_timing_slot(raw) else {
        return Err(EdidError::NoAvailableStandardTimingSlot);
    };
    let patched = patch_standard_timing(raw, slot, timing)?;
    Ok((patched, slot))
}

pub fn patch_standard_timing(
    raw: &[u8],
    slot: usize,
    timing: &StandardTiming,
) -> Result<Vec<u8>, EdidError> {
    let descriptor = encode_standard_timing(timing)?;
    let offset = standard_timing_offset(raw, slot)?;
    let mut patched = raw.to_vec();
    patched[offset..offset + STANDARD_TIMING_LEN].copy_from_slice(&descriptor);
    repair_block_checksum(&mut patched[..BASE_BLOCK_LEN]);
    Ok(patched)
}

pub fn standard_timing_locations(raw: &[u8]) -> Result<Vec<LocatedStandardTiming>, EdidError> {
    if raw.len() < BASE_BLOCK_LEN {
        return Err(EdidError::TooShort(raw.len()));
    }
    if raw[..8] != HEADER {
        return Err(EdidError::InvalidHeader);
    }

    Ok(parse_standard_timings(&raw[..BASE_BLOCK_LEN])
        .into_iter()
        .map(|timing| LocatedStandardTiming {
            slot: timing.slot,
            timing,
        })
        .collect())
}

pub fn insert_detailed_timing(
    raw: &[u8],
    timing: &TimingDescriptor,
) -> Result<(Vec<u8>, DtdLocation), EdidError> {
    if raw.len() < BASE_BLOCK_LEN {
        return Err(EdidError::TooShort(raw.len()));
    }
    if raw[..8] != HEADER {
        return Err(EdidError::InvalidHeader);
    }

    if let Some((slot, _)) = base_empty_dtd_slot(raw) {
        let location = DtdLocation::Base { slot };
        let patched = patch_detailed_timing(raw, location, timing)?;
        return Ok((patched, location));
    }

    let descriptor = encode_detailed_timing(timing)?;
    let mut patched = raw.to_vec();

    let extension_blocks = raw[126];
    for extension_index in 1..=extension_blocks {
        let block_start = usize::from(extension_index) * EDID_BLOCK_LEN;
        let block_end = block_start + EDID_BLOCK_LEN;
        if block_end > patched.len() {
            break;
        }

        let block = &patched[block_start..block_end];
        if block[0] != CTA_TAG {
            continue;
        }

        let Some((slot, relative_offset)) = cta_empty_dtd_slot(block) else {
            continue;
        };
        let offset = block_start + relative_offset;
        patched[offset..offset + DTD_LEN].copy_from_slice(&descriptor);
        repair_block_checksum(&mut patched[block_start..block_end]);
        return Ok((
            patched,
            DtdLocation::Cta {
                extension_index,
                slot,
            },
        ));
    }

    Err(EdidError::NoAvailableDetailedTimingSlot)
}

pub fn insert_cta_detailed_timing(
    raw: &[u8],
    extension_index: u8,
    timing: &TimingDescriptor,
) -> Result<(Vec<u8>, DtdLocation), EdidError> {
    if raw.len() < BASE_BLOCK_LEN {
        return Err(EdidError::TooShort(raw.len()));
    }
    if raw[..8] != HEADER {
        return Err(EdidError::InvalidHeader);
    }

    let block_start = usize::from(extension_index) * EDID_BLOCK_LEN;
    let block_end = block_start + EDID_BLOCK_LEN;
    if block_end > raw.len() {
        return Err(EdidError::TooShort(block_end));
    }

    let block = &raw[block_start..block_end];
    if block[0] != CTA_TAG {
        return Err(EdidError::NoAvailableDetailedTimingSlot);
    }

    let Some((slot, relative_offset)) = cta_empty_dtd_slot(block) else {
        return Err(EdidError::NoAvailableDetailedTimingSlot);
    };

    let descriptor = encode_detailed_timing(timing)?;
    let mut patched = raw.to_vec();
    let offset = block_start + relative_offset;
    patched[offset..offset + DTD_LEN].copy_from_slice(&descriptor);
    repair_block_checksum(&mut patched[block_start..block_end]);
    Ok((
        patched,
        DtdLocation::Cta {
            extension_index,
            slot,
        },
    ))
}

pub fn patch_detailed_timing(
    raw: &[u8],
    location: DtdLocation,
    timing: &TimingDescriptor,
) -> Result<Vec<u8>, EdidError> {
    let mut patched = raw.to_vec();
    let descriptor = encode_detailed_timing(timing)?;
    let (offset, block_start) = dtd_offset(raw, location)?;
    patched[offset..offset + DTD_LEN].copy_from_slice(&descriptor);
    repair_block_checksum(&mut patched[block_start..block_start + EDID_BLOCK_LEN]);
    Ok(patched)
}

pub fn delete_detailed_timing(raw: &[u8], location: DtdLocation) -> Result<Vec<u8>, EdidError> {
    let mut patched = raw.to_vec();
    let (offset, block_start) = dtd_offset(raw, location)?;
    patched[offset..offset + DTD_LEN].fill(0);
    repair_block_checksum(&mut patched[block_start..block_start + EDID_BLOCK_LEN]);
    Ok(patched)
}

#[cfg(test)]
pub fn patch_base_detailed_timing(
    raw: &[u8],
    slot: usize,
    timing: &TimingDescriptor,
) -> Result<Vec<u8>, EdidError> {
    if raw.len() < BASE_BLOCK_LEN {
        return Err(EdidError::TooShort(raw.len()));
    }
    if raw[..8] != HEADER {
        return Err(EdidError::InvalidHeader);
    }
    if slot >= DTD_SLOTS {
        return Err(EdidError::InvalidDetailedTimingSlot(slot));
    }

    patch_detailed_timing(raw, DtdLocation::Base { slot }, timing)
}

pub fn detailed_timing_locations(raw: &[u8]) -> Result<Vec<LocatedDetailedTiming>, EdidError> {
    if raw.len() < BASE_BLOCK_LEN {
        return Err(EdidError::TooShort(raw.len()));
    }
    if raw[..8] != HEADER {
        return Err(EdidError::InvalidHeader);
    }

    let mut timings = Vec::new();
    for (slot, descriptor) in raw[DTD_START..126].chunks_exact(DTD_LEN).enumerate() {
        if let Some(timing) = parse_detailed_timing(descriptor) {
            timings.push(LocatedDetailedTiming {
                location: DtdLocation::Base { slot },
                timing,
            });
        }
    }

    let extension_blocks = raw[126];
    for extension_index in 1..=extension_blocks {
        let block_start = usize::from(extension_index) * EDID_BLOCK_LEN;
        let block_end = block_start + EDID_BLOCK_LEN;
        if block_end > raw.len() {
            break;
        }
        let block = &raw[block_start..block_end];
        if block[0] != CTA_TAG {
            continue;
        }
        let Some(dtd_range) = cta_dtd_range(block) else {
            continue;
        };
        for (slot, descriptor) in block[dtd_range].chunks_exact(DTD_LEN).enumerate() {
            if let Some(timing) = parse_detailed_timing(descriptor) {
                timings.push(LocatedDetailedTiming {
                    location: DtdLocation::Cta {
                        extension_index,
                        slot,
                    },
                    timing,
                });
            }
        }
    }

    Ok(timings)
}

pub fn cta_dtd_slots(raw: &[u8]) -> Result<Vec<CtaDtdSlot>, EdidError> {
    if raw.len() < BASE_BLOCK_LEN {
        return Err(EdidError::TooShort(raw.len()));
    }
    if raw[..8] != HEADER {
        return Err(EdidError::InvalidHeader);
    }

    let mut slots = Vec::new();
    let extension_blocks = raw[126];
    for extension_index in 1..=extension_blocks {
        let block_start = usize::from(extension_index) * EDID_BLOCK_LEN;
        let block_end = block_start + EDID_BLOCK_LEN;
        if block_end > raw.len() {
            break;
        }
        let block = &raw[block_start..block_end];
        if block[0] != CTA_TAG {
            continue;
        }
        let Some(dtd_range) = cta_dtd_range(block) else {
            continue;
        };

        for (slot, descriptor) in block[dtd_range].chunks_exact(DTD_LEN).enumerate() {
            let is_free = descriptor.iter().all(|byte| *byte == 0);
            let timing = (!is_free)
                .then(|| parse_detailed_timing(descriptor))
                .flatten();
            slots.push(CtaDtdSlot {
                extension_index,
                revision: block[1],
                dtd_offset: block[2],
                slot,
                occupied_unknown: !is_free && timing.is_none(),
                timing,
                checksum_valid: block_checksum_valid(block),
            });
        }
    }

    Ok(slots)
}

pub fn encode_detailed_timing(timing: &TimingDescriptor) -> Result<[u8; DTD_LEN], EdidError> {
    validate_12_bit("h_active", timing.h_active)?;
    validate_12_bit("h_blanking", timing.h_blanking)?;
    validate_12_bit("v_active", timing.v_active)?;
    validate_12_bit("v_blanking", timing.v_blanking)?;
    validate_10_bit("h_front_porch", timing.h_front_porch)?;
    validate_10_bit("h_sync_width", timing.h_sync_width)?;
    validate_6_bit("v_front_porch", timing.v_front_porch)?;
    validate_6_bit("v_sync_width", timing.v_sync_width)?;

    let pixel_clock_10khz = timing.pixel_clock_khz / 10;
    if pixel_clock_10khz > u32::from(u16::MAX) {
        return Err(EdidError::PixelClockTooLarge(timing.pixel_clock_khz));
    }

    let mut descriptor = [0u8; DTD_LEN];
    let pixel_clock = pixel_clock_10khz as u16;
    descriptor[0..2].copy_from_slice(&pixel_clock.to_le_bytes());
    descriptor[2] = low_byte(timing.h_active);
    descriptor[3] = low_byte(timing.h_blanking);
    descriptor[4] = high_nibble(timing.h_active) << 4 | high_nibble(timing.h_blanking);
    descriptor[5] = low_byte(timing.v_active);
    descriptor[6] = low_byte(timing.v_blanking);
    descriptor[7] = high_nibble(timing.v_active) << 4 | high_nibble(timing.v_blanking);
    descriptor[8] = low_byte(timing.h_front_porch);
    descriptor[9] = low_byte(timing.h_sync_width);
    descriptor[10] = low_nibble(timing.v_front_porch) << 4 | low_nibble(timing.v_sync_width);
    descriptor[11] = ((timing.h_front_porch >> 8) as u8 & 0x03) << 6
        | ((timing.h_sync_width >> 8) as u8 & 0x03) << 4
        | ((timing.v_front_porch >> 4) as u8 & 0x03) << 2
        | ((timing.v_sync_width >> 4) as u8 & 0x03);
    descriptor[17] = 0x18
        | if timing.interlaced { 0x80 } else { 0 }
        | if timing.v_sync_positive { 0x04 } else { 0 }
        | if timing.h_sync_positive { 0x02 } else { 0 };
    Ok(descriptor)
}

pub fn encode_standard_timing(
    timing: &StandardTiming,
) -> Result<[u8; STANDARD_TIMING_LEN], EdidError> {
    if timing.width < 256 || timing.width > 2288 {
        return Err(EdidError::InvalidStandardTiming(
            "width must be between 256 and 2288 pixels",
        ));
    }
    if !timing.width.is_multiple_of(8) {
        return Err(EdidError::InvalidStandardTiming(
            "width must be divisible by 8 pixels",
        ));
    }
    if !(60..=123).contains(&timing.refresh_hz) {
        return Err(EdidError::InvalidStandardTiming(
            "refresh must be between 60 and 123 Hz",
        ));
    }
    if StandardTimingAspect::from_dimensions(timing.width, timing.height) != Some(timing.aspect) {
        return Err(EdidError::InvalidStandardTiming(
            "height must exactly match one EDID standard timing aspect ratio",
        ));
    }

    let width_byte = (timing.width / 8 - 31) as u8;
    let aspect_bits = match timing.aspect {
        StandardTimingAspect::SixteenTen => 0b00,
        StandardTimingAspect::FourThree => 0b01,
        StandardTimingAspect::FiveFour => 0b10,
        StandardTimingAspect::SixteenNine => 0b11,
    };
    let refresh_bits = (timing.refresh_hz - 60) as u8;
    Ok([width_byte, aspect_bits << 6 | refresh_bits])
}

pub fn block_checksum_valid(block: &[u8]) -> bool {
    block.iter().fold(0u8, |acc, byte| acc.wrapping_add(*byte)) == 0
}

fn repair_block_checksum(block: &mut [u8]) {
    let last = block.len() - 1;
    block[last] = 0;
    let sum_without_checksum = block[..last]
        .iter()
        .fold(0u8, |acc, byte| acc.wrapping_add(*byte));
    block[last] = 0u8.wrapping_sub(sum_without_checksum);
}

fn validate_12_bit(field: &'static str, value: u16) -> Result<(), EdidError> {
    validate_max(field, value, 0x0fff)
}

fn validate_10_bit(field: &'static str, value: u16) -> Result<(), EdidError> {
    validate_max(field, value, 0x03ff)
}

fn validate_6_bit(field: &'static str, value: u16) -> Result<(), EdidError> {
    validate_max(field, value, 0x003f)
}

fn validate_max(field: &'static str, value: u16, max: u16) -> Result<(), EdidError> {
    if value > max {
        Err(EdidError::TimingFieldTooLarge {
            field,
            value: u32::from(value),
        })
    } else {
        Ok(())
    }
}

fn low_byte(value: u16) -> u8 {
    value as u8
}

fn le_u16(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}

fn le_u24(bytes: &[u8]) -> u32 {
    u32::from(bytes[0]) | (u32::from(bytes[1]) << 8) | (u32::from(bytes[2]) << 16)
}

fn low_nibble(value: u16) -> u8 {
    (value as u8) & 0x0f
}

fn high_nibble(value: u16) -> u8 {
    ((value >> 8) as u8) & 0x0f
}

fn parse_cta_blocks(raw: &[u8], extension_blocks: u8) -> Vec<Cta861Block> {
    (1..=extension_blocks)
        .filter_map(|extension_index| {
            let block_start = usize::from(extension_index) * EDID_BLOCK_LEN;
            let block_end = block_start + EDID_BLOCK_LEN;
            if block_end > raw.len() {
                return None;
            }

            parse_cta_block(extension_index, &raw[block_start..block_end])
        })
        .collect()
}

fn parse_displayid_blocks(raw: &[u8], extension_blocks: u8) -> Vec<DisplayIdBlock> {
    (1..=extension_blocks)
        .filter_map(|extension_index| {
            let block_start = usize::from(extension_index) * EDID_BLOCK_LEN;
            let block_end = block_start + EDID_BLOCK_LEN;
            if block_end > raw.len() {
                return None;
            }

            parse_displayid_block(extension_index, &raw[block_start..block_end])
        })
        .collect()
}

fn parse_displayid_block(extension_index: u8, block: &[u8]) -> Option<DisplayIdBlock> {
    if block.len() != EDID_BLOCK_LEN || block.first().copied() != Some(DISPLAYID_TAG) {
        return None;
    }

    let version = block[1];
    let version_major = version >> 4;
    let version_minor = version & 0x0f;
    let payload_len = usize::from(block[2]).min(BLOCK_CHECKSUM_INDEX - DISPLAYID_HEADER_LEN);
    let product_type = block[3];
    let extension_count = block[4];
    let payload_end = DISPLAYID_HEADER_LEN + payload_len;

    let mut data_blocks = Vec::new();
    let mut detailed_timings = Vec::new();
    let mut offset = DISPLAYID_HEADER_LEN;
    let mut data_block_index = 0;
    while offset + DISPLAYID_DATA_BLOCK_HEADER_LEN <= payload_end {
        let tag = block[offset];
        if tag == 0 {
            break;
        }
        let revision = block[offset + 1];
        let block_payload_len = usize::from(block[offset + 2]);
        let payload_start = offset + DISPLAYID_DATA_BLOCK_HEADER_LEN;
        let payload_end = payload_start + block_payload_len;
        if payload_end > block.len().saturating_sub(1)
            || payload_end > DISPLAYID_HEADER_LEN + payload_len
        {
            break;
        }

        let payload = &block[payload_start..payload_end];
        data_blocks.push(DisplayIdDataBlock {
            tag,
            revision,
            payload_len: block_payload_len,
        });
        if tag == 0x03 {
            detailed_timings.extend(parse_displayid_type_i_dtds(
                extension_index,
                data_block_index,
                payload,
            ));
        }

        data_block_index += 1;
        offset = payload_end;
    }

    Some(DisplayIdBlock {
        extension_index,
        version_major,
        version_minor,
        product_type,
        extension_count,
        checksum_valid: block_checksum_valid(block),
        data_blocks,
        detailed_timings,
    })
}

fn parse_displayid_type_i_dtds(
    extension_index: u8,
    data_block_index: usize,
    payload: &[u8],
) -> Vec<DisplayIdDetailedTiming> {
    payload
        .chunks_exact(DISPLAYID_TYPE_I_DTD_LEN)
        .enumerate()
        .filter_map(|(descriptor_index, descriptor)| {
            parse_displayid_type_i_dtd(
                extension_index,
                data_block_index,
                descriptor_index,
                descriptor,
            )
        })
        .collect()
}

fn parse_displayid_type_i_dtd(
    extension_index: u8,
    data_block_index: usize,
    descriptor_index: usize,
    descriptor: &[u8],
) -> Option<DisplayIdDetailedTiming> {
    if descriptor.len() != DISPLAYID_TYPE_I_DTD_LEN {
        return None;
    }

    let pixel_clock_khz = (le_u24(&descriptor[0..3]) + 1) * 10;
    let raw_flags = descriptor[3];
    let h_active = le_u16(&descriptor[4..6]).checked_add(1)?;
    let h_blanking = le_u16(&descriptor[6..8]).checked_add(1)?;
    let h_front_porch = le_u16(&descriptor[8..10]).checked_add(1)?;
    let h_sync_width = le_u16(&descriptor[10..12]).checked_add(1)?;
    let v_active = le_u16(&descriptor[12..14]).checked_add(1)?;
    let v_blanking = le_u16(&descriptor[14..16]).checked_add(1)?;
    let v_front_porch = le_u16(&descriptor[16..18]).checked_add(1)?;
    let v_sync_width = le_u16(&descriptor[18..20]).checked_add(1)?;

    let h_back_porch = h_blanking
        .checked_sub(h_front_porch)?
        .checked_sub(h_sync_width)?;
    let v_back_porch = v_blanking
        .checked_sub(v_front_porch)?
        .checked_sub(v_sync_width)?;

    Some(DisplayIdDetailedTiming {
        extension_index,
        data_block_index,
        descriptor_index,
        raw_flags,
        preferred: raw_flags & 0x80 != 0,
        timing: TimingDescriptor {
            pixel_clock_khz,
            h_active,
            h_blanking,
            h_front_porch,
            h_sync_width,
            h_back_porch,
            v_active,
            v_blanking,
            v_front_porch,
            v_sync_width,
            v_back_porch,
            h_sync_positive: false,
            v_sync_positive: false,
            interlaced: raw_flags & 0x10 != 0,
        },
    })
}

fn parse_cta_block(extension_index: u8, block: &[u8]) -> Option<Cta861Block> {
    if block.first().copied() != Some(CTA_TAG) {
        return None;
    }

    let revision = block[1];
    let dtd_offset = block[2];
    let data_blocks = parse_cta_data_blocks(block);
    let (detailed_timings, available_dtd_slots) = if let Some(dtd_range) = cta_dtd_range(block) {
        let detailed_timings = block[dtd_range.clone()]
            .chunks_exact(DTD_LEN)
            .filter_map(parse_detailed_timing)
            .collect();
        let available_dtd_slots = block[dtd_range]
            .chunks_exact(DTD_LEN)
            .filter(|descriptor| descriptor.iter().all(|byte| *byte == 0))
            .count();
        (detailed_timings, available_dtd_slots)
    } else {
        (Vec::new(), 0)
    };

    Some(Cta861Block {
        extension_index,
        revision,
        dtd_offset,
        checksum_valid: block_checksum_valid(block),
        data_blocks,
        detailed_timings,
        available_dtd_slots,
    })
}

fn parse_cta_data_blocks(block: &[u8]) -> Vec<CtaDataBlock> {
    if block.len() != EDID_BLOCK_LEN || block[0] != CTA_TAG {
        return Vec::new();
    }

    let dtd_start = usize::from(block[2]);
    if !(CTA_HEADER_LEN..=BLOCK_CHECKSUM_INDEX).contains(&dtd_start) {
        return Vec::new();
    }

    let mut data_blocks = Vec::new();
    let mut offset = CTA_HEADER_LEN;
    while offset < dtd_start {
        let header = block[offset];
        if header == 0 {
            break;
        }
        let tag_code = header >> 5;
        let payload_len = usize::from(header & 0x1f);
        let payload_start = offset + 1;
        let payload_end = payload_start + payload_len;
        if payload_end > dtd_start {
            break;
        }

        let extended_tag = if tag_code == 7 && payload_len > 0 {
            Some(block[payload_start])
        } else {
            None
        };
        let payload = &block[payload_start..payload_end];
        let video_modes = if tag_code == 2 {
            parse_cta_video_modes(payload)
        } else {
            Vec::new()
        };
        data_blocks.push(CtaDataBlock {
            tag_code,
            extended_tag,
            payload_len,
            video_modes,
        });
        offset = payload_end;
    }

    data_blocks
}

fn parse_cta_video_modes(payload: &[u8]) -> Vec<CtaVideoDescriptor> {
    payload
        .iter()
        .map(|descriptor| {
            let native = descriptor & 0x80 != 0;
            let vic = u16::from(descriptor & 0x7f);
            cta_vic_mode(vic, native)
                .map(CtaVideoDescriptor::Known)
                .unwrap_or(CtaVideoDescriptor::Unknown { vic, native })
        })
        .collect()
}

fn cta_vic_mode(vic: u16, native: bool) -> Option<CtaVideoMode> {
    let (width, height, refresh_millihz, interlaced) = match vic {
        1 => (640, 480, 60_000, false),
        2 | 3 => (720, 480, 60_000, false),
        4 => (1280, 720, 60_000, false),
        5 => (1920, 1080, 60_000, true),
        16 => (1920, 1080, 60_000, false),
        17 | 18 => (720, 576, 50_000, false),
        19 => (1280, 720, 50_000, false),
        20 => (1920, 1080, 50_000, true),
        31 => (1920, 1080, 50_000, false),
        32 => (1920, 1080, 24_000, false),
        33 => (1920, 1080, 25_000, false),
        34 => (1920, 1080, 30_000, false),
        40 => (1920, 1080, 100_000, true),
        41 => (1280, 720, 100_000, false),
        47 => (1920, 1080, 120_000, true),
        48 => (1280, 720, 120_000, false),
        61 => (1280, 720, 24_000, false),
        62 => (1280, 720, 25_000, false),
        63 => (1280, 720, 30_000, false),
        64 => (1920, 1080, 120_000, false),
        93 => (3840, 2160, 24_000, false),
        94 => (3840, 2160, 25_000, false),
        95 => (3840, 2160, 30_000, false),
        96 => (3840, 2160, 50_000, false),
        97 => (3840, 2160, 60_000, false),
        98 => (4096, 2160, 24_000, false),
        99 => (4096, 2160, 25_000, false),
        100 => (4096, 2160, 30_000, false),
        101 => (4096, 2160, 50_000, false),
        102 => (4096, 2160, 60_000, false),
        _ => return None,
    };

    Some(CtaVideoMode {
        vic,
        native,
        width,
        height,
        refresh_millihz,
        interlaced,
    })
}

fn base_empty_dtd_slot(raw: &[u8]) -> Option<(usize, usize)> {
    raw[DTD_START..126]
        .chunks_exact(DTD_LEN)
        .enumerate()
        .find(|(_, descriptor)| descriptor.iter().all(|byte| *byte == 0))
        .map(|(slot, _)| (slot, DTD_START + slot * DTD_LEN))
}

fn cta_empty_dtd_slot(block: &[u8]) -> Option<(usize, usize)> {
    let dtd_range = cta_dtd_range(block)?;
    block[dtd_range.clone()]
        .chunks_exact(DTD_LEN)
        .enumerate()
        .find(|(_, descriptor)| descriptor.iter().all(|byte| *byte == 0))
        .map(|(slot, _)| (slot, dtd_range.start + slot * DTD_LEN))
}

fn empty_standard_timing_slot(raw: &[u8]) -> Option<usize> {
    raw[STANDARD_TIMING_START..STANDARD_TIMING_START + STANDARD_TIMING_SLOTS * STANDARD_TIMING_LEN]
        .chunks_exact(STANDARD_TIMING_LEN)
        .position(|timing| timing == [0x01, 0x01])
}

fn standard_timing_offset(raw: &[u8], slot: usize) -> Result<usize, EdidError> {
    if raw.len() < BASE_BLOCK_LEN {
        return Err(EdidError::TooShort(raw.len()));
    }
    if raw[..8] != HEADER {
        return Err(EdidError::InvalidHeader);
    }
    if slot >= STANDARD_TIMING_SLOTS {
        return Err(EdidError::InvalidStandardTimingSlot(slot));
    }

    Ok(STANDARD_TIMING_START + slot * STANDARD_TIMING_LEN)
}

fn dtd_offset(raw: &[u8], location: DtdLocation) -> Result<(usize, usize), EdidError> {
    if raw.len() < BASE_BLOCK_LEN {
        return Err(EdidError::TooShort(raw.len()));
    }
    if raw[..8] != HEADER {
        return Err(EdidError::InvalidHeader);
    }

    match location {
        DtdLocation::Base { slot } => {
            if slot >= DTD_SLOTS {
                return Err(EdidError::InvalidDetailedTimingSlot(slot));
            }
            Ok((DTD_START + slot * DTD_LEN, 0))
        }
        DtdLocation::Cta {
            extension_index,
            slot,
        } => {
            let block_start = usize::from(extension_index) * EDID_BLOCK_LEN;
            let block_end = block_start + EDID_BLOCK_LEN;
            if block_end > raw.len() {
                return Err(EdidError::TooShort(block_end));
            }

            let block = &raw[block_start..block_end];
            let Some(dtd_range) = cta_dtd_range(block) else {
                return Err(EdidError::NoAvailableDetailedTimingSlot);
            };
            let offset = dtd_range.start + slot * DTD_LEN;
            if offset + DTD_LEN > dtd_range.end {
                return Err(EdidError::InvalidDetailedTimingSlot(slot));
            }

            Ok((block_start + offset, block_start))
        }
    }
}

fn cta_dtd_range(block: &[u8]) -> Option<std::ops::Range<usize>> {
    if block.len() != EDID_BLOCK_LEN || block[0] != CTA_TAG {
        return None;
    }

    let dtd_start = usize::from(block[2]);
    if dtd_start == 0 || !(CTA_HEADER_LEN..BLOCK_CHECKSUM_INDEX).contains(&dtd_start) {
        return None;
    }

    Some(dtd_start..BLOCK_CHECKSUM_INDEX)
}

fn parse_manufacturer_id(byte_a: u8, byte_b: u8) -> Option<String> {
    let word = u16::from_be_bytes([byte_a, byte_b]);
    let chars = [
        ((word >> 10) & 0x1f) as u8,
        ((word >> 5) & 0x1f) as u8,
        (word & 0x1f) as u8,
    ];

    if chars.iter().any(|value| *value == 0 || *value > 26) {
        return None;
    }

    Some(
        chars
            .iter()
            .map(|value| char::from(b'A' + value - 1))
            .collect(),
    )
}

fn parse_descriptor_text(descriptor: &[u8]) -> Option<String> {
    let text = descriptor[5..18]
        .iter()
        .copied()
        .take_while(|byte| *byte != b'\n' && *byte != 0)
        .map(char::from)
        .collect::<String>()
        .trim()
        .to_string();

    (!text.is_empty()).then_some(text)
}

fn parse_established_timings(base: &[u8]) -> Vec<EstablishedTiming> {
    let definitions = [
        (35, 7, 720, 400, 70),
        (35, 6, 720, 400, 88),
        (35, 5, 640, 480, 60),
        (35, 4, 640, 480, 67),
        (35, 3, 640, 480, 72),
        (35, 2, 640, 480, 75),
        (35, 1, 800, 600, 56),
        (35, 0, 800, 600, 60),
        (36, 7, 800, 600, 72),
        (36, 6, 800, 600, 75),
        (36, 5, 832, 624, 75),
        (36, 4, 1024, 768, 87),
        (36, 3, 1024, 768, 60),
        (36, 2, 1024, 768, 70),
        (36, 1, 1024, 768, 75),
        (36, 0, 1280, 1024, 75),
        (37, 7, 1152, 870, 75),
    ];

    definitions
        .iter()
        .filter(|(byte, bit, _, _, _)| base[*byte] & (1 << bit) != 0)
        .map(|(_, _, width, height, refresh_hz)| EstablishedTiming {
            width: *width,
            height: *height,
            refresh_hz: *refresh_hz,
        })
        .collect()
}

fn parse_standard_timings(base: &[u8]) -> Vec<StandardTiming> {
    base[STANDARD_TIMING_START..STANDARD_TIMING_START + STANDARD_TIMING_SLOTS * STANDARD_TIMING_LEN]
        .chunks_exact(STANDARD_TIMING_LEN)
        .enumerate()
        .filter_map(|(slot, timing)| parse_standard_timing(slot, timing[0], timing[1]))
        .collect()
}

fn parse_standard_timing(slot: usize, byte_a: u8, byte_b: u8) -> Option<StandardTiming> {
    if (byte_a == 0x01 && byte_b == 0x01) || byte_a == 0x00 {
        return None;
    }

    let width = (u16::from(byte_a) + 31) * 8;
    let aspect = match (byte_b >> 6) & 0x03 {
        0 => StandardTimingAspect::SixteenTen,
        1 => StandardTimingAspect::FourThree,
        2 => StandardTimingAspect::FiveFour,
        _ => StandardTimingAspect::SixteenNine,
    };
    let height = match aspect {
        StandardTimingAspect::SixteenTen => width * 10 / 16,
        StandardTimingAspect::FourThree => width * 3 / 4,
        StandardTimingAspect::FiveFour => width * 4 / 5,
        StandardTimingAspect::SixteenNine => width * 9 / 16,
    };
    let refresh_hz = u16::from(byte_b & 0x3f) + 60;

    Some(StandardTiming {
        slot,
        width,
        height,
        refresh_hz,
        aspect,
    })
}

fn parse_detailed_timing(descriptor: &[u8]) -> Option<TimingDescriptor> {
    let pixel_clock_10khz = u16::from_le_bytes([descriptor[0], descriptor[1]]);
    if pixel_clock_10khz == 0 {
        return None;
    }

    let h_active = u16::from(descriptor[2]) | (u16::from(descriptor[4] & 0xf0) << 4);
    let h_blanking = u16::from(descriptor[3]) | (u16::from(descriptor[4] & 0x0f) << 8);
    let v_active = u16::from(descriptor[5]) | (u16::from(descriptor[7] & 0xf0) << 4);
    let v_blanking = u16::from(descriptor[6]) | (u16::from(descriptor[7] & 0x0f) << 8);
    let h_front_porch = u16::from(descriptor[8]) | (u16::from(descriptor[11] & 0xc0) << 2);
    let h_sync_width = u16::from(descriptor[9]) | (u16::from(descriptor[11] & 0x30) << 4);
    let v_front_porch =
        u16::from((descriptor[10] & 0xf0) >> 4) | (u16::from(descriptor[11] & 0x0c) << 2);
    let v_sync_width = u16::from(descriptor[10] & 0x0f) | (u16::from(descriptor[11] & 0x03) << 4);
    let h_back_porch = h_blanking.saturating_sub(h_front_porch + h_sync_width);
    let v_back_porch = v_blanking.saturating_sub(v_front_porch + v_sync_width);
    let flags = descriptor[17];

    Some(TimingDescriptor {
        pixel_clock_khz: u32::from(pixel_clock_10khz) * 10,
        h_active,
        h_blanking,
        h_front_porch,
        h_sync_width,
        h_back_porch,
        v_active,
        v_blanking,
        v_front_porch,
        v_sync_width,
        v_back_porch,
        h_sync_positive: flags & 0x02 != 0,
        v_sync_positive: flags & 0x04 != 0,
        interlaced: flags & 0x80 != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn minimal_base_edid(extension_blocks: u8) -> Vec<u8> {
        let mut edid = vec![0u8; 128];
        edid[..8].copy_from_slice(&HEADER);
        edid[8] = 0x04;
        edid[9] = 0x6d;
        edid[STANDARD_TIMING_START
            ..STANDARD_TIMING_START + STANDARD_TIMING_SLOTS * STANDARD_TIMING_LEN]
            .fill(0x01);
        edid[126] = extension_blocks;
        repair_block_checksum(&mut edid);
        edid
    }

    #[test]
    fn rejects_short_edid() {
        assert!(matches!(
            parse_edid(vec![0; 127]),
            Err(EdidError::TooShort(127))
        ));
    }

    #[test]
    fn parses_and_deletes_standard_timing() {
        let mut edid = minimal_base_edid(0);
        edid[STANDARD_TIMING_START] = (1920u16 / 8 - 31) as u8;
        edid[STANDARD_TIMING_START + 1] = 0b11_000000;
        repair_block_checksum(&mut edid);

        let parsed = parse_edid(edid.clone()).expect("parse");
        assert_eq!(parsed.standard_timings.len(), 1);
        assert_eq!(parsed.standard_timings[0].width, 1920);
        assert_eq!(parsed.standard_timings[0].height, 1080);
        assert_eq!(parsed.standard_timings[0].refresh_hz, 60);
        assert_eq!(
            parsed.standard_timings[0].aspect,
            StandardTimingAspect::SixteenNine
        );

        let patched = delete_standard_timing(&edid, 0).expect("delete standard timing");
        assert!(block_checksum_valid(&patched[..128]));
        let parsed = parse_edid(patched).expect("parse patched");
        assert!(parsed.standard_timings.is_empty());
    }

    #[test]
    fn inserts_and_replaces_standard_timing() {
        let edid = minimal_base_edid(0);
        let timing = StandardTiming {
            slot: 0,
            width: 1920,
            height: 1080,
            refresh_hz: 75,
            aspect: StandardTimingAspect::SixteenNine,
        };

        let (patched, slot) =
            insert_standard_timing(&edid, &timing).expect("insert standard timing");
        assert_eq!(slot, 0);
        assert!(block_checksum_valid(&patched[..128]));

        let replacement = StandardTiming {
            slot: 0,
            width: 1280,
            height: 1024,
            refresh_hz: 60,
            aspect: StandardTimingAspect::FiveFour,
        };
        let patched =
            patch_standard_timing(&patched, slot, &replacement).expect("patch standard timing");
        assert!(block_checksum_valid(&patched[..128]));

        let parsed = parse_edid(patched).expect("parse patched");
        assert_eq!(parsed.standard_timings.len(), 1);
        assert_eq!(parsed.standard_timings[0].width, 1280);
        assert_eq!(parsed.standard_timings[0].height, 1024);
        assert_eq!(
            parsed.standard_timings[0].aspect,
            StandardTimingAspect::FiveFour
        );
    }

    #[test]
    fn rejects_unencodable_standard_timing() {
        let timing = StandardTiming {
            slot: 0,
            width: 1366,
            height: 768,
            refresh_hz: 60,
            aspect: StandardTimingAspect::SixteenNine,
        };

        assert!(matches!(
            encode_standard_timing(&timing),
            Err(EdidError::InvalidStandardTiming(_))
        ));
    }

    #[test]
    fn reports_available_base_standard_and_cta_slots() {
        let timing = sample_timing();
        let mut edid = minimal_base_edid(1);
        edid = patch_base_detailed_timing(&edid, 0, &timing).expect("base DTD should patch");
        edid = patch_standard_timing(
            &edid,
            0,
            &StandardTiming {
                slot: 0,
                width: 1920,
                height: 1080,
                refresh_hz: 60,
                aspect: StandardTimingAspect::SixteenNine,
            },
        )
        .expect("standard timing should patch");

        let mut cta = vec![0u8; 128];
        cta[0] = CTA_TAG;
        cta[1] = 3;
        cta[2] = CTA_HEADER_LEN as u8;
        repair_block_checksum(&mut cta);
        edid.extend(cta);

        let summary = slot_summary(&edid).expect("slot summary");
        assert_eq!(summary.base_dtd_used, 1);
        assert_eq!(summary.base_dtd_free, 3);
        assert_eq!(summary.standard_used, 1);
        assert_eq!(summary.standard_free, 7);
        assert_eq!(summary.cta_dtd_used, 0);
        assert_eq!(summary.cta_dtd_free, 6);
    }

    #[test]
    fn reports_cta_dtd_slot_rows() {
        let mut edid = minimal_base_edid(1);
        let mut cta = vec![0u8; 128];
        cta[0] = CTA_TAG;
        cta[1] = 3;
        cta[2] = CTA_HEADER_LEN as u8;
        repair_block_checksum(&mut cta);
        edid.extend(cta);

        let slots = cta_dtd_slots(&edid).expect("CTA DTD slots");
        assert_eq!(slots.len(), 6);
        assert_eq!(slots[0].extension_index, 1);
        assert_eq!(slots[0].slot, 0);
        assert!(slots.iter().all(|slot| slot.timing.is_none()));
        assert!(slots.iter().all(|slot| !slot.occupied_unknown));
    }

    #[test]
    fn parses_cta_data_block_summaries() {
        let mut edid = minimal_base_edid(1);
        let mut cta = vec![0u8; 128];
        cta[0] = CTA_TAG;
        cta[1] = 3;
        cta[2] = 10;
        cta[4] = (2 << 5) | 1;
        cta[5] = 16;
        cta[6] = (7 << 5) | 2;
        cta[7] = 6;
        cta[8] = 1;
        repair_block_checksum(&mut cta);
        edid.extend(cta);

        let parsed = parse_edid(edid).expect("parse EDID");
        let blocks = &parsed.cta_blocks[0].data_blocks;
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].label(), "Video");
        assert_eq!(blocks[1].label(), "HDR Static");
        assert_eq!(blocks[0].video_modes.len(), 1);
        let CtaVideoDescriptor::Known(mode) = &blocks[0].video_modes[0] else {
            panic!("VIC 16 should be mapped");
        };
        assert_eq!(mode.vic, 16);
        assert_eq!(mode.width, 1920);
    }

    #[test]
    fn parses_displayid_type_i_detailed_timings() {
        let mut edid = minimal_base_edid(1);
        let mut displayid = vec![0u8; 128];
        displayid[0] = DISPLAYID_TAG;
        displayid[1] = 0x13;
        displayid[2] = 121;
        displayid[3] = 0;
        displayid[4] = 0;
        displayid[5] = 0x03;
        displayid[6] = 0x01;
        displayid[7] = DISPLAYID_TYPE_I_DTD_LEN as u8;
        displayid[8..28].copy_from_slice(&[
            0x19, 0x13, 0x01, 0x84, 0xff, 0x09, 0xaf, 0x00, 0x2f, 0x00, 0x1f, 0x00, 0x9f, 0x05,
            0x77, 0x00, 0x02, 0x00, 0x05, 0x00,
        ]);
        repair_block_checksum(&mut displayid);
        edid.extend(displayid);

        let parsed = parse_edid(edid).expect("parse EDID");
        assert_eq!(parsed.displayid_blocks.len(), 1);
        let block = &parsed.displayid_blocks[0];
        assert_eq!(block.version_major, 1);
        assert_eq!(block.version_minor, 3);
        assert_eq!(block.data_blocks[0].label(), "Type I timings");
        assert_eq!(block.detailed_timings.len(), 1);

        let row = &block.detailed_timings[0];
        assert!(row.preferred);
        assert_eq!(row.timing.h_active, 2560);
        assert_eq!(row.timing.v_active, 1440);
        assert_eq!(row.timing.pixel_clock_khz, 704_260);
        assert!((row.timing.refresh_hz().unwrap() - 165.003).abs() < 0.01);
    }

    #[test]
    fn preserves_unmapped_cta_vics() {
        let mut edid = minimal_base_edid(1);
        let mut cta = vec![0u8; 128];
        cta[0] = CTA_TAG;
        cta[1] = 3;
        cta[2] = 6;
        cta[4] = (2 << 5) | 1;
        cta[5] = 127;
        repair_block_checksum(&mut cta);
        edid.extend(cta);

        let parsed = parse_edid(edid).expect("parse EDID");
        assert_eq!(
            parsed.cta_blocks[0].data_blocks[0].video_modes[0],
            CtaVideoDescriptor::Unknown {
                vic: 127,
                native: false
            }
        );
    }

    #[test]
    fn encoded_detailed_timing_round_trips_through_parser() {
        let timing = sample_timing();

        let descriptor = encode_detailed_timing(&timing).expect("timing should encode");
        let parsed = parse_detailed_timing(&descriptor).expect("timing should parse");

        assert_eq!(parsed, timing);
    }

    #[test]
    fn patched_base_block_gets_repaired_checksum() {
        let edid = minimal_base_edid(0);
        let timing = sample_timing();

        let patched = patch_base_detailed_timing(&edid, 0, &timing).expect("EDID should patch");
        assert!(block_checksum_valid(&patched[..128]));

        let parsed = parse_edid(patched).expect("patched EDID should parse");
        assert_eq!(parsed.detailed_timings.first(), Some(&timing));
        assert!(parsed.checksum_valid);
    }

    #[test]
    fn insert_uses_empty_base_dtd_slot_first() {
        let edid = minimal_base_edid(0);
        let timing = sample_timing();

        let (patched, location) =
            insert_detailed_timing(&edid, &timing).expect("EDID should accept a DTD");

        assert_eq!(location, DtdLocation::Base { slot: 0 });
        assert!(block_checksum_valid(&patched[..128]));
        let parsed = parse_edid(patched).expect("patched EDID should parse");
        assert_eq!(parsed.detailed_timings.first(), Some(&timing));
    }

    #[test]
    fn insert_uses_cta_dtd_slot_when_base_slots_are_full() {
        let timing = sample_timing();
        let mut edid = minimal_base_edid(1);
        for slot in 0..DTD_SLOTS {
            edid = patch_base_detailed_timing(&edid, slot, &timing).expect("base DTD should patch");
        }

        let mut cta = vec![0u8; 128];
        cta[0] = CTA_TAG;
        cta[1] = 3;
        cta[2] = CTA_HEADER_LEN as u8;
        repair_block_checksum(&mut cta);
        edid.extend(cta);

        let (patched, location) =
            insert_detailed_timing(&edid, &timing).expect("CTA DTD slot should be used");

        assert_eq!(
            location,
            DtdLocation::Cta {
                extension_index: 1,
                slot: 0
            }
        );
        assert!(block_checksum_valid(&patched[..128]));
        assert!(block_checksum_valid(&patched[128..256]));

        let parsed = parse_edid(patched).expect("patched EDID should parse");
        assert_eq!(parsed.cta_blocks.len(), 1);
        assert_eq!(parsed.cta_blocks[0].detailed_timings.first(), Some(&timing));
    }

    #[test]
    fn explicit_cta_insert_uses_selected_extension() {
        let timing = sample_timing();
        let mut edid = minimal_base_edid(2);

        let mut first_cta = vec![0u8; 128];
        first_cta[0] = CTA_TAG;
        first_cta[1] = 3;
        first_cta[2] = CTA_HEADER_LEN as u8;
        repair_block_checksum(&mut first_cta);
        edid.extend(first_cta);

        let mut second_cta = vec![0u8; 128];
        second_cta[0] = CTA_TAG;
        second_cta[1] = 3;
        second_cta[2] = CTA_HEADER_LEN as u8;
        repair_block_checksum(&mut second_cta);
        edid.extend(second_cta);

        let (patched, location) =
            insert_cta_detailed_timing(&edid, 2, &timing).expect("CTA DTD insert");

        assert_eq!(
            location,
            DtdLocation::Cta {
                extension_index: 2,
                slot: 0
            }
        );
        assert!(block_checksum_valid(&patched[256..384]));
        let slots = cta_dtd_slots(&patched).expect("CTA DTD slots");
        let inserted = slots
            .iter()
            .find(|slot| slot.extension_index == 2 && slot.slot == 0)
            .expect("inserted slot");
        assert_eq!(inserted.timing.as_ref(), Some(&timing));
    }
}
