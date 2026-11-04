//! The little of libVLC 3 the player needs (libvlc.so from VLC 3, the stable series), behind a safe wrapper.
//!
//! VLC decodes (hardware decoding where it can), mixes the subtitles into the picture and plays the sound; each finished picture
//! lands in memory here (libVLC's "video callbacks"), and the window shows it as a texture. So the player draws everything around
//! the picture itself, in Yggdrasil's look, on any compositor. All VLC-specific code is in this file: libVLC 4 changes some of
//! these calls, and only this file would follow.

use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::sync::Mutex;

#[allow(non_camel_case_types)]
type libvlc_instance_t = c_void;
#[allow(non_camel_case_types)]
type libvlc_media_t = c_void;
#[allow(non_camel_case_types)]
type libvlc_media_player_t = c_void;

#[repr(C)]
struct TrackDescription {
    id: c_int,
    name: *mut c_char,
    next: *mut TrackDescription,
}

type LockCb = unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> *mut c_void;
type UnlockCb = unsafe extern "C" fn(*mut c_void, *mut c_void, *const *mut c_void);
type DisplayCb = unsafe extern "C" fn(*mut c_void, *mut c_void);
type FormatCb = unsafe extern "C" fn(*mut *mut c_void, *mut c_char, *mut c_uint, *mut c_uint, *mut c_uint, *mut c_uint) -> c_uint;
type CleanupCb = unsafe extern "C" fn(*mut c_void);

#[link(name = "vlc")]
extern "C" {
    fn libvlc_new(argc: c_int, argv: *const *const c_char) -> *mut libvlc_instance_t;
    fn libvlc_release(i: *mut libvlc_instance_t);
    fn libvlc_media_new_path(i: *mut libvlc_instance_t, path: *const c_char) -> *mut libvlc_media_t;
    fn libvlc_media_new_location(i: *mut libvlc_instance_t, mrl: *const c_char) -> *mut libvlc_media_t;
    /// libVLC 3: a second input played with the first (type 1: audio)
    fn libvlc_media_slaves_add(m: *mut libvlc_media_t, kind: c_int, priority: c_uint, uri: *const c_char) -> c_int;
    fn libvlc_media_add_option(m: *mut libvlc_media_t, option: *const c_char);
    fn libvlc_media_release(m: *mut libvlc_media_t);
    fn libvlc_media_player_new_from_media(m: *mut libvlc_media_t) -> *mut libvlc_media_player_t;
    fn libvlc_media_player_release(p: *mut libvlc_media_player_t);
    fn libvlc_media_player_play(p: *mut libvlc_media_player_t) -> c_int;
    fn libvlc_media_player_set_pause(p: *mut libvlc_media_player_t, pause: c_int);
    fn libvlc_media_player_stop(p: *mut libvlc_media_player_t);
    fn libvlc_media_player_get_state(p: *mut libvlc_media_player_t) -> c_int;
    fn libvlc_media_player_get_time(p: *mut libvlc_media_player_t) -> i64;
    fn libvlc_media_player_set_time(p: *mut libvlc_media_player_t, t: i64);
    fn libvlc_media_player_get_length(p: *mut libvlc_media_player_t) -> i64;
    fn libvlc_audio_set_volume(p: *mut libvlc_media_player_t, v: c_int) -> c_int;
    fn libvlc_audio_get_track_description(p: *mut libvlc_media_player_t) -> *mut TrackDescription;
    fn libvlc_audio_get_track(p: *mut libvlc_media_player_t) -> c_int;
    fn libvlc_audio_set_track(p: *mut libvlc_media_player_t, id: c_int) -> c_int;
    fn libvlc_video_get_spu_description(p: *mut libvlc_media_player_t) -> *mut TrackDescription;
    fn libvlc_video_get_spu(p: *mut libvlc_media_player_t) -> c_int;
    fn libvlc_video_set_spu(p: *mut libvlc_media_player_t, id: c_int) -> c_int;
    fn libvlc_track_description_list_release(t: *mut TrackDescription);
    fn libvlc_video_set_callbacks(p: *mut libvlc_media_player_t, lock: LockCb, unlock: Option<UnlockCb>, display: Option<DisplayCb>, opaque: *mut c_void);
    fn libvlc_video_set_format_callbacks(p: *mut libvlc_media_player_t, setup: FormatCb, cleanup: Option<CleanupCb>);
}

/// Where VLC is: playing, paused, at the end...
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Opening,
    Playing,
    Paused,
    Stopped,
    Ended,
    Error,
}

/// The latest picture: RGBA, ready for the screen.
pub struct Frame {
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<u8>,
    /// A picture the window has not shown yet.
    pub fresh: bool,
}

/// Shared between VLC's decoding thread and the window.
struct Pictures {
    /// What VLC is writing into (only its thread touches it between lock and display).
    back: Vec<u8>,
    w: usize,
    h: usize,
    shown: Mutex<Frame>,
    wake: Box<dyn Fn() + Send + Sync>,
}

