//! Host parameters and the persisted Gross Beat state.

use crate::shared::Shared;
use backspin_engine::{PARAMS, param_text};
use nih_plug::params::persist::PersistentField;
use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::sync::Arc;

/// The editor size at 100 %: the form of the harness capture (830 x 535) and the ShaperBox
/// section under it.
pub const EDITOR_SIZE: (u32, u32) = (
    backspin_ui::WIDTH as u32,
    (backspin_ui::HEIGHT + backspin_ui::SB_HEIGHT) as u32,
);

/// The nine parameters with their raw integer ranges, and the persisted state blob.
#[derive(Params)]
pub struct BackspinParams {
    /// The Gross Beat state blob (SaveRestoreState format, version 0x90003).
    #[persist = "gross-beat-state"]
    pub state: StateField,
    #[persist = "editor-state"]
    pub editor_state: Arc<EguiState>,
    /// The ShaperBox section: on or off and its preset (ShaperBox's own state format).
    #[persist = "shaperbox-state"]
    pub shaperbox: ShaperBoxField,

    #[id = "time_slot"]
    pub time_slot: IntParam,
    #[id = "volume_slot"]
    pub volume_slot: IntParam,
    #[id = "time_mix"]
    pub time_mix: IntParam,
    #[id = "volume_mix"]
    pub volume_mix: IntParam,
    #[id = "attack"]
    pub attack: IntParam,
    #[id = "release"]
    pub release: IntParam,
    #[id = "tension"]
    pub tension: IntParam,
    #[id = "hold"]
    pub hold: IntParam,
    #[id = "scratch"]
    pub scratch: IntParam,
    /// The Clamp clipper at the end of the chain.
    #[nested(group = "Clamp")]
    pub clamp: ClampParams,
}

/// Clamp's parameters (the ranges of the original's), with ids prefixed `clamp_`.
#[derive(Params)]
pub struct ClampParams {
    /// Off by default, so the chain sounds as before until it is turned on.
    #[id = "clamp_on"]
    pub on: BoolParam,
    #[id = "clamp_oversampling"]
    pub oversampling: IntParam,
    #[id = "clamp_in"]
    pub input: FloatParam,
    #[id = "clamp_out"]
    pub output: FloatParam,
    #[id = "clamp_ceiling"]
    pub ceiling: FloatParam,
    #[id = "clamp_shape"]
    pub shape: FloatParam,
    #[id = "clamp_type"]
    pub kind: IntParam,
    #[id = "clamp_mode"]
    pub mode: IntParam,
}

/// The OVERSAMPLING, TYPE and MODE items.
pub const CLAMP_OVERSAMPLING: [&str; 5] = ["Off", "2x", "4x", "8x", "16x"];
pub const CLAMP_TYPES: [&str; 2] = ["Type A", "Type B"];
pub const CLAMP_MODES: [&str; 6] = ["LR", "L", "R", "MS", "M", "S"];

impl Default for ClampParams {
    fn default() -> Self {
        let db = |name: &str, default: f32, min: f32, max: f32| {
            FloatParam::new(name, default, FloatRange::Linear { min, max })
                .with_step_size(0.5)
                .with_unit(" dB")
        };
        let items = |name: &str, list: &'static [&'static str]| {
            IntParam::new(
                name,
                0,
                IntRange::Linear {
                    min: 0,
                    max: list.len() as i32 - 1,
                },
            )
            .with_value_to_string(Arc::new(move |v| {
                list[v.clamp(0, list.len() as i32 - 1) as usize].into()
            }))
        };
        Self {
            on: BoolParam::new("Clamp", false),
            oversampling: items("Clamp oversampling", &CLAMP_OVERSAMPLING),
            input: db("Clamp in", 0.0, -24.0, 24.0),
            output: db("Clamp out", 0.0, -24.0, 24.0),
            ceiling: db("Clamp ceiling", 0.0, -48.0, 0.0),
            shape: FloatParam::new(
                "Clamp shape",
                1.0,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_step_size(0.01),
            kind: items("Clamp type", &CLAMP_TYPES),
            mode: items("Clamp mode", &CLAMP_MODES),
        }
    }
}

impl BackspinParams {
    pub fn new(shared: Arc<Shared>) -> Self {
        let p = |i: usize| {
            let info = PARAMS[i];
            let shared = shared.clone();
            IntParam::new(
                info.name,
                info.default,
                IntRange::Linear {
                    min: info.min,
                    max: info.max,
                },
            )
            .with_value_to_string(Arc::new(move |v| {
                let model = shared.model.lock();
                param_text(i, v, |b, s| model.banks[b].slots[s].display_name(s))
            }))
        };
        Self {
            state: StateField(shared.clone()),
            shaperbox: ShaperBoxField(shared.clone()),
            editor_state: EguiState::from_size(EDITOR_SIZE.0, EDITOR_SIZE.1),
            time_slot: p(0),
            volume_slot: p(1),
            time_mix: p(2),
            volume_mix: p(3),
            attack: p(4),
            release: p(5),
            tension: p(6),
            hold: p(7),
            scratch: p(8),
            clamp: ClampParams::default(),
        }
    }

    /// The parameters in index order.
    pub fn by_index(&self) -> [&IntParam; 9] {
        [
            &self.time_slot,
            &self.volume_slot,
            &self.time_mix,
            &self.volume_mix,
            &self.attack,
            &self.release,
            &self.tension,
            &self.hold,
            &self.scratch,
        ]
    }
}

/// The state blob as a persisted field. Saving reads the model; loading replaces it. The audio
/// engine takes the model in `initialize`, which nih-plug calls after every state load.
pub struct StateField(pub Arc<Shared>);

/// The blob in serialized form: hex text.
pub struct Blob(pub Vec<u8>);

impl Serialize for Blob {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut hex = String::with_capacity(self.0.len() * 2);
        for b in &self.0 {
            hex.push_str(&format!("{b:02x}"));
        }
        s.serialize_str(&hex)
    }
}

impl<'de> Deserialize<'de> for Blob {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let hex = String::deserialize(d)?;
        let bytes = (0..hex.len() / 2)
            .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16))
            .collect::<Result<Vec<u8>, _>>()
            .map_err(serde::de::Error::custom)?;
        Ok(Blob(bytes))
    }
}

impl<'a> PersistentField<'a, Blob> for StateField {
    fn set(&self, new_value: Blob) {
        let mut model = self.0.model.lock();
        *model = backspin_engine::Engine::new();
        model.load_state(&new_value.0);
        self.0.status.publish(&model);
    }

    fn map<F, R>(&self, f: F) -> R
    where
        F: Fn(&Blob) -> R,
    {
        f(&Blob(self.0.save_state()))
    }
}

/// The ShaperBox section as a persisted field.
pub struct ShaperBoxField(pub Arc<Shared>);

impl<'a> PersistentField<'a, Blob> for ShaperBoxField {
    fn set(&self, new_value: Blob) {
        *self.0.sb.model.lock() = crate::sb::Model::from_bytes(&new_value.0);
    }

    fn map<F, R>(&self, f: F) -> R
    where
        F: Fn(&Blob) -> R,
    {
        f(&Blob(self.0.sb.model.lock().to_bytes()))
    }
}
