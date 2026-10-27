//! The controls card through real inputs: at the start, on its keys, from the pause menu.

use super::*;
use crate::scene::SceneSetup;
use crate::settings::CONTROLS_DISMISSALS;
use crate::ui::controls::{Shown, ROWS, START_SECONDS};

fn shown(h: &Harness) -> Option<Shown> {
    h.ui().controls().shown()
}

/// Holds a pad button for `seconds` and lets go.
fn hold(h: &mut Harness, b: PadButton, seconds: f64) {
    h.input.press(RawInput::PadButton(b));
    for _ in 0..(seconds / DT).ceil() as usize {
        h.frame();
    }
    h.input.release(RawInput::PadButton(b));
    h.frame();
}

fn saved(store: &SharedStore) -> Settings {
    Settings::from_json(store.0.borrow().as_deref().unwrap_or("{}")).0
}

#[test]
fn the_card_lists_the_live_bindings_and_follows_a_remap() {
    let mut h = Harness::new();
    assert_eq!(shown(&h), None, "no store, no start card");
    h.key(Key::KeyH);
    assert_eq!(shown(&h), Some(Shown::Asked));
    let text = h.hud_text();
    for want in [
        "Controls",
        "Keyboard",
        "Controller",
        "W / Up",
        "RT",
        "A / D / Arrows",
        "RMB + Mouse",
        "D-pad up (hold)",
        "Start (hold)",
        "H / F1",
        "Select (hold)",
    ] {
        assert!(text.contains(want), "{want}: {text}");
    }
    for r in ROWS {
        assert!(text.contains(r.label), "{}: {text}", r.label);
    }
    // F1 closes it, and the pause menu does not open.
    h.key(Key::F1);
    assert_eq!(shown(&h), None);
    assert!(!h.app.menu_open());

    // Put the horn on J with the Bindings page: the card says so.
    let kb = DeviceKind::KeyboardMouse;
    h.open(&[
        ItemId::Open(Page::Settings),
        ItemId::Open(Page::Controls),
        ItemId::Open(Page::Device(kb)),
        ItemId::Open(Page::Bindings(kb)),
    ]);
    h.focus_on(ItemId::Bind(kb, Action::Horn, Sign::Full));
    h.pad(PadButton::South);
    h.key(Key::KeyJ);
    assert_eq!(
        h.app.bindings().get(kb, Action::Horn),
        &[Binding::new(RawInput::Key(Key::KeyJ), Sign::Positive)]
    );
    while h.app.menu_open() {
        h.pad(PadButton::East);
    }
    h.key(Key::F1);
    let text = h.hud_text();
    let horn = text.find("Horn").expect("horn row");
    assert!(
        text[horn..].starts_with("HornJ"),
        "{}",
        &text[horn..horn + 12]
    );
}

#[test]
fn it_shows_at_the_start_of_a_running_game_and_not_in_a_still_scene() {
    let store = SharedStore::default();
    let mut h = Harness::with_store(&store);
    assert!(matches!(shown(&h), Some(Shown::Start { .. })));
    assert!(h.ui().controls_up());
    let text = h.hud_text();
    assert!(text.contains("H or F1: controls"), "{text}");
    assert!(!text.contains("mph"), "it takes the driving HUD's place");
    // It pauses nothing.
    let t = h.app.ticks();
    h.frame();
    assert!(h.app.ticks() > t);

    let setup = SceneSetup::from_query("still=1&stream=0").unwrap();
    let mut app = App::from_setup(&setup).unwrap();
    app.set_settings_store(Box::new(SharedStore::default()));
    let mut input = InputMapper::default();
    app.frame(&mut input, DT, &crate::tests::wall_clock);
    assert_eq!(app.ui().controls().shown(), None);
    assert!(!app.ui().controls_up());
}

