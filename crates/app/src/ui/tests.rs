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
    let settings = h.app.settings().clone();

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
    // Choosing a preset (and back) makes the automatic one an explicit choice.
    let explicit = Settings {
        quality: Some(Quality::Medium),
        ..settings
    };
    assert_eq!(h.app.settings(), &explicit);
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
        backend: "WebGL2",
        draw_calls: 123,
        instances: 45,
        ..RenderStats::default()
    });
    h.key(Key::F3);
    let text = h.hud_text();
    assert!(text.contains("WebGL2  draws 123"), "{text}");
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

#[test]
fn drive_page_switches_the_handling_and_persists() {
    let store = SharedStore::default();
    let mut h = Harness::with_store(&store);
    assert_eq!(
        h.app.drive.assists,
        vehicle::Assists::ARCADE,
        "arcade by default"
    );
    h.open(&[ItemId::Open(Page::Settings), ItemId::Open(Page::Drive)]);
    assert_eq!(h.item(0).id, ItemId::Handling);
    h.pad(PadButton::DpadRight);
    assert_eq!(h.app.settings().handling, Handling::Light);
    assert_eq!(h.app.drive.assists, vehicle::Assists::LIGHT);
    h.pad(PadButton::DpadRight);
    assert_eq!(h.app.drive.assists, vehicle::Assists::OFF);
    h.pad(PadButton::Start);
    let again = Harness::with_store(&store);
    assert_eq!(again.app.settings().handling, Handling::Off);
    assert_eq!(again.app.drive.assists, vehicle::Assists::OFF);
}

#[test]
fn camera_page_sets_the_cockpit_and_the_camera_is_remembered() {
    let store = SharedStore::default();
    let mut h = Harness::with_store(&store);
    assert_eq!(h.app.hud_state().camera_mode, CameraMode::ChaseMid);
    h.open(&[ItemId::Open(Page::Settings), ItemId::Open(Page::Camera)]);
    // Camera: chase mid -> chase near, applied at once.
    h.pad(PadButton::DpadLeft);
    assert_eq!(h.app.settings().camera, CameraMode::ChaseNear);
    assert_eq!(h.app.camera.selected(), CameraMode::ChaseNear);
    // Cockpit FOV 62 -> 64, seat 2 cm up, hood only, no shake.
    h.focus_on(ItemId::CockpitFov);
    h.pad(PadButton::DpadRight);
    h.pad(PadButton::DpadRight);
    h.focus_on(ItemId::SeatHeight);
    h.pad(PadButton::DpadRight);
    h.pad(PadButton::DpadRight);
    h.focus_on(ItemId::Dashboard);
    h.pad(PadButton::DpadRight);
    h.focus_on(ItemId::CameraShake);
    h.pad(PadButton::South);
    let s = h.app.settings().clone();
    assert_eq!(
        (
            s.cockpit_fov_deg,
            s.seat_height_cm,
            s.dashboard,
            s.camera_shake
        ),
        (64.0, 2.0, false, false)
    );
    assert!(h.hud_text().contains("+2 cm"), "{}", h.hud_text());
    let prefs = h.app.camera.prefs;
    assert_eq!(prefs.cockpit_fov_deg, 64.0);
    assert!((prefs.seat_up_m - 0.02).abs() < 1e-6 && !prefs.shake);
    h.pad(PadButton::Start);

    // Cycling while driving is saved at once (no menu involved).
    h.key(Key::KeyC);
    assert_eq!(h.app.hud_state().camera_mode, CameraMode::ChaseMid);
    h.key(Key::KeyC);
    h.key(Key::KeyC);
    assert_eq!(h.app.hud_state().camera_mode, CameraMode::Cockpit);
    let (saved, _) = Settings::from_json(&store.load().expect("saved"));
    assert_eq!(saved.camera, CameraMode::Cockpit);
    // The cockpit draws its walls but not the dashboard (hood only), at the set FOV.
    let ids = h.app.car_meshes.lods[0];
    let view = h.app.scene_view(1.0, 16.0 / 9.0);
    let fov = 2.0 * (1.0 / view.camera.proj[1][1]).atan();
    assert!(
        (fov.to_degrees() - 64.0).abs() < 1e-3,
        "{}",
        fov.to_degrees()
    );
    let meshes: Vec<_> = view.instances.iter().map(|i| i.mesh).collect();
    assert!(meshes.contains(&ids.shell_inside) && !meshes.contains(&ids.interior));

    // The next session starts in the cockpit with the same settings...
    let again = Harness::with_store(&store);
    assert_eq!(again.app.hud_state().camera_mode, CameraMode::Cockpit);
    assert_eq!(again.app.settings(), h.app.settings());
    // ...unless the scene names a camera.
    let setup = crate::SceneSetup::from_query("cam=chase-far&stream=0").unwrap();
    let mut scene = App::from_setup(&setup).unwrap();
    scene.set_settings_store(Box::new(store.clone()));
    assert_eq!(scene.hud_state().camera_mode, CameraMode::ChaseFar);
}

#[test]
fn the_sound_follows_the_car_and_the_audio_page() {
    let store = SharedStore::default();
    let mut h = Harness::with_store(&store);
    let p = *h.app.audio_params();
    assert_eq!(p.engine.profile, audio::synth::EngineProfile::V8Flatplane);
    assert!(!p.paused && !p.muted && !p.horn);
    let idle = p.engine.rpm;
    // Throttle revs it; the horn key sounds the horn.
    h.input.axis(RawInput::PadAxis(PadAxis::R2), 1.0);
    h.input.press(RawInput::Key(Key::KeyH));
    for _ in 0..60 {
        h.frame();
    }
    let p = *h.app.audio_params();
    assert!(
        p.engine.rpm > idle + 500.0 && p.engine.throttle > 0.9,
        "{p:?}"
    );
    assert!(p.horn && p.speed_mps > 1.0);
    h.input.axis(RawInput::PadAxis(PadAxis::R2), 0.0);
    h.input.release(RawInput::Key(Key::KeyH));
    // Audio page: the master volume down a step, mute on; a menu pauses the sound.
    h.open(&[ItemId::Open(Page::Settings), ItemId::Open(Page::Audio)]);
    assert!(h.app.audio_params().paused);
    h.focus_on(ItemId::MasterVolume);
    h.pad(PadButton::DpadLeft);
    h.focus_on(ItemId::Mute);
    h.pad(PadButton::South);
    let p = *h.app.audio_params();
    assert!((p.master - 0.75).abs() < 1e-6 && p.muted);
    h.pad(PadButton::Start);
    assert!(!h.app.audio_params().paused);
    let saved = store.0.borrow().clone().expect("saved on close");
    let (s, _) = Settings::from_json(&saved);
    assert!(s.mute && (s.master_volume - 0.75).abs() < 1e-6);
}

