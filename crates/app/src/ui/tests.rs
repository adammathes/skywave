//! Headless UI tests: real raw inputs through an [`InputMapper`] into [`App::frame`], no
//! GPU. The gamepad tests use only pad buttons and sticks.

use std::cell::RefCell;
use std::rc::Rc;

use super::*;
use crate::input::{Key, PadAxis, PadButton};
use skywave_core::{Seed, SIM_DT};

const DT: f64 = 1.0 / 60.0;

/// A settings store the test can look into after handing it to the app.
#[derive(Clone, Default)]
struct SharedStore(Rc<RefCell<Option<String>>>);

impl SettingsStore for SharedStore {
    fn load(&self) -> Option<String> {
        self.0.borrow().clone()
    }

    fn save(&mut self, json: &str) {
        *self.0.borrow_mut() = Some(json.to_owned());
    }
}

struct Harness {
    app: App,
    input: InputMapper,
}

impl Harness {
    fn new() -> Harness {
        let mut app = App::new(Seed(1));
        app.set_can_quit(true);
        let mut h = Harness {
            app,
            input: InputMapper::default(),
        };
        h.frame();
        h
    }

    fn with_store(store: &SharedStore) -> Harness {
        let mut h = Harness::new();
        h.app.set_settings_store(Box::new(store.clone()));
        h.frame();
        h
    }

    fn frame(&mut self) {
        self.app
            .frame(&mut self.input, DT, &crate::tests::wall_clock);
    }

    /// A press and release of `raw` on consecutive frames.
    fn tap(&mut self, raw: RawInput) {
        self.input.press(raw);
        self.frame();
        self.input.release(raw);
        self.frame();
    }

    fn pad(&mut self, b: PadButton) {
        self.tap(RawInput::PadButton(b));
    }

    fn key(&mut self, k: Key) {
        self.tap(RawInput::Key(k));
    }

    fn stick(&mut self, a: PadAxis, v: f32) {
        self.input.axis(RawInput::PadAxis(a), v);
        self.frame();
        self.input.axis(RawInput::PadAxis(a), 0.0);
        self.frame();
    }

    fn ui(&self) -> &Ui {
        self.app.ui()
    }

    fn page(&self) -> Page {
        self.ui().menu().expect("menu open").page()
    }

    fn focus(&self) -> usize {
        self.ui().menu().expect("menu open").focus()
    }

    fn item(&self, i: usize) -> Item {
        self.ui().items()[i]
    }

    /// Moves the focus down to the item with `id` (pad only).
    fn focus_on(&mut self, id: ItemId) {
        let target = self
            .ui()
            .items()
            .iter()
            .position(|i| i.id == id)
            .unwrap_or_else(|| panic!("{id:?} not on {:?}", self.page()));
        while self.focus() != target {
            self.pad(PadButton::DpadDown);
        }
    }

    /// Opens the menu and walks to `path` by selecting each item in turn (pad only).
    fn open(&mut self, path: &[ItemId]) {
        if !self.app.menu_open() {
            self.pad(PadButton::Start);
        }
        for &id in path {
            self.focus_on(id);
            self.pad(PadButton::South);
        }
    }

    fn hud_text(&self) -> String {
        self.app.ui().hud().text.to_owned()
    }
}

/// Every item of every page reachable from `page`, depth first, as the menu shows them.
fn reachable(page: Page, app: &App, out: &mut Vec<(Page, usize)>) {
    let mut items = Vec::new();
    pages::build(page, &app.ui.page_ctx(&app.time.clock), &mut items);
    for (i, item) in items.iter().enumerate() {
        out.push((page, i));
        if let Widget::Submenu(sub) = item.widget {
            reachable(sub, app, out);
        }
    }
}

