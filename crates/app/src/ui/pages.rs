//! The menu tree: which items each page shows, built from the current settings, bindings
//! and clock.
//!
//! ```text
//! Pause:     Resume · Settings › · Quit (native only)
//! Settings:  Drive › · Camera › · Controls › · Video › · HUD › · Audio › · World › · About ›
//! Drive:     Handling (Arcade / Light / Off)
//! Camera:    Camera · Cockpit FOV · Seat height · Seat forward · Dashboard (Full cockpit /
//!            Hood only) · Camera shake
//! Controls:  Gamepad › · Keyboard & mouse ›
//! Device:    Invert look Y · Invert look X · Look sensitivity · Bindings › · Analog axes ›
//!            · [pad: Swap sticks · Throttle on right stick] · Reset to defaults
//! Bindings:  one row per action (and per direction of a two-way action): its inputs on
//!            this device; select to remap
//! Axes:      one row per analog action ›
//! Axis:      Sensitivity · Deadzone · Response curve · Invert · Reset
//! Video:     UI scale · Resolution scale · Bloom · Performance overlay
//! HUD:       Minimap · Rotate with car · Minimap zoom
//! Audio:     Master volume · Engine volume · World volume · Mute
//! World:     Time of day · Clock speed
//! About:     (license text)
//! Conflict:  Keep on <existing action> · Move to <new action>
//! ```

use crate::camera::CameraMode;
use crate::daylight::{Clock, CLOCK_SPEEDS};
use crate::input::{Action, AxisOptions, Bindings, DeviceKind, Sign};
use crate::map::view::MINIMAP_RANGES_M;
use crate::settings::{
    Handling, Settings, COCKPIT_FOV_MAX, COCKPIT_FOV_MIN, COCKPIT_FOV_STEP, RESOLUTION_SCALE_MIN,
    RESOLUTION_SCALE_STEP, SEAT_FORWARD_CM, SEAT_HEIGHT_CM, SEAT_STEP_CM, UI_SCALE_STEP,
    VOLUME_STEP,
};

use super::menu::{Item, Unit, Widget};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Pause,
    Settings,
    Drive,
    Camera,
    Controls,
    Device(DeviceKind),
    Bindings(DeviceKind),
    Axes(DeviceKind),
    Axis(DeviceKind, Action),
    Video,
    Hud,
    Audio,
    World,
    About,
    /// A remap hit an input another action uses; see `Ui::conflict`.
    Conflict,
}

/// What an item controls.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ItemId {
    Resume,
    Quit,
    Open(Page),
    /// Look Y (and free-fly look Y) invert on a device.
    InvertLookY(DeviceKind),
    InvertLookX(DeviceKind),
    /// Sensitivity of all four look axes on a device.
    LookSensitivity(DeviceKind),
    Sensitivity(DeviceKind, Action),
    Deadzone(DeviceKind, Action),
    Curve(DeviceKind, Action),
    Invert(DeviceKind, Action),
    ResetAction(DeviceKind, Action),
    Bind(DeviceKind, Action, Sign),
    Southpaw,
    SwapTriggers,
    ResetDevice(DeviceKind),
    Handling,
    CameraView,
    CockpitFov,
    SeatHeight,
    SeatForward,
    Dashboard,
    CameraShake,
    UiScale,
    ResolutionScale,
    Bloom,
    DebugOverlay,
    Minimap,
    MinimapRotate,
    MinimapZoom,
    MasterVolume,
    EngineVolume,
    WorldVolume,
    Mute,
    TimeOfDay,
    ClockSpeed,
    KeepExisting,
    UseNew,
}

/// Everything a page reads to show its items.
#[derive(Clone, Copy)]
pub struct PageCtx<'a> {
    pub settings: &'a Settings,
    pub bindings: &'a Bindings,
    pub clock: &'a Clock,
    pub can_quit: bool,
}

pub const CLOCK_SPEED_NAMES: [&str; CLOCK_SPEEDS.len()] = ["Frozen", "Real time", "10x", "60x"];
/// Names of [`MINIMAP_RANGES_M`].
pub const MINIMAP_RANGE_NAMES: [&str; MINIMAP_RANGES_M.len()] = ["100 m", "150 m", "250 m"];
/// Names of [`Handling::ALL`].
pub const HANDLING_NAMES: [&str; Handling::ALL.len()] = ["Arcade", "Light", "Off"];
/// Names of [`CameraMode::CYCLE`].
pub const CAMERA_NAMES: [&str; CameraMode::CYCLE.len()] =
    ["Cockpit", "Hood", "Chase near", "Chase mid", "Chase far"];
/// The Dashboard choice: index 0 draws it, 1 is hood only.
pub const DASHBOARD_NAMES: [&str; 2] = ["Full cockpit", "Hood only"];

