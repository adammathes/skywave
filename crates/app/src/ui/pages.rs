//! The menu tree: which items each page shows, built from the current settings, bindings
//! and clock.
//!
//! The menu is full screen: the tabs across the top (shoulder buttons or Q and E switch),
//! the focused tab's items in a list on the left and the focused item's detail on the
//! right. A tab is one page; a few rows open one page below it, never deeper.
//!
//! ```text
//! Game:      Resume · Reset car · Show controls · Fullscreen · Quit (native only); the
//!            credits and license beside them
//! Car:       Model · Paint · Driver · Drive this car; the turntable beside them, the
//!            specs under them, and Select on any row swaps the car in where you are
//! Worlds:    one row per world; its still, year, cars and stations beside them, Select
//!            goes there
//! Driving:   Handling · Headlights · Traffic · Time of day · Clock speed
//! Camera:    Camera · Cockpit FOV · Seat height · Seat forward · Dashboard · Show driver
//!            in first person · Camera shake
//! Display:   Look · Quality · UI scale · Minimap · Rotate with car · Minimap zoom ·
//!            Performance overlay · Show controls at start
//! Audio:     Master, engine, world and radio volume · Source (Radio / Tape, CD or MP3
//!            by era) · Deck mode · Preset · Radio in background · Stations › ·
//!            [Music ›, where there is or can be music]
//! Controls:  Gamepad buttons › · Keyboard keys › · Invert look · Stick look speed ·
//!            Mouse look speed · Swap sticks · Throttle on right stick · Reset controls
//!
//! Stations:  one on/off row per station of the preset and per added stream · [web: Add
//!            stream URL] · [Remove added streams, when there are some]
//! Music:     [web: Add music folder, or Pick music folder again] · one row per album,
//!            "Artist - Album" (select: play it) · "Remove <folder>" per music folder ·
//!            Remove all music
//! Bindings:  one row per action (and per direction of a two-way action): its inputs on
//!            this device; select to remap
//! Conflict:  Keep on <existing action> · Move to <new action>
//! ```

use crate::camera::CameraMode;
use crate::daylight::{Clock, CLOCK_SPEEDS};
use crate::deck::CarDeck;
use crate::input::{Action, AxisOptions, Bindings, DeviceKind, Sign};
use crate::map::view::MINIMAP_RANGES_M;
use crate::radio::{CarRadio, PRESET_TITLES};
use crate::settings::{
    Handling, Overlay, Quality, Settings, COCKPIT_FOV_MAX, COCKPIT_FOV_MIN, COCKPIT_FOV_STEP,
    SEAT_FORWARD_CM, SEAT_HEIGHT_CM, SEAT_STEP_CM, UI_SCALE_STEP, VOLUME_STEP,
};

use super::menu::{Item, Unit, Widget};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    /// Resume, reset, quit; the credits.
    Game,
    /// Pick a car and its paint, with a turntable preview.
    Car,
    /// Pick a world: one row per scenario.
    Worlds,
    Driving,
    Camera,
    Display,
    Audio,
    Controls,
    /// The bindings of one device.
    Bindings(DeviceKind),
    /// The stations of the preset on the dial, and the added streams.
    Stations,
    /// The deck's albums, and the music folders they come from.
    Music,
    /// A remap hit an input another action uses; see `Ui::conflict`.
    Conflict,
}

/// The tabs across the top of the menu, left to right.
pub const TABS: [Page; 8] = [
    Page::Game,
    Page::Car,
    Page::Worlds,
    Page::Driving,
    Page::Camera,
    Page::Display,
    Page::Audio,
    Page::Controls,
];

/// The tab a page is under: itself for a tab.
pub fn tab_of(page: Page) -> Page {
    match page {
        Page::Bindings(_) | Page::Conflict => Page::Controls,
        Page::Stations | Page::Music => Page::Audio,
        p => p,
    }
}

