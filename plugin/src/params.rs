//! Plugin parameters and persisted state.

use backspin_engine::{Bank, ClickReduction, Settings, SLOTS};
use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use parking_lot::RwLock;
use std::sync::Arc;

/// Click reduction choices.
#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickReductionParam {
    /// No crossfade.
    #[name = "Off"]
    Off,
    /// Short crossfade.
    #[name = "Low (drums)"]
    Low,
    /// Long crossfade.
    #[name = "High (melodic)"]
    High,
}

impl From<ClickReductionParam> for ClickReduction {
    fn from(value: ClickReductionParam) -> Self {
        match value {
            ClickReductionParam::Off => Self::Off,
            ClickReductionParam::Low => Self::Low,
            ClickReductionParam::High => Self::High,
        }
    }
}

/// All parameters.
#[derive(Params)]
pub struct BackspinParams {
    /// Editor window size.
    #[persist = "editor-state"]
    pub editor_state: Arc<EguiState>,

    /// Slot envelopes and settings.
    #[persist = "bank"]
    pub bank: Arc<RwLock<Bank>>,

    /// Active time slot (1-based in the UI).
    #[id = "time_slot"]
    pub time_slot: IntParam,
    /// Active volume slot.
    #[id = "volume_slot"]
    pub volume_slot: IntParam,
    /// Time mix.
    #[id = "time_mix"]
    pub time_mix: FloatParam,
    /// Volume mix.
    #[id = "volume_mix"]
    pub volume_mix: FloatParam,
    /// Volume smoothing attack.
    #[id = "attack"]
    pub attack: FloatParam,
    /// Volume smoothing release.
    #[id = "release"]
    pub release: FloatParam,
    /// Volume envelope tension.
    #[id = "tension"]
    pub tension: FloatParam,
    /// Bar repeat.
    #[id = "hold"]
    pub hold: BoolParam,
    /// Scratch time offset.
    #[id = "scratch"]
    pub scratch: FloatParam,

    /// MIDI notes select slots.
    #[id = "midi_trigger"]
    pub midi_trigger: BoolParam,
    /// Crossfade on jumps.
    #[id = "click_reduction"]
    pub click_reduction: EnumParam<ClickReductionParam>,
    /// Delay the wet signal by the attack time.
    #[id = "attack_comp"]
    pub attack_compensation: BoolParam,
    /// Output DC blocker.
    #[id = "remove_dc"]
    pub remove_dc: BoolParam,
    /// Sinc interpolation.
    #[id = "hq"]
    pub hq: BoolParam,
    /// Show the scratching clock.
    #[id = "view_scratch"]
    pub view_scratch: BoolParam,
}

fn percent(name: &str, default: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Linear { min: 0.0, max: 1.0 })
        .with_unit("%")
        .with_value_to_string(formatters::v2s_f32_percentage(0))
        .with_string_to_value(formatters::s2v_f32_percentage())
}

fn ms(name: &str, max: f32) -> FloatParam {
    FloatParam::new(name, 0.0, FloatRange::Skewed { min: 0.0, max, factor: 0.4 })
        .with_unit(" ms")
        .with_value_to_string(formatters::v2s_f32_rounded(1))
}

fn option(name: &str, default: bool) -> BoolParam {
    BoolParam::new(name, default).non_automatable()
}

impl Default for BackspinParams {
    fn default() -> Self {
        let slot = |name: &str| IntParam::new(name, 1, IntRange::Linear { min: 1, max: SLOTS as i32 });
        Self {
            editor_state: EguiState::from_size(1060, 640),
            bank: Arc::new(RwLock::new(Bank::default())),
            time_slot: slot("Time slot"),
            volume_slot: slot("Volume slot"),
            time_mix: percent("Time mix", 1.0),
            volume_mix: percent("Volume mix", 1.0),
            attack: ms("Attack", 500.0),
            release: ms("Release", 1000.0),
            tension: FloatParam::new("Tension", 0.0, FloatRange::Linear { min: -1.0, max: 1.0 })
                .with_unit("%")
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage()),
            hold: BoolParam::new("Hold", false),
            scratch: percent("Scratch", 0.0),
            midi_trigger: option("Trigger slots from MIDI notes", true),
            click_reduction: EnumParam::new("Click reduction", ClickReductionParam::Low)
                .non_automatable(),
            attack_compensation: option("Smoothing attack compensation", false),
            remove_dc: option("Remove DC offset", false),
            hq: option("HQ resampling", true),
            view_scratch: option("View scratching clock", true),
        }
    }
}

impl BackspinParams {
    /// Builds engine settings from the current values.
    pub fn settings(&self) -> Settings {
        Settings {
            time_slot: (self.time_slot.value() - 1).max(0) as usize,
            volume_slot: (self.volume_slot.value() - 1).max(0) as usize,
            time_mix: self.time_mix.value(),
            volume_mix: self.volume_mix.value(),
            attack_ms: self.attack.value(),
            release_ms: self.release.value(),
            tension: self.tension.value(),
            hold: self.hold.value(),
            scratch: self.scratch.value(),
            midi_trigger: self.midi_trigger.value(),
            click_reduction: self.click_reduction.value().into(),
            attack_compensation: self.attack_compensation.value(),
            remove_dc: self.remove_dc.value(),
            hq: self.hq.value(),
        }
    }
}
