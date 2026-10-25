//! A standalone build of the plugin, for looking at the editor without a host.

use backspin::Backspin;
use nih_plug::prelude::*;

fn main() {
    nih_export_standalone::<Backspin>();
}