/// Visits every item of the current page and its submenus with the pad alone: each item
/// gets the focus in turn (down wraps back to the first), submenus are entered and left
/// again, and every toggle, slider and choice is changed and changed back.
fn explore(h: &mut Harness, visited: &mut Vec<(Page, usize)>, changed: &mut usize) {
    let page = h.page();
    let len = h.ui().items().len();
    for i in 0..len {
        assert_eq!((h.page(), h.focus()), (page, i));
        visited.push((page, i));
        let before = h.item(i);
        match before.widget {
            Widget::Submenu(sub) => {
                h.pad(PadButton::South);
                assert_eq!(h.page(), sub);
                explore(h, visited, changed);
                h.pad(PadButton::East);
                assert_eq!((h.page(), h.focus()), (page, i), "back where we were");
            }
            Widget::Toggle(_) | Widget::Slider { .. } | Widget::Choice { .. } => {
                // Right, or Left when already at the maximum, changes it; the other way
                // undoes it.
                let (there, back) = match before.widget {
                    Widget::Slider { value, max, .. } if value >= max => {
                        (PadButton::DpadLeft, PadButton::DpadRight)
                    }
                    _ => (PadButton::DpadRight, PadButton::DpadLeft),
                };
                h.pad(there);
                assert_ne!(h.item(i).widget, before.widget, "{page:?} {}", before.label);
                h.pad(back);
                // Sliders come back onto their step grid (the clock may have run off it).
                match (h.item(i).widget, before.widget) {
                    (Widget::Slider { value: a, step, .. }, Widget::Slider { value: b, .. }) => {
                        assert!((a - b).abs() <= step * 0.5, "{page:?} {}", before.label);
                    }
                    (after, _) => assert_eq!(after, before.widget, "{page:?} {}", before.label),
                }
                *changed += 1;
            }
            Widget::Action | Widget::Capture { .. } => {}
        }
        h.pad(PadButton::DpadDown);
    }
    if len > 0 {
        assert_eq!(h.focus(), 0, "{page:?}: focus wraps");
    }
}

#[test]
fn gamepad_alone_reaches_every_item_and_changes_every_value() {
    let mut h = Harness::new();
    let ticks = h.app.ticks();
    assert!(ticks > 0);
    assert!(!h.app.menu_open());
    h.pad(PadButton::Start);
    assert_eq!(h.page(), Page::Pause);
    let paused_at = h.app.ticks();
    let bindings = h.app.bindings().clone();
    let settings = *h.app.settings();

    let mut visited = Vec::new();
    let mut changed = 0;
    explore(&mut h, &mut visited, &mut changed);

    let mut expected = Vec::new();
    reachable(Page::Pause, &h.app, &mut expected);
    assert_eq!(visited, expected);
    // Pause, Settings, Controls, 2 × (device, bindings, axes, 8 axes), Video, World, About.
    let mut pages: Vec<Page> = visited.iter().map(|v| v.0).collect();
    pages.dedup();
    assert!(pages.len() >= 25, "{}", pages.len());
    assert!(visited.len() > 150, "{}", visited.len());
    // Toggles, sliders and choices on the device, axis, video and world pages.
    assert!(changed >= 2 * (3 + 8 * 4) + 4 + 2, "{changed}");
    // Everything was put back, and the simulation stood still throughout.
    assert_eq!(h.app.bindings(), &bindings);
    assert_eq!(h.app.settings(), &settings);
    assert_eq!(h.app.ticks(), paused_at);

    // Back on the pause page closes the menu, and the simulation runs again.
    h.pad(PadButton::East);
    assert!(!h.app.menu_open());
    h.frame();
    assert!(h.app.ticks() > paused_at);
}

#[test]
fn invert_y_toggle_flips_gamepad_look_and_persists() {
    let store = SharedStore::default();
    let mut h = Harness::with_store(&store);
    let pad = DeviceKind::Gamepad;
    h.open(&[
        ItemId::Open(Page::Settings),
        ItemId::Open(Page::Controls),
        ItemId::Open(Page::Device(pad)),
    ]);
    assert_eq!(h.item(0).id, ItemId::InvertLookY(pad));
    assert_eq!(h.item(0).label, "Invert look Y");
    assert_eq!(h.item(0).widget, Widget::Toggle(false));
    assert!(h.hud_text().contains("Invert look Y"));
    h.pad(PadButton::South);
    assert_eq!(h.item(0).widget, Widget::Toggle(true));
    assert!(h.app.bindings().axis_options(pad, Action::LookY).invert);
    assert!(h.app.bindings().axis_options(pad, Action::FlyLookY).invert);
    assert!(!h.app.bindings().axis_options(pad, Action::LookX).invert);
    assert!(
        !h.app
            .bindings()
            .axis_options(DeviceKind::KeyboardMouse, Action::LookY)
            .invert
    );

    // The mapper has it: pushing the right stick down now looks up.
    h.input.axis(RawInput::PadAxis(PadAxis::RightY), 0.8);
    assert!(h.input.sample(0.0).axis(Action::LookY) < -0.5);
    assert!(h.input.state().axis(Action::FlyLookY) < -0.5);
    h.input.axis(RawInput::PadAxis(PadAxis::RightY), 0.0);

    // Leaving the menu saves; the JSON round-trips into a fresh app and its mapper.
    assert!(store.load().is_none());
    h.pad(PadButton::Start);
    assert!(!h.app.menu_open());
    let saved = store.load().expect("saved on close");
    let (_, bindings) = Settings::from_json(&saved);
    assert!(bindings.unwrap().axis_options(pad, Action::LookY).invert);

    let mut again = Harness::with_store(&store);
    assert!(again.app.bindings().axis_options(pad, Action::LookY).invert);
    again.input.axis(RawInput::PadAxis(PadAxis::RightY), 0.8);
    again.frame();
    assert!(again.input.state().axis(Action::LookY) < -0.5);
}

