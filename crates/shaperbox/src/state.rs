//! The ShaperBox state as typed values (core.md section 6), read from the JUCE ValueTree.
//! Members missing in the tree keep their defaults, as the plugin's versioned loader does.

use crate::shapers::filter::migrate_resonance_y;
use crate::valuetree::{Tree, Var};
use crate::wave::Point;

/// Shaper ids in state order (`ShaperId`).
pub const SHAPER_IDS: [&str; 16] = [
    "time",
    "crush",
    "filter",
    "pan",
    "reverb",
    "volume",
    "delay",
    "width",
    "noise",
    "drive",
    "dynamics",
    "liquid",
    "oscilloscope",
    "shift",
    "limiter",
    "pitch",
];

/// Visible shapers outside `moduleSlots` run in this order after the slots (`DAT_013096d8`).
pub const FALLBACK_ORDER: [usize; 13] = [0, 15, 9, 8, 2, 11, 1, 5, 3, 4, 7, 10, 12];

/// The last root `version` that the "69" -> "70" migration (`FUN_0068fd68`) applies to.
const RESONANCE_MIGRATION_VERSION: i32 = 69;

/// One LFO (`LfoState`).
#[derive(Clone, Debug, PartialEq)]
pub struct Lfo {
    pub points: Vec<Point>,
    pub timing_mode: i32,
    pub trigger_mode: i32,
    pub length: i32,
    pub speed: f32,
    pub smoothing: f32,
    pub one_shot_end_phase: f32,
}

impl Default for Lfo {
    fn default() -> Self {
        Self {
            points: Vec::new(),
            timing_mode: 0,
            trigger_mode: 0,
            length: 15,
            speed: 0.5,
            smoothing: 0.4,
            one_shot_end_phase: 0.984_375,
        }
    }
}

/// A section of a band: its id and LFO.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Section {
    pub id: String,
    pub lfo: Lfo,
}

/// One band: `mix`, its sections and its other leaves by id.
#[derive(Clone, Debug, PartialEq)]
pub struct Band {
    pub mix: f32,
    pub sections: Vec<Section>,
    pub values: Vec<(String, Var)>,
}

impl Default for Band {
    fn default() -> Self {
        Self {
            mix: 1.0,
            sections: Vec::new(),
            values: Vec::new(),
        }
    }
}

impl Band {
    pub fn float(&self, id: &str, default: f32) -> f32 {
        match self.values.iter().find(|(k, _)| k == id).map(|(_, v)| v) {
            Some(Var::Double(d)) => *d as f32,
            Some(Var::Int(i)) => *i as f32,
            _ => default,
        }
    }

    pub fn int(&self, id: &str, default: i32) -> i32 {
        match self.values.iter().find(|(k, _)| k == id).map(|(_, v)| v) {
            Some(Var::Int(i)) => *i,
            Some(Var::Double(d)) => *d as i32,
            _ => default,
        }
    }

    pub fn boolean(&self, id: &str, default: bool) -> bool {
        match self.values.iter().find(|(k, _)| k == id).map(|(_, v)| v) {
            Some(Var::Bool(b)) => *b,
            _ => default,
        }
    }

    pub fn string(&self, id: &str) -> Option<&str> {
        match self.values.iter().find(|(k, _)| k == id).map(|(_, v)| v) {
            Some(Var::String(s)) => Some(s),
            _ => None,
        }
    }

    pub fn section(&self, id: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.id == id)
    }
}

/// The band split of a shaper (`BandSplitState`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Split {
    pub low: f32,
    pub low_slope: i32,
    pub high: f32,
    pub high_slope: i32,
    pub selected: i32,
    pub solo: bool,
}

impl Default for Split {
    fn default() -> Self {
        Self {
            low: 0.0,
            low_slope: 0,
            high: 1.0,
            high_slope: 0,
            selected: 1,
            solo: false,
        }
    }
}

/// One shaper.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shaper {
    pub visible: bool,
    pub on: bool,
    pub split: Split,
    pub bands: [Band; 3],
}

/// The whole state.
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub name: String,
    pub shapers: [Shaper; 16],
    pub module_slots: Vec<usize>,
    pub master_mix: f32,
    pub bypassed: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            name: String::new(),
            shapers: Default::default(),
            module_slots: (0..16).collect(),
            master_mix: 1.0,
            bypassed: false,
        }
    }
}

fn id(t: &Tree) -> Option<&str> {
    match t.prop("id") {
        Some(Var::String(s)) => Some(s),
        _ => None,
    }
}