/// Mesh ids one registration of the built-in car `id` takes.
fn car_mesh_count(id: &str) -> usize {
    let spec = crate::vehicles::builtin_spec(id).unwrap();
    let shape = crate::vehicles::builtin_shape(&spec).unwrap();
    let model = crate::car::CarModel::build(&spec, &shape).unwrap();
    let mut n = 0;
    model.register(|_| {
        n += 1;
        render::MeshId(n - 1)
    });
    n as usize
}

/// The car's ground point `[x, z]` and heading (sim convention).
fn spot(app: &App) -> ([f32; 2], f32) {
    let pose = app.drive.pose(1.0);
    let f = pose.forward();
    ([pose.pos.x, pose.pos.z], (-f.x).atan2(-f.z))
}

/// The meshes one more paint of a car registers (its bodies and cabin walls).
fn paint_mesh_count(id: &str) -> usize {
    let spec = crate::vehicles::builtin_spec(id).unwrap();
    let shape = crate::vehicles::builtin_shape(&spec).unwrap();
    let model = crate::car::CarModel::build(&spec, &shape).unwrap();
    let base = model.register(|_| render::MeshId(0));
    let mut n = 0;
    model.register_paint(&base, |_| {
        n += 1;
        render::MeshId(n - 1)
    });
    n as usize
}

/// The Vehicle page's car and paint choosers' indices.
fn browsed(h: &Harness) -> (usize, usize) {
    let index = |i: usize| match h.item(i).widget {
        Widget::Choice { index, .. } => index,
        w => panic!("{w:?}"),
    };
    (index(0), index(1))
}

/// The turntable's meshes in a fresh scene view: its instances come last, the disc last of
/// them, and none casts a shadow or takes a street lamp.
fn turntable(h: &mut Harness) -> Vec<render::MeshId> {
    h.app.scene_view(1.0, 16.0 / 9.0);
    let range = h.app.preview_instances();
    let disc = h.app.turntable;
    let view = h.app.scene_view(1.0, 16.0 / 9.0);
    assert_eq!(range.end, view.instances.len());
    let shown = &view.instances[range];
    assert!(shown
        .iter()
        .all(|i| !i.casts_shadow && !i.receives_shadow && i.lights.is_empty()));
    let meshes: Vec<_> = shown.iter().map(|i| i.mesh).collect();
    assert!(meshes.is_empty() || meshes.last() == Some(&disc));
    meshes
}

