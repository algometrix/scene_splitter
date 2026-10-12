//! Which part of the window the keys pressed in a frame are meant for.
//!
//! Several things on screen listen for keys at once: a dialog, the player
//! and the review screen behind them. Each is drawn in its own place in the
//! frame, so without a rule a single key press reaches all of them. The rule
//! has two parts. A dialog in front of everything has the keyboard to
//! itself. And Escape, which every layer understands as "leave this", is
//! taken by the innermost layer that is open, so the layers behind it never
//! see it.

use eframe::egui;

/// Who may act on the keys pressed in this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KeyboardOwner {
    /// A dialog is open in front of everything else. The player and the
    /// review screen behind it leave every key alone.
    Dialog,
    /// No dialog is open. The player and the review screen share the keys,
    /// each with its own shortcuts.
    Screens,
}

impl KeyboardOwner {
    /// A screen behind a dialog must not react to keys. Neither must any
    /// screen while the person types into a text box, whose keys are text.
    pub fn lets_screens_use_keys(self, ctx: &egui::Context) -> bool {
        self == Self::Screens && !ctx.wants_keyboard_input()
    }
}

/// Whether Escape was pressed in this frame and nothing has taken it yet.
/// Asking takes it: whoever asks next in the same frame is told no.
///
/// The layers ask from the innermost outwards, which makes one press close
/// one thing.
pub fn take_escape(ctx: &egui::Context) -> bool {
    ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
}

/// Whether `key` was pressed in this frame with no Ctrl, Alt or Shift held,
/// and nothing has taken it yet. Asking takes it, as with Escape.
///
/// The player asks for its keys before the review screen behind it reads
/// its own. An arrow key that skips in the player therefore does not also
/// move through the scene list. The same key with Alt or Shift held is left
/// alone, for the screen that gives it a meaning.
pub fn take_plain_key(ctx: &egui::Context, key: egui::Key) -> bool {
    let is_this_key_pressed_plainly = |event: &egui::Event| {
        matches!(
            event,
            egui::Event::Key { key: pressed_key, pressed: true, modifiers, .. }
                if *pressed_key == key && modifiers.is_none()
        )
    };
    ctx.input_mut(|input| {
        let events_before = input.events.len();
        input
            .events
            .retain(|event| !is_this_key_pressed_plainly(event));
        input.events.len() != events_before
    })
}

/// Whether a button, box or other control has been given the keys with Tab.
/// Enter and Space press that control and the arrow keys move on from it,
/// so a screen must not also read them as its own.
pub fn a_control_has_the_keys(ctx: &egui::Context) -> bool {
    ctx.memory(|memory| memory.focused().is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_with_key(
        ctx: &egui::Context,
        key: egui::Key,
        modifiers: egui::Modifiers,
        in_frame: impl FnMut(&egui::Context),
    ) {
        let press = egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        let input = egui::RawInput {
            events: vec![press],
            modifiers,
            ..Default::default()
        };
        let _ = ctx.run(input, in_frame);
    }

    #[test]
    fn a_plain_key_is_taken_only_once_and_is_gone_for_whoever_reads_next() {
        let ctx = egui::Context::default();
        let mut taken = Vec::new();
        let mut still_pressed = true;
        frame_with_key(&ctx, egui::Key::ArrowLeft, egui::Modifiers::NONE, |ctx| {
            taken = vec![
                take_plain_key(ctx, egui::Key::ArrowRight),
                take_plain_key(ctx, egui::Key::ArrowLeft),
                take_plain_key(ctx, egui::Key::ArrowLeft),
            ];
            still_pressed = ctx.input(|input| input.key_pressed(egui::Key::ArrowLeft));
        });
        assert_eq!(taken, vec![false, true, false]);
        assert!(!still_pressed);
    }

    #[test]
    fn a_key_pressed_with_alt_or_shift_is_not_a_plain_key() {
        for modifiers in [egui::Modifiers::ALT, egui::Modifiers::SHIFT] {
            let ctx = egui::Context::default();
            let mut taken = true;
            let mut still_pressed = false;
            frame_with_key(&ctx, egui::Key::ArrowLeft, modifiers, |ctx| {
                taken = take_plain_key(ctx, egui::Key::ArrowLeft);
                still_pressed = ctx.input(|input| input.key_pressed(egui::Key::ArrowLeft));
            });
            assert!(!taken);
            assert!(still_pressed);
        }
    }

    #[test]
    fn no_control_has_the_keys_until_one_is_given_them() {
        let ctx = egui::Context::default();
        let mut button = None;
        let mut had_the_keys = Vec::new();
        for _ in 0..2 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                had_the_keys.push(a_control_has_the_keys(ctx));
                egui::CentralPanel::default().show(ctx, |ui| {
                    let response = ui.button("A button");
                    response.request_focus();
                    button = Some(response.id);
                });
            });
        }
        assert!(button.is_some());
        assert_eq!(had_the_keys, vec![false, true]);
    }

    fn frame_with_escape(ctx: &egui::Context, in_frame: impl FnMut(&egui::Context)) {
        let escape = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let input = egui::RawInput {
            events: vec![escape],
            ..Default::default()
        };
        let _ = ctx.run(input, in_frame);
    }

    #[test]
    fn one_press_of_escape_is_taken_only_once() {
        let ctx = egui::Context::default();
        let mut taken = Vec::new();
        frame_with_escape(&ctx, |ctx| {
            taken = vec![take_escape(ctx), take_escape(ctx)];
        });
        assert_eq!(taken, vec![true, false]);
    }

    #[test]
    fn without_a_press_there_is_no_escape_to_take() {
        let ctx = egui::Context::default();
        let mut taken = true;
        let _ = ctx.run(egui::RawInput::default(), |ctx| taken = take_escape(ctx));
        assert!(!taken);
    }

    #[test]
    fn screens_behind_a_dialog_get_no_keys() {
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            assert!(KeyboardOwner::Screens.lets_screens_use_keys(ctx));
            assert!(!KeyboardOwner::Dialog.lets_screens_use_keys(ctx));
        });
    }
}
