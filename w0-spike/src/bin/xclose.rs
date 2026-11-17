//! Spike helper: send WM_DELETE_WINDOW to an X11 window, exactly as a window manager's close
//! button does, so winit reports `WindowEvent::CloseRequested`. Disposable test tool.
#![allow(clippy::unwrap_used)]
#[cfg(not(unix))]
fn main() {
    eprintln!("xclose is an X11 helper; on Windows close windows by hand");
}

#[cfg(unix)]
fn main() {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{ClientMessageEvent, ConnectionExt as _, EventMask};
    let arg = std::env::args().nth(1).expect("usage: xclose <window id>");
    let win = if let Some(hex) = arg.strip_prefix("0x") { u32::from_str_radix(hex, 16).unwrap() } else { arg.parse().unwrap() };
    let (conn, _) = x11rb::connect(None).unwrap();
    let protocols = conn.intern_atom(false, b"WM_PROTOCOLS").unwrap().reply().unwrap().atom;
    let delete = conn.intern_atom(false, b"WM_DELETE_WINDOW").unwrap().reply().unwrap().atom;
    let event = ClientMessageEvent::new(32, win, protocols, [delete, 0, 0, 0, 0]);
    conn.send_event(false, win, EventMask::NO_EVENT, event).unwrap();
    conn.flush().unwrap();
    eprintln!("xclose: sent WM_DELETE_WINDOW to {win:#x}");
}