/// The size VLC is asked for: the video's own, but never wider than 3840 pixels (a picture that size already fills any screen,
/// and every pixel more is copied on every frame).
pub fn picture_size(w: u32, h: u32) -> (u32, u32) {
    if w <= 3840 || w == 0 {
        return (w.max(2) & !1, h.max(2) & !1);
    }
    let nh = (h as u64 * 3840 / w as u64) as u32;
    (3840, nh.max(2) & !1)
}

unsafe extern "C" fn format_cb(opaque: *mut *mut c_void, chroma: *mut c_char, w: *mut c_uint, h: *mut c_uint, pitches: *mut c_uint, lines: *mut c_uint) -> c_uint {
    let p = &mut *(*opaque as *mut Pictures);
    let (nw, nh) = picture_size(*w, *h);
    // "RV32": four bytes a pixel, blue green red and an unused byte (converted to RGBA when shown)
    std::ptr::copy_nonoverlapping(b"RV32".as_ptr() as *const c_char, chroma, 4);
    *w = nw;
    *h = nh;
    *pitches = nw * 4;
    *lines = nh;
    p.w = nw as usize;
    p.h = nh as usize;
    p.back = vec![0; p.w * p.h * 4];
    1
}

unsafe extern "C" fn lock_cb(opaque: *mut c_void, planes: *mut *mut c_void) -> *mut c_void {
    let p = &mut *(opaque as *mut Pictures);
    *planes = p.back.as_mut_ptr() as *mut c_void;
    std::ptr::null_mut()
}

unsafe extern "C" fn display_cb(opaque: *mut c_void, _picture: *mut c_void) {
    let p = &mut *(opaque as *mut Pictures);
    if let Ok(mut f) = p.shown.lock() {
        let n = p.w * p.h;
        f.rgba.resize(n * 4, 0);
        for (d, s) in f.rgba.chunks_exact_mut(4).zip(p.back.chunks_exact(4)) {
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
            d[3] = 255;
        }
        f.w = p.w;
        f.h = p.h;
        f.fresh = true;
    }
    (p.wake)();
}

/// One file being played.
pub struct Player {
    inst: *mut libvlc_instance_t,
    mp: *mut libvlc_media_player_t,
    pics: *mut Pictures,
}

// libVLC's player functions may be called from any thread
unsafe impl Send for Player {}

impl Player {
    /// Opens `path` (not playing yet). `wake` is called from VLC's thread after each new picture.
    pub fn open(path: &std::path::Path, wake: Box<dyn Fn() + Send + Sync>) -> Result<Player, String> {
        let cpath = CString::new(path.as_os_str().as_encoded_bytes()).map_err(|_| "the file name has a zero byte".to_string())?;
        Self::with_media(wake, |inst| unsafe { libvlc_media_new_path(inst, cpath.as_ptr()) }, None, None)
    }

    /// Opens a stream (an https:// address), with its sound as a separate stream when the site sends them apart (YouTube above
    /// 720p), from `start` seconds. Used by Huginn.
    pub fn open_stream(url: &str, audio: Option<&str>, start: Option<f64>, wake: Box<dyn Fn() + Send + Sync>) -> Result<Player, String> {
        Self::open_stream_with(url, audio, start, None, None, wake)
    }

    /// The same, asking with the User-Agent and Referer the site issued the streams for (yt-dlp reports them).
    pub fn open_stream_with(url: &str, audio: Option<&str>, start: Option<f64>, user_agent: Option<&str>, referer: Option<&str>, wake: Box<dyn Fn() + Send + Sync>) -> Result<Player, String> {
        // YouTube's streams go through the relay, which asks for them in pieces as YouTube's players do (see relay.rs)
        let url = crate::relay::local(url);
        let audio = audio.map(crate::relay::local);
        let audio = audio.as_deref();
        let curl = CString::new(url).map_err(|_| "the address has a zero byte".to_string())?;
        let mut opts = vec![];
        for (k, v) in [("http-user-agent", user_agent), ("http-referrer", referer)] {
            if let Some(v) = v.filter(|v| !v.contains(['\n', '\r', '\0'])) {
                opts.push(CString::new(format!(":{k}={v}")).map_err(|_| "a header has a zero byte".to_string())?);
            }
        }
        Self::with_media(wake, |inst| unsafe {
            let m = libvlc_media_new_location(inst, curl.as_ptr());
            if !m.is_null() {
                for o in &opts {
                    libvlc_media_add_option(m, o.as_ptr());
                }
            }
            m
        }, audio, start)
    }