/// The value of a `<type id=.. value=..>` leaf child.
fn leaf<'a>(t: &'a Tree, name: &str) -> Option<&'a Var> {
    t.children
        .iter()
        .find(|c| id(c) == Some(name))
        .and_then(|c| c.prop("value"))
}

fn f(t: &Tree, name: &str, d: f32) -> f32 {
    match leaf(t, name) {
        Some(Var::Double(v)) => *v as f32,
        Some(Var::Int(v)) => *v as f32,
        _ => d,
    }
}

fn i(t: &Tree, name: &str, d: i32) -> i32 {
    match leaf(t, name) {
        Some(Var::Int(v)) => *v,
        Some(Var::Double(v)) => *v as i32,
        _ => d,
    }
}

fn b(t: &Tree, name: &str, d: bool) -> bool {
    match leaf(t, name) {
        Some(Var::Bool(v)) => *v,
        _ => d,
    }
}

fn lfo(t: &Tree) -> Lfo {
    let d = Lfo::default();
    let mut points = Vec::new();
    if let Some(wave) = t.by_id("wave")
        && let Some(pts) = wave.by_id("points")
    {
        for p in &pts.children {
            points.push(Point {
                x: i(p, "x", 0),
                y: f(p, "y", 0.0),
                factor: i(p, "factor", 3),
            });
        }
    }
    Lfo {
        points,
        timing_mode: i(t, "timingMode", d.timing_mode),
        trigger_mode: i(t, "triggerMode", d.trigger_mode),
        length: i(t, "length", d.length),
        speed: f(t, "speed", d.speed),
        smoothing: f(t, "smoothing", d.smoothing),
        one_shot_end_phase: f(t, "oneShotEndPhase", d.one_shot_end_phase),
    }
}

fn band(t: &Tree) -> Band {
    let mut out = Band {
        mix: f(t, "mix", 1.0),
        ..Default::default()
    };
    for c in &t.children {
        if c.kind.starts_with("Section") {
            out.sections.push(Section {
                id: id(c).unwrap_or_default().to_string(),
                lfo: c.by_id("lfo").map(lfo).unwrap_or_default(),
            });
        } else if let (Some(k), Some(v)) = (id(c), c.prop("value"))
            && k != "mix"
        {
            out.values.push((k.to_string(), v.clone()));
        }
    }
    out
}

impl State {
    /// Reads a `PluginState` tree.
    pub fn from_tree(t: &Tree) -> Self {
        let mut s = State::default();
        if let Some(Var::String(n)) = t.prop("ProgramName") {
            s.name = n.clone();
        }
        for c in &t.children {
            let Some(cid) = id(c) else { continue };
            if let Some(k) = SHAPER_IDS.iter().position(|&n| n == cid) {
                let sh = &mut s.shapers[k];
                sh.visible = b(c, "VISIBLE", false);
                sh.on = b(c, "on", false);
                if let Some(sp) = c.by_id("split") {
                    let d = Split::default();
                    sh.split = Split {
                        low: f(sp, "lowFrequencyParameter", d.low),
                        low_slope: i(sp, "lowSlope", d.low_slope),
                        high: f(sp, "highFrequencyParameter", d.high),
                        high_slope: i(sp, "highSlope", d.high_slope),
                        selected: i(sp, "selectedBand", d.selected),
                        solo: b(sp, "soloSelectedBand", d.solo),
                    };
                }
                if let Some(bands) = c.by_id("bands") {
                    for (n, bt) in bands.children.iter().take(3).enumerate() {
                        sh.bands[n] = band(bt);
                    }
                }
                continue;
            }
            match cid {
                "moduleSlots" => {
                    s.module_slots = c
                        .children
                        .iter()
                        .filter_map(|e| match e.prop("value") {
                            Some(Var::Int(v)) if (0..16).contains(v) => Some(*v as usize),
                            _ => None,
                        })
                        .collect();
                }
                "masterMix" => s.master_mix = f(t, "masterMix", 1.0),
                "bypassed" => s.bypassed = b(t, "bypassed", false),
                _ => {}
            }
        }
        let version = match t.prop("version") {
            Some(Var::String(v)) => v.trim().parse().ok(),
            Some(Var::Int(v)) => Some(*v),
            _ => None,
        };
        if version.is_some_and(|v| v <= RESONANCE_MIGRATION_VERSION) {
            s.migrate_resonance();
        }
        s
    }

    /// The "69" -> "70" migration: the resonance wave points of the Warm Filter types move to
    /// the new resonance curve (shapers_a.md 6.7).
    fn migrate_resonance(&mut self) {
        for band in &mut self.shapers[2].bands {
            let typ = band.int("typeWithoutBypass", 2);
            if let Some(sec) = band.sections.iter_mut().find(|s| s.id == "resonance") {
                for p in &mut sec.lfo.points {
                    p.y = migrate_resonance_y(p.y, typ);
                }
            }
        }
    }

