//! Editor (placeholder while the port is in progress).

use crate::params::BackspinParams;
use backspin_engine::Status;
use nih_plug::prelude::Editor;
use nih_plug_egui::{create_egui_editor, egui};
use parking_lot::Mutex;
use std::sync::Arc;

pub fn create(params: Arc<BackspinParams>, _status: Arc<Mutex<Status>>) -> Option<Box<dyn Editor>> {
    create_egui_editor(params.editor_state.clone(), (), |_, _| {}, move |ctx, _setter, _state| {
        egui::CentralPanel::default().show(ctx, |ui| ui.label("Backspin"));
    })
}
