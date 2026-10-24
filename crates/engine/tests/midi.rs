//! Gate 6: MIDI slot selection and the key-held stack (state.md section 5). The audio side is in
//! the parity matrix (`cargo xtask parity --group midi`).

use backspin_engine::Engine;

fn on(note: u32) -> u32 {
    0x90 | note << 8 | 100 << 16
}

fn off(note: u32) -> u32 {
    0x80 | note << 8
}

fn engine() -> Engine {
    let mut e = Engine::new();
    e.set_sample_rate(48000);
    e.set_block_size(512);
    e.set_time_sig(96);
    e.set_tempo(120.0);
    e.midi_trigger = 1;
    for b in &mut e.banks {
        b.slots[1].key_held = 1;
        b.slots[2].key_held = 1;
    }
    e.set_playing(true);
    e
}

#[test]
fn midi_is_off_in_a_fresh_instance() {
    let mut e = Engine::new();
    assert!(!e.midi_in(on(61)));
    assert_eq!(e.params[0], 0);
}

#[test]
fn notes_select_time_and_volume_slots() {
    let mut e = engine();
    assert!(e.midi_in(on(65)));
    assert_eq!(e.params[0], 5);
    assert!(e.midi_in(on(24 + 7)));
    assert_eq!(e.params[1], 7);
    // Notes outside 24..95 are consumed but select nothing.
    assert!(e.midi_in(on(100)));
    assert_eq!((e.params[0], e.params[1]), (5, 7));
    // Only note on and note off are handled.
    assert!(!e.midi_in(0xb0 | 1 << 8));
}

#[test]
fn key_held_returns_to_the_previous_slot() {
    let mut e = engine();
    e.midi_in(on(63));
    assert_eq!(e.params[0], 3);
    e.midi_in(on(61));
    assert_eq!(e.params[0], 1);
    assert_eq!(e.banks[0].key_stack, [3, 1]);
    e.midi_in(off(61));
    assert_eq!(e.params[0], 3);
    assert!(e.banks[0].key_stack.is_empty());
}

#[test]
fn nested_key_held_notes_unwind_in_order() {
    let mut e = engine();
    e.midi_in(on(61));
    e.midi_in(on(62));
    assert_eq!(e.banks[0].key_stack, [0, 1, 2]);
    e.midi_in(off(62));
    assert_eq!(e.params[0], 1);
    e.midi_in(off(61));
    assert_eq!(e.params[0], 0);
    assert!(e.banks[0].key_stack.is_empty());
}

#[test]
fn velocity_zero_is_note_off_and_unknown_offs_are_ignored() {
    let mut e = engine();
    e.midi_in(on(61));
    e.midi_in(0x90 | 61 << 8);
    assert_eq!(e.params[0], 0);
    // A note off for a key that is not held does nothing, even for a key-held slot.
    e.midi_in(on(61));
    e.midi_in(off(61) | 3);
    assert_eq!(e.params[0], 1);
}

#[test]
fn the_same_slot_does_not_retrigger() {
    let mut e = engine();
    e.midi_in(on(64));
    let trig = e.banks[0].trigger_time;
    e.midi_in(off(64));
    e.midi_in(on(64));
    assert_eq!(e.banks[0].trigger_time, trig);
    assert_eq!(e.params[0], 4);
}