/// The look axes "Invert look" and "Look sensitivity" set together: the driving look and
/// the free-fly camera's look.
pub const LOOK_X: [Action; 2] = [Action::LookX, Action::FlyLookX];
pub const LOOK_Y: [Action; 2] = [Action::LookY, Action::FlyLookY];

/// Rows of the bindings page, in order: `(action, sign)`, two rows (left/right or
/// up/down) for each two-way action.
pub fn binding_rows() -> impl Iterator<Item = (Action, Sign)> {
    Action::ALL.into_iter().flat_map(|a| {
        let rows: [Option<(Action, Sign)>; 2] = if a.is_bipolar() {
            [Some((a, Sign::Negative)), Some((a, Sign::Positive))]
        } else {
            [Some((a, Sign::Full)), None]
        };
        rows.into_iter().flatten()
    })
}

pub fn title(page: Page) -> &'static str {
    match page {
        Page::Pause => "Paused",
        Page::Settings => "Settings",
        Page::Drive => "Drive",
        Page::Camera => "Camera",
        Page::Controls => "Controls",
        Page::Device(d) => super::names::device_label(d),
        Page::Bindings(_) => "Bindings",
        Page::Axes(_) => "Analog axes",
        Page::Axis(_, a) => super::names::action_label(a, Sign::Full),
        Page::Video => "Video",
        Page::Hud => "HUD",
        Page::Audio => "Audio",
        Page::World => "World",
        Page::About => "About",
        Page::Conflict => "Input already in use",
    }
}

/// A line of explanation shown under a page's items.
pub fn note(page: Page) -> Option<&'static str> {
    match page {
        Page::About => Some(
            "Skywave is free software under the BSD-3-Clause license.\n\
             Code, font and art: ours. Music: whatever you tune in.",
        ),
        Page::Bindings(_) => Some("Select a row, then press the new input."),
        Page::Video => Some("Resolution scale is saved for an upcoming renderer option."),
        Page::Drive => Some(
            "Arcade: grippy, short stops, forgiving.\n\
             Light: looser, the car slides more. Off: no help.",
        ),
        Page::Camera => Some("The camera you pick or cycle to is the one the game starts in."),
        _ => None,
    }
}

fn slider(value: f32, min: f32, max: f32, step: f32, unit: Unit) -> Widget {
    Widget::Slider {
        value,
        min,
        max,
        step,
        unit,
    }
}

