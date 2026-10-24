//! The editor (ui.md).

use crate::params::BackspinParams;
use crate::shared::Shared;
use nih_plug::prelude::Editor;
use nih_plug_egui::{create_egui_editor, egui};
use std::sync::Arc;

pub fn create(params: Arc<BackspinParams>, _shared: Arc<Shared>) -> Option<Box<dyn Editor>> {
    create_egui_editor(
        params.editor_state.clone(),
        (),
        |_, _| {},
        move |ctx, _setter, _state| {
            egui::CentralPanel::default().show(ctx, |ui| ui.label("Backspin"));
        },
    )
}
