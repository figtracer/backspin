//! The nine automatable parameters (state.md section 2).

/// The number of parameters.
pub const NUM_PARAMS: usize = 9;

/// A parameter: its name, raw integer range and default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParamInfo {
    /// The name as the host shows it.
    pub name: &'static str,
    /// The name as the original returns it (FPN_Param), FL hint codes included.
    pub fl_name: &'static str,
    /// Raw minimum.
    pub min: i32,
    /// Raw maximum.
    pub max: i32,
    /// Raw default.
    pub default: i32,
}

/// The parameters in index order.
pub const PARAMS: [ParamInfo; NUM_PARAMS] = [
    ParamInfo {
        name: "Time slot",
        fl_name: "Time slot",
        min: 0,
        max: 35,
        default: 0,
    },
    ParamInfo {
        name: "Volume slot",
        fl_name: "Volume slot",
        min: 0,
        max: 35,
        default: 0,
    },
    ParamInfo {
        name: "Time mix",
        fl_name: "^b^aTime mix",
        min: 0,
        max: 256,
        default: 256,
    },
    ParamInfo {
        name: "Volume mix",
        fl_name: "^b^aVolume mix",
        min: 0,
        max: 256,
        default: 256,
    },
    ParamInfo {
        name: "Volume smoothing attack",
        fl_name: "^b^aVolume smoothing attack",
        min: 0,
        max: 10000,
        default: 1765,
    },
    ParamInfo {
        name: "Volume smoothing release",
        fl_name: "^b^aVolume smoothing release",
        min: 0,
        max: 10000,
        default: 5000,
    },
    ParamInfo {
        name: "Volume envelope tension",
        fl_name: "^b^aVolume envelope tension",
        min: -256,
        max: 256,
        default: 0,
    },
    ParamInfo {
        name: "Hold delay (bar repeat)",
        fl_name: "Hold delay (bar repeat)",
        min: 0,
        max: 1,
        default: 0,
    },
    ParamInfo {
        name: "Time offset (scratching)",
        fl_name: "^b^aTime offset (scratching)",
        min: 0,
        max: 65536,
        default: 65536,
    },
];

/// The smoothing time in ms of a raw attack or release value (0x6a150):
/// `f32(exp(f32(f32(v) * 1e-4f) * ln 501) - 1)`.
pub fn smoothing_ms(v: i32) -> f64 {
    let x = (v as f32 * 1e-4f32) as f64;
    ((x * 6.2166061010848646).exp() - 1.0) as f32 as f64
}

/// The value text of parameter `index` at raw `value` (FPN_ParamValue, 0x692e0). Slot names come
/// from `slot_name`.
pub fn param_text(index: usize, value: i32, slot_name: impl Fn(usize, usize) -> String) -> String {
    match index {
        0 | 1 => slot_name(index, value.clamp(0, 35) as usize),
        2 | 3 | 6 => format!("{}%", round_half_away(value as f64 * 0.00390625 * 100.0)),
        4 => format!("{:.2}ms", smoothing_ms(value)),
        5 => format!("{:.2}ms", smoothing_ms(value) * 2.0),
        8 => format!(
            "-{}",
            time_text(
                -((value as f32 * (1.0 / 65536.0) - 1.0) as f64) * 8.0,
                false
            )
        ),
        _ => String::new(),
    }
}

/// The editor's time text (FUN_000a96d0) in 4/4 with 96 ticks per beat: `BBB:BB:TTT` as
/// bars, beats and ticks, 1-based when `one_based`, with a leading `-` for negative values.
pub fn time_text(x: f64, one_based: bool) -> String {
    let (beats_per_bar, ticks_per_beat) = (4.0, 96.0);
    let bars = (x.abs() / beats_per_bar) as i64;
    let rem = x - bars as f64 * beats_per_bar;
    let beats = rem.abs() as i64;
    let ticks = ((rem - beats as f64) * ticks_per_beat).round_ties_even() as i64;
    let k = one_based as i64;
    let text = format!("{:03}:{:02}:{:03}", bars + k, beats + k, ticks);
    if x < 0.0 { format!("-{text}") } else { text }
}

fn round_half_away(x: f64) -> i64 {
    x.round() as i64
}
