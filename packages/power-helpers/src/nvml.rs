//! The first NVIDIA card through libnvidia-ml, opened at run time (dlopen, as Python's ctypes did): without the driver the
//! helpers still run and say "libnvidia-ml.so.1: cannot open shared object file ...". Each call returns NVML's own code, so
//! each helper words its errors as its Python version did.
//!
//! A dry run never calls a set function: it prints the call instead ("dry run: nvmlDeviceSetFanSpeed_v2(fan 0, 45)"). With
//! --root the card is a stand-in read from ROOT/nvml.json, so a dry run against a fake tree touches no card at all:
//!   {"temp": 55 or [40, 60, ...] (one reading each, then the card is lost), "fans": [45, 45], "min": 24, "max": 100,
//!    "core": 0, "mem": 0, "max_core": 2100, "fail": {"nvmlInit_v2": 3, ...}}
//! (--real-card takes the real card instead, read only, for a dry run against a fake tree with libnvidia-ml loaded.)
//!
//! Sharing: nvmlInit costs a process about 26 MB (the 620 driver maps libcuda and starts a thread). A Ctx with a Slot (the
//! merged service gives every part the same one) makes Card::open hand out the card that is already open in the process,
//! so the parts share one initialisation; it is shut down when the last of them lets go. NVML is thread-safe, so the parts
//! call it from their own threads with no lock of ours around the calls. A fake card is never shared (each Card::open reads
//! ROOT/nvml.json afresh, as a separate process would), so a dry run of the merged service reads it as the helpers run one
//! by one do.

use std::collections::VecDeque;
use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use crate::json::Json;
use crate::sys::Ctx;

pub const SUCCESS: i32 = 0;
/// NVML_ERROR_FUNCTION_NOT_FOUND: this driver's libnvidia-ml has no such function.
pub const FUNCTION_NOT_FOUND: i32 = 13;
/// NVML_ERROR_GPU_IS_LOST
pub const GPU_IS_LOST: i32 = 15;

pub const TEMPERATURE_GPU: u32 = 0;
pub const CLOCK_GRAPHICS: u32 = 0;
pub const CLOCK_MEM: u32 = 2;

/// nvmlClockOffset_t (version 1)
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ClockOffset {
    version: c_uint,
    kind: c_uint,
    pstate: c_uint,
    offset_mhz: c_int,
    min_offset_mhz: c_int,
    max_offset_mhz: c_int,
}

const CLOCK_OFFSET_V1: c_uint = std::mem::size_of::<ClockOffset>() as c_uint | (1 << 24);

/// Why the card could not be opened.
#[derive(Debug, Clone, PartialEq)]
pub enum OpenError {
    /// dlopen's own message
    Library(String),
    Init(i32),
    Handle(i32),
}

struct Lib(usize);

static LIB: OnceLock<Lib> = OnceLock::new();

/// libnvidia-ml.so.1, opened once and kept (a failure is tried again next time, as a new ctypes.CDLL would).
fn lib() -> Result<&'static Lib, String> {
    if let Some(l) = LIB.get() {
        return Ok(l);
    }
    let name = CString::new("libnvidia-ml.so.1").unwrap();
    let h = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
    if h.is_null() {
        let e = unsafe { libc::dlerror() };
        let msg = if e.is_null() {
            "libnvidia-ml.so.1: cannot be opened".to_string()
        } else {
            unsafe { CStr::from_ptr(e) }.to_string_lossy().into_owned()
        };
        return Err(msg);
    }
    Ok(LIB.get_or_init(|| Lib(h as usize)))
}

impl Lib {
    fn sym(&self, name: &str) -> Option<*mut c_void> {
        let c = CString::new(name).ok()?;
        let p = unsafe { libc::dlsym(self.0 as *mut c_void, c.as_ptr() as *const c_char) };
        (!p.is_null()).then_some(p)
    }
}

/// Calls an NVML function by name with the given C signature; FUNCTION_NOT_FOUND when the library has no such function.
macro_rules! call {
    ($lib:expr, $name:literal, fn($($t:ty),*), $($a:expr),*) => {{
        match $lib.sym($name) {
            None => FUNCTION_NOT_FOUND,
            Some(p) => {
                let f: unsafe extern "C" fn($($t),*) -> c_int = unsafe { std::mem::transmute(p) };
                unsafe { f($($a),*) }
            }
        }
    }};
}

struct Fake {
    temps: VecDeque<u32>,
    temp_fixed: Option<u32>,
    fans: Vec<u32>,
    min: u32,
    max: u32,
    core: i32,
    mem: i32,
    max_core: u32,
    fail: Vec<(String, i32)>,
}