#[test]
fn vehicle_page_swaps_the_car_in_place_and_remembers_it() {
    let builtin = &crate::vehicles::BUILTIN;
    let store = SharedStore::default();
    let mut h = Harness::with_store(&store);
    assert_eq!(h.app.drive.spec.id, "gannet_gt");
    let gannet_ids = h.app.car_meshes;
    let before = h.app.mesh_count();
    assert_eq!(before, h.app.meshes().len());
    assert_eq!(
        h.app.take_new_meshes().count(),
        0,
        "nothing pending at load"
    );

    // Lights on, the cockpit camera, then 1.5 s flat out and a turn.
    h.key(Key::KeyL);
    h.key(Key::KeyC);
    h.key(Key::KeyC);
    assert_eq!(h.app.hud_state().camera_mode, CameraMode::Cockpit);
    h.input.axis(RawInput::PadAxis(PadAxis::R2), 1.0);
    h.input.axis(RawInput::PadAxis(PadAxis::LeftX), 0.4);
    for _ in 0..90 {
        h.frame();
    }
    h.input.axis(RawInput::PadAxis(PadAxis::R2), 0.0);
    h.input.axis(RawInput::PadAxis(PadAxis::LeftX), 0.0);
    assert!(h.app.drive.state.speed_mps > 5.0);
    assert!(h.app.drive.odometer_m() > 5.0);

    // Pause > Vehicle: the car chooser focused on the car driven, its specs beside it.
    h.open(&[ItemId::Open(Page::Vehicle)]);
    assert_eq!(h.page(), Page::Vehicle);
    let ids: Vec<ItemId> = h.ui().items().iter().map(|i| i.id).collect();
    assert_eq!(ids, [ItemId::VehicleCar, ItemId::Paint, ItemId::Vehicle(0)]);
    assert_eq!((h.focus(), browsed(&h)), (0, (0, 0)));
    let text = h.hud_text();
    let gannet = &roster::roster()[0];
    for line in [&gannet.name, &gannet.stats, &gannet.engine, &gannet.drive] {
        assert!(text.contains(line.as_str()), "{text}");
    }
    assert!(
        text.contains("0-60 mph") && text.contains("5.6 s"),
        "{text}"
    );
    assert!(text.contains("155 mph") && text.contains("1980s"), "{text}");
    // From the cockpit the turntable stands where the cabin was: no car of our own.
    assert_eq!(turntable(&mut h).len(), 8);
    let world = h.app.world_instances;
    let view = h.app.scene_view(1.0, 16.0 / 9.0);
    assert!(view.instances[world..]
        .iter()
        .all(|i| i.mesh != gannet_ids.lods[0].interior));
    let (at, heading) = spot(&h.app);

    // Right: the wagon on the turntable, built and queued once; Select drives it, the menu
    // closing on the new car where the old one was.
    let wagon_meshes = car_mesh_count(builtin[1].0);
    h.pad(PadButton::DpadRight);
    assert_eq!(browsed(&h), (1, 0));
    assert!(h.hud_text().contains("Homestead Wagon"));
    assert_eq!(h.app.mesh_count(), before + wagon_meshes);
    assert_eq!(h.app.drive.spec.id, "gannet_gt", "not yet");
    h.pad(PadButton::South);
    assert!(!h.app.menu_open());
    assert_eq!(h.app.drive.spec.id, builtin[1].0);
    assert_eq!(h.app.ui().vehicle(), 1);
    let (now, now_heading) = spot(&h.app);
    assert!((now[0] - at[0]).abs() < 0.05 && (now[1] - at[1]).abs() < 0.05);
    assert!(
        (now_heading - heading).abs() < 1e-3,
        "{now_heading} {heading}"
    );
    assert!(h.app.drive.state.speed_mps.abs() < 0.5, "too fast to keep");
    assert!(h.app.drive.odometer_m() < 0.5);
    assert!(h.app.drive.headlights);
    assert_eq!(h.app.hud_state().camera_mode, CameraMode::Cockpit);

    // Its meshes are new ids after the old ones, waiting for the renderer; the old car's
    // stay registered (for switching back).
    assert_eq!(h.app.mesh_count(), before + wagon_meshes);
    let ids = h.app.car_meshes;
    assert_eq!(ids.lods[0].body[0], render::MeshId(before as u32));
    assert_eq!(
        ids.dial_ticks,
        render::MeshId((before + wagon_meshes - 1) as u32)
    );
    assert_eq!(h.app.take_new_meshes().count(), wagon_meshes);
    assert_eq!(h.app.take_new_meshes().count(), 0);
    assert!(turntable(&mut h).is_empty(), "the page is closed");
    let view = h.app.scene_view(1.0, 1.5);
    assert!(view.instances.iter().any(|i| i.mesh == ids.lods[0].body[1]));
    assert!(view
        .instances
        .iter()
        .all(|i| i.mesh != gannet_ids.lods[0].body[0]));

    // Its name on the HUD for 2 s.
    assert!(h.hud_text().contains("Homestead Wagon"), "{}", h.hud_text());
    for _ in 0..((VEHICLE_TOAST_SECONDS as f64 / DT) as usize + 2) {
        h.frame();
    }
    assert!(!h.hud_text().contains("Homestead Wagon"));

    // Saved at once.
    let (saved, _) = Settings::from_json(&store.load().expect("saved"));
    assert_eq!((saved.vehicle, saved.paint), (Some("homestead_wagon"), 0));

    // Back to the first car: its meshes again, nothing new.
    h.open(&[ItemId::Open(Page::Vehicle)]);
    assert_eq!(browsed(&h), (1, 0), "starts on the car driven");
    h.pad(PadButton::DpadLeft);
    h.pad(PadButton::South);
    assert_eq!(h.app.drive.spec.id, builtin[0].0);
    assert_eq!(h.app.car_meshes, gannet_ids);
    assert_eq!(h.app.mesh_count(), before + wagon_meshes);
    assert_eq!(h.app.take_new_meshes().count(), 0);
    // Picking the car already driven, from any row, changes nothing.
    h.open(&[ItemId::Open(Page::Vehicle), ItemId::Paint]);
    assert_eq!(h.app.car_meshes, gannet_ids);
    assert!(!h.app.menu_open());

    // The next session starts in the saved car, its meshes queued after the load's.
    let mut h = Harness::with_store(&store);
    h.key(Key::KeyV);
    let mut again = Harness::with_store(&store);
    assert_eq!(again.app.drive.spec.id, builtin[1].0, "V saved the wagon");
    assert_eq!(again.app.ui().vehicle(), 1);
    assert_eq!(again.app.take_new_meshes().count(), wagon_meshes);
    assert_eq!(
        again.app.mesh_count(),
        again.app.meshes().len() + wagon_meshes
    );
    // ...unless the scene names a car.
    let setup =
        crate::SceneSetup::from_query("car=seabreeze_convertible&stream=0&traffic=off").unwrap();
    let mut scene = App::from_setup(&setup).unwrap();
    scene.set_settings_store(Box::new(store.clone()));
    assert_eq!(scene.drive.spec.id, "seabreeze_convertible");
    assert_eq!(scene.take_new_meshes().count(), 0);
    let setup = crate::SceneSetup::from_query("car=gannet_gt&stream=0&traffic=off").unwrap();
    let mut scene = App::from_setup(&setup).unwrap();
    scene.set_settings_store(Box::new(store.clone()));
    assert_eq!(scene.drive.spec.id, "gannet_gt");
}

#[test]
fn the_vehicle_page_cycles_every_car_on_one_turntable() {
    let builtin = &crate::vehicles::BUILTIN;
    let mut h = Harness::new();
    assert!(turntable(&mut h).is_empty(), "driving");
    h.open(&[ItemId::Open(Page::Vehicle)]);
    let before = h.app.mesh_count();
    let new: usize = builtin[1..].iter().map(|(id, _)| car_mesh_count(id)).sum();
    for lap in 0..2 {
        for k in 1..=builtin.len() {
            h.pad(PadButton::DpadRight);
            let i = k % builtin.len();
            assert_eq!(browsed(&h), (i, 0), "lap {lap}");
            let car = &roster::roster()[i];
            let text = h.hud_text();
            assert!(
                text.contains(&car.name) && text.contains(&car.engine),
                "{text}"
            );
            assert!(
                text.contains(&format!("{} of {}", i + 1, builtin.len())),
                "{text}"
            );
            // Exactly one car on the disc: the browsed one's meshes, in its first paint.
            let meshes = turntable(&mut h);
            let spec = crate::vehicles::builtin_spec(builtin[i].0).unwrap();
            let open = crate::vehicles::builtin_shape(&spec)
                .unwrap()
                .roof
                .convertible();
            assert_eq!(meshes.len(), if open { 10 } else { 8 }, "{}", builtin[i].0);
            let ids = h
                .app
                .cars
                .iter()
                .find(|c| c.id == builtin[i].0)
                .unwrap()
                .meshes;
            let lod = ids.lods[0];
            assert_eq!(
                meshes[..7],
                [
                    lod.body[0],
                    lod.glass,
                    lod.lights[0][0],
                    lod.wheel,
                    lod.wheel,
                    lod.wheel,
                    lod.wheel
                ]
            );
            assert_eq!(h.app.drive.spec.id, builtin[0].0, "only looking");
        }
        // Each car registered once, the first lap; the second reuses them all.
        assert_eq!(h.app.mesh_count(), before + new, "lap {lap}");
    }
    assert_eq!(h.app.take_new_meshes().count(), new);
    // Left goes back round the other way.
    h.pad(PadButton::DpadLeft);
    let last = builtin.len() - 1;
    assert_eq!(browsed(&h), (last, 0));
    // The paint row and Drive keep the car; Back leaves without driving it.
    h.pad(PadButton::DpadDown);
    h.pad(PadButton::DpadDown);
    assert_eq!(h.item(2).id, ItemId::Vehicle(last));
    h.pad(PadButton::East);
    assert_eq!(h.page(), Page::Pause);
    assert!(turntable(&mut h).is_empty(), "the page is closed");
    assert_eq!(h.app.drive.spec.id, builtin[0].0);
}