#[test]
fn it_goes_on_any_input_or_when_its_time_is_up() {
    // Time: gone after START_SECONDS, not counted as dismissed.
    let store = SharedStore::default();
    let mut h = Harness::with_store(&store);
    let frames = (f64::from(START_SECONDS) / DT).ceil() as usize;
    for _ in 0..frames - 2 {
        h.frame();
    }
    assert!(shown(&h).is_some());
    for _ in 0..4 {
        h.frame();
    }
    assert_eq!(shown(&h), None);
    assert!(h.hud_text().contains("mph"));
    assert_eq!(h.app.settings().controls_dismissed, 0);

    // A pad trigger dismisses it, and still drives the car.
    let mut h = Harness::with_store(&store);
    assert!(shown(&h).is_some());
    h.input.axis(RawInput::PadAxis(PadAxis::R2), 1.0);
    for _ in 0..30 {
        h.frame();
    }
    assert_eq!(shown(&h), None);
    assert!(h.app.hud_state().speed_mps.abs() > 0.5);
    assert_eq!(saved(&store).controls_dismissed, 1);

    // H dismisses it rather than toggling it back.
    let mut h = Harness::with_store(&store);
    h.key(Key::KeyH);
    assert_eq!(shown(&h), None);
    // Esc dismisses it and opens the pause menu as usual.
    let mut h = Harness::with_store(&SharedStore::default());
    h.key(Key::Escape);
    assert_eq!(shown(&h), None);
    assert_eq!(h.page(), Page::Pause);
}

#[test]
fn after_three_dismissed_sessions_it_stops_showing_at_the_start() {
    let store = SharedStore::default();
    for n in 1..=CONTROLS_DISMISSALS {
        let mut h = Harness::with_store(&store);
        assert!(
            matches!(shown(&h), Some(Shown::Start { .. })),
            "session {n}"
        );
        h.key(Key::KeyW);
        assert_eq!(saved(&store).controls_dismissed, n);
    }
    let mut h = Harness::with_store(&store);
    assert_eq!(shown(&h), None);
    assert!(!saved(&store).controls_at_start);
    // The key still brings it up.
    h.key(Key::F1);
    assert_eq!(shown(&h), Some(Shown::Asked));
    h.key(Key::F1);

    // Settings › HUD › Show controls at start, back on: three more sessions.
    h.open(&[ItemId::Open(Page::Settings), ItemId::Open(Page::Hud)]);
    h.focus_on(ItemId::ControlsAtStart);
    assert_eq!(h.ui().items()[h.focus()].widget, Widget::Toggle(false));
    h.pad(PadButton::DpadRight);
    while h.app.menu_open() {
        h.pad(PadButton::East);
    }
    let s = saved(&store);
    assert!(s.controls_at_start && s.controls_dismissed == 0);
    let h = Harness::with_store(&store);
    assert!(matches!(shown(&h), Some(Shown::Start { .. })));

    // Switched off, it does not show at all.
    let off = SharedStore::default();
    let mut h = Harness::with_store(&off);
    h.key(Key::KeyW);
    h.open(&[ItemId::Open(Page::Settings), ItemId::Open(Page::Hud)]);
    h.focus_on(ItemId::ControlsAtStart);
    h.pad(PadButton::DpadRight);
    while h.app.menu_open() {
        h.pad(PadButton::East);
    }
    let h = Harness::with_store(&off);
    assert_eq!(shown(&h), None);
}

#[test]
fn the_pause_menu_and_the_pad_alone_open_and_close_it() {
    let mut h = Harness::new();
    // Start held: the card, without the pause menu.
    hold(&mut h, PadButton::Start, 1.1);
    assert_eq!(shown(&h), Some(Shown::Asked));
    assert!(!h.app.menu_open());
    // Driving goes on under it.
    let t = h.app.ticks();
    h.frame();
    assert!(h.app.ticks() > t);
    // B closes it.
    h.pad(PadButton::East);
    assert_eq!(shown(&h), None);
    assert!(!h.app.menu_open());
    // A tap of Start is still the pause menu; its Controls item shows the card over it.
    h.pad(PadButton::Start);
    assert_eq!(h.page(), Page::Pause);
    h.focus_on(ItemId::Controls);
    h.pad(PadButton::South);
    assert_eq!(shown(&h), Some(Shown::Asked));
    assert!(h.hud_text().contains("Controller"));
    assert!(!h.hud_text().contains("Resume"), "the card, not the menu");
    let paused = h.app.ticks();
    // Start (a tap) closes the card and leaves the menu as it was, still paused.
    h.pad(PadButton::Start);
    assert_eq!(shown(&h), None);
    assert_eq!(h.page(), Page::Pause);
    assert_eq!(h.ui().items()[h.focus()].id, ItemId::Controls);
    assert_eq!(h.app.ticks(), paused);
    // Start held toggles it too.
    h.pad(PadButton::South);
    hold(&mut h, PadButton::Start, 1.1);
    assert_eq!(shown(&h), None);
    assert_eq!(h.page(), Page::Pause);
}

