//! The plan, fuzzed: any bytes on standard input are read and checked as the program does, and given to the whole program on
//! a computer that runs nothing (every program "fails" and is only written down). Never a panic; a refused plan (exit code 2)
//! runs no program; every run ends with DONE or an ERROR line.
#![no_main]

use install_system::install::Host;
use install_system::py::R;
use install_system::sys::{Output, System};
use libfuzzer_sys::fuzz_target;

#[derive(Default)]
struct Nothing {
    calls: usize,
    last: String,
}

impl System for Nothing {
    fn run(&mut self, _argv: &[String], _stdin: Option<&str>) -> R<Output> {
        self.calls += 1;
        Ok(Output {
            code: 1,
            stdout: String::new(),
            stderr: "not run\n".into(),
        })
    }
    fn stream(
        &mut self,
        _argv: &[String],
        _line: &mut dyn FnMut(&str) -> R<Option<String>>,
    ) -> R<(i32, String)> {
        self.calls += 1;
        Ok((1, String::new()))
    }
    fn sleep(&mut self, _seconds: u64) {}
    fn random(&mut self, n: usize) -> Vec<u8> {
        vec![0; n]
    }
    fn account_exists(&mut self, login: &str) -> bool {
        login == "taken"
    }
    fn euid(&mut self) -> u32 {
        0
    }
    fn out(&mut self, line: &str) -> R<()> {
        self.last = line.to_string();
        Ok(())
    }
    fn err(&mut self, _line: &str) {}
}

fuzz_target!(|data: &[u8]| {
    if let Ok(plan) = install_system::read_plan(data) {
        let os = install_system::conf::Os::default();
        let mut exists = |l: &str| l == "taken";
        let _ = install_system::plan::check_plan(
            &plan,
            &os,
            &mut install_system::plan::Ask {
                zoneinfo: "/usr/share/zoneinfo",
                account_exists: &mut exists,
            },
        );
        let _ = plan.dumps();
        let _ = plan.py_str();
    }
    // the whole program; the live system's places point nowhere, so no install gets past its first check
    let host = Host {
        integration_dirs: vec![],
        os_release: "/nonexistent".into(),
        zoneinfo: "/usr/share/zoneinfo".into(),
        efi: "/nonexistent".into(),
        resolv_conf: "/nonexistent".into(),
    };
    let env = install_system::Env {
        sfs: Some("/nonexistent".into()),
        target: Some("/nonexistent/target".into()),
    };
    let mut sys = Nothing::default();
    let code = install_system::main_with(&[], &mut &data[..], &env, host, &mut sys);
    assert!([1, 2].contains(&code), "{code}");
    assert!(code != 2 || sys.calls == 0, "a refused plan ran a program");
    assert!(
        sys.last == "DONE" || sys.last.starts_with("ERROR "),
        "{}",
        sys.last
    );
});