/// What an item controls.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ItemId {
    Resume,
    /// Put the car back on the nearest lane (and close the menu).
    ResetCar,
    /// Show the controls card over the menu.
    Controls,
    /// Fullscreen on/off (the platform does it; the menu stays open).
    Fullscreen,
    Quit,
    Open(Page),
    /// Swap to the built-in car with this index in [`crate::vehicles::BUILTIN`] (in the
    /// paint chosen on the page).
    Vehicle(usize),
    /// The Car page's car being looked at; the choice is a position in this world's
    /// [`crate::scenario::Lineup`].
    VehicleCar,
    /// Load the world with this index in [`crate::scenario::BUILTIN`].
    World(usize),
    /// The Car page's paint: an entry of the car's palette.
    Paint,
    /// The player's driver: one of `traffic::figure::PRESETS`, saved as it is chosen.
    Driver,
    /// Look Y (and free-fly look Y) invert, on both devices.
    InvertLook,
    /// Sensitivity of all four look axes on a device.
    LookSensitivity(DeviceKind),
    Bind(DeviceKind, Action, Sign),
    Southpaw,
    SwapTriggers,
    /// Both devices' bindings and axis options back to the defaults.
    ResetControls,
    Handling,
    Headlights,
    CameraView,
    CockpitFov,
    SeatHeight,
    SeatForward,
    Dashboard,
    CockpitDriver,
    CameraShake,
    Look,
    Quality,
    UiScale,
    DebugOverlay,
    Minimap,
    MinimapRotate,
    MinimapZoom,
    ControlsAtStart,
    MasterVolume,
    /// The radio in the background: keep playing (0) or mute (1).
    BackgroundRadio,
    EngineVolume,
    WorldVolume,
    TimeOfDay,
    ClockSpeed,
    Traffic,
    RadioVolume,
    RadioPreset,
    /// Row `i` of `CarRadio::rows`: the station on the dial or not.
    Station(usize),
    AddStream,
    ClearStreams,
    AddMusic,
    ClearMusic,
    /// Folder `i` of `CarDeck::folders`: take it out of the library for good.
    RemoveFolder(usize),
    /// The radio or the deck.
    Source,
    DeckMode,
    /// Album `i` of the deck's library: play it.
    Album(usize),
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
    pub radio: &'a CarRadio,
    pub deck: &'a CarDeck,
    /// The platform can ask the user for a line of text (a stream URL).
    pub text_entry: bool,
    /// The platform can ask the user for a music folder, and whether last session's must
    /// be picked again.
    pub music_picker: bool,
    pub music_again: bool,
    /// The preset in effect (the saved one, else the one picked for the device).
    pub quality: Quality,
    /// The look in effect (a scene's `look=` until one is picked, else the saved one).
    pub look: render::Look,
    /// The Car page's car (a [`crate::vehicles::BUILTIN`] index) and paint.
    pub browse: [usize; 2],
    /// The player's driver (a `traffic::figure::PRESETS` index).
    pub driver: usize,
    /// The world the app is: an index into [`crate::scenario::BUILTIN`].
    pub scenario: usize,
}

/// Option names of the paint chooser, one per palette entry (the page draws swatches).
pub const PAINT_NAMES: [&str; super::roster::MAX_PAINTS] = [
    "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12",
];

pub const CLOCK_SPEED_NAMES: [&str; CLOCK_SPEEDS.len()] = ["Frozen", "Real time", "10x", "60x"];
/// Names of [`MINIMAP_RANGES_M`].
pub const MINIMAP_RANGE_NAMES: [&str; MINIMAP_RANGES_M.len()] = ["Auto", "500 m", "1 km", "2 km"];
/// The Headlights choice: index 0 is Auto, 1 Manual.
pub const HEADLIGHT_NAMES: [&str; 2] = ["Auto", "Manual"];
/// Names of [`Handling::ALL`].
pub const HANDLING_NAMES: [&str; Handling::ALL.len()] = ["Arcade", "Light", "Off"];
/// Names of [`CameraMode::CYCLE`].
pub const CAMERA_NAMES: [&str; CameraMode::CYCLE.len()] =
    ["Cockpit", "Hood", "Chase near", "Chase mid", "Chase far"];
