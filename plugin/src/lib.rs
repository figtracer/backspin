//! Backspin: a time and volume mapping effect.
//!
//! This crate connects the [`backspin_engine`] to plugin hosts through nih-plug (CLAP and VST3)
//! and provides the egui editor.

mod editor;
mod params;

use backspin_engine::{Bank, Engine, NoteEvent as EngineNote, Status, Transport};
use nih_plug::prelude::*;
use params::BackspinParams;
use parking_lot::Mutex;
use std::sync::Arc;

const MAX_EVENTS: usize = 512;

/// The plugin.
pub struct Backspin {
    params: Arc<BackspinParams>,
    engine: Option<Engine>,
    fallback: Bank,
    status: Arc<Mutex<Status>>,
    events: Vec<EngineNote>,
}

impl Default for Backspin {
    fn default() -> Self {
        let params = Arc::new(BackspinParams::default());
        let fallback = params.bank.read().clone();
        Self {
            params,
            engine: None,
            fallback,
            status: Arc::new(Mutex::new(Status::default())),
            events: Vec::with_capacity(MAX_EVENTS),
        }
    }
}

impl Plugin for Backspin {
    const NAME: &'static str = "Backspin";
    const VENDOR: &'static str = "figtracer";
    const URL: &'static str = "https://github.com/figtracer/backspin";
    const EMAIL: &'static str = "noreply@example.com";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(1),
            ..AudioIOLayout::const_default()
        },
    ];

    const MIDI_INPUT: MidiConfig = MidiConfig::Basic;
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), self.status.clone())
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        let mut engine = Engine::new(buffer_config.sample_rate);
        engine.reset(&self.params.settings());
        context.set_latency_samples(engine.latency());
        self.engine = Some(engine);
        {
            let mut bank = self.params.bank.write();
            bank.sanitize();
            self.fallback = bank.clone();
        }
        true
    }

    fn reset(&mut self) {
        if let Some(engine) = &mut self.engine {
            engine.reset(&self.params.settings());
        }
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let Some(engine) = &mut self.engine else { return ProcessStatus::Normal };

        self.events.clear();
        while let Some(event) = context.next_event() {
            if self.events.len() >= MAX_EVENTS {
                continue;
            }
            match event {
                NoteEvent::NoteOn { timing, note, velocity, .. } if velocity > 0.0 => {
                    self.events.push(EngineNote { timing: timing as usize, note, on: true })
                }
                NoteEvent::NoteOn { timing, note, .. } | NoteEvent::NoteOff { timing, note, .. } => {
                    self.events.push(EngineNote { timing: timing as usize, note, on: false })
                }
                _ => {}
            }
        }

        let t = context.transport();
        let transport = Transport { playing: t.playing, tempo: t.tempo, pos_beats: t.pos_beats() };
        let settings = self.params.settings();

        let channels = buffer.as_slice();
        let (left, right): (&mut [f32], &mut [f32]) = match channels {
            [l, r, ..] => (l, r),
            [mono] => {
                // Mono: process a copy as the right channel and discard it.
                let len = mono.len();
                let mut scratch = [0.0f32; 4096];
                let n = len.min(scratch.len());
                scratch[..n].copy_from_slice(&mono[..n]);
                if let Some(bank) = self.params.bank.try_read() {
                    engine.process(&bank, &settings, &transport, &self.events, &mut mono[..n], &mut scratch[..n]);
                } else {
                    engine.process(&self.fallback, &settings, &transport, &self.events, &mut mono[..n], &mut scratch[..n]);
                }
                self.publish_status();
                return ProcessStatus::Normal;
            }
            [] => return ProcessStatus::Normal,
        };

        if let Some(bank) = self.params.bank.try_read() {
            engine.process(&bank, &settings, &transport, &self.events, left, right);
        } else {
            engine.process(&self.fallback, &settings, &transport, &self.events, left, right);
        }
        self.publish_status();
        ProcessStatus::Normal
    }
}

impl Backspin {
    fn publish_status(&self) {
        if let (Some(engine), Some(mut status)) = (&self.engine, self.status.try_lock()) {
            *status = engine.status();
        }
    }
}

impl ClapPlugin for Backspin {
    const CLAP_ID: &'static str = "com.figtracer.backspin";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Time and volume mapping effect");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] =
        &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Mono, ClapFeature::Glitch];
}

impl Vst3Plugin for Backspin {
    const VST3_CLASS_ID: [u8; 16] = *b"BackspinFigTrc01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Delay, Vst3SubCategory::Stereo];
}

nih_export_clap!(Backspin);
nih_export_vst3!(Backspin);