/// Fills `out` with the items of `page`.
pub fn build(page: Page, ctx: &PageCtx, out: &mut Vec<Item>) {
    out.clear();
    let mut add = |id: ItemId, label: &'static str, widget: Widget| {
        out.push(Item { id, label, widget });
    };
    let open = |p: Page| (ItemId::Open(p), Widget::Submenu(p));
    match page {
        Page::Pause => {
            add(ItemId::Resume, "Resume", Widget::Action);
            let (id, w) = open(Page::Settings);
            add(id, "Settings", w);
            if ctx.can_quit {
                add(ItemId::Quit, "Quit", Widget::Action);
            }
        }
        Page::Settings => {
            for (p, label) in [
                (Page::Drive, "Drive"),
                (Page::Camera, "Camera"),
                (Page::Controls, "Controls"),
                (Page::Video, "Video"),
                (Page::Hud, "HUD"),
                (Page::Audio, "Audio"),
                (Page::World, "World"),
                (Page::About, "About"),
            ] {
                let (id, w) = open(p);
                add(id, label, w);
            }
        }
        Page::Controls => {
            for d in [DeviceKind::Gamepad, DeviceKind::KeyboardMouse] {
                let (id, w) = open(Page::Device(d));
                add(id, super::names::device_label(d), w);
            }
        }
        Page::Device(d) => {
            let b = ctx.bindings;
            add(
                ItemId::InvertLookY(d),
                "Invert look Y",
                Widget::Toggle(b.axis_options(d, Action::LookY).invert),
            );
            add(
                ItemId::InvertLookX(d),
                "Invert look X",
                Widget::Toggle(b.axis_options(d, Action::LookX).invert),
            );
            add(
                ItemId::LookSensitivity(d),
                "Look sensitivity",
                slider(
                    b.axis_options(d, Action::LookX).sensitivity,
                    AxisOptions::SENSITIVITY_MIN,
                    AxisOptions::SENSITIVITY_MAX,
                    0.05,
                    Unit::Times,
                ),
            );
            let (id, w) = open(Page::Bindings(d));
            add(id, "Bindings", w);
            let (id, w) = open(Page::Axes(d));
            add(id, "Analog axes", w);
            if d == DeviceKind::Gamepad {
                add(ItemId::Southpaw, "Preset: swap sticks", Widget::Action);
                add(
                    ItemId::SwapTriggers,
                    "Preset: throttle on right stick",
                    Widget::Action,
                );
            }
            add(ItemId::ResetDevice(d), "Reset to defaults", Widget::Action);
        }
        Page::Bindings(d) => {
            for (action, sign) in binding_rows() {
                add(
                    ItemId::Bind(d, action, sign),
                    super::names::action_label(action, sign),
                    Widget::Capture {
                        device: d,
                        action,
                        sign,
                    },
                );
            }
        }
        Page::Axes(d) => {
            for a in Action::ANALOG {
                add(
                    ItemId::Open(Page::Axis(d, a)),
                    super::names::action_label(a, Sign::Full),
                    Widget::Submenu(Page::Axis(d, a)),
                );
            }
        }
        Page::Axis(d, a) => {
            let o = ctx.bindings.axis_options(d, a);
            add(
                ItemId::Sensitivity(d, a),
                "Sensitivity",
                slider(
                    o.sensitivity,
                    AxisOptions::SENSITIVITY_MIN,
                    AxisOptions::SENSITIVITY_MAX,
                    0.05,
                    Unit::Times,
                ),
            );
            add(
                ItemId::Deadzone(d, a),
                "Deadzone",
                slider(
                    o.deadzone,
                    0.0,
                    AxisOptions::DEADZONE_MAX,
                    0.01,
                    Unit::Plain,
                ),
            );
            add(
                ItemId::Curve(d, a),
                "Response curve",
                slider(
                    o.curve,
                    AxisOptions::CURVE_MIN,
                    AxisOptions::CURVE_MAX,
                    0.05,
                    Unit::Plain,
                ),
            );
            add(ItemId::Invert(d, a), "Invert", Widget::Toggle(o.invert));
            add(ItemId::ResetAction(d, a), "Reset this axis", Widget::Action);
        }
        Page::Drive => {
            let index = Handling::ALL
                .iter()
                .position(|&h| h == ctx.settings.handling)
                .unwrap_or(0);
            add(
                ItemId::Handling,
                "Handling",
                Widget::Choice {
                    options: &HANDLING_NAMES,
                    index,
                },
            );
        }
        Page::Camera => {
            let s = ctx.settings;
            let index = CameraMode::CYCLE
                .iter()
                .position(|&m| m == s.camera)
                .unwrap_or(3);
            add(
                ItemId::CameraView,
                "Camera",
                Widget::Choice {
                    options: &CAMERA_NAMES,
                    index,
                },
            );
            add(
                ItemId::CockpitFov,
                "Cockpit FOV",
                slider(
                    s.cockpit_fov_deg,
                    COCKPIT_FOV_MIN,
                    COCKPIT_FOV_MAX,
                    COCKPIT_FOV_STEP,
                    Unit::Degrees,
                ),
            );
            add(
                ItemId::SeatHeight,
                "Seat height",
                slider(
                    s.seat_height_cm,
                    SEAT_HEIGHT_CM[0],
                    SEAT_HEIGHT_CM[1],
                    SEAT_STEP_CM,
                    Unit::Centimeters,
                ),
            );
            add(
                ItemId::SeatForward,
                "Seat forward",
                slider(
                    s.seat_forward_cm,
                    SEAT_FORWARD_CM[0],
                    SEAT_FORWARD_CM[1],
                    SEAT_STEP_CM,
                    Unit::Centimeters,
                ),
            );
            add(
                ItemId::Dashboard,
                "Dashboard",
                Widget::Choice {
                    options: &DASHBOARD_NAMES,
                    index: usize::from(!s.dashboard),
                },
            );
            add(
                ItemId::CameraShake,
                "Camera shake",
                Widget::Toggle(s.camera_shake),
            );
        }
        Page::Video => {
            let s = ctx.settings;
            add(
                ItemId::UiScale,
                "UI scale",
                slider(
                    s.ui_scale,
                    render::UI_SCALE_MIN,
                    render::UI_SCALE_MAX,
                    UI_SCALE_STEP,
                    Unit::Times,
                ),
            );
            add(
                ItemId::ResolutionScale,
                "Resolution scale",
                slider(
                    s.resolution_scale,
                    RESOLUTION_SCALE_MIN,
                    1.0,
                    RESOLUTION_SCALE_STEP,
                    Unit::Percent,
                ),
            );
            add(ItemId::Bloom, "Bloom", Widget::Toggle(s.bloom));
            add(
                ItemId::DebugOverlay,
                "Performance overlay",
                Widget::Toggle(s.debug_overlay),
            );
        }
        Page::Hud => {
            let s = ctx.settings;
            add(ItemId::Minimap, "Minimap", Widget::Toggle(s.minimap));
            add(
                ItemId::MinimapRotate,
                "Rotate with car",
                Widget::Toggle(s.minimap_rotate),
            );
            let index = MINIMAP_RANGES_M
                .iter()
                .position(|&r| r == s.minimap_range_m)
                .unwrap_or(crate::map::view::DEFAULT_MINIMAP_RANGE);
            add(
                ItemId::MinimapZoom,
                "Minimap zoom",
                Widget::Choice {
                    options: &MINIMAP_RANGE_NAMES,
                    index,
                },
            );
        }
        Page::Audio => {
            let s = ctx.settings;
            let volume = |v: f32| slider(v, 0.0, 1.0, VOLUME_STEP, Unit::Percent);
            add(
                ItemId::MasterVolume,
                "Master volume",
                volume(s.master_volume),
            );
            add(
                ItemId::EngineVolume,
                "Engine volume",
                volume(s.engine_volume),
            );
            add(ItemId::WorldVolume, "World volume", volume(s.world_volume));
            add(ItemId::Mute, "Mute", Widget::Toggle(s.mute));
        }
        Page::World => {
            let hours = (ctx.clock.seconds_of_day / 3600.0) as f32;
            add(
                ItemId::TimeOfDay,
                "Time of day",
                slider(hours, 0.0, 24.0, 0.25, Unit::Clock),
            );
            let index = CLOCK_SPEEDS
                .iter()
                .position(|&s| s == ctx.settings.clock_speed)
                .unwrap_or(2);
            add(
                ItemId::ClockSpeed,
                "Clock speed",
                Widget::Choice {
                    options: &CLOCK_SPEED_NAMES,
                    index,
                },
            );
        }
        Page::About => {}
        Page::Conflict => {
            add(ItemId::KeepExisting, "Keep on ", Widget::Action);
            add(ItemId::UseNew, "Move to ", Widget::Action);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_direction_has_a_binding_row() {
        let rows: Vec<_> = binding_rows().collect();
        let bipolar = Action::ALL.iter().filter(|a| a.is_bipolar()).count();
        assert_eq!(rows.len(), Action::COUNT + bipolar);
        for a in Action::ALL {
            assert!(rows.iter().any(|&(r, _)| r == a), "{a:?}");
        }
    }

    #[test]
    fn pages_reflect_the_state() {
        let settings = Settings::default();
        let mut bindings = Bindings::default();
        let mut o = bindings.axis_options(DeviceKind::Gamepad, Action::LookY);
        o.invert = true;
        bindings.set_axis_options(DeviceKind::Gamepad, Action::LookY, o);
        let clock = Clock::new(100, 6.5 * 3600.0);
        let ctx = PageCtx {
            settings: &settings,
            bindings: &bindings,
            clock: &clock,
            can_quit: false,
        };
        let mut items = Vec::new();
        build(Page::Pause, &ctx, &mut items);
        assert!(
            items.iter().all(|i| i.id != ItemId::Quit),
            "no quit on the web"
        );
        build(Page::Device(DeviceKind::Gamepad), &ctx, &mut items);
        assert_eq!(items[0].widget, Widget::Toggle(true));
        assert_eq!(items[1].widget, Widget::Toggle(false));
        build(Page::Device(DeviceKind::KeyboardMouse), &ctx, &mut items);
        assert_eq!(items[0].widget, Widget::Toggle(false));
        assert!(items.iter().all(|i| i.id != ItemId::Southpaw));
        build(Page::Hud, &ctx, &mut items);
        assert_eq!(items[0].widget, Widget::Toggle(true));
        assert_eq!(items[1].widget, Widget::Toggle(true));
        assert!(matches!(items[2].widget, Widget::Choice { index: 1, .. }));
        build(Page::Audio, &ctx, &mut items);
        assert!(matches!(items[0].widget, Widget::Slider { value, .. } if value == 0.8));
        assert_eq!(items[3].widget, Widget::Toggle(false));
        build(Page::World, &ctx, &mut items);
        assert!(matches!(items[0].widget, Widget::Slider { value, .. } if value == 6.5));
        assert!(matches!(items[1].widget, Widget::Choice { index: 2, .. }));
        build(Page::Drive, &ctx, &mut items);
        assert!(
            matches!(items[0].widget, Widget::Choice { index: 0, .. }),
            "arcade"
        );
        build(Page::Camera, &ctx, &mut items);
        assert!(
            matches!(items[0].widget, Widget::Choice { index: 3, .. }),
            "chase mid"
        );
        assert!(matches!(items[1].widget, Widget::Slider { value, .. } if value == 62.0));
        assert!(
            matches!(items[4].widget, Widget::Choice { index: 0, .. }),
            "dashboard"
        );
        assert_eq!(items[5].widget, Widget::Toggle(true));
    }
}
