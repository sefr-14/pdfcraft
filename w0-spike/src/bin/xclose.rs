//! Spike helper: send WM_DELETE_WINDOW to an X11 window, exactly as a window manager's close
//! button does, so winit reports `WindowEvent::CloseRequested`. Disposable test tool.
#![allow(clippy::unwrap_used)]
#[cfg(not(unix))]
fn main() {
    eprintln!("xclose is an X11 helper; on Windows close windows by hand");
}

#[cfg(unix)]
fn main() {
    use x11rb::protocol::xproto::{ClientMessageEvent, ConnectionExt as _, EventMask};
    let arg = std::env::args().nth(1).expect("usage: xclose <window id>");
    let win = if let Some(hex) = arg.strip_prefix("0x") { u32::from_str_radix(hex, 16).unwrap() } else { arg.parse().unwrap() };
    let (conn, _) = x11rb::connect(None).unwrap();
    let protocols = conn.intern_atom(false, b"WM_PROTOCOLS").unwrap().reply().unwrap().atom;
    let delete = conn.intern_atom(false, b"WM_DELETE_WINDOW").unwrap().reply().unwrap().atom;
    let event = ClientMessageEvent::new(32, win, protocols, [delete, 0, 0, 0, 0]);
    if std::env::var_os("XCLOSE_NOCHECK").is_some() {
        // The first W0 run's behaviour: queue, flush and exit without waiting for the server.
        use x11rb::connection::Connection as _;
        let _cookie = conn.send_event(false, win, EventMask::NO_EVENT, event).unwrap();
        conn.flush().unwrap();
        if let Some(ms) = std::env::var("XCLOSE_LINGER_MS").ok().and_then(|v| v.parse::<u64>().ok()) {
            std::thread::sleep(std::time::Duration::from_millis(ms)); // keep the connection open
        }
        eprintln!("xclose: sent WM_DELETE_WINDOW to {win:#x} (no check)");
        return;
    }
    // `check()` waits for the server's answer, so a BadWindow (stale or wrong id) is reported
    // instead of being lost as an asynchronous error.
    match conn.send_event(false, win, EventMask::NO_EVENT, event).unwrap().check() {
        Ok(()) => eprintln!("xclose: sent WM_DELETE_WINDOW to {win:#x} (server accepted)"),
        Err(e) => {
            eprintln!("xclose: server REJECTED WM_DELETE_WINDOW to {win:#x}: {e:?}");
            std::process::exit(2);
        }
    }
}