#[test]
fn the_performance_overlay_hides_under_the_card() {
    let mut h = Harness::new();
    // Each mode: hidden while the card is up, back (the same mode) when it goes.
    for mode in [Overlay::Fps, Overlay::Full] {
        h.key(Key::F3);
        assert_eq!(overlay_shown(&h), Some(mode));
        h.key(Key::KeyH);
        assert_eq!(overlay_shown(&h), None, "{mode:?} under the card");
        h.key(Key::KeyH);
        assert_eq!(overlay_shown(&h), Some(mode));
    }
}

/// The card's quads as drawn now: all inside the safe area, and no two text boxes
/// overlapping.
fn check_layout(h: &Harness, what: &str, scale: f32) {
    let [sx, sy, sw, sh] = render::HudLayout::safe_rect(scale);
    let hud = h.ui().hud();
    let mut boxes = Vec::new();
    for item in hud.items {
        let (x, y, w, bh) = match *item {
            render::HudItem::Text {
                x,
                y,
                scale: s,
                text,
                ..
            } => {
                let t = hud.str(text);
                let b = (
                    x,
                    y,
                    render::font::text_width(t) as f32 * s,
                    render::font::GLYPH_H as f32 * s,
                );
                boxes.push((b, t.to_owned()));
                b
            }
            render::HudItem::Rect { color, .. } if color == SHADE_TEST => continue,
            render::HudItem::Rect { x, y, w, h, .. } => (x, y, w, h),
            render::HudItem::Image(i) => (i.x, i.y, i.w, i.h),
        };
        let at = format!("{what}: {item:?}");
        assert!(x >= sx - 0.5 && x + w <= sx + sw + 0.5, "{at}");
        assert!(y >= sy - 0.5 && y + bh <= sy + sh + 0.5, "{at}");
    }
    for (i, (a, ta)) in boxes.iter().enumerate() {
        for (b, tb) in &boxes[i + 1..] {
            let apart =
                a.0 + a.2 <= b.0 || b.0 + b.2 <= a.0 || a.1 + a.3 <= b.1 || b.1 + b.3 <= a.1;
            assert!(apart, "{what}: {ta:?} over {tb:?}");
        }
    }
}

