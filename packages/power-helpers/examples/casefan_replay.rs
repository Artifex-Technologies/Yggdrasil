//! casefan's service loop over recorded temperatures, for parity/compare.py: reads {"setting": {...}, "temps": [[cpu, gpu],
//! ...]} on stdin and prints each write ("6 25") and the exit code. The setting is read once, as the service reads it; the clock
//! starts at 1000 s and moves 2 s a round; past the last reading the sensor fails, as the Python tests' fakes do.

use std::io::Read;

use power_helpers::casefan::{self, Cfg, Env, Fail};
use power_helpers::json::{self, Json};
use power_helpers::py::OsErr;

struct Replay {
    cfg: Option<Cfg>,
    temps: std::collections::VecDeque<(i64, i64)>,
    clock: f64,
}

impl Env for Replay {
    fn read_cfg(&mut self) -> Option<Result<Cfg, String>> {
        self.cfg.take().map(Ok)
    }
    fn load(&mut self) -> Result<Cfg, String> {
        casefan::clean(&Json::Null)
    }
    fn read_cpu(&mut self) -> Result<i64, Fail> {
        self.temps.front().map(|t| t.0).ok_or(Fail::Sensor("the recorded readings ran out".into()))
    }
    fn read_gpu(&mut self) -> Result<i64, Fail> {
        Ok(self.temps.pop_front().map(|t| t.1).unwrap_or(0))
    }
    fn write(&mut self, port: &str, pct: i64) -> Result<(), OsErr> {
        println!("{port} {pct}");
        Ok(())
    }
    fn sleep(&mut self, s: i64) {
        self.clock += s as f64;
    }
    fn should_stop(&mut self) -> bool {
        false
    }
    fn clock(&mut self) -> f64 {
        self.clock
    }
}

fn main() {
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text).unwrap();
    let v = json::parse(&text).expect("JSON on stdin");
    let o = v.obj().expect("an object");
    let cfg = casefan::clean(o.at("setting")).expect("a usable setting");
    let temps = o.at("temps").arr().expect("temps").iter().map(|p| {
        let p = p.arr().unwrap();
        (p[0].int().unwrap(), p[1].int().unwrap())
    });
    let code = casefan::run(&mut Replay { cfg: Some(cfg), temps: temps.collect(), clock: 1000.0 });
    println!("exit {code}");
}