/// Names of [`Quality::ALL`].
pub const QUALITY_NAMES: [&str; Quality::ALL.len()] =
    ["Low", "Battery", "Deck", "Medium", "High", "Ultra"];
/// Names of [`render::Look::ALL`].
pub const LOOK_NAMES: [&str; render::Look::ALL.len()] = [
    "Standard",
    "Dusk layers",
    "Amber haze",
    "Poster",
    "Toon",
    "Night video",
    "Low-res",
    "Ink",
];
/// Names of [`Overlay::ALL`].
pub const OVERLAY_NAMES: [&str; Overlay::ALL.len()] = ["Off", "FPS", "Full"];
/// The Radio in background choice: index 0 keeps playing, 1 mutes.
pub const BACKGROUND_RADIO_NAMES: [&str; 2] = ["Keep playing", "Mute"];
/// The Source choice: the radio, or the deck by era.
pub const SOURCE_NAMES: [[&str; 2]; 3] = [["Radio", "Tape"], ["Radio", "CD"], ["Radio", "MP3"]];
/// Names of [`audio::deck::Mode::ALL`].
pub const DECK_MODE_NAMES: [&str; 3] = ["Normal", "Shuffle", "Continuous"];

/// The look axes "Invert look" and "Look sensitivity" set: the driving look and the
/// free-fly camera's look.
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
        Page::Game => "Game",
        Page::Car => "Car",
        Page::Worlds => "Worlds",
        Page::Driving => "Driving",
        Page::Camera => "Camera",
        Page::Display => "Display",
        Page::Audio => "Audio",
        Page::Controls => "Controls",
        Page::Bindings(DeviceKind::Gamepad) => "Gamepad buttons",
        Page::Bindings(DeviceKind::KeyboardMouse) => "Keyboard keys",
        Page::Stations => "Stations",
        Page::Music => "Music",
        Page::Conflict => "Input already in use",
    }
}

/// The Game tab's detail: the credits.
pub const ABOUT: &str = "Created by Adam Mathes\n\
                         \n\
                         Skywave is free software under the\n\
                         BSD-3-Clause license.\n\
                         \n\
                         Code, font and art: ours.\n\
                         Music: whatever you tune in.";

