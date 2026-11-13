//! The bar's clock wakes the panel once a minute, on the minute, and once a second only while something shows seconds (the
//! calendar, Fylgja, a countdown). It is a timer on the wall clock (timerfd, CLOCK_REALTIME, an absolute time): after a suspend
//! it fires as soon as the computer wakes, and when the clock is set (NTP, Settings) it fires at once (CANCEL_ON_SET), so the
//! bar never shows a stale time. A new time zone (/etc/localtime replaced) is noticed through a watch on /etc.

use std::os::fd::{AsFd, OwnedFd};

use rustix::time::{Itimerspec, TimerfdClockId, TimerfdFlags, TimerfdTimerFlags, Timespec};

pub struct Clock {
    fd: OwnedFd,
}

/// The next whole minute (or second) after `now` (seconds since 1970, and nanoseconds).
pub fn next_tick(now: (i64, i64), per_second: bool) -> i64 {
    let step = if per_second { 1 } else { 60 };
    (now.0.div_euclid(step) + 1) * step
}

impl Clock {
    pub fn new() -> std::io::Result<Clock> {
        let fd = rustix::time::timerfd_create(TimerfdClockId::Realtime, TimerfdFlags::CLOEXEC | TimerfdFlags::NONBLOCK)?;
        Ok(Clock { fd })
    }

    /// A second handle to the timer, for the event loop to watch.
    pub fn poll_fd(&self) -> std::io::Result<OwnedFd> {
        self.fd.as_fd().try_clone_to_owned()
    }

    /// Fire at the next minute (or second), once.
    pub fn arm(&self, per_second: bool) {
        let now = rustix::time::clock_gettime(rustix::time::ClockId::Realtime);
        let at = next_tick((now.tv_sec, now.tv_nsec as i64), per_second);
        let spec = Itimerspec { it_interval: Timespec { tv_sec: 0, tv_nsec: 0 }, it_value: Timespec { tv_sec: at, tv_nsec: 0 } };
        let _ = rustix::time::timerfd_settime(&self.fd, TimerfdTimerFlags::ABSTIME | TimerfdTimerFlags::CANCEL_ON_SET, &spec);
    }

    /// Takes the expiry (or the news that the clock was set: ECANCELED), so the timer is not ready again until it is armed.
    pub fn read(&self) {
        let mut buf = [0u8; 8];
        let _ = rustix::io::read(&self.fd, &mut buf);
    }
}

/// A watch on /etc for the time zone being replaced (timedatectl swaps the /etc/localtime link). Only the folder's own entries
/// changing wakes it, which at idle never happens.
pub fn zone_watch() -> std::io::Result<OwnedFd> {
    zone_watch_in(std::path::Path::new("/etc"))
}

fn zone_watch_in(etc: &std::path::Path) -> std::io::Result<OwnedFd> {
    use rustix::fs::inotify::{self, CreateFlags, WatchFlags};
    let fd = inotify::init(CreateFlags::CLOEXEC | CreateFlags::NONBLOCK)?;
    inotify::add_watch(&fd, etc, WatchFlags::CREATE | WatchFlags::MOVED_TO | WatchFlags::DELETE | WatchFlags::ONLYDIR)?;
    Ok(fd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_land_on_the_minute_or_the_second() {
        // 23.5 s past a minute
        let t = 1_794_501_683;
        assert_eq!(next_tick((t, 500_000_000), false), 1_794_501_720, "the next whole minute");
        assert_eq!(next_tick((t, 500_000_000), true), t + 1, "the next whole second");
        assert_eq!(next_tick((1_794_501_720, 0), false), 1_794_501_780, "on the minute: the one after");
    }

    #[test]
    fn a_new_time_zone_is_noticed_and_a_file_written_in_place_is_not() {
        use rustix::fs::inotify::Reader;
        let etc = std::env::temp_dir().join(format!("ygg-clock-etc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&etc);
        std::fs::create_dir_all(&etc).unwrap();
        std::fs::write(etc.join("hostname"), "a\n").unwrap();
        let fd = zone_watch_in(&etc).unwrap();
        let names = |fd: &OwnedFd| {
            let mut buf = [std::mem::MaybeUninit::<u8>::uninit(); 1024];
            let mut r = Reader::new(fd, &mut buf);
            let mut v = vec![];
            while let Ok(e) = r.next() {
                v.extend(e.file_name().map(|n| n.to_string_lossy().into_owned()));
            }
            v
        };
        std::fs::write(etc.join("hostname"), "b\n").unwrap();
        assert!(names(&fd).is_empty(), "a file written in place wakes nothing");
        // as timedatectl does: a new link beside, renamed over /etc/localtime
        std::os::unix::fs::symlink("/usr/share/zoneinfo/Europe/Paris", etc.join(".localtime.tmp")).unwrap();
        std::fs::rename(etc.join(".localtime.tmp"), etc.join("localtime")).unwrap();
        assert!(names(&fd).contains(&"localtime".to_string()));
        let _ = std::fs::remove_dir_all(&etc);
    }

    #[test]
    fn the_timer_fires_on_the_second() {
        let c = Clock::new().unwrap();
        c.arm(true);
        let start = std::time::Instant::now();
        let mut pfd = [rustix::event::PollFd::new(&c.fd, rustix::event::PollFlags::IN)];
        let n = rustix::event::poll(&mut pfd, Some(&Timespec { tv_sec: 2, tv_nsec: 0 })).unwrap();
        assert_eq!(n, 1, "it fired");
        assert!(start.elapsed() <= std::time::Duration::from_millis(1100));
        let now = rustix::time::clock_gettime(rustix::time::ClockId::Realtime);
        assert!(now.tv_nsec < 100_000_000, "on a whole second ({} ns past it)", now.tv_nsec);
        c.read();
        let n = rustix::event::poll(&mut pfd, Some(&Timespec { tv_sec: 0, tv_nsec: 0 })).unwrap();
        assert_eq!(n, 0, "read: not ready again until armed");
    }
}
