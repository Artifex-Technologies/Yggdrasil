//! The disks it could install onto (--disks, and the check before an install), from lsblk as the script read it: values kept
//! as lsblk gave them where the script passed them on, and the script's exceptions (a Crash) where lsblk's JSON is not what it
//! expected.

use crate::install::Ctx;
use crate::plan::MIB;
use crate::py::{Fail, R};
use crate::pyjson::{dict, int, loads, s, Int, Py, CLAMP, OTHER_NESTING};

/// A partition as the installer is told of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub path: Py,
    pub number: Int,
    pub start: Int,
    pub size: Int,
    pub fstype: Py,
    pub label: Py,
    pub efi: bool,
    pub mounted: bool,
}

/// A disk it could install onto.
#[derive(Clone, Debug, PartialEq)]
pub struct Disk {
    pub path: Py,
    pub size: Int,
    pub model: String,
    pub removable: bool,
    pub transport: Py,
    pub big_enough: bool,
    pub table: Py,
    pub partitions: Vec<Part>,
    pub free: Vec<(i128, i128)>,
}

impl Disk {
    /// The disk as json.dumps writes it for the window.
    pub fn to_py(&self) -> Py {
        dict(vec![
            ("path", self.path.clone()),
            ("size", Py::Int(self.size.clone())),
            ("model", s(&self.model)),
            ("removable", Py::Bool(self.removable)),
            ("transport", self.transport.clone()),
            ("big_enough", Py::Bool(self.big_enough)),
            ("table", self.table.clone()),
            (
                "partitions",
                Py::List(
                    self.partitions
                        .iter()
                        .map(|q| {
                            dict(vec![
                                ("path", q.path.clone()),
                                ("number", Py::Int(q.number.clone())),
                                ("start", Py::Int(q.start.clone())),
                                ("size", Py::Int(q.size.clone())),
                                ("fstype", q.fstype.clone()),
                                ("label", q.label.clone()),
                                ("efi", Py::Bool(q.efi)),
                                ("mounted", Py::Bool(q.mounted)),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("free", Py::List(self.free.iter().map(|(a, b)| dict(vec![("start", int(*a)), ("size", int(*b))])).collect())),
        ])
    }
}

/// A disk's partition table, partitions and free space.
type Layout = (Py, Vec<Part>, Vec<(i128, i128)>);

fn crash<T>(m: String) -> R<T> {
    Err(Fail::Crash(m))
}

/// d.get(key) on a value json.loads made: an AttributeError unless it is an object.
pub fn get<'a>(v: &'a Py, key: &str) -> R<Option<&'a Py>> {
    match v {
        Py::Dict(d) => Ok(d.get(key)),
        _ => crash(format!("'{}' object has no attribute 'get'", v.type_name())),
    }
}

/// d.get(key, None) or default (Python's `d.get(key) or x`): the value if it is true, else the default.
fn get_or<'a>(v: &'a Py, key: &str, default: &'a Py) -> R<&'a Py> {
    Ok(get(v, key)?.filter(|x| x.truthy()).unwrap_or(default))
}

/// d[key]: a KeyError unless it is there.
pub fn index<'a>(v: &'a Py, key: &str) -> R<&'a Py> {
    match v {
        Py::Dict(d) => d.get(key).map_or_else(|| crash(pycompat::repr(key)), Ok),
        Py::List(_) => crash("list indices must be integers or slices, not str".into()),
        Py::Str(_) => crash("string indices must be integers, not 'str'".into()),
        _ => crash(format!("'{}' object is not subscriptable", v.type_name())),
    }
}

/// for x in v: a TypeError unless it can be iterated (text gives its characters, an object its keys).
pub fn iterate(v: &Py) -> R<Vec<Py>> {
    match v {
        Py::List(l) => Ok(l.clone()),
        Py::Str(t) => Ok(t.chars().map(|c| Py::Str(c.to_string())).collect()),
        Py::Dict(d) => Ok(d.iter().map(|(k, _)| s(k)).collect()),
        _ => crash(format!("'{}' object is not iterable", v.type_name())),
    }
}

/// int(v)
pub fn to_int(v: &Py) -> R<Int> {
    match v {
        Py::Int(i) => Ok(i.clone()),
        Py::Bool(b) => Ok(Int::new(*b as i128)),
        Py::Float(f) if f.is_nan() => crash("cannot convert float NaN to integer".into()),
        Py::Float(f) if f.is_infinite() => crash("cannot convert float infinity to integer".into()),
        Py::Float(f) => {
            let t = f.trunc();
            Ok(if t.abs() < 1e30 {
                Int::new(t as i128)
            } else {
                Int { text: format!("{t:.0}"), value: if t > 0.0 { CLAMP } else { -CLAMP } }
            })
        }
        Py::Str(t) => match pycompat::int(t) {
            Some(n) => Ok(Int::new(n)),
            None => crash(format!("invalid literal for int() with base 10: {}", pycompat::repr(t))),
        },
        _ => crash(format!("int() argument must be a string, a bytes-like object or a real number, not '{}'", v.type_name())),
    }
}

/// An argument for a program: text (subprocess refused anything else).
pub fn arg(v: &Py) -> R<String> {
    match v {
        Py::Str(t) => Ok(t.clone()),
        _ => crash(format!("expected str, bytes or os.PathLike object, not {}", v.type_name())),
    }
}

/// x.strip() of a value that should be text.
fn strip_text(v: &Py) -> R<String> {
    match v {
        Py::Str(t) => Ok(pycompat::strip(t).to_string()),
        _ => crash(format!("'{}' object has no attribute 'strip'", v.type_name())),
    }
}

/// The gaps between partitions (start, size in bytes), each starting on a MiB; slivers under 16 MiB (alignment) are not offered.
pub fn free_space(used: &[(i128, i128)], first: i128, last: i128) -> Vec<(i128, i128)> {
    let mut gaps = vec![];
    let mut at = first;
    let mut sorted = used.to_vec();
    sorted.sort();
    sorted.push((last, 0));
    for (start, size) in sorted {
        let a = -((-at.max(first)).div_euclid(MIB)) * MIB;
        let b = start.min(last);
        if b - a >= 16 * MIB {
            gaps.push((a, (b - a).div_euclid(MIB) * MIB));
        }
        at = at.max(start + size);
    }
    gaps
}

/// Partition n of a disk: /dev/sda2, /dev/nvme0n1p2, /dev/vda2.
pub fn part(disk: &str, n: u32) -> String {
    if disk.chars().last().is_some_and(|c| c.is_ascii_digit()) {
        format!("{disk}p{n}")
    } else {
        format!("{disk}{n}")
    }
}

impl Ctx<'_> {
    /// The disk the live system runs from (never offered).
    pub fn live_disk(&mut self) -> R<Option<String>> {
        let src = pycompat::strip(&self.capture(&["findmnt", "-no", "SOURCE", "/run/archiso/bootmnt"], false)?).to_string();
        if src.is_empty() {
            return Ok(None);
        }
        let parent = pycompat::strip(&self.capture(&["lsblk", "-no", "PKNAME", &src], false)?).to_string();
        Ok(Some(if parent.is_empty() { src } else { format!("/dev/{parent}") }))
    }

    /// Whole disks it could install onto: not the live medium, not read-only, not a loop or zram device.
    pub fn disks(&mut self) -> R<Vec<Disk>> {
        let text = self.capture(&["lsblk", "-J", "-b", "-d", "-o", "NAME,PATH,SIZE,MODEL,TRAN,RO,TYPE,RM"], true)?;
        // (json's own words for an empty text; other broken output is described in words of our own)
        let out = loads(&text, OTHER_NESTING).or_else(|_| {
            crash(if text.is_empty() { "Expecting value: line 1 column 1 (char 0)".into() } else { "lsblk's output is not JSON".into() })
        })?;
        let live = self.live_disk()?;
        let mut found = vec![];
        let empty_list = Py::List(vec![]);
        for d in iterate(get(&out, "blockdevices")?.unwrap_or(&empty_list))? {
            if get(&d, "type")?.and_then(Py::as_str) != Some("disk") || get(&d, "ro")?.is_some_and(Py::truthy) {
                continue;
            }
            let path = index(&d, "path")?;
            // d["path"] == live (None when there is no live medium: only a null path is equal to it)
            let is_live = match &live {
                Some(l) => path.as_str() == Some(l.as_str()),
                None => *path == Py::None,
            };
            if is_live {
                continue;
            }
            match index(&d, "name")? {
                Py::Str(n) if ["loop", "zram", "sr"].iter().any(|p| n.starts_with(p)) => continue,
                Py::Str(_) => {}
                other => return crash(format!("'{}' object has no attribute 'startswith'", other.type_name())),
            }
            let zero = int(0);
            let size = to_int(get_or(&d, "size", &zero)?)?;
            let empty = s("");
            let model = strip_text(get_or(&d, "model", &empty)?)?;
            let removable = get(&d, "rm")?.is_some_and(Py::truthy);
            let transport = get_or(&d, "tran", &empty)?.clone();
            let big_enough = size.value >= self.os.min_bytes;
            let (table, partitions, free) = self.disk_layout(path, size.value)?;
            found.push(Disk { path: path.clone(), size, model, removable, transport, big_enough, table, partitions, free });
        }
        Ok(found)
    }

    /// What is on a disk: its partition table (gpt, dos or none), its partitions (with what is on them) and its free space, in
    /// bytes. From lsblk, which needs no root (the installer lists the disks as the live user).
    fn disk_layout(&mut self, disk: &Py, size: i128) -> R<Layout> {
        let d = arg(disk)?;
        // output that is not UTF-8 or not JSON is a ValueError, which the script caught: nothing on the disk then
        let text = match self.capture(
            &["lsblk", "-J", "-b", "-o", "PATH,START,SIZE,FSTYPE,LABEL,PARTLABEL,PARTTYPE,PARTN,PTTYPE,MOUNTPOINTS,TYPE", &d],
            false,
        ) {
            Err(Fail::Crash(m)) if m.starts_with("'utf-8' codec can't decode") => Some(String::new()),
            Err(e) => return Err(e),
            Ok(t) if t.is_empty() => Some("{}".to_string()),
            Ok(t) => Some(t),
        };
        let tree = match text.filter(|t| !t.is_empty()).map(|t| loads(&t, OTHER_NESTING)) {
            Some(Ok(v)) => get(&v, "blockdevices")?.cloned().unwrap_or(Py::List(vec![])),
            _ => Py::List(vec![]),
        };
        let mut flat = vec![];
        fn walk(items: &Py, flat: &mut Vec<Py>) -> R<()> {
            for x in iterate(items)? {
                let children = get(&x, "children")?.filter(|c| c.truthy()).cloned().unwrap_or(Py::List(vec![]));
                flat.push(x);
                walk(&children, flat)?;
            }
            Ok(())
        }
        walk(&tree, &mut flat)?;
        let empty = s("");
        let mut table = empty.clone();
        for x in &flat {
            if get(x, "path")? == Some(disk) {
                table = get_or(x, "pttype", &empty)?.clone();
                break;
            }
        }
        let mut parts = vec![];
        let zero = int(0);
        for x in &flat {
            if get(x, "type")?.and_then(Py::as_str) != Some("part") || matches!(get(x, "start")?, None | Some(Py::None)) {
                continue;
            }
            let path = index(x, "path")?.clone();
            let number = to_int(get_or(x, "partn", &zero)?)?;
            let start = Int::new(to_int(index(x, "start")?)?.value * 512);
            let size = to_int(get_or(x, "size", &zero)?)?;
            let fstype = get_or(x, "fstype", &empty)?.clone();
            let label = match get_or(x, "label", &Py::None)? {
                Py::None => get_or(x, "partlabel", &empty)?.clone(),
                l => l.clone(),
            };
            let parttype = get_or(x, "parttype", &empty)?;
            let efi = match parttype {
                Py::Str(t) => ["c12a7328-f81f-11d2-ba4b-00a0c93ec93b", "0xef"].contains(&t.to_lowercase().as_str()),
                other => return crash(format!("'{}' object has no attribute 'lower'", other.type_name())),
            };
            let mut mounted = false;
            for m in iterate(get_or(x, "mountpoints", &Py::List(vec![]))?)? {
                if m.truthy() {
                    mounted = true;
                    break;
                }
            }
            parts.push(Part { path, number, start, size, fstype, label, efi, mounted });
        }
        parts.sort_by_key(|q| q.start.value);
        let used: Vec<(i128, i128)> = parts.iter().map(|q| (q.start.value, q.size.value)).collect();
        let free = free_space(&used, MIB, size - MIB);
        Ok((table, parts, free))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_space_lies_between_the_partitions() {
        let gb = 1024 * MIB;
        let parts = [(MIB, 300 * MIB), (301 * MIB, 299 * gb)];
        let gaps = free_space(&parts, MIB, 500 * gb - MIB);
        assert_eq!(gaps.len(), 1);
        assert!(gaps[0].0 >= 301 * MIB + 299 * gb);
        assert_eq!(gaps[0].0 % MIB, 0);
        assert_eq!(free_space(&[], MIB, 100 * MIB), vec![(MIB, 99 * MIB)]);
        assert_eq!(free_space(&[(2 * MIB, 10 * MIB)], MIB, 12 * MIB), vec![], "a 1 MiB sliver is not free space");
        assert_eq!(free_space(&[(5 * MIB + 7, 3)], MIB, 100 * MIB), vec![(6 * MIB, 94 * MIB)], "a gap starts on a MiB");
    }

    #[test]
    fn partition_names() {
        assert_eq!(part("/dev/sda", 2), "/dev/sda2");
        assert_eq!(part("/dev/nvme0n1", 2), "/dev/nvme0n1p2");
        assert_eq!(part("/dev/mmcblk0", 1), "/dev/mmcblk0p1");
    }

    #[test]
    fn values_as_int_made_them() {
        assert_eq!(to_int(&s(" 42 ")).unwrap().value, 42);
        assert_eq!(to_int(&Py::Float(-2.7)).unwrap().value, -2);
        assert_eq!(to_int(&Py::Bool(true)).unwrap().value, 1);
        assert_eq!(to_int(&s("x")), Err(Fail::Crash("invalid literal for int() with base 10: 'x'".into())));
        assert_eq!(
            to_int(&Py::List(vec![])),
            Err(Fail::Crash("int() argument must be a string, a bytes-like object or a real number, not 'list'".into()))
        );
    }
}