#[test]
fn remap_capture_resolves_conflicts_both_ways() {
    let mut h = Harness::new();
    let pad = DeviceKind::Gamepad;
    let btn = |b| Binding::new(RawInput::PadButton(b), Sign::Positive);
    h.open(&[
        ItemId::Open(Page::Settings),
        ItemId::Open(Page::Controls),
        ItemId::Open(Page::Device(pad)),
        ItemId::Open(Page::Bindings(pad)),
    ]);
    let horn = ItemId::Bind(pad, Action::Horn, Sign::Full);
    h.focus_on(horn);
    assert!(h.hud_text().contains("LS click"), "current binding shown");
    let before = h.app.bindings().clone();

    // Y is the camera button: binding it to the horn asks first.
    h.pad(PadButton::South);
    assert!(h.ui().capture().is_some());
    assert!(h.hud_text().contains("Press the new input for"));
    h.pad(PadButton::North);
    assert!(h.ui().capture().is_none());
    assert_eq!(h.page(), Page::Conflict);
    let c = h.ui().conflict().unwrap().conflict;
    assert_eq!((c.existing, c.wanted), (Action::CameraCycle, Action::Horn));
    assert!(h.hud_text().contains("Y is already bound to Camera."));
    assert!(h.hud_text().contains("Keep on Camera"));
    assert!(h.hud_text().contains("Move to Horn"));
    assert_eq!(h.app.bindings(), &before, "nothing changes while asking");

    // Keep it where it is.
    assert_eq!(h.item(h.focus()).id, ItemId::KeepExisting);
    h.pad(PadButton::South);
    assert_eq!(h.page(), Page::Bindings(pad));
    assert_eq!(h.item(h.focus()).id, horn);
    assert_eq!(h.app.bindings(), &before);
    assert!(matches!(
        h.ui().toast(),
        Some(Toast::Kept(_, Action::CameraCycle))
    ));

    // Again, and move it: Y becomes the horn (replacing the stick click) and leaves the
    // camera.
    h.pad(PadButton::South);
    h.pad(PadButton::North);
    assert_eq!(h.page(), Page::Conflict);
    h.pad(PadButton::DpadDown);
    assert_eq!(h.item(h.focus()).id, ItemId::UseNew);
    h.pad(PadButton::South);
    assert_eq!(h.page(), Page::Bindings(pad));
    assert_eq!(
        h.app.bindings().get(pad, Action::Horn),
        &[btn(PadButton::North)]
    );
    assert!(h.app.bindings().get(pad, Action::CameraCycle).is_empty());
    assert!(matches!(
        h.ui().toast(),
        Some(Toast::Bound(_, Action::Horn, _))
    ));
    // The mapper follows.
    h.input.press(RawInput::PadButton(PadButton::North));
    assert!(h.input.sample(0.0).pressed(Action::Horn));
    assert!(!h.input.state().pressed(Action::CameraCycle));
    h.input.release(RawInput::PadButton(PadButton::North));

    // Back on the pad cancels a capture; so does waiting.
    h.pad(PadButton::South);
    h.pad(PadButton::East);
    assert!(h.ui().capture().is_none());
    assert_eq!(h.page(), Page::Bindings(pad));
    assert_eq!(h.ui().toast(), Some(Toast::Cancelled));
    h.pad(PadButton::South);
    for _ in 0..(CAPTURE_TIMEOUT as usize * 60 + 10) {
        h.frame();
    }
    assert!(h.ui().capture().is_none());
    assert!(!h.input.is_capturing());

    // A key while remapping the pad is refused, and the capture keeps waiting.
    h.pad(PadButton::South);
    h.key(Key::KeyJ);
    assert!(h.ui().capture().is_some());
    assert_eq!(h.ui().toast(), Some(Toast::WrongDevice(pad)));
    // A free button binds straight away.
    h.pad(PadButton::West);
    assert_eq!(
        h.app.bindings().get(pad, Action::Horn),
        &[btn(PadButton::West)]
    );

    // Steering takes a whole stick axis, whichever row is remapped; the right stick's X
    // is looking, so that asks first.
    h.focus_on(ItemId::Bind(pad, Action::Steer, Sign::Negative));
    h.pad(PadButton::South);
    h.stick(PadAxis::RightX, -0.9);
    assert_eq!(h.page(), Page::Conflict);
    assert_eq!(h.ui().conflict().unwrap().conflict.existing, Action::LookX);
    h.pad(PadButton::DpadDown);
    h.pad(PadButton::South);
    assert!(h.app.bindings().get(pad, Action::LookX).is_empty());
    assert_eq!(
        h.app.bindings().get(pad, Action::Steer),
        &[Binding::new(RawInput::PadAxis(PadAxis::RightX), Sign::Full)]
    );
}

