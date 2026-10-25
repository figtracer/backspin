//! Host parameters and the persisted Gross Beat state.

use crate::shared::Shared;
use backspin_engine::{PARAMS, param_text};
use nih_plug::params::persist::PersistentField;
use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::sync::Arc;

/// The editor size at 100 %: the canonical 824 x 560 of ui.md 2.1.
pub const EDITOR_SIZE: (u32, u32) = (824, 560);

/// The nine parameters with their raw integer ranges, and the persisted state blob.
#[derive(Params)]
pub struct BackspinParams {
    /// The Gross Beat state blob (SaveRestoreState format, version 0x90003).
    #[persist = "gross-beat-state"]
    pub state: StateField,
    #[persist = "editor-state"]
    pub editor_state: Arc<EguiState>,

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
