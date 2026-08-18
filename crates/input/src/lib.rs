use std::time::Duration;

use crossbeam_channel::{bounded, Receiver, Sender};
use litedroid_core::{AndroidButton, InputEvent, MouseButton, Result};

// ---------------------------------------------------------------------------
// x11_to_linux_keycode
// ---------------------------------------------------------------------------

/// Map an X11 keycode (with the standard +8 offset) to a Linux evdev keycode.
///
/// Returns 0 for unmapped keycodes.
pub fn x11_to_linux_keycode(x11_keycode: u32) -> u32 {
    // Lookup table: (x11_keycode, linux_evdev_keycode)
    const MAP: &[(u32, u32)] = &[
        // Row: ESC, 1-9, 0
        (9, 1),
        (10, 2),
        (11, 3),
        (12, 4),
        (13, 5),
        (14, 6),
        (15, 7),
        (16, 8),
        (17, 9),
        (18, 10),
        (19, 11),
        // QWERTYUIOP
        (24, 16),
        (25, 17),
        (26, 18),
        (27, 19),
        (28, 20),
        (29, 21),
        (30, 22),
        (31, 23),
        (32, 24),
        (33, 25),
        // ASDFGHJKL
        (38, 30),
        (39, 31),
        (40, 32),
        (41, 33),
        (42, 34),
        (43, 35),
        (44, 36),
        (45, 37),
        (46, 38),
        // ZXCVBNM
        (52, 44),
        (53, 45),
        (54, 46),
        (55, 47),
        (56, 48),
        (57, 49),
        (58, 50),
        // TAB, ENTER, BACKSPACE, SPACE
        (23, 15),
        (36, 28),
        (22, 14),
        (65, 57),
        // Modifiers
        (50, 42),
        (37, 29),
        (64, 56),
        // F1 – F12
        (67, 59),
        (68, 60),
        (69, 61),
        (70, 62),
        (71, 63),
        (72, 64),
        (73, 65),
        (74, 66),
        (75, 67),
        (76, 68),
        (77, 87),
        (78, 88),
        // Navigation
        (110, 102), // HOME
        (115, 107), // END
        (113, 105), // LEFT
        (111, 103), // UP
        (114, 106), // RIGHT
        (116, 108), // DOWN
    ];

    MAP.iter()
        .find(|&&(x, _)| x == x11_keycode)
        .map(|&(_, linux)| linux)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// InputManager
// ---------------------------------------------------------------------------

/// Central input event queue. The host-side UI injects events; the VMM or
/// device model consumes them.
pub struct InputManager {
    tx: Sender<InputEvent>,
    rx: Receiver<InputEvent>,
    display_width: u32,
    display_height: u32,
}

impl InputManager {
    /// Create a new input manager with a bounded channel of capacity 1024.
    pub fn new(display_width: u32, display_height: u32) -> Self {
        let (tx, rx) = bounded(1024);
        Self {
            tx,
            rx,
            display_width,
            display_height,
        }
    }

    // -- Mouse ---------------------------------------------------------------

    pub fn inject_mouse_down(&self, x: i32, y: i32, button: MouseButton) {
        let _ = self.tx.try_send(InputEvent::MouseDown { button, x, y });
    }

    pub fn inject_mouse_up(&self, x: i32, y: i32, button: MouseButton) {
        let _ = self.tx.try_send(InputEvent::MouseUp { button, x, y });
    }

    pub fn inject_mouse_move(&self, x: i32, y: i32) {
        let _ = self.tx.try_send(InputEvent::MouseMove { x, y });
    }

    // -- Keyboard -------------------------------------------------------------

    pub fn inject_key_down(&self, keycode: u32) {
        let _ = self.tx.try_send(InputEvent::KeyDown { keycode });
    }

    pub fn inject_key_up(&self, keycode: u32) {
        let _ = self.tx.try_send(InputEvent::KeyUp { keycode });
    }

    // -- Touch ---------------------------------------------------------------

    pub fn inject_touch_start(&self, tracking_id: u32, x: i32, y: i32) {
        let _ = self.tx.try_send(InputEvent::TouchStart {
            slot: tracking_id,
            x,
            y,
        });
    }

    pub fn inject_touch_move(&self, tracking_id: u32, x: i32, y: i32) {
        let _ = self.tx.try_send(InputEvent::TouchMove {
            slot: tracking_id,
            x,
            y,
        });
    }

    pub fn inject_touch_end(&self, tracking_id: u32) {
        let _ = self.tx.try_send(InputEvent::TouchEnd { slot: tracking_id });
    }

    // -- Android buttons ------------------------------------------------------

    pub fn inject_android_button(&self, button: AndroidButton) {
        let _ = self.tx.try_send(InputEvent::AndroidButton(button));
    }

    // -- Consumption ----------------------------------------------------------

    /// Try to receive an event without blocking.
    pub fn try_recv(&self) -> Option<InputEvent> {
        self.rx.try_recv().ok()
    }

    /// Receive an event with a timeout.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<InputEvent> {
        self.rx
            .recv_timeout(timeout)
            .map_err(|_| litedroid_core::LiteDroidError::InputError("channel recv timeout".into()))
    }

    // -- Accessors -----------------------------------------------------------

    pub fn display_width(&self) -> u32 {
        self.display_width
    }

    pub fn display_height(&self) -> u32 {
        self.display_height
    }
}