#[test]
fn keyboard_remap_replaces_one_direction() {
    let mut h = Harness::new();
    let kb = DeviceKind::KeyboardMouse;
    h.key(Key::Escape);
    assert_eq!(h.page(), Page::Pause);
    for id in [
        ItemId::Open(Page::Settings),
        ItemId::Open(Page::Controls),
        ItemId::Open(Page::Device(kb)),
        ItemId::Open(Page::Bindings(kb)),
    ] {
        let target = h.ui().items().iter().position(|i| i.id == id).unwrap();
        while h.focus() != target {
            h.key(Key::ArrowDown);
        }
        h.key(Key::Enter);
    }
    let target = h
        .ui()
        .items()
        .iter()
        .position(|i| i.id == ItemId::Bind(kb, Action::Steer, Sign::Negative))
        .unwrap();
    while h.focus() != target {
        h.key(Key::ArrowDown);
    }
    h.key(Key::Enter);
    h.key(Key::KeyJ);
    let key = |k, s| Binding::new(RawInput::Key(k), s);
    assert_eq!(
        h.app.bindings().get(kb, Action::Steer),
        &[
            key(Key::KeyD, Sign::Positive),
            key(Key::ArrowRight, Sign::Positive),
            key(Key::KeyJ, Sign::Negative),
        ]
    );
    // Esc goes back a page at a time, then closes.
    let depth = h.ui().menu().unwrap().depth();
    for _ in 0..depth {
        assert!(h.app.menu_open());
        h.key(Key::Escape);
    }
    assert!(!h.app.menu_open());
}

#[test]
fn holding_right_repeats_on_a_slider() {
    let mut h = Harness::new();
    h.open(&[ItemId::Open(Page::Settings), ItemId::Open(Page::Video)]);
    h.focus_on(ItemId::UiScale);
    let right = RawInput::PadButton(PadButton::DpadRight);
    h.input.press(right);
    for _ in 0..(0.6 / DT) as usize {
        h.frame();
    }
    h.input.release(right);
    h.frame();
    // One step on the press, then repeats after the delay: 1.0 → 2.0 (clamped).
    assert_eq!(h.app.settings().ui_scale, 2.0);
    assert_eq!(h.app.scene_view(0.0, 1.0).hud.ui_scale, 2.0);
}

#[test]
fn world_page_drives_the_clock() {
    let mut h = Harness::new();
    h.open(&[ItemId::Open(Page::Settings), ItemId::Open(Page::World)]);
    // A quarter hour later, on the slider's 15-minute grid.
    let want = (h.app.time.clock.seconds_of_day / 900.0).round() * 900.0 + 900.0;
    h.focus_on(ItemId::TimeOfDay);
    h.pad(PadButton::DpadRight);
    assert_eq!(h.app.time.clock.seconds_of_day, want);
    // Paused: the clock does not run on its own.
    for _ in 0..60 {
        h.frame();
    }
    assert_eq!(h.app.time.clock.seconds_of_day, want);
    h.focus_on(ItemId::ClockSpeed);
    h.pad(PadButton::DpadRight);
    assert_eq!(h.app.settings().clock_speed, 60.0);
    h.pad(PadButton::Start);
    // Running again at 60x: a frame's sim steps move it 60 game seconds per real second.
    let t = h.app.time.clock.seconds_of_day;
    let ticks = h.app.ticks();
    h.frame();
    let steps = (h.app.ticks() - ticks) as f64;
    assert!(steps > 0.0);
    let moved = h.app.time.clock.seconds_of_day - t;
    assert!((moved - steps * SIM_DT * 60.0).abs() < 1e-6, "{moved}");
    // The sun follows the clock.
    let d = h.app.time.daylight();
    assert_eq!(h.app.scene_view(0.0, 1.0).sun.direction, d.sun_dir);
}