    /// The processing order (`FUN_0060c56c`): visible shapers in `moduleSlots`, then other
    /// visible shapers in the fallback order. Only shapers that are also `on` process.
    pub fn order(&self) -> Vec<usize> {
        let mut out: Vec<usize> = Vec::new();
        for &k in &self.module_slots {
            if self.shapers[k].visible && !out.contains(&k) {
                out.push(k);
            }
        }
        for &k in &FALLBACK_ORDER {
            if self.shapers[k].visible && !out.contains(&k) {
                out.push(k);
            }
        }
        out.retain(|&k| self.shapers[k].on);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(kind: &str, id: &str, value: Var) -> Tree {
        Tree {
            kind: kind.into(),
            props: vec![
                ("id".into(), Var::String(id.into())),
                ("value".into(), value),
            ],
            children: Vec::new(),
        }
    }

    fn node(kind: &str, id: Option<&str>, children: Vec<Tree>) -> Tree {
        Tree {
            kind: kind.into(),
            props: id
                .map(|i| vec![("id".into(), Var::String(i.into()))])
                .unwrap_or_default(),
            children,
        }
    }

    /// A Filter state whose band `b` has type `types[b]` and a resonance wave of `ys`.
    fn filter_tree(version: &str, types: [i32; 3], ys: &[f64]) -> Tree {
        let points: Vec<Tree> = ys
            .iter()
            .enumerate()
            .map(|(k, &y)| {
                node(
                    "Point",
                    None,
                    vec![
                        leaf("int", "x", Var::Int(k as i32 * 1024)),
                        leaf("float", "y", Var::Double(y)),
                        leaf("int", "factor", Var::Int(3)),
                    ],
                )
            })
            .collect();
        let section = |id: &str| {
            let wave = node(
                "WaveStateModel",
                Some("wave"),
                vec![node("enumeration", Some("points"), points.clone())],
            );
            node(
                "SectionEnvState",
                Some(id),
                vec![node("LfoState", Some("lfo"), vec![wave])],
            )
        };
        let bands = types
            .iter()
            .map(|&t| {
                node(
                    "FilterBandState",
                    None,
                    vec![
                        section("cutoff"),
                        section("resonance"),
                        leaf("int", "typeWithoutBypass", Var::Int(t)),
                    ],
                )
            })
            .collect();
        let mut root = node(
            "PluginState",
            None,
            vec![node(
                "FilterState",
                Some("filter"),
                vec![node("enumeration", Some("bands"), bands)],
            )],
        );
        root.props
            .push(("version".into(), Var::String(version.into())));
        root
    }

    fn ys(s: &State, band: usize, section: &str) -> Vec<f32> {
        s.shapers[2].bands[band]
            .section(section)
            .unwrap()
            .lfo
            .points
            .iter()
            .map(|p| p.y)
            .collect()
    }

    /// Against `migrate_resonance_y` of `.amp/in/sb/data/sb_filter_exact.py`.
    #[test]
    fn warm_resonance_waves_migrate_up_to_version_69() {
        let input = [0.0, 0.5, -1.0, 1.0, -0.3, -0.6, 0.25];
        let s = State::from_tree(&filter_tree("56", [2, 15, 1], &input));
        let lp = [
            0x3ed1_fcc8,
            0x3f44_7fe4,
            0xbf80_0000,
            0x3f28_94b6,
            0x3e0e_2900,
            0xbe3e_7018,
            0x3f1b_9246,
        ];
        let notch = [
            0x3f60_0000,
            0x3f7e_0000,
            0xbf80_0000,
            0x3f80_0000,
            0x3f24_9ad6,
            0x3e39_2398,
            0x3f75_e000,
        ];
        let got: Vec<u32> = ys(&s, 0, "resonance").iter().map(|y| y.to_bits()).collect();
        assert_eq!(got, lp);
        let got: Vec<u32> = ys(&s, 1, "resonance").iter().map(|y| y.to_bits()).collect();
        assert_eq!(got, notch);
        let unchanged: Vec<f32> = input.iter().map(|&y| y as f32).collect();
        assert_eq!(ys(&s, 2, "resonance"), unchanged);
        assert_eq!(ys(&s, 0, "cutoff"), unchanged);
        for v in ["69", "70", "75"] {
            let s = State::from_tree(&filter_tree(v, [2, 15, 1], &input));
            let want = if v == "69" { lp[0] } else { 0 };
            assert_eq!(ys(&s, 0, "resonance")[0].to_bits(), want, "version {v}");
        }
    }
}
