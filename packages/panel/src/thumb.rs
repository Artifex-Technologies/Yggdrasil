//! A picture of a window for the dock's window list, taken through KWin's screenshot service (`org.kde.KWin.ScreenShot2`).
//! KWin only answers programs whose desktop file asks for that interface (labyrinth-panel.desktop does), and the picture
//! never leaves this process: it is shrunk to a thumbnail, shown, and dropped.

use std::collections::HashMap;
use std::io::Read;
use std::os::fd::{FromRawFd, OwnedFd};

use resvg::tiny_skia::Pixmap;
use zbus::blocking::Connection;
use zbus::zvariant::{OwnedValue, Value};

/// One window's picture, scaled to fit `box_w` x `box_h` pixels (aspect kept).
pub fn capture(uuid: &str, box_w: u32, box_h: u32) -> Result<Pixmap, String> {
    let conn = Connection::session().map_err(|e| e.to_string())?;
    let mut fds = [0i32; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err("pipe".into());
    }
    let (read_end, write_end) = unsafe { (std::fs::File::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    // KWin writes the image into the pipe while the call is open, so something has to be reading already
    let reader = std::thread::spawn(move || {
        let mut f = read_end;
        let mut buf = Vec::new();
        let _ = f.read_to_end(&mut buf);
        buf
    });
    let mut options: HashMap<&str, Value> = HashMap::new();
    options.insert("include-cursor", Value::from(false));
    options.insert("include-decoration", Value::from(false));
    let fd = zbus::zvariant::Fd::from(&write_end);
    let reply = conn.call_method(Some("org.kde.KWin"), "/org/kde/KWin/ScreenShot2", Some("org.kde.KWin.ScreenShot2"), "CaptureWindow", &(uuid, options, fd));
    drop(write_end);
    let reply = reply.map_err(|e| e.to_string())?;
    let result: HashMap<String, OwnedValue> = reply.body().deserialize().map_err(|e| e.to_string())?;
    let num = |k: &str| result.get(k).and_then(|v| u32::try_from(v).ok()).unwrap_or(0);
    let (w, h, stride, format) = (num("width"), num("height"), num("stride"), num("format"));
    let data = reader.join().map_err(|_| "reader".to_string())?;
    if w == 0 || h == 0 || stride < w * 4 || data.len() < (stride * h) as usize {
        return Err("no picture".into());
    }
    // QImage Format_ARGB32_Premultiplied (6) or Format_ARGB32 (5), little endian: B G R A
    let mut src = Pixmap::new(w, h).ok_or("size")?;
    let out = src.data_mut();
    for y in 0..h as usize {
        for x in 0..w as usize {
            let i = y * stride as usize + x * 4;
            let (b, g, r, a) = (data[i], data[i + 1], data[i + 2], data[i + 3]);
            let o = (y * w as usize + x) * 4;
            if format == 5 {
                let p = |c: u8| ((c as u32 * a as u32 + 127) / 255) as u8;
                out[o..o + 4].copy_from_slice(&[p(r), p(g), p(b), a]);
            } else {
                out[o..o + 4].copy_from_slice(&[r, g, b, a]);
            }
        }
    }
    let scale = (box_w as f32 / w as f32).min(box_h as f32 / h as f32).min(1.0);
    let (dw, dh) = (((w as f32 * scale).round() as u32).max(1), ((h as f32 * scale).round() as u32).max(1));
    crate::paint::area_resize(&src, dw, dh).ok_or_else(|| "resize".to_string())
}
