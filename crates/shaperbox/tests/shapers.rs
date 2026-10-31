//! Spot checks of the Time, Crush and Drive band DSP against the reference models in
//! `.amp/in/sb` (the end-to-end comparisons against the plugin need the offline harness).

use backspin_shaperbox::ftz::FlushToZero;
use backspin_shaperbox::shapers::{
    Wave,
    crush::{CrushBand, CrushParams},
    drive::{Curve, internal_style},
    filter::FilterBand,
    time::{TimeBand, range_seconds},
};
use backspin_shaperbox::smoother::Smoother;

/// Drive curves at w = 0.5 against `.amp/in/sb/scripts/drive_curves.py` (internal styles).
#[test]
fn drive_curves_match_the_reference() {
    let cases: [(i32, [f32; 3]); 10] = [
        (0, [-0.214_092_14, 0.202_576_05, 0.296_360_1]),
        (1, [-0.228_045_1, 0.225_931_82, 0.228_124_5]),
        (3, [-0.343_463_14, 0.180_423_1, 0.392_944_62]),
        (5, [0.176_382_4, 0.169_169_83, 0.319_225_24]),
        (6, [0.159_270_82, 0.151_775_72, 0.299_803_9]),
        (7, [-0.161_516_73, 0.221_484_12, 0.228_790_5]),
        (8, [-0.056_248_07, 0.236_000_7, 0.000_192_289]),
        (9, [-0.173_349_95, 0.235_814_86, 0.142_117_5]),
        (10, [-0.069_291_41, 0.218_230_47, 0.165_132_04]),
        (11, [-0.199_523_52, 0.209_912_1, 0.209_041_1]),
    ];
    for (style, want) in cases {
        let c = Curve::new(style, 0.5, 0.5, 0.0, 0.0);
        for (x, y) in [-0.5, 0.1, 0.7].into_iter().zip(want) {
            let got = c.apply(x, &mut 0.0);
            assert!((got - y).abs() < 2e-6, "style {style} x {x}: {got} vs {y}");
        }
    }
    let grip = Curve::new(3, 0.5, 0.5, 0.5, 0.0).apply(0.03, &mut 0.0);
    assert!((grip - 0.042_113_315).abs() < 2e-6, "grip {grip}");
    let accent = Curve::new(3, 0.5, 1.0, 0.0, 0.0).apply(0.7, &mut 0.0);
    assert!((accent - 0.455_953_54).abs() < 2e-6, "accent {accent}");
    let push = Curve::new(8, 0.6, 0.5, 0.0, 0.3).apply(0.2, &mut 0.0);
    assert!((push - 0.032_281_364).abs() < 2e-6, "push {push}");
    // Presets skip Smooth Clip; preset 11 is the identity.
    assert_eq!(internal_style(0), 0);
    assert_eq!(internal_style(1), 2);
    let id = Curve::new(internal_style(11), 0.5, 0.5, 0.0, 0.0);
    assert_eq!(id.apply(0.3, &mut 0.0), 0.3);
}

fn sine(n: usize, hz: f32) -> Vec<[f32; 2]> {
    (0..n)
        .map(|i| {
            let v = 0.5 * (std::f32::consts::TAU * hz * i as f32 / 48000.0).sin();
            [v, v]
        })
        .collect()
}

/// At the top of both waves Crush is the DC blocker alone (0.5 * 10/1000 rad of phase at 1 kHz);
/// at 8 levels the output moves in steps of 1/8 scaled by the post gain.
#[test]
fn crush_bypass_and_plateaus() {
    let p = CrushParams {
        dc_offset: 0.0,
        jitter: 0.0,
        treble: 1.0,
        damp: false,
        dither: false,
    };
    let x = sine(4800, 1000.0);
    let mut buf = x.clone();
    let mut band = CrushBand::new(48000.0, p);
    for b in buf.chunks_mut(512) {
        band.process(b, Wave::Constant(1.0), Wave::Constant(1.0), p);
    }
    let err = x[2400..]
        .iter()
        .zip(&buf[2400..])
        .map(|(a, b)| (a[0] - b[0]).abs())
        .fold(0.0, f32::max);
    assert!(err < 1e-2, "bypass error {err}");

    // w = 0.5: 8 levels and gain 0.96875, so a slow ramp moves in jumps of 0.96875 / 8
    // (the quantiser spreads a jump over the crossing sample and the next).
    let ramp: Vec<[f32; 2]> = (0..512).map(|i| [i as f32 / 512.0; 2]).collect();
    let mut buf = ramp.clone();
    let mut band = CrushBand::new(48000.0, p);
    band.process(&mut buf, Wave::Constant(0.5), Wave::Constant(1.0), p);
    let rise: f32 = buf.windows(2).map(|w| (w[1][0] - w[0][0]).max(0.0)).sum();
    assert!((rise - 8.0 * 0.968_75 / 8.0).abs() < 0.02, "rise {rise}");
}

