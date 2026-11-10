//! For parity/compare.py: the Rust matching and sampling on the cases given on standard input (JSON), one JSON line out per
//! case, as parity/compare.py's Python side prints them for the script.
//!
//!   {"mode": "match", "ids": [...], "units": [...]}                              -> [app or null, ...]
//!   {"mode": "sample", "path": P, "ids": [...], "steps": [[SECONDS, [UNIT...]]]} -> per step: {"apps": [...]} or {"error": true}
//!                                                                                   and the file's text ("" when there is none)

use std::collections::BTreeSet;
use std::io::Read;
use std::path::Path;

use labyrinth_appusage::{match_unit, sample};
use pycompat::json::{self, Value};

fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

fn main() {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).unwrap();
    for line in input.lines().filter(|l| !l.trim().is_empty()) {
        let case = json::loads(line).expect("a JSON case per line");
        let ids: BTreeSet<String> = strings(&case["ids"]).into_iter().collect();
        match case["mode"].as_str() {
            Some("match") => {
                let out: Vec<Value> = strings(&case["units"]).iter().map(|u| match_unit(u, &ids).map(Value::from).unwrap_or(Value::Null)).collect();
                println!("{}", json::dumps(&Value::from(out)));
            }
            Some("sample") => {
                let path = Path::new(case["path"].as_str().unwrap());
                let mut out = vec![];
                for step in case["steps"].as_array().unwrap() {
                    let secs = step[0].as_i64().unwrap();
                    let units = strings(&step[1]);
                    let r = match sample(secs, &ids, &units, path) {
                        Ok(apps) => json::json!({"apps": apps.into_iter().collect::<Vec<_>>()}),
                        Err(_) => json::json!({"error": true}),
                    };
                    let text = std::fs::read_to_string(path).unwrap_or_default();
                    out.push(json::json!([r, text]));
                }
                println!("{}", json::dumps(&Value::from(out)));
            }
            _ => panic!("unknown mode"),
        }
    }
}