    fn with_media(wake: Box<dyn Fn() + Send + Sync>, media: impl FnOnce(*mut libvlc_instance_t) -> *mut libvlc_media_t, audio: Option<&str>, start: Option<f64>) -> Result<Player, String> {
        // YGGDRASIL_PLAYER_DEBUG=1: VLC says what it does (on standard error)
        let talk = if std::env::var_os("YGGDRASIL_PLAYER_DEBUG").is_some() { "--verbose=2" } else { "--quiet" };
        let args: Vec<CString> = ["--no-video-title-show", "--no-osd", "--no-snapshot-preview", talk, "--no-stats"].iter().map(|a| CString::new(*a).unwrap()).collect();
        let argv: Vec<*const c_char> = args.iter().map(|a| a.as_ptr()).collect();
        let caudio = match audio {
            Some(a) => Some(CString::new(a).map_err(|_| "the address has a zero byte".to_string())?),
            None => None,
        };
        unsafe {
            let inst = libvlc_new(argv.len() as c_int, argv.as_ptr());
            if inst.is_null() {
                return Err("VLC could not start (is libvlc installed?)".into());
            }
            let m = media(inst);
            if m.is_null() {
                libvlc_release(inst);
                return Err("VLC could not open it".into());
            }
            if let Some(a) = &caudio {
                libvlc_media_slaves_add(m, 1, 4, a.as_ptr());
            }
            if let Some(t) = start.filter(|t| t.is_finite() && *t > 0.0) {
                if let Ok(o) = CString::new(format!(":start-time={t:.1}")) {
                    libvlc_media_add_option(m, o.as_ptr());
                }
            }
            let mp = libvlc_media_player_new_from_media(m);
            libvlc_media_release(m);
            if mp.is_null() {
                libvlc_release(inst);
                return Err("VLC could not make a player".into());
            }
            let pics = Box::into_raw(Box::new(Pictures { back: vec![], w: 0, h: 0, shown: Mutex::new(Frame { w: 0, h: 0, rgba: vec![], fresh: false }), wake }));
            libvlc_video_set_format_callbacks(mp, format_cb, None);
            libvlc_video_set_callbacks(mp, lock_cb, None, Some(display_cb), pics as *mut c_void);
            Ok(Player { inst, mp, pics })
        }
    }

    pub fn play(&self) {
        unsafe {
            libvlc_media_player_play(self.mp);
        }
    }
    pub fn set_pause(&self, pause: bool) {
        unsafe { libvlc_media_player_set_pause(self.mp, pause as c_int) }
    }
    pub fn state(&self) -> State {
        match unsafe { libvlc_media_player_get_state(self.mp) } {
            0..=2 => State::Opening,
            3 => State::Playing,
            4 => State::Paused,
            5 => State::Stopped,
            6 => State::Ended,
            _ => State::Error,
        }
    }
    /// Where it is, in milliseconds (-1 before it knows).
    pub fn time(&self) -> i64 {
        unsafe { libvlc_media_player_get_time(self.mp) }
    }
    pub fn set_time(&self, ms: i64) {
        unsafe { libvlc_media_player_set_time(self.mp, ms.max(0)) }
    }
    /// How long it is, in milliseconds (0 or -1 before it knows).
    pub fn length(&self) -> i64 {
        unsafe { libvlc_media_player_get_length(self.mp) }
    }
    /// 0 to 125 (VLC's 100 is the file's own loudness).
    pub fn set_volume(&self, v: i32) {
        unsafe {
            libvlc_audio_set_volume(self.mp, v.clamp(0, 125));
        }
    }
    pub fn audio_tracks(&self) -> Vec<(i32, String)> {
        unsafe { tracks(libvlc_audio_get_track_description(self.mp)) }
    }
    pub fn audio_track(&self) -> i32 {
        unsafe { libvlc_audio_get_track(self.mp) }
    }
    pub fn set_audio_track(&self, id: i32) {
        unsafe {
            libvlc_audio_set_track(self.mp, id);
        }
    }
    /// The subtitle tracks (id -1 is "off").
    pub fn subtitles(&self) -> Vec<(i32, String)> {
        unsafe { tracks(libvlc_video_get_spu_description(self.mp)) }
    }
    pub fn subtitle(&self) -> i32 {
        unsafe { libvlc_video_get_spu(self.mp) }
    }
    pub fn set_subtitle(&self, id: i32) {
        unsafe {
            libvlc_video_set_spu(self.mp, id);
        }
    }
    /// Runs `f` on the latest picture (the window takes it from here).
    pub fn with_frame<R>(&self, f: impl FnOnce(&mut Frame) -> R) -> Option<R> {
        let pics = unsafe { &*self.pics };
        pics.shown.lock().ok().map(|mut g| f(&mut g))
    }
}

unsafe fn tracks(list: *mut TrackDescription) -> Vec<(i32, String)> {
    let mut out = vec![];
    let mut t = list;
    while !t.is_null() {
        let name = if (*t).name.is_null() { String::new() } else { CStr::from_ptr((*t).name).to_string_lossy().into_owned() };
        out.push(((*t).id, name));
        t = (*t).next;
    }
    if !list.is_null() {
        libvlc_track_description_list_release(list);
    }
    out
}

impl Drop for Player {
    fn drop(&mut self) {
        unsafe {
            // stop first: it waits for VLC's threads, so nothing calls back into the pictures after they are freed
            libvlc_media_player_stop(self.mp);
            libvlc_media_player_release(self.mp);
            libvlc_release(self.inst);
            drop(Box::from_raw(self.pics));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn big_pictures_are_scaled_to_3840_wide_keeping_the_shape() {
        assert_eq!(picture_size(1920, 1080), (1920, 1080));
        assert_eq!(picture_size(7680, 4320), (3840, 2160));
        assert_eq!(picture_size(1279, 721), (1278, 720), "even sizes");
    }
}