#[test]
fn the_card_fits_the_deck_at_scale_1_and_scrolls_on_a_tv_at_2() {
    for scale in [1.0, 2.0] {
        let mut h = Harness::new();
        h.app.set_target_size(1280, 800);
        h.app.ui.force_ui_scale(scale);
        h.key(Key::KeyH);
        check_layout(&h, &format!("x{scale}"), scale);
        if scale == 1.0 {
            assert!(!h.ui().controls().scrolls(), "one screen on the Deck");
            assert!(h.hud_text().contains("Screenshot"));
            continue;
        }
        // Two lines a row: it scrolls down and up, and the radio and lights stay as they
        // were; then B closes it.
        assert!(h.ui().controls().scrolls());
        assert!(!h.hud_text().contains("Screenshot"));
        for _ in 0..40 {
            h.pad(PadButton::DpadDown);
        }
        assert_eq!(shown(&h), Some(Shown::Asked));
        check_layout(&h, "x2 scrolled", scale);
        assert!(h.hud_text().contains("Screenshot"));
        assert!(!h.hud_text().contains("Steer"));
        for _ in 0..40 {
            h.pad(PadButton::DpadUp);
        }
        assert!(h.hud_text().contains("Steer"));
        assert!(!h.app.radio().is_on() && !h.app.drive.headlights);
        // Keyboard Up and Down scroll it and do not drive.
        h.input.press(RawInput::Key(Key::ArrowDown));
        h.frame();
        assert!(!h.hud_text().contains("Steer"));
        h.input.release(RawInput::Key(Key::ArrowDown));
        h.key(Key::ArrowUp);
        assert!(h.hud_text().contains("Steer"));
        assert!(h.app.hud_state().speed_mps.abs() < 0.05);
        h.pad(PadButton::DpadLeft);
        assert_eq!(shown(&h), Some(Shown::Asked), "left and right do nothing");
        h.pad(PadButton::East);
        assert_eq!(shown(&h), None);
    }
    // The start card's footer fits too.
    let mut h = Harness::with_store(&SharedStore::default());
    h.app.set_target_size(1280, 800);
    h.frame();
    check_layout(&h, "start", 1.0);
    // At scale 2 the start card scrolls on up and down (and its time starts again)
    // rather than going.
    let mut h = Harness::new();
    h.app.set_target_size(1280, 800);
    h.app.ui.force_ui_scale(2.0);
    h.app.set_settings_store(Box::new(SharedStore::default()));
    for _ in 0..60 {
        h.frame();
    }
    assert!(h.hud_text().contains("[Up/Down] More"));
    h.pad(PadButton::DpadDown);
    assert_eq!(
        shown(&h),
        Some(Shown::Start {
            remaining: START_SECONDS - (DT as f32).min(0.1)
        })
    );
    assert!(!h.hud_text().contains("Steer"));
    assert!(!h.app.radio().is_on());
    h.key(Key::KeyW);
    assert_eq!(shown(&h), None);
}

#[test]
fn over_the_road_the_card_keeps_the_keys_from_the_car_and_the_radio() {
    let mut h = Harness::new();
    let lights = h.app.drive.headlights;
    h.key(Key::KeyH);
    // Menu keys that drive or tune do neither, and leave the card up.
    h.pad(PadButton::DpadDown);
    h.pad(PadButton::DpadUp);
    h.pad(PadButton::DpadRight);
    h.pad(PadButton::South);
    h.key(Key::Space);
    h.key(Key::Enter);
    assert_eq!(shown(&h), Some(Shown::Asked));
    assert!(!h.app.radio().is_on());
    assert_eq!(h.app.drive.headlights, lights);
    // Past half lock on the stick and the throttle: still up, and the car coasts.
    h.input.axis(RawInput::PadAxis(PadAxis::LeftX), 0.6);
    h.input.axis(RawInput::PadAxis(PadAxis::R2), 1.0);
    for _ in 0..30 {
        h.frame();
    }
    assert_eq!(shown(&h), Some(Shown::Asked));
    assert!(h.app.hud_state().speed_mps.abs() < 0.05);
    // B closes it and the next frame drives again.
    h.pad(PadButton::East);
    assert_eq!(shown(&h), None);
    for _ in 0..30 {
        h.frame();
    }
    assert!(h.app.hud_state().speed_mps.abs() > 0.5);
    h.input.axis(RawInput::PadAxis(PadAxis::LeftX), 0.0);
    h.input.axis(RawInput::PadAxis(PadAxis::R2), 0.0);
    // Esc closes it without opening the pause menu.
    h.key(Key::KeyH);
    h.key(Key::Escape);
    assert_eq!(shown(&h), None);
    assert!(!h.app.menu_open());
}

#[test]
fn the_close_hint_names_the_cards_keys_or_only_back() {
    let mut h = Harness::new();
    h.key(Key::KeyH);
    assert!(
        h.hud_text().contains("[H / F1 / Esc] Close"),
        "{}",
        h.hud_text()
    );
    h.key(Key::KeyH);
    let kb = DeviceKind::KeyboardMouse;
    h.app.ui.bindings.set(kb, Action::Controls, vec![]);
    h.app.ui.bindings_changed = true;
    h.frame();
    h.open(&[]);
    h.focus_on(ItemId::Controls);
    h.key(Key::Enter);
    assert!(h.hud_text().contains("[Esc] Close"), "{}", h.hud_text());
}