impl Fake {
    fn load(ctx: &Ctx) -> Result<Fake, String> {
        let not_there = || "libnvidia-ml.so.1: cannot open shared object file: No such file or directory".to_string();
        let v = ctx.read_json(&ctx.path("/nvml.json")).map_err(|_| not_there())?;
        let o = v.obj().ok_or_else(not_there)?;
        let u = |k: &str, d: u32| o.at(k).int().map(|x| x.clamp(0, u32::MAX as i64) as u32).unwrap_or(d);
        let (temps, temp_fixed) = match o.at("temp") {
            Json::Arr(a) => (a.iter().filter_map(|x| x.int()).map(|x| x.max(0) as u32).collect(), None),
            t => (VecDeque::new(), Some(t.int().map(|x| x.max(0) as u32).unwrap_or(40))),
        };
        Ok(Fake {
            temps,
            temp_fixed,
            fans: o.at("fans").arr().map(|a| a.iter().filter_map(|x| x.int()).map(|x| x.max(0) as u32).collect()).unwrap_or_default(),
            min: u("min", 0),
            max: u("max", 100),
            core: o.at("core").int().unwrap_or(0) as i32,
            mem: o.at("mem").int().unwrap_or(0) as i32,
            max_core: u("max_core", 0),
            fail: o
                .at("fail")
                .obj()
                .map(|f| f.iter().filter_map(|(k, v)| Some((k.to_string(), v.int()? as i32))).collect())
                .unwrap_or_default(),
        })
    }

    fn fails(&self, name: &str) -> Option<i32> {
        self.fail.iter().find(|(k, _)| k == name).map(|(_, c)| *c)
    }
}

enum Backend {
    Real { lib: &'static Lib, dev: *mut c_void },
    Fake(Mutex<Fake>),
}

/// The card itself: nvmlInit_v2 done and the handle taken; nvmlShutdown when the last Card holding it is dropped.
struct Inner {
    backend: Backend,
}

// NVML is thread-safe (its documentation: calls may be made from several threads at once), and a device handle stays valid
// until nvmlShutdown, which only Inner's drop calls, once nothing can use the handle any more.
unsafe impl Send for Inner {}
unsafe impl Sync for Inner {}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Backend::Real { lib, .. } = &self.backend {
            call!(lib, "nvmlShutdown", fn(),);
        }
    }
}

/// The card that is open in the process, for the merged service's parts to share (see the top of this file).
#[derive(Default)]
pub struct Slot(Mutex<Weak<Inner>>);

impl Slot {
    pub fn new() -> Slot {
        Slot::default()
    }

    /// Whether a card is open through this slot now.
    pub fn is_open(&self) -> bool {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).strong_count() > 0
    }
}

/// The first card, open (nvmlInit_v2 done) until dropped (nvmlShutdown, once no other Card shares it).
pub struct Card {
    inner: Arc<Inner>,
    dry: bool,
}

impl Card {
    pub fn open(ctx: &Ctx) -> Result<Card, OpenError> {
        if ctx.fake_card() {
            let f = Fake::load(ctx).map_err(OpenError::Library)?;
            if let Some(c) = f.fails("nvmlInit_v2") {
                return Err(OpenError::Init(c));
            }
            if let Some(c) = f.fails("nvmlDeviceGetHandleByIndex_v2") {
                return Err(OpenError::Handle(c));
            }
            return Ok(Card { inner: Arc::new(Inner { backend: Backend::Fake(Mutex::new(f)) }), dry: ctx.dry });
        }
        let Some(slot) = &ctx.cards else {
            return Ok(Card { inner: Arc::new(open_real()?), dry: ctx.dry });
        };
        // the slot is held while a card is opened, so two parts starting together open it once
        let mut held = slot.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(inner) = held.upgrade() {
            return Ok(Card { inner, dry: ctx.dry });
        }
        let inner = Arc::new(open_real()?);
        *held = Arc::downgrade(&inner);
        Ok(Card { inner, dry: ctx.dry })
    }

    fn backend(&self) -> &Backend {
        &self.inner.backend
    }

    /// Whether this card and another are the same opened card (the merged service's parts share one).
    pub fn same(&self, other: &Card) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    fn fake_get<T>(&self, name: &str, f: impl FnOnce(&mut Fake) -> Result<T, i32>) -> Result<T, i32> {
        let Backend::Fake(fk) = self.backend() else { unreachable!() };
        let mut fk = fk.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = fk.fails(name) {
            return Err(c);
        }
        f(&mut fk)
    }

    fn ok(rc: i32) -> Result<(), i32> {
        if rc == SUCCESS {
            Ok(())
        } else {
            Err(rc)
        }
    }

