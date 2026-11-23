use super::*;

fn request(width: u16, height: u16, refresh_hz: f64, interlaced: bool) -> CvtRequest {
    CvtRequest {
        width,
        height,
        refresh_hz,
        interlaced,
    }
}

/// CEA 1080p60 with H+ V-, the "current" timing used for the CRU references.
fn current_1080p() -> TimingDescriptor {
    TimingDescriptor {
        pixel_clock_khz: 148_500,
        h_active: 1920,
        h_blanking: 280,
        h_front_porch: 88,
        h_sync_width: 44,
        h_back_porch: 148,
        v_active: 1080,
        v_blanking: 45,
        v_front_porch: 4,
        v_sync_width: 5,
        v_back_porch: 36,
        h_sync_positive: true,
        v_sync_positive: false,
        interlaced: false,
    }
}

/// Outputs of CRU's own `DetailedResolutionClass` (compiled from the CRU
/// source) for each preset, with `current_1080p` as the existing timing:
/// (preset index, interlaced, width, height, refresh mHz, [h front, sync,
/// back], [v front, sync, back], pixel clock in 10 kHz, H+, V+).
#[allow(clippy::type_complexity)]
const CRU_REFERENCE: &[(
    usize,
    bool,
    u16,
    u16,
    u32,
    [u16; 3],
    [u16; 3],
    u32,
    bool,
    bool,
)] = &[
    (
        1,
        false,
        1920,
        1080,
        60000,
        [88, 44, 148],
        [4, 5, 36],
        14850,
        true,
        true,
    ),
    (
        1,
        false,
        1920,
        1080,
        144000,
        [48, 32, 80],
        [3, 5, 69],
        34655,
        true,
        false,
    ),
    (
        1,
        false,
        1280,
        720,
        25000,
        [48, 32, 80],
        [3, 5, 6],
        2643,
        true,
        false,
    ),
    (
        1,
        true,
        1920,
        1080,
        50000,
        [528, 44, 148],
        [4, 10, 31],
        7425,
        true,
        true,
    ),
    (
        1,
        false,
        2560,
        1440,
        165000,
        [48, 32, 80],
        [3, 5, 111],
        69968,
        true,
        false,
    ),
    (
        2,
        false,
        1920,
        1080,
        50000,
        [528, 44, 148],
        [4, 5, 36],
        14850,
        true,
        true,
    ),
    (
        2,
        false,
        3840,
        2160,
        60000,
        [176, 88, 296],
        [8, 10, 72],
        59400,
        true,
        true,
    ),
    (
        2,
        false,
        3840,
        2160,
        120000,
        [176, 88, 296],
        [8, 10, 72],
        118800,
        true,
        true,
    ),
    (
        2,
        false,
        7680,
        4320,
        60000,
        [552, 176, 592],
        [16, 20, 44],
        237600,
        true,
        true,
    ),
    (
        2,
        true,
        1440,
        480,
        59940,
        [38, 124, 114],
        [8, 6, 31],
        2700,
        false,
        false,
    ),
    (
        2,
        false,
        1366,
        768,
        60000,
        [70, 143, 213],
        [3, 3, 24],
        8581,
        true,
        true,
    ),
    (
        3,
        false,
        1024,
        768,
        85000,
        [48, 96, 208],
        [1, 3, 36],
        9451,
        true,
        true,
    ),
    (
        3,
        false,
        1280,
        1024,
        60000,
        [48, 112, 248],
        [1, 3, 38],
        10797,
        true,
        true,
    ),
    (
        3,
        false,
        1600,
        1200,
        100000,
        [128, 176, 304],
        [3, 4, 67],
        28130,
        false,
        true,
    ),
    (
        3,
        true,
        1024,
        768,
        60000,
        [32, 96, 128],
        [6, 8, 21],
        3084,
        false,
        true,
    ),
    (
        4,
        false,
        1920,
        1080,
        75000,
        [88, 44, 148],
        [4, 5, 36],
        18563,
        true,
        true,
    ),
    (
        4,
        false,
        1366,
        768,
        60000,
        [70, 143, 213],
        [3, 3, 24],
        8581,
        true,
        true,
    ),
    (
        4,
        false,
        2560,
        1440,
        144000,
        [48, 32, 80],
        [3, 5, 33],
        58008,
        true,
        false,
    ),
    (
        5,
        false,
        1920,
        1080,
        100000,
        [88, 44, 148],
        [4, 5, 36],
        24750,
        true,
        true,
    ),
    (
        5,
        false,
        3840,
        2160,
        120000,
        [176, 88, 296],
        [8, 10, 72],
        118800,
        true,
        true,
    ),
    (
        6,
        false,
        2560,
        1440,
        165000,
        [48, 32, 80],
        [3, 5, 52],
        67320,
        true,
        false,
    ),
    (
        6,
        true,
        1920,
        1080,
        50000,
        [48, 32, 80],
        [6, 10, 29],
        5850,
        true,
        false,
    ),
    (
        6,
        false,
        1920,
        1080,
        23976,
        [48, 32, 80],
        [3, 5, 37],
        5611,
        true,
        false,
    ),
    (
        7,
        false,
        2560,
        1440,
        165000,
        [48, 32, 80],
        [3, 5, 2],
        65076,
        true,
        false,
    ),
    (
        7,
        false,
        1920,
        1080,
        144000,
        [24, 32, 24],
        [3, 5, 12],
        31680,
        true,
        false,
    ),
    (
        7,
        false,
        1280,
        1024,
        75000,
        [48, 32, 80],
        [3, 7, 16],
        11340,
        true,
        false,
    ),
    (
        8,
        false,
        1920,
        1080,
        85000,
        [144, 208, 368],
        [3, 5, 162],
        28050,
        false,
        true,
    ),
    (
        8,
        true,
        1920,
        1080,
        60000,
        [64, 192, 384],
        [6, 10, 29],
        8640,
        false,
        true,
    ),
    (
        9,
        false,
        1920,
        1080,
        60000,
        [128, 200, 328],
        [3, 5, 32],
        17311,
        false,
        true,
    ),
    (
        9,
        false,
        1024,
        768,
        85000,
        [72, 104, 176],
        [3, 4, 34],
        9463,
        false,
        true,
    ),
    (
        9,
        true,
        1920,
        1080,
        60000,
        [64, 192, 256],
        [6, 10, 29],
        8208,
        false,
        true,
    ),
    (
        10,
        false,
        1920,
        1080,
        60000,
        [48, 32, 80],
        [3, 5, 23],
        13866,
        true,
        false,
    ),
    (
        10,
        false,
        3840,
        2160,
        60000,
        [48, 32, 80],
        [3, 5, 54],
        53328,
        true,
        false,
    ),
    (
        10,
        false,
        2560,
        1440,
        240000,
        [48, 32, 80],
        [3, 5, 171],
        105689,
        true,
        false,
    ),
    (
        11,
        false,
        1920,
        1080,
        60000,
        [8, 32, 40],
        [17, 8, 6],
        13332,
        true,
        false,
    ),
    (
        11,
        false,
        3840,
        2160,
        144000,
        [8, 32, 40],
        [140, 8, 6],
        130621,
        true,
        false,
    ),
    (
        11,
        true,
        1920,
        1080,
        60000,
        [8, 32, 40],
        [4, 16, 13],
        6678,
        true,
        false,
    ),
    (
        12,
        false,
        1920,
        1080,
        60000,
        [120, 208, 328],
        [1, 3, 34],
        17280,
        false,
        true,
    ),
    (
        12,
        false,
        1280,
        1024,
        75000,
        [88, 136, 224],
        [1, 3, 41],
        13855,
        false,
        true,
    ),
    (
        13,
        false,
        1920,
        1080,
        48000,
        [88, 44, 148],
        [4, 5, 317],
        14848,
        true,
        false,
    ),
    (
        13,
        false,
        1920,
        1080,
        30000,
        [88, 44, 148],
        [4, 5, 1161],
        14850,
        true,
        false,
    ),
];

