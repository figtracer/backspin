//! Gate 2: the shapes, prepare and the renderer.
//!
//! `data/shapes.txt` comes from `scripts/shape_vectors.py`, a model written from the formulas in
//! envelope.md section 2. Every weight must match bit for bit.

use backspin_envelope::{Cursor, Envelope, Point, SLOT_LENGTH, Shape, step_count};

fn vectors() -> impl Iterator<Item = (String, f32, f64, f64)> {
    include_str!("data/shapes.txt").lines().map(|l| {
        let f: Vec<&str> = l.split(' ').collect();
        let t = f32::from_bits(u32::from_str_radix(f[1], 16).unwrap());
        let x = f64::from_bits(u64::from_str_radix(f[2], 16).unwrap());
        let w = f64::from_bits(u64::from_str_radix(f[3], 16).unwrap());
        (f[0].to_string(), t, x, w)
    })
}

#[test]
fn shapes_match_the_formulas() {
    let mut n = 0;
    for (kind, t, x, want) in vectors() {
        let got = if let Some(i) = kind.strip_prefix("smooth") {
            let ys = [0.1f32, 0.8, 0.3, 0.9, 0.9];
            let dxs = [0.0, 0.5, 1.25, 0.75, 1.5];
            let pts = ys
                .iter()
                .zip(dxs)
                .map(|(&y, dx)| Point::new(dx, y, t, Shape::SMOOTH))
                .collect();
            let env = Envelope::with_points(4.0, pts);
            env.points[i.parse::<usize>().unwrap()].eval(x)
        } else {
            let shape: u8 = kind.parse().unwrap();
            let env = Envelope::with_points(
                1.0,
                vec![
                    Point::new(0.0, 0.0, 0.0, Shape::HOLD),
                    Point::new(1.0, 1.0, t, shape),
                ],
            );
            env.points[1].eval(x)
        };
        // Bit for bit with the vectors (made on macOS) where the platform libm is Apple's; other
        // libms round some exp/ln/sin/cos results differently by an ulp.
        let ulps = got.to_bits().abs_diff(want.to_bits());
        let allowed = if cfg!(target_os = "macos") { 0 } else { 4 };
        assert!(
            ulps <= allowed,
            "{kind} t={t} x={x}: {got} != {want} ({ulps} ulps)"
        );
        n += 1;
    }
    assert!(n > 15000);
}

#[test]
fn step_counts_match_the_table() {
    let table = [
        (0.0, 1000),
        (0.1, 437),
        (0.25, 126),
        (0.5, 16),
        (0.625, 6),
        (0.735, 2),
        (0.9, 1),
        (1.0, 0),
    ];
    for (t, n) in table {
        for shape in [Shape::SMOOTH_STAIRS, Shape::PULSE, Shape::WAVE] {
            assert_eq!(step_count(&Point::new(1.0, 1.0, t, shape)), n, "t={t}");
            assert_eq!(step_count(&Point::new(1.0, 1.0, -t, shape)), n, "t={t}");
        }
        assert_eq!(
            step_count(&Point::new(1.0, 1.0, t, Shape::STAIRS)),
            n + 1,
            "t={t}"
        );
    }
    assert_eq!(
        step_count(&Point::new(1.0, 1.0, 0.5, Shape::SINGLE_CURVE)),
        -1
    );
}

/// A slot envelope with one segment of every shape.
fn all_shapes() -> Envelope {
    let mut pts = vec![Point::new(0.0, 0.2, 0.0, Shape::HOLD)];
    for s in 0..13u8 {
        let y = if s % 2 == 0 { 0.9 } else { 0.1 };
        let dx = if s < 12 { 0.25 } else { 1.0 };
        pts.push(Point::new(dx, y, 0.37 - s as f32 * 0.05, s));
    }
    Envelope::with_points(SLOT_LENGTH, pts)
}

#[test]
fn renderer_loops_every_four_beats() {
    let env = all_shapes();
    // 1/64 beat per sample: all positions are exact, so the loop repeats bit for bit.
    let step = 1.0 / 64.0;
    let mut c = Cursor::new(&env);
    let mut out = vec![0f64; 256 * 4];
    c.render_f64(&env, step, &mut out);
    for i in 0..256 * 3 {
        assert_eq!(out[i].to_bits(), out[i + 256].to_bits(), "sample {i}");
    }
    // A real tempo: 48 kHz, 120 BPM. The steps are not exact, so check that the position comes
    // back to the start after each 96000 samples (one loop), within rounding.
    let spb = ((48000i64 * 240) as f32 / 120.0) as f64;
    let mut c = Cursor::new(&env);
    let mut out = vec![0f32; 96000];
    c.render_f32(&env, 4.0 / spb, &mut out[..1]);
    let start = c.position(&env);
    for _ in 0..3 {
        c.render_f32(&env, 4.0 / spb, &mut out);
        let pos = c.position(&env);
        assert!(
            (pos - start).abs() < 1e-9,
            "position {pos} after a loop, started at {start}"
        );
    }
}

#[test]
fn renderer_starts_one_step_in_and_jumps_at_hold_ends() {
    let env = Envelope::with_points(
        4.0,
        vec![
            Point::new(0.0, 1.0, 0.0, Shape::HOLD),
            Point::new(2.0, 0.5, 0.0, Shape::HOLD),
            Point::new(2.0, 1.0, 0.0, Shape::HOLD),
        ],
    );
    let mut c = Cursor::new(&env);
    let mut out = vec![0f64; 16];
    c.render_f64(&env, 0.5, &mut out);
    // Output i is at position (i + 1) / 2 beats; a Hold segment keeps its start value until the
    // end point, and the value at the end point belongs to the next segment.
    let want = [1.0, 1.0, 1.0, 0.5, 0.5, 0.5, 0.5, 1.0];
    for i in 0..16 {
        assert_eq!(out[i], want[i % 8], "sample {i}");
    }
}

#[test]
fn big_step_matches_the_original_skip() {
    // A skipped lane takes one step of n samples; the result differs from n single steps only in
    // the last bits of the phase.
    let env = all_shapes();
    let step = 4.0 / 96000.0;
    let mut a = Cursor::new(&env);
    a.advance(&env, 1000.0 * step);
    let mut b = Cursor::new(&env);
    let mut tmp = vec![0f32; 1000];
    b.render_f32(&env, step, &mut tmp);
    assert_eq!(a.idx, b.idx);
    assert!((a.phase - b.phase).abs() < 1e-9);
}