    pub fn temp(&self) -> Result<u32, i32> {
        match self.backend() {
            Backend::Real { lib, dev } => {
                let mut v: c_uint = 0;
                Self::ok(call!(lib, "nvmlDeviceGetTemperature", fn(*mut c_void, c_uint, *mut c_uint), *dev, TEMPERATURE_GPU, &mut v))?;
                Ok(v)
            }
            Backend::Fake(_) => self.fake_get("nvmlDeviceGetTemperature", |f| match f.temp_fixed {
                Some(t) => Ok(t),
                None => f.temps.pop_front().ok_or(GPU_IS_LOST),
            }),
        }
    }

    pub fn num_fans(&self) -> Result<u32, i32> {
        match self.backend() {
            Backend::Real { lib, dev } => {
                let mut v: c_uint = 0;
                Self::ok(call!(lib, "nvmlDeviceGetNumFans", fn(*mut c_void, *mut c_uint), *dev, &mut v))?;
                Ok(v)
            }
            Backend::Fake(_) => self.fake_get("nvmlDeviceGetNumFans", |f| Ok(f.fans.len() as u32)),
        }
    }

    pub fn fan_speed(&self, i: u32) -> Result<u32, i32> {
        match self.backend() {
            Backend::Real { lib, dev } => {
                let mut v: c_uint = 0;
                Self::ok(call!(lib, "nvmlDeviceGetFanSpeed_v2", fn(*mut c_void, c_uint, *mut c_uint), *dev, i, &mut v))?;
                Ok(v)
            }
            Backend::Fake(_) => self.fake_get("nvmlDeviceGetFanSpeed_v2", |f| f.fans.get(i as usize).copied().ok_or(2)),
        }
    }

    pub fn min_max_fan_speed(&self) -> Result<(u32, u32), i32> {
        match self.backend() {
            Backend::Real { lib, dev } => {
                let (mut lo, mut hi): (c_uint, c_uint) = (0, 0);
                Self::ok(call!(lib, "nvmlDeviceGetMinMaxFanSpeed", fn(*mut c_void, *mut c_uint, *mut c_uint), *dev, &mut lo, &mut hi))?;
                Ok((lo, hi))
            }
            Backend::Fake(_) => self.fake_get("nvmlDeviceGetMinMaxFanSpeed", |f| Ok((f.min, f.max))),
        }
    }

    pub fn clock_offset(&self, kind: u32) -> Result<i32, i32> {
        match self.backend() {
            Backend::Real { lib, dev } => {
                let mut o = ClockOffset { version: CLOCK_OFFSET_V1, kind, pstate: 0, ..Default::default() };
                Self::ok(call!(lib, "nvmlDeviceGetClockOffsets", fn(*mut c_void, *mut ClockOffset), *dev, &mut o))?;
                Ok(o.offset_mhz)
            }
            Backend::Fake(_) => self.fake_get("nvmlDeviceGetClockOffsets", |f| Ok(if kind == CLOCK_MEM { f.mem } else { f.core })),
        }
    }

    pub fn max_clock(&self, kind: u32) -> Result<u32, i32> {
        match self.backend() {
            Backend::Real { lib, dev } => {
                let mut v: c_uint = 0;
                Self::ok(call!(lib, "nvmlDeviceGetMaxClockInfo", fn(*mut c_void, c_uint, *mut c_uint), *dev, kind, &mut v))?;
                Ok(v)
            }
            Backend::Fake(_) => self.fake_get("nvmlDeviceGetMaxClockInfo", |f| Ok(f.max_core)),
        }
    }

    // ------------------------------------------------------------ the calls that change the card (never in a dry run)

    /// A dry run's set call: printed, never made. A stand-in card may be told to refuse it ("fail"), so the way a refusal is
    /// handled can be seen too.
    fn dry_say(&self, name: &str, what: String) -> Result<(), i32> {
        if let Backend::Fake(f) = self.backend() {
            if let Some(c) = f.lock().unwrap_or_else(|e| e.into_inner()).fails(name) {
                return Err(c);
            }
        }
        crate::out!("dry run: {what}");
        Ok(())
    }

    pub fn set_fan_speed(&self, i: u32, pct: u32) -> Result<(), i32> {
        if self.dry {
            return self.dry_say("nvmlDeviceSetFanSpeed_v2", format!("nvmlDeviceSetFanSpeed_v2(fan {i}, {pct})"));
        }
        match self.backend() {
            Backend::Real { lib, dev } => Self::ok(call!(lib, "nvmlDeviceSetFanSpeed_v2", fn(*mut c_void, c_uint, c_uint), *dev, i, pct)),
            Backend::Fake(_) => Ok(()),
        }
    }