/// What an item does, shown beside the list when it is focused (lines of at most about 40
/// characters, so they fit the detail column at any UI scale up to 1.5).
pub fn help(id: ItemId) -> &'static str {
    match id {
        ItemId::Resume => "Back to the road.",
        ItemId::ResetCar => "Put the car back on the nearest lane.",
        ItemId::Controls => "Every input on one card.",
        ItemId::Fullscreen => "Fill the screen, or go back to a window.",
        ItemId::Quit => "Close Skywave. Settings are saved.",
        ItemId::Handling => {
            "Arcade: grippy, short stops, forgiving.\n\
             Light: looser, the car slides more.\n\
             Off: no help."
        }
        ItemId::Headlights => "Auto: on at dusk, off at dawn.\nManual: your switch only.",
        ItemId::Traffic => "How many other cars are on the road.",
        ItemId::TimeOfDay => "Set the clock. Left and right scrub it.",
        ItemId::ClockSpeed => "How fast the day goes by.",
        ItemId::CameraView => "The camera the game starts in.\nCycle it while driving, too.",
        ItemId::CockpitFov => "How wide the cockpit camera sees.",
        ItemId::SeatHeight | ItemId::SeatForward => "Move the seat in the cockpit camera.",
        ItemId::Dashboard => "Draw the dashboard, wheel and gauges\nin the cockpit camera.",
        ItemId::CockpitDriver => "See your hands and arms on the wheel.",
        ItemId::CameraShake => "The camera feels the road.",
        ItemId::Look => "A visual style for the whole world.",
        ItemId::Quality => {
            "Shadows, glow, resolution and frame rate.\n\
             Deck holds the panel's 90 fps.\n\
             Battery draws every other frame,\n\
             a little smaller."
        }
        ItemId::UiScale => "The size of menus and the HUD.",
        ItemId::Minimap => "The round map in the corner.",
        ItemId::MinimapRotate => "Turn the minimap so up is ahead.",
        ItemId::MinimapZoom => "Auto zooms out as you speed up.",
        ItemId::DebugOverlay => "Frame rate and timings in the corner.",
        ItemId::ControlsAtStart => "Show the controls card when the game\nstarts.",
        ItemId::MasterVolume => "Everything at once. 0% is silence.",
        ItemId::EngineVolume => "Your car's engine.",
        ItemId::WorldVolume => "Traffic, birds, the city.",
        ItemId::RadioVolume => "The radio and your own music.",
        ItemId::Source => {
            "Real internet radio, or your own music\n\
             on the era's deck."
        }
        ItemId::DeckMode => "How the deck plays your albums.",
        ItemId::RadioPreset => "The set of stations on the dial.",
        ItemId::BackgroundRadio => "When the game is not the focused\nwindow.",
        ItemId::Open(Page::Stations) => {
            "Pick the stations on the dial.\n\
             Streams play where the network allows;\n\
             nothing is stored."
        }
        ItemId::Open(Page::Music) => {
            "Play your own MP3s on the deck.\n\
             Files never leave this computer."
        }
        ItemId::Open(Page::Bindings(_)) | ItemId::Bind(..) => {
            "Select a row, then press the new input.\n\
             (hold): let go after a second."
        }
        ItemId::InvertLook => "Push up to look down.",
        ItemId::LookSensitivity(_) => "How fast looking around turns.",
        ItemId::Southpaw => "Steer and look on the other sticks.",
        ItemId::SwapTriggers => "Throttle and brake on the right stick.",
        ItemId::ResetControls => "Every binding back to the default,\non both devices.",
        ItemId::Station(_) => "On the dial, or skipped.",
        ItemId::AddStream => "Add an internet radio stream (https).",
        ItemId::ClearStreams => "Remove every stream you added.",
        ItemId::AddMusic => "Pick a folder of MP3s.",
        ItemId::ClearMusic | ItemId::RemoveFolder(_) => "Forget this music. Files stay put.",
        ItemId::Album(_) => "Play this album on the deck.",
        _ => "",
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

fn choice(options: &'static [&'static str], index: usize) -> Widget {
    Widget::Choice { options, index }
}

/// Position of `x` in `all`, else `default`.
fn index_of<T: PartialEq>(all: &[T], x: &T, default: usize) -> usize {
    all.iter().position(|a| a == x).unwrap_or(default)
}

