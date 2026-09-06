// ============================================================================
// Canary Engine
// https://github.com/HylightGames/canary
//
// Copyright (c) 2026-present Canary Engine contributors
//
// Licensed under the MIT License.
// See LICENSE in the project root for details.
// ============================================================================

//! Real, end-to-end verification of [`WinitWindow`]/[`WinitInput`] against
//! an actual (if virtual) X11 display -- not a mock, per this project's
//! standing rule that a claim like "this creates a real window" has to be
//! checked against the real thing.
//!
//! `#[ignore]`d by default: unlike the rest of this crate's tests, these
//! need a live X11 display (`DISPLAY` pointing at a running X server, real
//! or `Xvfb`), which isn't guaranteed in every environment `cargo test`
//! runs in -- notably the default `cargo test --workspace` job in CI,
//! which runs across Ubuntu, macOS, and Windows and doesn't set one up.
//! Run explicitly, with a display available:
//!
//! ```sh
//! Xvfb :99 -screen 0 1280x1024x24 &
//! DISPLAY=:99 cargo test -p canary-platform --features winit-backend \
//!     --test winit_backend_window -- --ignored
//! ```
//!
//! CI runs exactly this, automatically, in a dedicated Linux-only job --
//! see `.github/workflows/ci.yml`'s `windowing-integration` job -- so
//! this is exercised on every push, not just documented as "you can run
//! this manually."
//!
//! Linux/X11-only for now: finding the test window and sending it a
//! graceful close signal is implemented directly against the X11
//! protocol (via `x11rb`), since neither `xdotool windowclose` (destroys
//! the window directly, bypassing the graceful-close path entirely) nor
//! `wmctrl -c` (requires an actual window manager maintaining
//! `_NET_CLIENT_LIST`, which bare `Xvfb` doesn't have) turned out to
//! test the real thing -- confirmed by trying both and inspecting what
//! `winit` actually received. An equivalent macOS/Windows test is real,
//! intended future work, not fixed here.
//!
//! Two more things confirmed the direct way rather than assumed, both
//! real `winit` constraints rather than test-harness bugs:
//! - `winit::event_loop::EventLoop::new()` panics if called off the
//!   process's actual main thread -- which every `cargo test` function
//!   runs on by default. This is why the test below uses
//!   [`WinitWindow::new_for_testing`], not [`WinitWindow::new`] (real
//!   game code should keep using the latter).
//! - `winit` allows creating only **one** `EventLoop` per process, ever
//!   -- a second attempt anywhere in the same test binary fails with
//!   `EventLoopError::RecreationAttempt`, even in a different
//!   `#[test]` fn, even after the first attempt already panicked. This
//!   is why this file has exactly one `#[test]` function rather than
//!   one per concern: `cargo test`'s default harness runs each test
//!   function on its own thread within one shared process, not a
//!   separate process per test, so splitting across functions doesn't
//!   avoid this.

#![cfg(all(feature = "winit-backend", target_os = "linux"))]

use std::time::{Duration, Instant};

use canary_platform::winit_backend::WinitWindow;
use canary_platform::{InputSource, Window, WindowDescriptor};

use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConnectionExt, EventMask, GetPropertyType, Window as X11Window,
};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

/// Finds the (single, first-matching) top-level window whose `WM_NAME`
/// equals `title`, by walking the root window's children -- since bare
/// `Xvfb` has no window manager to ask via a higher-level API.
fn find_window_by_title(conn: &RustConnection, root: X11Window, title: &str) -> Option<X11Window> {
    let tree = conn.query_tree(root).ok()?.reply().ok()?;
    for child in tree.children {
        let prop = conn
            .get_property(
                false,
                child,
                AtomEnum::WM_NAME,
                GetPropertyType::ANY,
                0,
                u32::MAX,
            )
            .ok()?
            .reply()
            .ok()?;
        if let Ok(name) = String::from_utf8(prop.value) {
            if name == title {
                return Some(child);
            }
        }
    }
    None
}

/// Sends a real ICCCM `WM_DELETE_WINDOW` `ClientMessage` -- the same
/// signal a window manager forwards when a user clicks a window's close
/// button -- directly to `window`, without needing a window manager
/// running. See this file's module docs for why this, rather than
/// `xdotool windowclose` or `wmctrl -c`.
fn send_close_request(conn: &RustConnection, window: X11Window) {
    let wm_protocols = conn
        .intern_atom(false, b"WM_PROTOCOLS")
        .expect("intern_atom request failed")
        .reply()
        .expect("intern_atom reply failed")
        .atom;
    let wm_delete_window = conn
        .intern_atom(false, b"WM_DELETE_WINDOW")
        .expect("intern_atom request failed")
        .reply()
        .expect("intern_atom reply failed")
        .atom;

    let event = ClientMessageEvent::new(
        32,
        window,
        wm_protocols,
        [wm_delete_window, x11rb::CURRENT_TIME, 0, 0, 0],
    );
    conn.send_event(false, window, EventMask::NO_EVENT, event)
        .expect("send_event request failed")
        .check()
        .expect("send_event failed");
    conn.flush().expect("flush failed");
}