    pub fn set_default_fan_speed(&self, i: u32) -> Result<(), i32> {
        if self.dry {
            return self.dry_say("nvmlDeviceSetDefaultFanSpeed_v2", format!("nvmlDeviceSetDefaultFanSpeed_v2(fan {i})"));
        }
        match self.backend() {
            Backend::Real { lib, dev } => Self::ok(call!(lib, "nvmlDeviceSetDefaultFanSpeed_v2", fn(*mut c_void, c_uint), *dev, i)),
            Backend::Fake(_) => Ok(()),
        }
    }

    pub fn set_clock_offset(&self, kind: u32, mhz: i32) -> Result<(), i32> {
        if self.dry {
            return self.dry_say("nvmlDeviceSetClockOffsets", format!("nvmlDeviceSetClockOffsets(type {kind}, pstate 0, {mhz} MHz)"));
        }
        match self.backend() {
            Backend::Real { lib, dev } => {
                let mut o = ClockOffset { version: CLOCK_OFFSET_V1, kind, pstate: 0, offset_mhz: mhz, ..Default::default() };
                Self::ok(call!(lib, "nvmlDeviceSetClockOffsets", fn(*mut c_void, *mut ClockOffset), *dev, &mut o))
            }
            Backend::Fake(_) => Ok(()),
        }
    }

    pub fn set_locked_clocks(&self, max_mhz: u32) -> Result<(), i32> {
        if self.dry {
            return self.dry_say("nvmlDeviceSetGpuLockedClocks", format!("nvmlDeviceSetGpuLockedClocks(0, {max_mhz})"));
        }
        match self.backend() {
            Backend::Real { lib, dev } => {
                Self::ok(call!(lib, "nvmlDeviceSetGpuLockedClocks", fn(*mut c_void, c_uint, c_uint), *dev, 0, max_mhz))
            }
            Backend::Fake(_) => Ok(()),
        }
    }

    pub fn reset_locked_clocks(&self) -> Result<(), i32> {
        if self.dry {
            return self.dry_say("nvmlDeviceResetGpuLockedClocks", "nvmlDeviceResetGpuLockedClocks()".into());
        }
        match self.backend() {
            Backend::Real { lib, dev } => Self::ok(call!(lib, "nvmlDeviceResetGpuLockedClocks", fn(*mut c_void), *dev)),
            Backend::Fake(_) => Ok(()),
        }
    }
}

/// nvmlInit_v2 and the first card's handle. A failed handle shuts NVML down again (each nvmlInit is counted, and the merged
/// service lives on after a failed open).
fn open_real() -> Result<Inner, OpenError> {
    let lib = lib().map_err(OpenError::Library)?;
    let rc = crate::sys::without_stop_signals(|| call!(lib, "nvmlInit_v2", fn(),));
    if rc != SUCCESS {
        return Err(OpenError::Init(rc));
    }
    let mut dev: *mut c_void = std::ptr::null_mut();
    let rc = call!(lib, "nvmlDeviceGetHandleByIndex_v2", fn(c_uint, *mut *mut c_void), 0, &mut dev);
    if rc != SUCCESS {
        call!(lib, "nvmlShutdown", fn(),);
        return Err(OpenError::Handle(rc));
    }
    Ok(Inner { backend: Backend::Real { lib, dev } })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{ctx, TempDir};

    #[test]
    fn parts_with_one_slot_share_the_card_and_let_it_go_together() {
        let d = TempDir::new();
        d.files("", &[("nvml.json", r#"{"temp": [40, 41, 42], "fans": [30]}"#)]);
        // a fake card is never shared: each open reads nvml.json afresh, as a separate process would
        let mut a = ctx(d.path());
        a.cards = Some(Arc::new(Slot::new()));
        let (x, y) = (Card::open(&a).unwrap(), Card::open(&a).unwrap());
        assert!(!x.same(&y));
        assert_eq!((x.temp(), y.temp()), (Ok(40), Ok(40)));
        // the slot's own rule, on stand-in cards: one card while anyone holds it, a new one after the last lets go
        let slot = Slot::new();
        let first = Arc::new(Inner { backend: Backend::Fake(Mutex::new(Fake::load(&a).unwrap())) });
        *slot.0.lock().unwrap() = Arc::downgrade(&first);
        assert!(slot.is_open());
        let again = slot.0.lock().unwrap().upgrade().unwrap();
        assert!(Arc::ptr_eq(&first, &again));
        drop((first, again));
        assert!(!slot.is_open(), "shut down once the last part let go");
    }
}