/// Fills `out` with the items of `page`.
pub fn build(page: Page, ctx: &PageCtx, out: &mut Vec<Item>) {
    out.clear();
    let mut add = |id: ItemId, label: &'static str, widget: Widget| {
        out.push(Item { id, label, widget });
    };
    let open = |p: Page| (ItemId::Open(p), Widget::Submenu(p));
    let s = ctx.settings;
    match page {
        Page::Game => {
            add(ItemId::Resume, "Resume", Widget::Action);
            add(ItemId::ResetCar, "Reset car", Widget::Action);
            add(ItemId::Controls, "Show controls", Widget::Action);
            add(ItemId::Fullscreen, "Fullscreen", Widget::Action);
            if ctx.can_quit {
                add(ItemId::Quit, "Quit", Widget::Action);
            }
        }
        Page::Car => {
            let [car, paint] = ctx.browse;
            let lineup = crate::scenario::lineup(ctx.scenario);
            let car = car.min(super::roster::names().len().saturating_sub(1));
            let index = lineup.position(car).unwrap_or(0);
            add(ItemId::VehicleCar, "Model", choice(&lineup.names, index));
            let paints = super::roster::roster()
                .get(car)
                .map_or(1, |c| c.paints.len())
                .clamp(1, PAINT_NAMES.len());
            let index = paint.min(paints - 1);
            add(
                ItemId::Paint,
                "Paint",
                choice(&PAINT_NAMES[..paints], index),
            );
            let index = ctx.driver % traffic::figure::PRESETS;
            let names = &traffic::figure::PRESET_NAMES;
            add(ItemId::Driver, "Driver", choice(names, index));
            add(ItemId::Vehicle(car), "Drive this car", Widget::Action);
        }
        Page::Worlds => {
            for (i, w) in crate::scenario::all().iter().enumerate() {
                add(ItemId::World(i), w.title.as_str(), Widget::Action);
            }
        }
        Page::Driving => {
            let index = index_of(&Handling::ALL, &s.handling, 0);
            add(ItemId::Handling, "Handling", choice(&HANDLING_NAMES, index));
            let index = usize::from(!s.headlights_auto);
            add(
                ItemId::Headlights,
                "Headlights",
                choice(&HEADLIGHT_NAMES, index),
            );
            let index = index_of(&crate::npc::DENSITIES, &s.traffic, 2);
            add(
                ItemId::Traffic,
                "Traffic",
                choice(&crate::npc::DENSITY_NAMES, index),
            );
            let hours = (ctx.clock.seconds_of_day / 3600.0) as f32;
            add(
                ItemId::TimeOfDay,
                "Time of day",
                slider(hours, 0.0, 24.0, 0.25, Unit::Clock),
            );
            let index = index_of(&CLOCK_SPEEDS, &s.clock_speed, 2);
            add(
                ItemId::ClockSpeed,
                "Clock speed",
                choice(&CLOCK_SPEED_NAMES, index),
            );
        }
        Page::Camera => {
            let index = index_of(&CameraMode::CYCLE, &s.camera, 3);
            add(ItemId::CameraView, "View", choice(&CAMERA_NAMES, index));
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
            let [lo, hi] = SEAT_HEIGHT_CM;
            add(
                ItemId::SeatHeight,
                "Seat height",
                slider(s.seat_height_cm, lo, hi, SEAT_STEP_CM, Unit::Centimeters),
            );
            let [lo, hi] = SEAT_FORWARD_CM;
            add(
                ItemId::SeatForward,
                "Seat forward",
                slider(s.seat_forward_cm, lo, hi, SEAT_STEP_CM, Unit::Centimeters),
            );
            add(ItemId::Dashboard, "Dashboard", Widget::Toggle(s.dashboard));
            add(
                ItemId::CockpitDriver,
                "Show driver in first person",
                Widget::Toggle(s.cockpit_driver),
            );
            add(
                ItemId::CameraShake,
                "Camera shake",
                Widget::Toggle(s.camera_shake),
            );
        }
        Page::Display => {
            let index = index_of(&render::Look::ALL, &ctx.look, 0);
            add(ItemId::Look, "Look", choice(&LOOK_NAMES, index));
            let index = index_of(&Quality::ALL, &ctx.quality, 0);
            add(ItemId::Quality, "Quality", choice(&QUALITY_NAMES, index));
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
            add(ItemId::Minimap, "Minimap", Widget::Toggle(s.minimap));
            add(
                ItemId::MinimapRotate,
                "Rotate with car",
                Widget::Toggle(s.minimap_rotate),
            );
            let index = index_of(
                &MINIMAP_RANGES_M,
                &s.minimap_range_m,
                crate::map::view::DEFAULT_MINIMAP_RANGE,
            );
            add(
                ItemId::MinimapZoom,
                "Minimap zoom",
                choice(&MINIMAP_RANGE_NAMES, index),
            );
            let index = index_of(&Overlay::ALL, &s.overlay, 0);
            add(
                ItemId::DebugOverlay,
                "Performance overlay",
                choice(&OVERLAY_NAMES, index),
            );
            add(
                ItemId::ControlsAtStart,
                "Show controls at start",
                Widget::Toggle(s.controls_at_start),
            );
        }
        Page::Audio => {
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
            add(ItemId::RadioVolume, "Radio volume", volume(s.radio.volume));
            let kind = ctx.deck.kind() as usize;
            let index = usize::from(s.radio.deck.source == audio::deck::Source::Deck);
            add(ItemId::Source, "Source", choice(&SOURCE_NAMES[kind], index));
            let index = index_of(&audio::deck::Mode::ALL, &ctx.deck.deck().mode(), 0);
            add(
                ItemId::DeckMode,
                "Deck mode",
                choice(&DECK_MODE_NAMES, index),
            );
            let index = ctx.radio.preset_index();
            add(ItemId::RadioPreset, "Preset", choice(&PRESET_TITLES, index));
            let index = usize::from(s.mute_radio_unfocused);
            add(
                ItemId::BackgroundRadio,
                "Radio in background",
                choice(&BACKGROUND_RADIO_NAMES, index),
            );
            let (id, w) = open(Page::Stations);
            add(id, "Stations", w);
            if !ctx.deck.folders().is_empty() || ctx.music_picker || ctx.music_again {
                let (id, w) = open(Page::Music);
                add(id, "Music", w);
            }
        }
        Page::Controls => {
            for (d, label) in [
                (DeviceKind::Gamepad, "Gamepad buttons"),
                (DeviceKind::KeyboardMouse, "Keyboard keys"),
            ] {
                let (id, w) = open(Page::Bindings(d));
                add(id, label, w);
            }
            let b = ctx.bindings;
            let invert = b.axis_options(DeviceKind::Gamepad, Action::LookY).invert;
            add(ItemId::InvertLook, "Invert look", Widget::Toggle(invert));
            for (d, label) in [
                (DeviceKind::Gamepad, "Stick look speed"),
                (DeviceKind::KeyboardMouse, "Mouse look speed"),
            ] {
                add(
                    ItemId::LookSensitivity(d),
                    label,
                    slider(
                        b.axis_options(d, Action::LookX).sensitivity,
                        AxisOptions::SENSITIVITY_MIN,
                        AxisOptions::SENSITIVITY_MAX,
                        0.05,
                        Unit::Times,
                    ),
                );
            }
            add(ItemId::Southpaw, "Swap sticks", Widget::Action);
            add(
                ItemId::SwapTriggers,
                "Throttle on right stick",
                Widget::Action,
            );
            add(ItemId::ResetControls, "Reset controls", Widget::Action);
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
        Page::Stations => {
            for (i, row) in ctx.radio.rows().iter().enumerate() {
                // The label is the station's name, drawn from the row.
                add(
                    ItemId::Station(i),
                    "",
                    Widget::Toggle(s.radio.is_enabled(&row.id)),
                );
            }
            if ctx.text_entry {
                add(ItemId::AddStream, "Add stream URL", Widget::Action);
            }
            if !s.radio.custom.is_empty() {
                add(ItemId::ClearStreams, "Remove added streams", Widget::Action);
            }
        }
        Page::Music => {
            if ctx.music_again {
                add(ItemId::AddMusic, "Pick music folder again", Widget::Action);
            } else if ctx.music_picker {
                add(ItemId::AddMusic, "Add music folder", Widget::Action);
            }
            // Drawn as "Artist - Album" and "Remove <folder>" from the deck.
            for i in 0..ctx.deck.deck().library().albums.len() {
                add(ItemId::Album(i), "", Widget::Action);
            }
            for i in 0..ctx.deck.folders().len() {
                add(ItemId::RemoveFolder(i), "", Widget::Action);
            }
            if !ctx.deck.folders().is_empty() || ctx.music_again {
                add(ItemId::ClearMusic, "Remove all music", Widget::Action);
            }
        }
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

    /// Every page is a tab or opens from a tab's row (two levels at most), and every
    /// label means one thing: no row shares its label with a tab it does not open.
    #[test]
    fn the_tree_is_two_levels_and_labels_are_unique() {
        let settings = Settings::default();
        let bindings = Bindings::default();
        let clock = Clock::new(100, 6.5 * 3600.0);
        let radio = CarRadio::new();
        let deck = CarDeck::default();
        let ctx = PageCtx {
            settings: &settings,
            bindings: &bindings,
            clock: &clock,
            can_quit: true,
            radio: &radio,
            deck: &deck,
            text_entry: true,
            music_picker: true,
            music_again: false,
            quality: Quality::Deck,
            look: render::Look::Standard,
            browse: [3, 1],
            driver: 0,
            scenario: 0,
        };
        let mut items = Vec::new();
        let mut labels: Vec<&str> = TABS.iter().map(|&t| title(t)).collect();
        for tab in TABS {
            assert_eq!(tab_of(tab), tab);
            build(tab, &ctx, &mut items);
            assert!(!items.is_empty(), "{tab:?}");
            for item in items.clone() {
                if let Widget::Submenu(p) = item.widget {
                    assert_eq!(tab_of(p), tab, "{p:?} opens from its own tab");
                    let mut sub = Vec::new();
                    build(p, &ctx, &mut sub);
                    assert!(
                        sub.iter().all(|i| !matches!(i.widget, Widget::Submenu(_))),
                        "{p:?} opens nothing deeper"
                    );
                }
                if !item.label.is_empty() && tab != Page::Worlds {
                    assert!(!labels.contains(&item.label), "{} twice", item.label);
                    labels.push(item.label);
                }
            }
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
        let radio = CarRadio::new();
        let deck = CarDeck::default();
        let ctx = PageCtx {
            settings: &settings,
            bindings: &bindings,
            clock: &clock,
            can_quit: false,
            radio: &radio,
            deck: &deck,
            text_entry: false,
            music_picker: false,
            music_again: false,
            quality: Quality::Deck,
            look: render::Look::Standard,
            browse: [3, 1],
            driver: 0,
            scenario: 0,
        };
        let mut items = Vec::new();
        build(Page::Game, &ctx, &mut items);
        let ids: Vec<ItemId> = items.iter().map(|i| i.id).collect();
        assert_eq!(
            ids,
            [
                ItemId::Resume,
                ItemId::ResetCar,
                ItemId::Controls,
                ItemId::Fullscreen,
            ],
            "no quit on the web"
        );
        build(Page::Worlds, &ctx, &mut items);
        let ids: Vec<ItemId> = items.iter().map(|i| i.id).collect();
        assert_eq!(
            ids,
            [
                ItemId::World(0),
                ItemId::World(1),
                ItemId::World(2),
                ItemId::World(3)
            ]
        );
        assert_eq!(items[1].label, "Mirage Bowl");
        assert!(items.iter().all(|i| i.widget == Widget::Action));
        build(Page::Car, &ctx, &mut items);
        let ids: Vec<ItemId> = items.iter().map(|i| i.id).collect();
        assert_eq!(
            ids,
            [
                ItemId::VehicleCar,
                ItemId::Paint,
                ItemId::Driver,
                ItemId::Vehicle(3)
            ]
        );
        let Widget::Choice { options, index } = items[0].widget else {
            panic!("{:?}", items[0])
        };
        // The coast's cars only: no 2001 car lists in 1984.
        assert_eq!(
            (options.len(), index),
            (crate::scenario::lineup(0).cars.len(), 3)
        );
        assert_eq!(options[3], "Regent Coupe Cabrio");
        let Widget::Choice { options, index } = items[1].widget else {
            panic!("{:?}", items[1])
        };
        assert_eq!((options.len(), index), (7, 1), "the Regent's palette");
        let Widget::Choice { options, .. } = items[2].widget else {
            panic!("{:?}", items[2])
        };
        assert_eq!(options, traffic::figure::PRESET_NAMES);
        assert_eq!(items[3].widget, Widget::Action);
        assert!(ABOUT.contains("Created by Adam Mathes"));
        build(Page::Controls, &ctx, &mut items);
        assert_eq!(items[2].id, ItemId::InvertLook);
        assert_eq!(items[2].widget, Widget::Toggle(true));
        build(Page::Display, &ctx, &mut items);
        assert_eq!(items[0].id, ItemId::Look);
        assert!(
            matches!(items[0].widget, Widget::Choice { index: 0, options } if options.len() == 8),
            "standard of eight"
        );
        assert_eq!(items[1].id, ItemId::Quality);
        assert!(
            matches!(items[1].widget, Widget::Choice { index: 2, .. }),
            "deck"
        );
        assert_eq!(items[3].widget, Widget::Toggle(true));
        assert_eq!(items[4].widget, Widget::Toggle(true));
        assert!(matches!(items[5].widget, Widget::Choice { index: 0, .. }));
        build(Page::Audio, &ctx, &mut items);
        assert!(matches!(items[0].widget, Widget::Slider { value, .. } if value == 0.8));
        assert_eq!(items[3].id, ItemId::RadioVolume);
        assert!(matches!(items[3].widget, Widget::Slider { value, .. } if value == 0.8));
        assert_eq!(items[4].id, ItemId::Source);
        assert!(
            matches!(items[4].widget, Widget::Choice { index: 0, options } if options[1] == "Tape"),
            "the radio; the deck is a tape deck in 1984"
        );
        assert_eq!(items[5].id, ItemId::DeckMode);
        assert!(matches!(items[5].widget, Widget::Choice { index: 0, .. }));
        assert!(matches!(items[6].widget, Widget::Choice { index: 0, .. }));
        assert_eq!(items[7].id, ItemId::BackgroundRadio);
        assert!(
            matches!(items[7].widget, Widget::Choice { index: 0, .. }),
            "keeps playing"
        );
        assert_eq!(items[8].id, ItemId::Open(Page::Stations));
        assert_eq!(items.len(), 9, "no music to show");
        build(Page::Music, &ctx, &mut items);
        assert!(items.is_empty());
        build(Page::Stations, &ctx, &mut items);
        assert_eq!(items.len(), radio.rows().len());
        assert!(items.iter().all(|i| i.widget == Widget::Toggle(true)));
        build(Page::Driving, &ctx, &mut items);
        assert!(
            matches!(items[0].widget, Widget::Choice { index: 0, .. }),
            "arcade"
        );
        assert_eq!(items[2].id, ItemId::Traffic);
        assert!(
            matches!(items[2].widget, Widget::Choice { index: 2, .. }),
            "normal traffic"
        );
        assert!(matches!(items[3].widget, Widget::Slider { value, .. } if value == 6.5));
        assert!(matches!(items[4].widget, Widget::Choice { index: 2, .. }));
        build(Page::Camera, &ctx, &mut items);
        assert!(
            matches!(items[0].widget, Widget::Choice { index: 3, .. }),
            "chase mid"
        );
        assert!(matches!(items[1].widget, Widget::Slider { value, .. } if value == 62.0));
        assert_eq!(items[4].widget, Widget::Toggle(true), "dashboard");
        assert_eq!(items[5].id, ItemId::CockpitDriver);
        assert_eq!(items[5].widget, Widget::Toggle(true));
        assert_eq!(items[6].widget, Widget::Toggle(true));
    }
}