#[test]
fn hud_shows_speed_time_and_the_overlay() {
    let mut h = Harness::new();
    let text = h.hud_text();
    // The car at rest, at the default golden hour.
    assert!(text.contains("mph"), "{text}");
    assert!(text.contains("17:40"), "{text}");
    assert!(!text.contains("--") && !text.contains("draws"), "{text}");
    // The free-fly camera has no car: the speed is a placeholder.
    h.app.set_fly(true);
    h.frame();
    assert!(h.hud_text().contains("--"));
    h.app.set_fly(false);
    // Full throttle on the right trigger: the HUD follows the car.
    h.input.axis(RawInput::PadAxis(PadAxis::R2), 1.0);
    for _ in 0..120 {
        h.frame();
    }
    h.input.axis(RawInput::PadAxis(PadAxis::R2), 0.0);
    let mph = h.app.hud_state().speed_mps.abs() * MPH_PER_MPS;
    assert!(mph > 5.0, "{mph}");
    assert!(h.hud_text().contains(&format!("{mph:.0}")));

    // F3 toggles the overlay, which shows the renderer's counters and a frame graph.
    h.app.set_render_stats(RenderStats {
        draw_calls: 123,
        instances: 45,
        ..RenderStats::default()
    });
    h.key(Key::F3);
    let text = h.hud_text();
    assert!(text.contains("draws 123"), "{text}");
    assert!(text.contains("instances 45"), "{text}");
    assert!(text.contains("fps"));
    assert!(text.contains("steps/frame"));
    assert!(text.contains("sun ") && text.contains("cam "));
    let rects = h
        .app
        .scene_view(0.0, 1.0)
        .hud
        .items
        .iter()
        .filter(|i| matches!(i, render::HudItem::Rect { .. }))
        .count();
    assert!(rects >= FRAME_HISTORY, "graph bars: {rects}");
    assert!(h.app.settings().debug_overlay);
    h.key(Key::F3);
    assert!(!h.hud_text().contains("draws"));
}

#[test]
fn quit_is_offered_only_where_it_works() {
    let mut h = Harness::new();
    h.pad(PadButton::Start);
    assert!(h.ui().items().iter().any(|i| i.id == ItemId::Quit));
    h.focus_on(ItemId::Quit);
    h.pad(PadButton::South);
    assert!(h.app.quit_requested());

    let mut web = Harness::new();
    web.app.set_can_quit(false);
    web.pad(PadButton::Start);
    assert!(web.ui().items().iter().all(|i| i.id != ItemId::Quit));
    // Resume closes the menu.
    web.pad(PadButton::South);
    assert!(!web.app.menu_open());
}

#[test]
fn open_settings_starts_on_the_settings_page() {
    let mut h = Harness::new();
    h.app.open_settings();
    h.frame();
    assert_eq!(h.page(), Page::Settings);
    assert!(h.hud_text().contains("Controls"));
    h.pad(PadButton::East);
    assert_eq!(h.page(), Page::Pause);
}

#[test]
fn a_scene_time_stays_frozen_under_the_clock_speed_setting() {
    let setup = crate::SceneSetup::from_query("?t=22:30&stream=0").unwrap();
    let mut app = App::from_setup(&setup).unwrap();
    app.set_settings_store(Box::new(SharedStore::default()));
    let mut input = InputMapper::default();
    let t = app.time.clock.seconds_of_day;
    for _ in 0..120 {
        app.frame(&mut input, DT, &crate::tests::wall_clock);
    }
    assert_eq!(app.time.clock.seconds_of_day, t);
    assert_eq!(app.time.clock.speed, 0.0);

    // Without a scene time the setting drives the clock.
    let mut h = Harness::with_store(&SharedStore::default());
    let t = h.app.time.clock.seconds_of_day;
    for _ in 0..120 {
        h.frame();
    }
    assert!(h.app.time.clock.seconds_of_day > t);
}