/// A moving wave near the top reads the newest samples (taps past the newest sample read it), so
/// the output follows the input instead of dropping at block ends.
#[test]
fn time_moving_wave_small_delay() {
    let x = sine(2048, 500.0);
    let mut buf = x.clone();
    let mut tb = TimeBand::new(48000.0);
    let r = range_seconds(5, 120.0, 1.0);
    let w = vec![0.999; 512];
    for b in buf.chunks_mut(512) {
        tb.process(b, Wave::PerSample(&w), r, None);
    }
    // 0.005 s * 0.001 = 0.24 samples of delay.
    let d = 48000.0 * 0.005 * 0.001;
    for i in [1000usize, 1023, 1535, 2047] {
        let t = i as f32 - d;
        let want = 0.5 * (std::f32::consts::TAU * 500.0 * t / 48000.0).sin();
        assert!(
            (buf[i][0] - want).abs() < 5e-3,
            "sample {i}: {} vs {want}",
            buf[i][0]
        );
    }
}

/// Filter samples 10, 100 and 255 (L, R bits) against `FilterBand.process` of
/// `.amp/in/sb/data/sb_filter_exact.py` at cutoff 0.5, resonance 0.75: (type, drive, clipping
/// curve, stereo offset, samples).
#[test]
fn filter_matches_the_reference_bits() {
    let cases: [(i32, f32, i32, f32, [u32; 6]); 10] = [
        (
            0,
            0.182575,
            0,
            0.5,
            [
                0xbd38d184, 0x3d157d75, 0x3d365040, 0xbd21972e, 0xbc96e3ef, 0x3c9f4c7d,
            ],
        ),
        (
            2,
            0.182575,
            0,
            0.5,
            [
                0xbd6bb827, 0xbcd3dae6, 0xbd0bb0d9, 0xbdc8723a, 0x3d2e7769, 0x3d49cdc5,
            ],
        ),
        (
            7,
            1.0,
            1,
            0.3,
            [
                0x3e9e0996, 0x3f4ece49, 0x3e59da5b, 0x3d5c5662, 0xbdf759e8, 0x3f20c1a0,
            ],
        ),
        (
            13,
            0.182575,
            0,
            0.5,
            [
                0xbcfcf850, 0xbbde9721, 0x3da7ebd6, 0xbdbc1608, 0xbcb542c6, 0x3bc2eeed,
            ],
        ),
        (
            14,
            0.182575,
            0,
            0.3,
            [
                0x3e95ca6e, 0x3f3e6a5e, 0x3e6d38ba, 0xbdab25c4, 0xbe07401e, 0x3f3a3160,
            ],
        ),
        (
            15,
            0.182575,
            1,
            0.5,
            [
                0x3e88b7b0, 0x3f420522, 0x3e4f5d62, 0x3c8bd8ce, 0xbdb5db68, 0x3f296c6b,
            ],
        ),
        (
            17,
            0.5,
            0,
            0.5,
            [
                0x3e697bf2, 0x3f30e4f8, 0x3e4731b0, 0xbda80a11, 0xbd826d9b, 0x3f2c998c,
            ],
        ),
        (
            19,
            0.182575,
            0,
            0.5,
            [
                0x3e6136e6, 0x3f4e3219, 0x3ed5e552, 0xbdcc396f, 0xbe29a72e, 0x3f2528b1,
            ],
        ),
        (
            5,
            0.182575,
            0,
            0.7,
            [
                0x3e9e7b88, 0x3f4d763a, 0x3e03fd4e, 0x3df1081d, 0x3cbf4578, 0x3f163116,
            ],
        ),
        (
            8,
            0.182575,
            0,
            0.5,
            [
                0x3eb32894, 0x3f4d07f4, 0x3de01952, 0x3e3e0712, 0xbdc4ef04, 0x3f1db94c,
            ],
        ),
    ];
    let _ftz = FlushToZero::new();
    let x: Vec<[f32; 2]> = (0..256)
        .map(|i| {
            [
                ((i * 37) % 101 - 50) as f32 / 60.0,
                ((i * 53) % 89 - 44) as f32 / 50.0,
            ]
        })
        .collect();
    for (typ, drive, clip, so, want) in cases {
        let mut band = FilterBand::new(48000.0, typ, drive, true, clip);
        let so = Smoother::new(so, 48000.0, 0.0);
        let mut buf = x.clone();
        for b in buf.chunks_mut(100) {
            band.process(b, Wave::Constant(0.5), Wave::Constant(0.75), &so);
        }
        let got: Vec<u32> = [10, 100, 255]
            .iter()
            .flat_map(|&i| buf[i].map(f32::to_bits))
            .collect();
        // Bit for bit where the platform libm is Apple's (the plugin's); other libms round some
        // tanf/expf results differently by an ulp, which the filter state carries a little.
        if cfg!(target_os = "macos") {
            assert_eq!(got, want, "type {typ}");
        } else {
            let ulps = |a: u32, b: u32| (a as i32).abs_diff(b as i32);
            let worst = got.iter().zip(&want).map(|(&a, &b)| ulps(a, b)).max();
            assert!(worst <= Some(64), "type {typ}: {got:?} != {want:?}");
        }
    }
}