#[test]
fn presets_match_cru_reference_outputs() {
    for &(index, interlaced, width, height, millihz, h, v, pclock, h_pos, v_pos) in CRU_REFERENCE {
        let preset = TimingPreset::ALL[index];
        let timing = timing_for_preset(
            preset,
            request(width, height, f64::from(millihz) / 1000.0, interlaced),
            Some(&current_1080p()),
        )
        .unwrap_or_else(|error| panic!("{preset:?} {width}x{height}@{millihz}: {error}"));
        let context = format!(
            "{preset:?} {width}x{height}{} @{millihz}",
            if interlaced { "i" } else { "" }
        );

        assert_eq!(
            (timing.h_active, timing.v_active),
            (width, height),
            "{context}"
        );
        assert_eq!(
            [
                timing.h_front_porch,
                timing.h_sync_width,
                timing.h_back_porch
            ],
            h,
            "{context}"
        );
        assert_eq!(
            [
                timing.v_front_porch,
                timing.v_sync_width,
                timing.v_back_porch
            ],
            v,
            "{context}"
        );
        assert_eq!(timing.h_blanking, h.iter().sum::<u16>(), "{context}");
        assert_eq!(timing.v_blanking, v.iter().sum::<u16>(), "{context}");
        assert_eq!(timing.pixel_clock_khz, pclock * 10, "{context}");
        assert_eq!(
            (timing.h_sync_positive, timing.v_sync_positive),
            (h_pos, v_pos),
            "{context}"
        );
        assert_eq!(timing.interlaced, interlaced, "{context}");
    }
}