#[test]
#[ignore = "needs a live X11 display; see this file's module docs"]
fn real_window_lifecycle_end_to_end() {
    // A single test function, deliberately: `winit` allows creating only
    // one `EventLoop` per process, ever (confirmed the direct way -- a
    // second attempt in a separate #[test] fn, even after the first had
    // already panicked, failed with `EventLoopError::RecreationAttempt`).
    // `cargo test`'s default harness runs each #[test] fn on its own
    // thread within one shared process, not a separate process per test,
    // so splitting window-creation and key-input assertions across two
    // functions doesn't avoid this -- everything that needs a real
    // `WinitWindow` has to live in one test.
    let title = "canary-platform winit_backend integration test window";
    let descriptor = WindowDescriptor {
        title: title.to_string(),
        width: 640,
        height: 480,
    };

    let mut window = WinitWindow::new_for_testing(descriptor.clone())
        .expect("failed to create a real winit window -- is DISPLAY set to a running X server?");
    let mut input = window.input_source();

    assert_eq!(window.descriptor(), &descriptor);
    assert!(
        !window.should_close(),
        "a freshly created window should not report should_close()"
    );

    // Drive it through several real poll cycles -- this is what actually
    // pumps the underlying winit event loop; see winit_backend's module
    // docs for why this crate's `Window::poll_events` is the bridge.
    for _ in 0..10 {
        window.poll_events();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !window.should_close(),
        "an unmolested window should still not report should_close() after several polls"
    );

    let (conn, screen_num) =
        RustConnection::connect(None).expect("failed to connect to the X server");
    let root = conn.setup().roots[screen_num].root;
    let x11_window = {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(w) = find_window_by_title(&conn, root, title) {
                break w;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the test window to appear in the X11 window tree"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    };

    // --- Real synthesized key input, via the XTEST extension ---
    let xtest_available = conn
        .extension_information(x11rb::protocol::xtest::X11_EXTENSION_NAME)
        .expect("querying for the XTEST extension failed")
        .is_some();
    assert!(
        xtest_available,
        "the XTEST X11 extension is required to synthesize input for this test; \
         Xvfb should provide it by default"
    );

    conn.set_input_focus(
        x11rb::protocol::xproto::InputFocus::PARENT,
        x11_window,
        x11rb::CURRENT_TIME,
    )
    .expect("set_input_focus request failed")
    .check()
    .expect("set_input_focus failed");
    conn.flush().expect("flush failed");
    std::thread::sleep(Duration::from_millis(100));

    // Keycode 25 is 'W' on a standard US XKB layout (confirmed against
    // this sandbox's Xvfb via `xmodmap -pke`).
    const W_KEYCODE: u8 = 25;
    conn.xtest_fake_input(
        x11rb::protocol::xproto::KEY_PRESS_EVENT,
        W_KEYCODE,
        x11rb::CURRENT_TIME,
        root,
        0,
        0,
        0,
    )
    .expect("xtest_fake_input (press) request failed")
    .check()
    .expect("xtest_fake_input (press) failed");
    conn.xtest_fake_input(
        x11rb::protocol::xproto::KEY_RELEASE_EVENT,
        W_KEYCODE,
        x11rb::CURRENT_TIME,
        root,
        0,
        0,
        0,
    )
    .expect("xtest_fake_input (release) request failed")
    .check()
    .expect("xtest_fake_input (release) failed");
    conn.flush().expect("flush failed");

    let mut observed = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while observed.is_empty() {
        window.poll_events();
        observed.extend(input.poll());
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the synthesized key press to be observed; observed so far: {observed:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        observed.contains(&canary_platform::InputEvent::KeyPressed(
            canary_platform::Key::W
        )),
        "expected a KeyPressed(Key::W) event, got: {observed:?}"
    );

    // --- Graceful close, via a real WM_DELETE_WINDOW ClientMessage ---
    send_close_request(&conn, x11_window);

    let deadline = Instant::now() + Duration::from_secs(5);
    while !window.should_close() {
        window.poll_events();
        assert!(
            Instant::now() < deadline,
            "timed out waiting for should_close() to become true after a real WM_DELETE_WINDOW"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}
