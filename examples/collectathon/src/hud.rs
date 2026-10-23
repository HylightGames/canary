//! The HUD build pass: score/goal readout plus the reset button.
//!
//! The build pass renders the game's immutable view into text and widgets
//! before the frame and hands the UI no live `World` borrow — the driver
//! calls this inside the backend's frame with last frame's extracted
//! state, so the HUD trails the scene by exactly one outer frame. That lag
//! is structural, not a bug: input routes UI-before-gameplay, so the build
//! pass runs before the simulation pass whose results it would need to be
//! current. Button presses return as intents; the simulation applies them
//! at its boundary ([`crate::game::register_apply_ui_intents`]).
//!
//! All visible strings resolve through `canary_loc::LocaleBundle` at the
//! call site, immediately before the `UiBuilder` calls — never inside
//! `canary-ui-core`. Message IDs are stable (`hud-status`, `hud-reset`,
//! `hud-goal-done`, `hud-goal-open`); numbers travel as Fluent arguments.

use canary_loc::{key, LocaleBundle};
use canary_ui_core::{UiBuilder, UiId};
use fluent::FluentArgs;

/// The HUD reset button's id: the build pass below and the intents stage
/// agree on this value, never on a live `World` borrow.
pub const RESET_BUTTON: UiId = UiId::new("reset");

/// The immutable view the HUD renders: extracted from the world after the
/// previous frame's simulation pass.
#[derive(Debug, Clone, Copy, Default)]
pub struct HudState {
    /// Points banked so far.
    pub score: u32,
    /// Pickups collected so far, goal included.
    pub collected: u32,
    /// Total pickups in the room, goal included.
    pub total: u32,
    /// True once every room pickup (goal included) is collected.
    pub goal_reached: bool,
    /// Player x in logical pixels.
    pub player_x: f32,
    /// Player y in logical pixels.
    pub player_y: f32,
}

/// Builds one HUD frame: a localized score/goal readout plus the reset button.
///
/// `bundle` is the game-layer [`LocaleBundle`] (English default with a
/// German fallback pack under `assets/locales/<locale>/*.ftl`). The goal
/// word resolves first, then travels as the `$goal` argument of
/// `hud-status` alongside the numeric arguments.
pub fn build_hud(builder: &mut dyn UiBuilder, bundle: &LocaleBundle, state: &HudState) {
    let goal_key = if state.goal_reached {
        key!("hud-goal-done")
    } else {
        key!("hud-goal-open")
    };
    let goal = bundle.resolve(&goal_key, None);
    let mut args = FluentArgs::new();
    args.set("score", state.score);
    args.set("collected", state.collected);
    args.set("total", state.total);
    args.set("goal", goal.as_str());
    args.set("player_x", format!("{:.0}", state.player_x));
    args.set("player_y", format!("{:.0}", state.player_y));
    let status = bundle.resolve(&key!("hud-status"), Some(&args));
    builder.label(&status);
    let reset = bundle.resolve(&key!("hud-reset"), None);
    builder.button(RESET_BUTTON, &reset);
}

#[cfg(test)]
mod tests {
    use super::*;
    use canary_loc::LocKey;
    use fluent::FluentResource;
    use unic_langid::langid;

    fn resource(source: &str) -> FluentResource {
        FluentResource::try_new(source.to_owned()).expect("test .ftl parses")
    }

    fn en_source() -> &'static str {
        "hud-status = Score: { $score }  Collected: { $collected }/{ $total }  Goal: { $goal }  Player: ({ $player_x }, { $player_y })\nhud-reset = Reset\nhud-goal-done = done\nhud-goal-open = open\n"
    }

    fn de_source() -> &'static str {
        "hud-status = Punktestand: { $score }  Gesammelt: { $collected }/{ $total }  Ziel: { $goal }  Spieler: ({ $player_x }, { $player_y })\nhud-reset = Zurücksetzen\nhud-goal-done = fertig\nhud-goal-open = offen\n"
    }

    fn bundle_for(
        locales: &[&str],
        load: impl FnMut(&unic_langid::LanguageIdentifier) -> Vec<FluentResource>,
    ) -> LocaleBundle {
        let requested: Vec<unic_langid::LanguageIdentifier> = locales
            .iter()
            .map(|s| s.parse().expect("locale parses"))
            .collect();
        let available = requested.clone();
        let default = langid!("en");
        LocaleBundle::new(&requested, &available, default, load)
    }

    fn state() -> HudState {
        HudState {
            score: 7,
            collected: 2,
            total: 4,
            goal_reached: false,
            player_x: 12.0,
            player_y: 34.0,
        }
    }

    struct Recorder {
        labels: Vec<String>,
        buttons: Vec<String>,
    }

    impl UiBuilder for Recorder {
        fn label(&mut self, text: &str) {
            self.labels.push(text.to_owned());
        }
        fn button(&mut self, _id: UiId, label: &str) -> bool {
            self.buttons.push(label.to_owned());
            false
        }
    }

    #[test]
    fn english_output_uses_english_strings() {
        let bundle = bundle_for(&["en"], |_| vec![resource(en_source())]);
        let mut ui = Recorder {
            labels: Vec::new(),
            buttons: Vec::new(),
        };
        build_hud(&mut ui, &bundle, &state());
        assert_eq!(ui.labels.len(), 1);
        assert!(ui.labels[0].contains("Score:"), "{}", ui.labels[0]);
        assert!(ui.labels[0].contains('7'), "{}", ui.labels[0]);
        assert!(ui.labels[0].contains("Goal:"), "{}", ui.labels[0]);
        assert!(ui.labels[0].contains("open"), "{}", ui.labels[0]);
        assert_eq!(ui.buttons, vec!["Reset".to_owned()]);
    }

    #[test]
    fn german_output_uses_german_strings() {
        let bundle = bundle_for(&["de"], |_| vec![resource(de_source())]);
        let mut ui = Recorder {
            labels: Vec::new(),
            buttons: Vec::new(),
        };
        build_hud(&mut ui, &bundle, &state());
        assert!(ui.labels[0].contains("Punktestand:"), "{}", ui.labels[0]);
        assert!(ui.labels[0].contains('7'), "{}", ui.labels[0]);
        assert!(ui.labels[0].contains("Ziel:"), "{}", ui.labels[0]);
        assert!(ui.labels[0].contains("offen"), "{}", ui.labels[0]);
        assert_eq!(ui.buttons, vec!["Zurücksetzen".to_owned()]);
    }

    #[test]
    fn missing_key_in_primary_falls_back_to_english() {
        let en = langid!("en");
        let de = langid!("de");
        let bundle = LocaleBundle::new(
            std::slice::from_ref(&de),
            &[en.clone(), de.clone()],
            en.clone(),
            |locale| {
                if *locale == de {
                    vec![resource("hud-reset = Zurücksetzen\n")]
                } else {
                    vec![resource(en_source())]
                }
            },
        );
        let status: LocKey = canary_loc::key!("hud-status");
        let mut args = fluent::FluentArgs::new();
        args.set("score", 1);
        args.set("collected", 0);
        args.set("total", 1);
        args.set("goal", "open");
        args.set("player_x", "0");
        args.set("player_y", "0");
        let resolved = bundle.resolve(&status, Some(&args));
        assert!(resolved.contains("Score:"), "{resolved}");
    }

    #[test]
    fn unknown_key_returns_the_key_itself() {
        let bundle = bundle_for(&["en"], |_| vec![resource(en_source())]);
        let missing: LocKey = canary_loc::key!("hud-does-not-exist");
        assert_eq!(bundle.resolve(&missing, None), "hud-does-not-exist");
    }
}
