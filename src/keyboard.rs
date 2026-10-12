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

#[cfg(test)]
mod tests {
    use super::*;

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
