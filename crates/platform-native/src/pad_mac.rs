//! macOS gamepads through Apple's Game Controller framework, as browsers read them there.
//!
//! Once a pad the system knows (Xbox, DualShock 4, DualSense, Switch Pro, MFi, ...) connects,
//! the system's game controller service switches it to the vendor report mode the framework
//! decodes. IOKit HID, gilrs's macOS backend, still finds the device but its elements then
//! carry no input (a moment of buttons after connecting, then nothing). So any controller the
//! framework offers with the extended gamepad profile is read here, and gilrs drives only the
//! pads the framework does not know (generic HID pads).
//!
//! The profile names controls by position like the standard mapping, and its sticks are +Y up
//! like gilrs's, so a reading is fed through `pad::apply` as gilrs `Button`/`Axis` values. The
//! whole profile is read every focused frame (a few dozen messages, no allocation): it is
//! state, not events.

use std::ffi::{c_char, CStr};
use std::ptr::null_mut;

use app::{pad_table, InputMapper};
use objc2::rc::autoreleasepool;
use objc2::runtime::{AnyClass, AnyObject, Bool, MessageReceiver, Sel};
use objc2::{class, sel};

use crate::pad::{apply, note, PadEvent, AXES, BUTTONS};

#[link(name = "GameController", kind = "framework")]
unsafe extern "C" {}

/// The standard-mapping buttons read from the profile's d-pad instead of the profile.
const DPAD: std::ops::Range<usize> = 12..16;

/// What [`GameControllers::poll`] found.
pub struct Reading {
    /// A framework controller is the pad: gilrs's input is ignored.
    pub driving: bool,
    /// The controller being read changed (or went away) this frame.
    pub changed: bool,
}

/// The controller being read, if any.
pub struct GameControllers {
    /// The `GCExtendedGamepad` getter of each standard-mapping button (the d-pad's
    /// `up`/`down`/`left`/`right` for [`DPAD`]).
    buttons: [Sel; 17],
    /// The `GCController` being read, by address, 0 for none. Only compared, never
    /// dereferenced: a new controller at a freed one's address would just go unannounced.
    current: usize,
    verbose: bool,
}

/// `obj.sel` for a getter returning an object; nil in, nil out.
///
/// # Safety
/// `obj` is nil or a live object whose `sel` takes no arguments and returns an object.
unsafe fn object(obj: *mut AnyObject, sel: Sel) -> *mut AnyObject {
    if obj.is_null() {
        return null_mut();
    }
    unsafe { obj.send_message(sel, ()) }
}

/// `obj.sel` for a `float` getter; 0 for nil.
///
/// # Safety
/// As [`object`], for a getter returning `float`.
unsafe fn float(obj: *mut AnyObject, sel: Sel) -> f32 {
    if obj.is_null() {
        return 0.0;
    }
    unsafe { obj.send_message(sel, ()) }
}

/// `obj.sel` for a `BOOL` getter; false for nil.
///
/// # Safety
/// As [`object`], for a getter returning `BOOL`.
unsafe fn flag(obj: *mut AnyObject, sel: Sel) -> bool {
    if obj.is_null() {
        return false;
    }
    let b: Bool = unsafe { obj.send_message(sel, ()) };
    b.as_bool()
}