/// The HUD's map quads this frame: (minimap circles, full-map rounded rects).
fn map_quads(h: &mut Harness) -> (usize, usize) {
    let mut out = (0, 0);
    for item in h.app.scene_view(0.0, 16.0 / 9.0).hud.items {
        if let render::HudItem::Image(i) = item {
            match (i.texture, i.mask) {
                (render::HudTexture::Map, render::HudMask::Circle) => out.0 += 1,
                (render::HudTexture::Map, render::HudMask::Rounded(_)) => out.1 += 1,
                _ => {}
            }
        }
    }
    out
}

/// The first image item showing `texture`.
fn first_image(h: &mut Harness, texture: render::HudTexture) -> render::HudImage {
    h.app
        .scene_view(0.0, 1.0)
        .hud
        .items
        .iter()
        .find_map(|i| match i {
            render::HudItem::Image(m) if m.texture == texture => Some(*m),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no {texture:?} image"))
}

#[test]
fn minimap_shows_while_driving_and_hides_under_the_menu() {
    let mut h = Harness::new();
    assert_eq!(map_quads(&mut h), (1, 0));
    // The car arrow sits at the circle's center, pointing up while the map turns with it.
    let circle = first_image(&mut h, render::HudTexture::Map);
    let arrow = first_image(&mut h, render::HudTexture::Arrow);
    let mid = |x: f32, w: f32| x + w * 0.5;
    assert!((mid(arrow.x, arrow.w) - mid(circle.x, circle.w)).abs() < 1e-3);
    assert!((mid(arrow.y, arrow.h) - mid(circle.y, circle.h)).abs() < 1e-3);
    assert_eq!(arrow.rotation, 0.0);
    // The car faces east: the map turns a quarter left so east is up.
    assert!((circle.rotation + core::f32::consts::FRAC_PI_2).abs() < 1e-3);
    assert_eq!(circle.w, crate::map::view::MINIMAP_DIAMETER);
    // Bottom-left, inside the safe area.
    let [sx, sy, _, sh] = HudLayout::safe_rect(1.0);
    assert!(circle.x > sx && circle.y + circle.h < sy + sh);
    assert!(circle.x < sx + 10.0 && circle.y + circle.h > sy + sh - 10.0);
    assert!(h.hud_text().contains('N'));

    h.pad(PadButton::Start);
    assert_eq!(map_quads(&mut h), (0, 0));
    h.pad(PadButton::Start);
    assert_eq!(map_quads(&mut h), (1, 0));
}

#[test]
fn hud_page_switches_the_minimap_and_its_zoom() {
    let store = SharedStore::default();
    let mut h = Harness::with_store(&store);
    h.open(&[ItemId::Open(Page::Settings), ItemId::Open(Page::Hud)]);
    assert_eq!(h.page(), Page::Hud);
    // Rotate off: north-up; zoom 150 -> 250 m.
    h.focus_on(ItemId::MinimapRotate);
    h.pad(PadButton::DpadRight);
    h.focus_on(ItemId::MinimapZoom);
    h.pad(PadButton::DpadRight);
    assert!(!h.app.settings().minimap_rotate);
    assert_eq!(h.app.settings().minimap_range_m, 250.0);
    h.pad(PadButton::Start);
    let saved = store.0.borrow().clone().expect("saved on close");
    let (s, _) = Settings::from_json(&saved);
    assert!(!s.minimap_rotate && s.minimap_range_m == 250.0);
    // North-up: the map does not turn; the arrow does (the car faces east).
    assert_eq!(first_image(&mut h, render::HudTexture::Map).rotation, 0.0);
    let arrow = first_image(&mut h, render::HudTexture::Arrow);
    assert!((arrow.rotation - core::f32::consts::FRAC_PI_2).abs() < 1e-3);
    // Minimap off.
    h.open(&[ItemId::Open(Page::Settings), ItemId::Open(Page::Hud)]);
    h.focus_on(ItemId::Minimap);
    h.pad(PadButton::South);
    assert!(!h.app.settings().minimap);
    h.pad(PadButton::Start);
    assert_eq!(map_quads(&mut h), (0, 0));
}

#[test]
fn map_toggle_opens_the_full_map_without_pausing() {
    let mut h = Harness::new();
    // A Select (Back) tap opens it; the sim keeps running under it.
    h.pad(PadButton::Select);
    assert!(h.app.ui().full_map_open());
    assert!(!h.app.menu_open());
    assert_eq!(map_quads(&mut h), (0, 1));
    assert!(h.hud_text().contains("Map") && h.hud_text().contains("17:40"));
    let ticks = h.app.ticks();
    h.frame();
    assert!(h.app.ticks() > ticks);
    // The throttle does not reach the car while the map is up.
    h.input.axis(RawInput::PadAxis(PadAxis::R2), 1.0);
    for _ in 0..30 {
        h.frame();
    }
    h.input.axis(RawInput::PadAxis(PadAxis::R2), 0.0);
    assert!(h.app.hud_state().speed_mps.abs() < 0.5);
    // R1 zooms in, the stick pans, L1 zooms back out.
    let full = |h: &Harness| h.app.ui().map().full;
    let start = full(&h);
    assert_eq!(start.zoom, 1.0);
    h.input.press(RawInput::PadButton(PadButton::R1));
    for _ in 0..60 {
        h.frame();
    }
    h.input.release(RawInput::PadButton(PadButton::R1));
    assert!(full(&h).zoom > 2.0, "{}", full(&h).zoom);
    h.input.axis(RawInput::PadAxis(PadAxis::LeftX), 1.0);
    for _ in 0..20 {
        h.frame();
    }
    h.input.axis(RawInput::PadAxis(PadAxis::LeftX), 0.0);
    h.frame();
    assert!(full(&h).center[0] > start.center[0] + 10.0);
    h.input.press(RawInput::PadButton(PadButton::L1));
    for _ in 0..120 {
        h.frame();
    }
    h.input.release(RawInput::PadButton(PadButton::L1));
    h.frame();
    assert_eq!(full(&h).zoom, 1.0);
    // Back (B) closes it: no menu opens.
    h.pad(PadButton::East);
    assert!(!h.app.ui().full_map_open() && !h.app.menu_open());
    assert_eq!(map_quads(&mut h), (1, 0));
    // M toggles it on the keyboard and the wheel zooms; Esc closes it without the menu.
    h.key(Key::KeyM);
    assert!(h.app.ui().full_map_open());
    h.input.axis(RawInput::MouseWheel, 4.0);
    h.frame();
    assert_eq!(full(&h).zoom, 2.0);
    h.key(Key::Escape);
    assert!(!h.app.ui().full_map_open() && !h.app.menu_open());
    h.key(Key::KeyM);
    h.key(Key::KeyM);
    assert!(!h.app.ui().full_map_open());
    // With the menu open the map stays shut.
    h.pad(PadButton::Start);
    h.pad(PadButton::Select);
    assert!(h.app.menu_open() && !h.app.ui().full_map_open());
}

#[test]
fn scene_keys_open_the_map_and_scale_the_ui() {
    let setup = crate::SceneSetup::from_query("?map=1&ui=1.5&still=1").unwrap();
    let mut app = App::from_setup(&setup).unwrap();
    let store = SharedStore::default();
    *store.0.borrow_mut() = Some(Settings::default().to_json(&Bindings::default()));
    app.set_settings_store(Box::new(store));
    assert!(app.ui().full_map_open());
    assert_eq!(app.settings().ui_scale, 1.5);
    let mut input = InputMapper::default();
    app.frame(&mut input, DT, &crate::tests::wall_clock);
    assert_eq!(app.scene_view(1.0, 1.0).hud.ui_scale, 1.5);
}

#[test]
fn full_map_labels_points_of_interest_in_view() {
    let mut h = Harness::new();
    let mut place = place::Place::test_grid();
    place.pois.push(place::Poi {
        pos: [-300.0, -300.0],
        kind: place::PoiKind::Diner,
        name_seed: 1,
    });
    let image = crate::map::MapImage::rasterize(&place);
    let labels = crate::map::labels(&place);
    let name = labels[0].name.clone();
    assert_eq!(name, crate::map::names::poi_name(place::PoiKind::Diner, 1));
    h.app
        .ui
        .set_map(crate::map::view::MapGeom::of(&image), labels);
    h.frame();
    assert!(!h.hud_text().contains(&name), "no labels on the minimap");
    h.key(Key::KeyM);
    assert!(h.hud_text().contains(&name));
    // Zoomed in on the middle of town, the corner diner is out of view.
    h.input.axis(RawInput::MouseWheel, 8.0);
    h.frame();
    assert!(!h.hud_text().contains(&name));
}