#[test]
fn preset_list_matches_cru_order() {
    let labels: Vec<_> = TimingPreset::ALL
        .iter()
        .map(|preset| preset.label())
        .collect();
    assert_eq!(
        labels,
        [
            "Manual",
            "Automatic PC",
            "Automatic HDTV",
            "Automatic CRT",
            "Native PC",
            "Native HDTV",
            "Exact",
            "Exact reduced",
            "Exact CRT",
            "CVT standard",
            "CVT-RB standard",
            "CVT-RB2 standard",
            "GTF standard",
            "Vertical total calculator (QFT)",
        ]
    );
    assert_eq!(TimingPreset::Manual.cycle(-1), TimingPreset::VerticalTotal);
}

#[test]
fn automatic_tables_reproduce_their_dtd_valid_rows() {
    for (preset, table) in [
        (TimingPreset::AutomaticPc, AUTOMATIC_PC),
        (TimingPreset::AutomaticHdtv, AUTOMATIC_HDTV),
        (TimingPreset::AutomaticCrt, AUTOMATIC_CRT),
    ] {
        for row in table {
            let millihz = (row.min_v_rate + row.max_v_rate) / 2;
            let timing = timing_for_preset(
                preset,
                request(
                    row.h_active as u16,
                    row.v_active as u16,
                    millihz as f64 / 1000.0,
                    row.interlaced,
                ),
                None,
            )
            .unwrap();
            let porches = [
                timing.h_front_porch,
                timing.h_sync_width,
                timing.h_back_porch,
                timing.v_front_porch,
                timing.v_sync_width,
                timing.v_back_porch,
            ]
            .map(i64::from);
            let row_porches = [row.h[0], row.h[1], row.h[2], row.v[0], row.v[1], row.v[2]];
            let fits_dtd = row.h.iter().all(|&value| value <= 1023)
                && row.h.iter().sum::<i64>() <= 4095
                && row.v[0] / i64::from(1 + u8::from(row.interlaced)) <= 63;
            if fits_dtd {
                assert_eq!(
                    porches, row_porches,
                    "{preset:?} {}x{}",
                    row.h_active, row.v_active
                );
                assert!((timing.refresh_hz().unwrap() * 1000.0 - millihz as f64).abs() < 50.0);
            } else if row.h_active <= 4095 && table_clock_10khz(row, millihz) <= 65535 {
                // CRU falls back to CVT-RB when the table porches cannot be
                // encoded but the active area and clock can. Otherwise it
                // keeps the table timing for validation to report.
                assert_ne!(
                    porches, row_porches,
                    "{preset:?} {}x{}",
                    row.h_active, row.v_active
                );
            }
        }
    }
}

fn table_clock_10khz(row: &TableTiming, millihz: i64) -> i64 {
    let h_total = row.h_active + row.h.iter().sum::<i64>();
    let v_total = row.v_active + row.v.iter().sum::<i64>();
    let divisor = 10_000_000 * (1 + i64::from(row.interlaced));
    (millihz * h_total * v_total + divisor - 1) / divisor
}

#[test]
fn interlaced_presets_produce_encodable_half_lines() {
    for preset in TimingPreset::ALL.into_iter().skip(1) {
        let timing = timing_for_preset(
            preset,
            request(1920, 1080, 60.0, true),
            Some(&current_1080p()),
        )
        .unwrap();
        assert!(timing.interlaced);
        assert!(
            timing.edid_vertical_fields().is_ok(),
            "{preset:?}: {timing:?}"
        );
        assert!(
            (timing.refresh_hz().unwrap() - 60.0).abs() < 0.1,
            "{preset:?}"
        );
    }
    assert!(
        timing_for_preset(
            TimingPreset::CvtRbStandard,
            request(1920, 1081, 60.0, true),
            None,
        )
        .is_err()
    );
}

#[test]
fn vertical_total_keeps_pixel_clock_and_needs_a_timing() {
    let timing = timing_for_preset(
        TimingPreset::VerticalTotal,
        request(1920, 1080, 30.0, false),
        Some(&current_1080p()),
    )
    .unwrap();
    assert_eq!(timing.pixel_clock_khz, 148_500);
    assert_eq!(timing.v_total(), 2250);
    assert!(
        timing_for_preset(
            TimingPreset::VerticalTotal,
            request(1920, 1080, 30.0, false),
            None
        )
        .is_err()
    );
}

#[test]
fn extreme_requests_fail_or_validate_without_overflow() {
    for preset in TimingPreset::ALL.into_iter().skip(1) {
        for (width, height) in [(1, 1), (u16::MAX, u16::MAX), (u16::MAX, 1), (1, u16::MAX)] {
            for interlaced in [false, true] {
                for refresh_hz in [0.0, 0.001, 60.0, 2173.9, 1e9, 1e100, -1.0, f64::NAN] {
                    if let Ok(timing) = timing_for_preset(
                        preset,
                        request(width, height, refresh_hz, interlaced),
                        Some(&current_1080p()),
                    ) && width == u16::MAX
                    {
                        assert!(!crate::validation::validate_timing(&timing).is_empty());
                    }
                }
            }
        }
    }
}