/// An `NSString`'s text, `"?"` for nil.
///
/// # Safety
/// `s` is nil or a live `NSString`.
unsafe fn text(s: *mut AnyObject) -> String {
    let p: *const c_char = if s.is_null() {
        std::ptr::null()
    } else {
        unsafe { s.send_message(sel!(UTF8String), ()) }
    };
    if p.is_null() {
        return "?".into();
    }
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

impl GameControllers {
    pub fn new(verbose: bool) -> GameControllers {
        GameControllers {
            buttons: [
                sel!(buttonA),
                sel!(buttonB),
                sel!(buttonX),
                sel!(buttonY),
                sel!(leftShoulder),
                sel!(rightShoulder),
                sel!(leftTrigger),
                sel!(rightTrigger),
                sel!(buttonOptions),
                sel!(buttonMenu),
                sel!(leftThumbstickButton),
                sel!(rightThumbstickButton),
                sel!(up),
                sel!(down),
                sel!(left),
                sel!(right),
                sel!(buttonHome),
            ],
            current: 0,
            verbose,
        }
    }

    /// Whether a framework controller is the pad.
    pub fn driving(&self) -> bool {
        self.current != 0
    }

    /// The first connected controller with an extended gamepad profile, and that profile.
    ///
    /// # Safety
    /// Main thread, inside an autorelease pool; the pointers live until the pool drains.
    unsafe fn first() -> Option<(*mut AnyObject, *mut AnyObject)> {
        let class: *const AnyClass = class!(GCController);
        let list = unsafe { object(class.cast_mut().cast(), sel!(controllers)) };
        if list.is_null() {
            return None;
        }
        let count: usize = unsafe { list.send_message(sel!(count), ()) };
        (0..count).find_map(|i| {
            let c: *mut AnyObject = unsafe { list.send_message(sel!(objectAtIndex:), (i,)) };
            let pad = unsafe { object(c, sel!(extendedGamepad)) };
            (!pad.is_null()).then_some((c, pad))
        })
    }

    /// `"vendor name" (product category)` of a controller.
    ///
    /// # Safety
    /// As [`GameControllers::first`]; `c` is a live `GCController`.
    unsafe fn describe(c: *mut AnyObject) -> String {
        unsafe {
            let name = text(object(c, sel!(vendorName)));
            let category = text(object(c, sel!(productCategory)));
            format!("{name:?} ({category})")
        }
    }

    /// One line for the startup summary: the controllers the framework knows so far (it
    /// finds them as the app's run loop starts, so often none yet: they are announced as
    /// they appear).
    pub fn summary(&self) -> String {
        autoreleasepool(|_| unsafe {
            match Self::first() {
                Some((c, _)) => Self::describe(c),
                None => "none yet".into(),
            }
        })
    }

    /// Takes over the pad while the framework has a controller, feeding its whole state
    /// while `focused`. Allocates nothing unless the controller changes.
    pub fn poll(&mut self, input: &mut InputMapper, focused: bool) -> Reading {
        autoreleasepool(|_| {
            // SAFETY: the shell polls on the main thread, inside this pool.
            let found = unsafe { Self::first() };
            let id = found.map_or(0, |(c, _)| c as usize);
            let changed = id != self.current;
            if changed {
                pad_table::release_gamepad(input);
                match found {
                    // SAFETY: as above.
                    Some((c, _)) => note(
                        self.verbose,
                        format_args!("game controller active: {}", unsafe { Self::describe(c) }),
                    ),
                    None => note(self.verbose, format_args!("game controller gone")),
                }
                self.current = id;
            }
            if let (Some((_, pad)), true) = (found, focused) {
                // SAFETY: `pad` is a live GCExtendedGamepad; every getter below is one of
                // its properties (or of its d-pad, sticks and buttons), nil where the
                // controller lacks the control.
                unsafe { self.feed(input, pad) };
            }
            Reading {
                driving: id != 0,
                changed,
            }
        })
    }

    /// # Safety
    /// See [`GameControllers::poll`].
    unsafe fn feed(&self, input: &mut InputMapper, pad: *mut AnyObject) {
        unsafe {
            let dpad = object(pad, sel!(dpad));
            for (i, &sel) in self.buttons.iter().enumerate() {
                let owner = if DPAD.contains(&i) { dpad } else { pad };
                let b = object(owner, sel);
                let value = float(b, sel!(value));
                let pressed = flag(b, sel!(isPressed));
                apply(input, PadEvent::Button(BUTTONS[i], value, pressed));
            }
            let sticks = [sel!(leftThumbstick), sel!(rightThumbstick)];
            for (i, &(axis, _)) in AXES.iter().enumerate() {
                let stick = object(pad, sticks[i / 2]);
                let half = if i % 2 == 0 { sel!(xAxis) } else { sel!(yAxis) };
                let value = float(object(stick, half), sel!(value));
                apply(input, PadEvent::Axis(axis, value));
            }
        }
    }
}