#[test]
fn the_paint_round_trips_through_the_settings_and_the_scene_key() {
    let store = SharedStore::default();
    let mut h = Harness::with_store(&store);
    let regent = roster::index_of("regent_coupe_cabrio").unwrap();
    h.open(&[ItemId::Open(Page::Vehicle)]);
    while browsed(&h).0 != regent {
        h.pad(PadButton::DpadRight);
    }
    // Down to the paint, two to the right: the turntable wears it at once.
    h.pad(PadButton::DpadDown);
    let plain = turntable(&mut h)[0];
    h.pad(PadButton::DpadRight);
    h.pad(PadButton::DpadRight);
    assert_eq!(browsed(&h), (regent, 2));
    let painted = turntable(&mut h);
    assert_ne!(painted[0], plain, "another body");
    let cars = h.app.mesh_count();
    let added = 2 * paint_mesh_count("regent_coupe_cabrio");
    // Browsing back to a paint seen is free.
    h.pad(PadButton::DpadLeft);
    h.pad(PadButton::DpadRight);
    assert_eq!(h.app.mesh_count(), cars);
    h.pad(PadButton::South);
    assert!(!h.app.menu_open());
    assert_eq!(h.app.drive.spec.id, "regent_coupe_cabrio");
    assert_eq!(h.app.ui().vehicle_paint(), 2);
    let ids = h.app.car_meshes;
    assert_eq!(ids.lods[0].body[0], painted[0]);
    assert_eq!(h.app.mesh_count(), cars, "built while browsing");
    assert!(added > 0);

    // Saved with the car, and the next session starts in it.
    let (saved, _) = Settings::from_json(&store.load().expect("saved"));
    assert_eq!(
        (saved.vehicle, saved.paint),
        (Some("regent_coupe_cabrio"), 2)
    );
    let again = Harness::with_store(&store);
    assert_eq!(again.app.drive.spec.id, "regent_coupe_cabrio");
    assert_eq!(again.app.ui().vehicle_paint(), 2);
    // The page opens on it.
    let mut again = again;
    again.open(&[ItemId::Open(Page::Vehicle)]);
    assert_eq!(browsed(&again), (regent, 2));
    // Another car starts in its first paint.
    again.pad(PadButton::DpadRight);
    assert_eq!(browsed(&again).1, 0);

    // The scene key: a car in a paint, the same body as the one picked above.
    let setup = crate::SceneSetup::from_query("car=regent_coupe_cabrio&paint=2&stream=0").unwrap();
    let scene = App::from_setup(&setup).unwrap();
    assert_eq!(scene.ui().vehicle_paint(), 2);
    let body = scene.car_meshes.lods[0].body[0].0 as usize;
    assert_eq!(scene.meshes()[body], {
        let spec = crate::vehicles::builtin_spec("regent_coupe_cabrio").unwrap();
        let shape = crate::vehicles::builtin_shape(&spec).unwrap();
        crate::car::CarModel::build_painted(&spec, &shape, 2)
            .unwrap()
            .lods[0]
            .body[0]
            .clone()
    });
    let setup = crate::SceneSetup::from_query("car=regent_coupe_cabrio&paint=7&stream=0").unwrap();
    assert!(App::from_setup(&setup).is_err(), "seven paints");
    // Without car=, the paint goes to the car the game starts in: the saved Regent here,
    // and it is not saved; past that car's palette, its first paint.
    let saved = SharedStore::default();
    *saved.0.borrow_mut() =
        Some(r#"{ "drive": { "vehicle": "regent_coupe_cabrio", "paint": 2 } }"#.to_owned());
    for (query, paint) in [("paint=5", 5), ("paint=7", 0), ("", 2)] {
        let setup = crate::SceneSetup::from_query(&format!("{query}&stream=0")).unwrap();
        let mut scene = App::from_setup(&setup).unwrap();
        scene.set_settings_store(Box::new(saved.clone()));
        assert_eq!(scene.drive.spec.id, "regent_coupe_cabrio", "{query}");
        assert_eq!(scene.ui().vehicle_paint(), paint, "{query}");
        assert_eq!(Settings::from_json(&saved.load().unwrap()).0.paint, 2);
    }
    // ...and with nothing saved, the default car (eight paints).
    let setup = crate::SceneSetup::from_query("paint=7&stream=0").unwrap();
    let mut scene = App::from_setup(&setup).unwrap();
    scene.set_settings_store(Box::new(SharedStore::default()));
    assert_eq!(
        (scene.drive.spec.id.as_str(), scene.ui().vehicle_paint()),
        ("gannet_gt", 7)
    );
    // A saved paint the car does not have is its first.
    let store = SharedStore::default();
    *store.0.borrow_mut() =
        Some(r#"{ "drive": { "vehicle": "homestead_wagon", "paint": 11 } }"#.to_owned());
    let h = Harness::with_store(&store);
    assert_eq!(h.app.drive.spec.id, "homestead_wagon");
    assert_eq!(h.app.ui().vehicle_paint(), 0);
}

#[test]
fn the_vehicle_page_fits_the_deck_and_a_tv() {
    // The widest rows: the longest name, and the most paints.
    let r = roster::roster();
    let longest = (0..r.len()).max_by_key(|&i| r[i].name.len()).unwrap();
    let most = (0..r.len()).max_by_key(|&i| r[i].paints.len()).unwrap();
    // The Seabreeze and the Appaloosa Convertible tie.
    assert_eq!(r[longest].name.len(), "Seabreeze Convertible".len());
    assert_eq!(r[most].paints.len(), roster::MAX_PAINTS.min(8));
    for target in [(1280, 800), (1920, 1080)] {
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0] {
            let mut h = Harness::new();
            h.app.set_target_size(target.0, target.1);
            h.app.ui.force_ui_scale(scale);
            h.open(&[ItemId::Open(Page::Vehicle)]);
            for car in [most, longest] {
                while browsed(&h).0 != car {
                    h.pad(PadButton::DpadRight);
                }
                // On the paint row, its last swatch chosen, then back on the car row.
                h.pad(PadButton::DpadDown);
                h.pad(PadButton::DpadLeft);
                h.pad(PadButton::DpadUp);
                assert_eq!(browsed(&h), (car, r[car].paints.len() - 1));
                fits(
                    &mut h,
                    &format!("{target:?} x{scale} {}", r[car].name),
                    scale,
                );
            }
        }
    }
}

/// The Vehicle page as drawn now fits the safe area at `scale`: every glyph and box but
/// the shade inside it, the panel left of the stage and only the car's name and number
/// (and their shadows) over it, the rows always shown and the specs as room allows, and the
/// stage (when there is room for it) a sane rectangle on screen with the turntable in it.
fn fits(h: &mut Harness, what: &str, scale: f32) {
    let ui = h.ui();
    let [sx, sy, sw, sh] = render::HudLayout::safe_rect(scale);
    let hud = ui.hud();
    let stage = ui.preview().map(|p| p.2);
    for item in hud.items {
        let (x, y, w, h) = match *item {
            render::HudItem::Text {
                x,
                y,
                scale: s,
                text,
                ..
            } => {
                let t = hud.str(text);
                let w = render::font::text_width(t) as f32 * s;
                let h = render::font::GLYPH_H as f32 * s;
                (x, y, w, h)
            }
            render::HudItem::Rect { color, .. } if color == SHADE_TEST => continue,
            render::HudItem::Rect { x, y, w, h, .. } => (x, y, w, h),
            render::HudItem::Image(i) => (i.x, i.y, i.w, i.h),
        };
        let at = format!("{what}: {item:?}");
        assert!(x >= sx - 0.5 && x + w <= sx + sw + 0.5, "{at}");
        assert!(y >= sy - 0.5 && y + h <= sy + sh + 0.5, "{at}");
        if let Some([stx, sty, ..]) = stage {
            if x < stx {
                assert!(x + w <= stx, "into the stage: {at}");
            } else {
                assert!(y + h <= sty + 4.0, "over the turntable: {at}");
            }
        }
    }
    let text = h.hud_text();
    for want in ["Car", "Paint", "Drive this car"] {
        assert!(text.contains(want), "{what}: {text}");
    }
    if scale <= 1.5 {
        assert!(text.contains("Top speed"), "{what}: {text}");
        assert!(stage.is_some(), "{what}: room for the turntable");
    }
    match stage {
        Some(stage) => {
            let [x0, y0, x1, y1] = h.app.stage_ndc(stage, 16.0 / 9.0);
            let inside = |v: f32| (-1.0..=1.0).contains(&v);
            assert!(
                inside(x0) && inside(x1) && inside(y0) && inside(y1),
                "{what}"
            );
            assert!(
                x1 - x0 > 0.3 && y1 - y0 > 0.6,
                "{what}: {:?}",
                [x0, y0, x1, y1]
            );
            assert!(!turntable(h).is_empty(), "{what}");
        }
        None => assert!(turntable(h).is_empty(), "{what}"),
    }
}

#[test]
fn nothing_near_the_eye_cuts_into_the_turntable() {
    // Busy traffic round the start, then the page: no NPC car or prop instance is left
    // within CLEAR_M of the eye, and the turntable's own instances are all there.
    let setup = crate::SceneSetup::from_query("place=town&t=12:00&traffic=busy&stream=0").unwrap();
    let mut h = Harness {
        app: App::from_setup(&setup).unwrap(),
        input: InputMapper::default(),
    };
    for _ in 0..240 {
        h.frame();
    }
    assert!(h.app.npcs.count() > 0, "traffic is running");
    h.open(&[ItemId::Open(Page::Vehicle)]);
    let n = turntable(&mut h).len();
    assert_eq!(n, 8);
    let world = h.app.world_instances;
    let eye = h.app.camera.pose(&h.app.drive, 1.0).eye;
    let range = h.app.preview_instances();
    let view = h.app.scene_view(1.0, 16.0 / 9.0);
    for inst in &view.instances[world..range.start] {
        let at = glam::Vec3::from_slice(&inst.transform[3][..3]);
        assert!(
            at.distance(eye) > crate::preview::CLEAR_M,
            "{at} by the eye {eye}"
        );
    }
}

/// `draw::SHADE`, the dimming round the menu (it covers the whole target on purpose).
const SHADE_TEST: [u8; 4] = [0, 0, 0, 110];

#[test]
fn v_cycles_through_every_builtin_and_wraps() {
    let builtin = &crate::vehicles::BUILTIN;
    let mut h = Harness::new();
    let before = h.app.mesh_count();
    let new: usize = builtin[1..].iter().map(|(id, _)| car_mesh_count(id)).sum();
    for lap in 0..2 {
        for k in 1..=builtin.len() {
            h.key(Key::KeyV);
            let i = k % builtin.len();
            assert_eq!(h.app.drive.spec.id, builtin[i].0, "lap {lap}");
            assert_eq!(h.app.ui().vehicle(), i);
            let name = &roster::roster()[i].name;
            assert!(h.hud_text().contains(name.as_str()), "{}", h.hud_text());
        }
        // Each car registered once, the first lap; the second reuses them all.
        assert_eq!(h.app.mesh_count(), before + new, "lap {lap}");
    }
    assert_eq!(h.app.take_new_meshes().count(), new);
    // V does nothing under the menu.
    h.pad(PadButton::Start);
    h.key(Key::KeyV);
    assert_eq!(h.app.drive.spec.id, builtin[0].0);
}

#[test]
fn a_slow_roll_survives_the_switch() {
    let mut h = Harness::new();
    let f = h.app.drive.pose(1.0).forward();
    let v = f * 3.0;
    h.app.drive.state.vel = v.to_array();
    h.app.drive.state.speed_mps = 3.0;
    h.app.switch_vehicle("homestead_wagon").unwrap();
    assert_eq!(h.app.drive.state.speed_mps, 3.0);
    assert_eq!(h.app.drive.state.vel, v.to_array());
    assert!(h.app.switch_vehicle("hovercar").is_err());
    assert_eq!(h.app.drive.spec.id, "homestead_wagon");
}

/// Puts the car off the road in a block, tipped over, sliding, with miles on it.
fn strand(app: &mut App) {
    let s = &mut app.drive.state;
    s.pos = [-150.0, 3.0, 47.0];
    s.rot = glam::Quat::from_rotation_z(1.4).to_array();
    s.vel = [2.0, 0.0, -1.0];
    s.speed_mps = 2.0;
    s.odometer_m = 1234.0;
    app.drive.prev = app.drive.state;
}

/// The car is at rest, upright, on a lane's centerline heading along it, with its miles.
fn assert_on_a_lane(app: &App) {
    let (at, heading) = spot(app);
    let hit = app
        .lanes
        .nearest([at[0], 0.0, at[1]], 1.0)
        .expect("on a lane");
    assert!(hit.dist < 0.05, "{hit:?}");
    let lane_heading = (-hit.dir[0]).atan2(-hit.dir[1]);
    let turn = (heading - lane_heading + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
        - std::f32::consts::PI;
    assert!(turn.abs() < 1e-3, "{heading} {lane_heading}");
    let up = app.drive.pose(1.0).rot * glam::Vec3::Y;
    assert!(up.y > 0.999, "{up}");
    // At rest (the sim has stepped since, settling on its springs).
    assert!(app.drive.state.speed_mps.abs() < 0.01);
    assert!(glam::Vec3::from(app.drive.state.vel).length() < 0.05);
    // The miles stay (plus any the stranded car slid before the reset).
    assert!((1234.0..1240.0).contains(&app.drive.odometer_m()));
}

#[test]
fn r_puts_a_stuck_car_back_on_the_nearest_lane() {
    let mut h = Harness::new();
    strand(&mut h.app);
    let p = h.app.drive.state.pos;
    let foot = [p[0], p[1] - h.app.drive.spec.cg_height_m, p[2]];
    let want = h.app.lanes.nearest(foot, 200.0).unwrap();
    h.key(Key::KeyR);
    assert_on_a_lane(&h.app);
    let (at, _) = spot(&h.app);
    assert!((at[0] - want.pos[0]).abs() < 0.05 && (at[1] - want.pos[2]).abs() < 0.05);
    // "Car reset" for 2 s.
    assert!(h.hud_text().contains("Car reset"), "{}", h.hud_text());
    for _ in 0..((VEHICLE_TOAST_SECONDS as f64 / DT) as usize + 2) {
        h.frame();
    }
    assert!(!h.hud_text().contains("Car reset"));
    // Far from every road the car goes back to its start, miles kept.
    h.app.drive.state.pos = [5000.0, 0.0, 5000.0];
    h.app.drive.state.odometer_m = 99.0;
    h.key(Key::KeyR);
    let (at, heading) = spot(&h.app);
    assert!((at[0] - h.app.start.0[0]).abs() < 0.05 && (at[1] - h.app.start.0[2]).abs() < 0.05);
    assert!((heading - h.app.start.1).abs() < 1e-3);
    assert!((h.app.drive.odometer_m() - 99.0).abs() < 0.01);
}

#[test]
fn holding_select_resets_and_a_tap_opens_the_map() {
    let mut h = Harness::new();
    strand(&mut h.app);
    let select = RawInput::PadButton(PadButton::Select);
    // Half a second is not enough.
    h.input.press(select);
    for _ in 0..30 {
        h.frame();
    }
    assert!(!h.hud_text().contains("Car reset"));
    // A second is; letting go then does not open the map.
    for _ in 0..32 {
        h.frame();
    }
    h.input.release(select);
    h.frame();
    assert_on_a_lane(&h.app);
    assert!(h.hud_text().contains("Car reset"));
    assert!(!h.ui().full_map_open());
    // A tap opens the map and resets nothing.
    strand(&mut h.app);
    h.pad(PadButton::Select);
    assert!(h.ui().full_map_open());
    let (at, _) = spot(&h.app);
    assert!(
        (at[0] + 150.0).abs() < 1.0 && (at[1] - 47.0).abs() < 1.0,
        "{at:?}"
    );
}

#[test]
fn the_pause_menu_resets_the_car() {
    let mut h = Harness::new();
    strand(&mut h.app);
    h.open(&[ItemId::ResetCar]);
    assert!(!h.app.menu_open());
    assert_on_a_lane(&h.app);
    assert!(h.hud_text().contains("Car reset"));
}

#[test]
fn reset_is_listed_on_both_bindings_pages() {
    for (device, input) in [
        (DeviceKind::Gamepad, "Select (hold)"),
        (DeviceKind::KeyboardMouse, "R"),
    ] {
        let mut h = Harness::new();
        h.open(&[
            ItemId::Open(Page::Settings),
            ItemId::Open(Page::Controls),
            ItemId::Open(Page::Device(device)),
            ItemId::Open(Page::Bindings(device)),
        ]);
        h.focus_on(ItemId::Bind(device, Action::Reset, Sign::Full));
        let text = h.hud_text();
        let row = text
            .lines()
            .find(|l| l.contains("Reset car"))
            .unwrap_or_else(|| panic!("{text}"));
        assert!(row.contains(input), "{row}");
    }
}

#[test]
fn about_page_credits_the_author() {
    let mut h = Harness::new();
    h.open(&[ItemId::Open(Page::Settings), ItemId::Open(Page::About)]);
    let text = h.hud_text();
    let credit = text.find("Created by Adam Mathes").expect("credit");
    assert!(credit < text.find("BSD-3-Clause").unwrap(), "{text}");
}

#[test]
fn radio_keys_tune_stations_and_the_last_one_comes_back() {
    use audio::{PlayerEvent, RadioCommand, RadioState};
    let store = SharedStore::default();
    let mut h = Harness::with_store(&store);
    let cmds: Vec<_> = h.app.radio_commands().collect();
    // The radio's 0.8 times the master volume's 0.8.
    assert_eq!(cmds, [RadioCommand::Volume(0.8 * 0.8)], "off until a key");
    assert!(h.app.ui().radio().toast().is_none());
    assert_eq!(h.app.audio_params().tunes, 0);
    h.key(Key::BracketRight);
    assert!(h.app.ui().radio().toast().is_some());
    assert!(h.hud_text().contains("Radio Paradise Mellow Mix"));
    // The dial sweeps (in the mixer, at the radio's volume), then the station plays.
    assert_eq!(h.app.audio_params().tunes, 1);
    assert_eq!(h.app.audio_params().tune_volume, 0.8);
    assert!(h
        .app
        .radio_commands()
        .all(|c| !matches!(c, RadioCommand::Play { .. })));
    for _ in 0..((audio::TUNE_SECONDS as f64 / DT) as usize + 1) {
        h.frame();
    }
    let cmds: Vec<_> = h.app.radio_commands().collect();
    let first = h.app.radio().channels()[0].clone();
    assert!(
        matches!(&cmds[..], [RadioCommand::Play { url, .. }] if *url == first.urls[0]),
        "{cmds:?}"
    );
    let saved = store.0.borrow().clone().unwrap();
    let (s, _) = Settings::from_json(&saved);
    assert!(s.radio.on);
    assert_eq!(s.radio.last_station.as_deref(), Some(first.id.as_str()));
    // The engine and the world duck under the station once it plays.
    assert_eq!(h.app.audio_params().radio, 0.0);
    let token = h.app.radio().token();
    h.app.radio_event(PlayerEvent::Started { token });
    h.frame();
    assert_eq!(h.app.audio_params().radio, 0.8);
    // Every URL fails: the station is in error and the dial still turns.
    for _ in &first.urls {
        let token = h.app.radio().token();
        h.app.radio_event(PlayerEvent::Failed { token });
    }
    assert!(matches!(h.app.radio().state(), RadioState::Error { .. }));
    h.key(Key::BracketRight);
    assert_eq!(h.app.radio().station(), Some(1));
    // A new session starts with the radio off, whatever the last one saved; power brings
    // the same station back.
    let mut fresh = App::new(Seed(1));
    fresh.set_settings_store(Box::new(store.clone()));
    let mut input = InputMapper::default();
    fresh.frame(&mut input, DT, &crate::tests::wall_clock);
    assert!(!fresh.radio().is_on());
    assert!(fresh
        .radio_commands()
        .all(|c| !matches!(c, RadioCommand::Play { .. })));
    input.press(RawInput::Key(Key::Backslash));
    fresh.frame(&mut input, DT, &crate::tests::wall_clock);
    input.release(RawInput::Key(Key::Backslash));
    fresh.frame(&mut input, DT, &crate::tests::wall_clock);
    assert!(fresh.radio().is_on());
    assert_eq!(fresh.radio().station(), Some(1));
    // Off is saved too.
    h.key(Key::Backslash);
    assert!(!h.app.radio().is_on());
    let (s, _) = Settings::from_json(&store.0.borrow().clone().unwrap());
    assert!(!s.radio.on);
}

#[test]
fn the_master_volume_and_mute_reach_the_radio() {
    use audio::RadioCommand;
    let mut h = Harness::new();
    h.frame();
    h.app.radio_commands().for_each(drop);
    h.open(&[ItemId::Open(Page::Settings), ItemId::Open(Page::Audio)]);
    h.focus_on(ItemId::MasterVolume);
    h.pad(PadButton::DpadLeft);
    let cmds: Vec<_> = h.app.radio_commands().collect();
    assert_eq!(cmds, [RadioCommand::Volume(0.8 * 0.75)]);
    h.focus_on(ItemId::Mute);
    h.pad(PadButton::South);
    let cmds: Vec<_> = h.app.radio_commands().collect();
    assert_eq!(cmds, [RadioCommand::Volume(0.0)], "muted");
    h.pad(PadButton::South);
    let cmds: Vec<_> = h.app.radio_commands().collect();
    assert_eq!(cmds, [RadioCommand::Volume(0.8 * 0.75)], "unmuted");
}

#[test]
fn still_scenes_keep_the_radio_off() {
    let setup = crate::SceneSetup {
        still: true,
        ..crate::SceneSetup::default()
    };
    let mut app = App::from_setup(&setup).unwrap();
    let mut input = InputMapper::default();
    app.set_settings_store(Box::new(crate::MemoryStore {
        text: Some(r#"{ "radio": { "on": true, "last_station": "somafm_u80s" } }"#.to_string()),
        saves: 0,
    }));
    for _ in 0..3 {
        input.press(RawInput::Key(Key::BracketRight));
        app.frame(&mut input, DT, &crate::tests::wall_clock);
        input.release(RawInput::Key(Key::BracketRight));
        app.frame(&mut input, DT, &crate::tests::wall_clock);
    }
    assert!(!app.radio().is_on());
    assert!(!app.ui().radio().started());
    assert_eq!(app.radio_commands().count(), 0);
    assert!(app.ui().radio().toast().is_none());
}

#[test]
fn music_folders_are_picked_from_the_audio_page() {
    let mut h = Harness::new();
    let label = |h: &Harness| {
        h.ui()
            .items()
            .iter()
            .find(|i| i.id == ItemId::AddMusic)
            .map(|i| i.label)
    };
    h.open(&[ItemId::Open(Page::Settings), ItemId::Open(Page::Audio)]);
    assert_eq!(label(&h), None, "no picker on this platform");
    h.app.set_music_picker(true, true);
    h.pad(PadButton::DpadDown);
    assert_eq!(label(&h), Some("Pick music folder again"));
    h.focus_on(ItemId::AddMusic);
    h.pad(PadButton::South);
    assert!(h.app.take_music_request());
    assert!(!h.app.take_music_request());
    let rows = h.app.ui().radio().rows().len();
    let track = |i: usize| audio::Track {
        key: format!("0/{i}"),
        label: format!("Song {i}"),
        secs: 200.0,
    };
    h.app
        .add_music_folder("r1", "Yacht", vec![track(0), track(1)], true, true);
    assert_eq!(
        h.app.ui().toast(),
        Some(Toast::MusicAdded {
            tracks: 2,
            kept: true
        })
    );
    h.frame();
    assert!(h.hud_text().contains("Yacht: 2 tracks on the dial"));
    assert_eq!(h.app.ui().radio().rows().len(), rows + 1);
    h.app
        .add_music_folder("r1", "Yacht", vec![track(0)], false, true);
    h.app
        .add_music_folder("r2", "Empty", Vec::new(), true, false);
    assert_eq!(h.app.ui().toast(), Some(Toast::MusicEmpty));
    assert_eq!(
        h.app.ui().radio().rows().len(),
        rows + 1,
        "replaced; empty left out"
    );
    let mine = h.app.radio().channels().last().unwrap().clone();
    assert_eq!((mine.name.as_str(), mine.tracks.len()), ("Yacht", 1));
    // Another folder of the same name is another station.
    h.app
        .add_music_folder("r3", "Yacht", vec![track(0)], true, false);
    assert_eq!(h.app.ui().radio().rows().len(), rows + 2);
    h.frame();
    assert!(h.hud_text().contains("(pick it again next time)"));
    h.pad(PadButton::DpadUp);
    assert_eq!(label(&h), Some("Add music folder"), "picked again");
    // Removing one from the Stations page forgets it; "Remove music folders" the rest.
    h.open(&[ItemId::Open(Page::Stations)]);
    let remove = h
        .ui()
        .items()
        .iter()
        .find_map(|i| match i.id {
            ItemId::RemoveFolder(r) if h.app.ui().radio().rows()[r].id == mine.id => Some(i.id),
            _ => None,
        })
        .unwrap();
    h.focus_on(remove);
    h.pad(PadButton::South);
    assert_eq!(h.app.take_forgotten_music(), ["r1"]);
    assert_eq!(h.app.ui().radio().rows().len(), rows + 1);
    h.pad(PadButton::East);
    h.focus_on(ItemId::ClearMusic);
    h.pad(PadButton::South);
    assert_eq!(h.app.take_forgotten_music(), ["*"]);
    assert_eq!(h.app.ui().radio().rows().len(), rows);
    h.app.music_picker_refused();
    assert_eq!(h.app.ui().toast(), Some(Toast::MusicNeedsKeyboard));
}

#[test]
fn streams_can_be_added_by_url() {
    let mut h = Harness::new();
    h.app.set_text_entry(true);
    let rows = h.app.ui().radio().rows().len();
    h.app.add_stream_url("https://radio.example.org/live");
    assert_eq!(h.app.ui().radio().rows().len(), rows + 1);
    assert_eq!(h.app.ui().toast(), Some(Toast::StreamAdded));
    assert!(h.app.radio().channels().iter().any(|c| c.id == "custom_1"));
    h.app.add_stream_url("not a url");
    assert_eq!(h.app.ui().toast(), Some(Toast::StreamRejected));
    h.app.add_stream_url("  ");
    assert_eq!(h.app.ui().radio().rows().len(), rows + 1);
    assert!(!h.app.take_stream_url_request());
}

#[test]
fn the_dash_shows_the_station_in_the_cockpit() {
    let count = |h: &Harness| h.hud_text().matches("Radio P").count();
    let mut h = Harness::new();
    h.app.set_target_size(1280, 720);
    h.key(Key::BracketRight);
    h.frame();
    assert_eq!(
        count(&h),
        2,
        "the toast and its shadow, from the chase camera"
    );
    h.app.set_camera_mode(crate::CameraMode::Cockpit);
    h.frame();
    assert_eq!(count(&h), 3, "and the dash: {}", h.hud_text());
    h.key(Key::Backslash);
    assert_eq!(count(&h), 0, "off: nothing on the dash");
}

#[test]
fn quality_presets_apply_to_the_renderer_and_save() {
    let store = SharedStore::default();
    let mut h = Harness::with_store(&store);
    // Nothing saved: auto, Medium on an unknown GPU, Deck on the Deck.
    assert_eq!(h.app.settings().quality, None);
    assert_eq!(h.app.ui().quality(), Quality::Medium);
    let medium = h.app.renderer_options().quality;
    assert_eq!(medium, Quality::Medium.render(1.0));
    h.app.set_gpu_info(&GpuInfo {
        name: "AMD Custom GPU 0405 (RADV VANGOGH)".into(),
        backend: "Vulkan".into(),
        integrated: true,
        linux: true,
        surface: (1280, 800),
    });
    assert_eq!(h.app.ui().quality(), Quality::Deck);
    assert_eq!(h.app.scene_view(0.0, 1.6).quality.cascades, 2);
    // Low on the Video page: cheaper shadows, 75% resolution, saved.
    h.open(&[ItemId::Open(Page::Settings), ItemId::Open(Page::Video)]);
    h.focus_on(ItemId::Quality);
    h.pad(PadButton::DpadLeft);
    assert_eq!(h.app.settings().quality, Some(Quality::Low));
    assert_eq!(h.app.settings().resolution_scale, 0.75);
    let low = h.app.renderer_options().quality;
    assert_eq!(low, Quality::Low.render(0.75));
    assert_ne!(low, medium);
    assert_eq!(h.app.scene_view(0.0, 1.6).quality, low);
    h.pad(PadButton::Start);
    let saved = store.0.borrow().clone().expect("saved");
    assert!(saved.contains("\"quality\": \"low\""), "{saved}");
    // A forced preset wins for the session with its own scale, unsaved.
    h.app.force_quality(Some(Quality::Ultra));
    assert_eq!(
        h.app.scene_view(0.0, 1.6).quality,
        Quality::Ultra.render(1.0)
    );
    assert_eq!(h.app.settings().quality, Some(Quality::Low));
}
