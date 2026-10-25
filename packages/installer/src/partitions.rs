//! Choosing partitions by hand (as Calamares's manual partitioning): what is on the disk, what is to be deleted and made, and
//! which partition becomes /, /boot/efi, /home or swap. Nothing changes on the disk here: the choices go into the plan the
//! operating system's install program carries out (docs/INSTALLER.md, mode "manual").

use eframe::egui::{self, pos2, vec2, Color32, Rect, RichText, Sense, Stroke, StrokeKind};
use yggdrasil_ui::theme::Palette;

const MIB: u64 = 1024 * 1024;
const GB: u64 = 1024 * 1024 * 1024;
/// Smaller gaps are alignment, not space to install into.
const MIN_GAP: u64 = 16 * MIB;
pub const MIN_ROOT: u64 = 20 * GB;
pub const MIN_ESP: u64 = 260 * MIB;

/// What a partition can be used for, and the file systems each may be formatted as (the first is the suggestion).
pub const USES: [(&str, &str, &[&str]); 7] = [
    ("", "Not used", &[]),
    ("/", "/  (the system)", &["btrfs", "ext4", "xfs"]),
    ("/home", "/home  (your files)", &["btrfs", "ext4", "xfs"]),
    ("/boot/efi", "/boot/efi  (EFI system partition)", &["vfat"]),
    ("swap", "Swap", &["swap"]),
    ("/var", "/var", &["btrfs", "ext4", "xfs"]),
    ("/srv", "/srv", &["btrfs", "ext4", "xfs"]),
];

fn fs_options(mount: &str) -> &'static [&'static str] {
    USES.iter().find(|(m, _, _)| *m == mount).map(|(_, _, f)| *f).unwrap_or(&[])
}

#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub path: String,
    pub number: u32,
    pub start: u64,
    pub size: u64,
    pub fstype: String,
    pub label: String,
    pub efi: bool,
    pub mounted: bool,
}

/// A partition to be made.
#[derive(Clone, Debug, PartialEq)]
pub struct NewPart {
    pub id: String,
    pub start: u64,
    pub size: u64,
}

/// What a partition is used for: its mount point and the file system to make (None: kept as it is).
#[derive(Clone, Debug, PartialEq)]
pub struct Use {
    pub mount: String,
    pub format: Option<String>,
}

/// One piece of the disk as it will be.
#[derive(Clone, Debug, PartialEq)]
pub enum Seg {
    Existing(Part),
    New(NewPart),
    Free { start: u64, size: u64 },
}

impl Seg {
    pub fn start(&self) -> u64 {
        match self {
            Seg::Existing(p) => p.start,
            Seg::New(n) => n.start,
            Seg::Free { start, .. } => *start,
        }
    }
    pub fn size(&self) -> u64 {
        match self {
            Seg::Existing(p) => p.size,
            Seg::New(n) => n.size,
            Seg::Free { size, .. } => *size,
        }
    }
    /// How the plan names it: the device, "new:id", or "free:start".
    pub fn key(&self) -> String {
        match self {
            Seg::Existing(p) => p.path.clone(),
            Seg::New(n) => format!("new:{}", n.id),
            Seg::Free { start, .. } => format!("free:{start}"),
        }
    }
}

/// The choices made so far, for one disk.
#[derive(Clone, Debug, Default)]
pub struct Manual {
    pub new_table: bool,
    pub delete: Vec<u32>,
    pub create: Vec<NewPart>,
    /// By key (device or "new:id").
    pub uses: Vec<(String, Use)>,
    pub selected: Option<String>,
    next_id: u32,
    /// The form for a new partition: size in GB, use, file system.
    form_size: f64,
    form_use: String,
    form_fs: String,
}

impl Manual {
    pub fn use_of(&self, key: &str) -> Option<&Use> {
        self.uses.iter().find(|(k, _)| k == key).map(|(_, u)| u)
    }

    fn set_use(&mut self, key: &str, u: Option<Use>) {
        self.uses.retain(|(k, _)| k != key);
        if let Some(u) = u {
            // a mount point belongs to one partition (swap may be several): the one chosen last keeps it
            if u.mount != "swap" {
                self.uses.retain(|(_, x)| x.mount != u.mount);
            }
            self.uses.push((key.to_string(), u));
        }
    }

    /// The disk as it will be: what is kept, what is new, and the free space in between.
    pub fn segments(&self, disk_size: u64, parts: &[Part]) -> Vec<Seg> {
        let mut used: Vec<Seg> = vec![];
        if !self.new_table {
            used.extend(parts.iter().filter(|p| !self.delete.contains(&p.number)).cloned().map(Seg::Existing));
        }
        used.extend(self.create.iter().cloned().map(Seg::New));
        used.sort_by_key(Seg::start);
        let mut out = vec![];
        let (first, last) = (MIB, disk_size.saturating_sub(MIB));
        let mut at = first;
        for s in used {
            gap(&mut out, at, s.start());
            at = at.max(s.start() + s.size());
            out.push(s);
        }
        gap(&mut out, at, last);
        out
    }

    /// The parts of the install plan that describe the disk (mode "manual").
    pub fn plan(&self, parts: &[Part]) -> serde_json::Value {
        let kind = |key: &str| match self.use_of(key).map(|u| u.mount.as_str()) {
            Some("/boot/efi") => "efi",
            Some("swap") => "swap",
            _ => "linux",
        };
        let deleted: Vec<u32> = if self.new_table { vec![] } else { self.delete.clone() };
        let alive = |key: &str| key.starts_with("new:") || parts.iter().any(|p| p.path == key && !self.new_table && !self.delete.contains(&p.number));
        serde_json::json!({
            "mode": "manual",
            "new_table": self.new_table,
            "delete": deleted,
            "create": self.create.iter().map(|n| serde_json::json!({"id": n.id, "start": n.start, "size": n.size, "kind": kind(&format!("new:{}", n.id))})).collect::<Vec<_>>(),
            "use": self.uses.iter().filter(|(k, u)| !u.mount.is_empty() && alive(k)).map(|(k, u)| serde_json::json!({"part": k, "mount": u.mount, "format": u.format})).collect::<Vec<_>>(),
        })
    }

    /// What is still missing before it can be installed (None: ready).
    pub fn problem(&self, disk_size: u64, parts: &[Part], table: &str) -> Option<String> {
        if !self.new_table && table.is_empty() {
            return Some("The disk has no partition table yet: make a new one".into());
        }
        let segs = self.segments(disk_size, parts);
        let size_of = |key: &str| segs.iter().find(|s| s.key() == key).map(Seg::size);
        let find = |mount: &str| self.uses.iter().find(|(k, u)| u.mount == mount && size_of(k).is_some());
        let Some((rk, ru)) = find("/") else { return Some("Choose a partition for /, the system".into()) };
        if ru.format.is_none() {
            return Some("The partition for / has to be formatted".into());
        }
        if size_of(rk).unwrap_or(0) < MIN_ROOT {
            return Some(format!("/ needs at least {} GB", MIN_ROOT / GB));
        }
        let Some((ek, eu)) = find("/boot/efi") else { return Some("Choose the EFI partition for /boot/efi".into()) };
        if size_of(ek).unwrap_or(0) < MIN_ESP {
            return Some(format!("The EFI partition needs at least {} MB", MIN_ESP / MIB));
        }
        if eu.format.is_none() {
            if let Some(Seg::Existing(p)) = segs.iter().find(|s| s.key() == *ek) {
                if p.fstype != "vfat" {
                    return Some("The EFI partition chosen has no FAT file system: format it".into());
                }
            }
        }
        for (k, u) in &self.uses {
            if let Some(Seg::Existing(p)) = segs.iter().find(|s| s.key() == *k) {
                if p.mounted && !u.mount.is_empty() {
                    return Some(format!("{} is in use (mounted)", p.path));
                }
            }
        }
        None
    }

    /// Lines for the summary: what will be done to the disk.
    pub fn describe(&self, disk_size: u64, parts: &[Part]) -> Vec<String> {
        let mut out = vec![];
        if self.new_table {
            out.push("New partition table: everything on the disk is erased".into());
        }
        for n in &self.delete {
            if let Some(p) = parts.iter().find(|p| p.number == *n) {
                out.push(format!("Delete {} ({}{})", p.path, size_text(p.size), if p.label.is_empty() { String::new() } else { format!(", {}", p.label) }));
            }
        }
        for s in self.segments(disk_size, parts) {
            let Some(u) = self.use_of(&s.key()).filter(|u| !u.mount.is_empty()) else { continue };
            let what = match &u.format {
                Some(f) => format!("formatted as {}", fs_name(f)),
                None => "kept as it is".into(),
            };
            let name = match &s {
                Seg::Existing(p) => p.path.clone(),
                Seg::New(_) => format!("new {} partition", size_text(s.size())),
                Seg::Free { .. } => continue,
            };
            out.push(format!("{} as {}, {}", name, u.mount, what));
        }
        out
    }
}

fn gap(out: &mut Vec<Seg>, from: u64, to: u64) {
    let a = from.div_ceil(MIB) * MIB;
    if to > a && to - a >= MIN_GAP {
        out.push(Seg::Free { start: a, size: (to - a) / MIB * MIB });
    }
}

impl Manual {
    /// The screenshot demo (`yggdrasil-installer --demo disk-manual`): beside Windows, the free space as the new system, the EFI
    /// partition kept and used.
    pub fn demo(parts: &[Part], free_start: u64, free_size: u64) -> Manual {
        let mut m = Manual { next_id: 1, ..Manual::default() };
        m.create.push(NewPart { id: "1".into(), start: free_start, size: free_size });
        if let Some(efi) = parts.iter().find(|p| p.efi) {
            m.uses.push((efi.path.clone(), Use { mount: "/boot/efi".into(), format: None }));
        }
        m.uses.push(("new:1".into(), Use { mount: "/".into(), format: Some("btrfs".into()) }));
        m.selected = Some("new:1".into());
        m
    }
}

pub fn size_text(b: u64) -> String {
    if b >= 1000 * GB {
        format!("{:.1} TB", b as f64 / (1024.0 * GB as f64))
    } else if b >= GB {
        format!("{:.0} GB", b as f64 / GB as f64)
    } else {
        format!("{} MB", b / MIB)
    }
}

fn fs_name(f: &str) -> &str {
    match f {
        "vfat" => "FAT32",
        "swap" => "swap",
        x => x,
    }
}

/// What a partition holds, in words ("Windows (NTFS)", "EFI (FAT32)").
fn content(p: &Part) -> String {
    let fs = match p.fstype.as_str() {
        "" => "no file system".to_string(),
        "ntfs" => "NTFS".into(),
        "vfat" => "FAT32".into(),
        "BitLocker" => "BitLocker (encrypted)".into(),
        "crypto_LUKS" => "LUKS (encrypted)".into(),
        x => x.to_string(),
    };
    let kind = if p.efi { "EFI system partition".to_string() } else { p.label.clone() };
    if kind.is_empty() { fs } else { format!("{kind} ({fs})") }
}

/// The manual partitioning editor for one disk.
pub fn ui(ui: &mut egui::Ui, pal: &Palette, m: &mut Manual, disk_size: u64, parts: &[Part], table: &str) {
    let segs = m.segments(disk_size, parts);
    if m.selected.as_ref().map(|k| !segs.iter().any(|s| &s.key() == k)).unwrap_or(false) {
        m.selected = None;
    }

    // the disk as a bar: each piece as wide as its share
    let w = ui.available_width().min(620.0);
    let (bar, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
    let total = disk_size.max(1) as f32;
    let mut x = bar.left();
    for s in &segs {
        let sw = (s.size() as f32 / total * bar.width()).max(3.0);
        let r = Rect::from_min_size(pos2(x, bar.top()), vec2(sw, bar.height())).shrink2(vec2(1.0, 0.0));
        let used = m.use_of(&s.key()).map(|u| !u.mount.is_empty()).unwrap_or(false);
        let fill = match s {
            Seg::Free { .. } => Color32::TRANSPARENT,
            _ if used => pal.accent,
            Seg::New(_) => pal.accent.gamma_multiply(0.55),
            Seg::Existing(_) => pal.field,
        };
        ui.painter().rect_filled(r, 4.0, fill);
        let sel = m.selected.as_deref() == Some(&s.key());
        ui.painter().rect_stroke(r, 4.0, Stroke::new(if sel { 2.0 } else { 1.0 }, if sel { pal.text } else { pal.border }), StrokeKind::Inside);
        x += sw;
    }
    ui.add_space(8.0);

    // the pieces as rows, to choose one
    egui::ScrollArea::vertical().id_salt("parts").max_height(170.0).show(ui, |ui| {
        for s in &segs {
            let key = s.key();
            let (title, sub) = match s {
                Seg::Existing(p) => (format!("{}   {}", p.path, size_text(p.size)), content(p) + if p.mounted { " · in use" } else { "" }),
                Seg::New(n) => (format!("New partition   {}", size_text(n.size)), "made when installing".to_string()),
                Seg::Free { size, .. } => (format!("Free space   {}", size_text(*size)), "select to make a partition here".to_string()),
            };
            let role = m.use_of(&key).filter(|u| !u.mount.is_empty()).map(|u| format!("as {}{}", u.mount, if u.format.is_some() { ", formatted" } else { ", kept" })).unwrap_or_default();
            let on = m.selected.as_deref() == Some(&key);
            let (r, resp) = ui.allocate_exact_size(vec2(w, 40.0), Sense::click());
            if on {
                ui.painter().rect_filled(r, 6.0, pal.select);
            } else if resp.hovered() {
                ui.painter().rect_filled(r, 6.0, pal.hover);
            }
            ui.painter().text(pos2(r.left() + 10.0, r.top() + 13.0), egui::Align2::LEFT_CENTER, title, egui::FontId::proportional(14.0), pal.text);
            ui.painter().text(pos2(r.left() + 10.0, r.top() + 29.0), egui::Align2::LEFT_CENTER, sub, egui::FontId::proportional(12.0), pal.dim);
            ui.painter().text(pos2(r.right() - 10.0, r.center().y), egui::Align2::RIGHT_CENTER, role, egui::FontId::proportional(13.0), pal.accent);
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                m.selected = Some(key.clone());
                if let Seg::Free { size, .. } = s {
                    m.form_size = *size as f64 / GB as f64;
                    m.form_use = if m.uses.iter().any(|(_, u)| u.mount == "/") { "/home".into() } else { "/".into() };
                    m.form_fs = fs_options(&m.form_use).first().unwrap_or(&"").to_string();
                }
            }
        }
    });
    ui.add_space(8.0);

    // what can be done to the one selected
    match segs.iter().find(|s| Some(s.key()) == m.selected) {
        Some(Seg::Free { start, size }) => {
            let (start, size) = (*start, *size);
            ui.label(RichText::new("New partition in this free space").strong());
            ui.horizontal(|ui| {
                ui.label("Size");
                // the exact size of the gap is the most: "All of it" really takes all of it (not the whole GB below it)
                let max = (size as f64 / GB as f64).max(0.3);
                ui.add(egui::DragValue::new(&mut m.form_size).range(0.3..=max).speed(1.0).suffix(" GB").max_decimals(1));
                if ui.small_button("All of it").clicked() {
                    m.form_size = size as f64 / GB as f64;
                }
            });
            use_choice(ui, "new-use", &mut m.form_use, &mut m.form_fs, true);
            if ui.button("Add this partition").clicked() {
                let all = m.form_size >= size as f64 / GB as f64 - 0.05;
                let bytes = if all { size } else { ((m.form_size * GB as f64) as u64 / MIB * MIB).min(size) };
                m.next_id += 1;
                let id = format!("p{}", m.next_id);
                m.create.push(NewPart { id: id.clone(), start, size: bytes });
                let u = (!m.form_use.is_empty()).then(|| Use { mount: m.form_use.clone(), format: Some(m.form_fs.clone()) });
                m.set_use(&format!("new:{id}"), u);
                m.selected = Some(format!("new:{id}"));
            }
        }
        Some(Seg::New(n)) => {
            let key = format!("new:{}", n.id);
            ui.label(RichText::new(format!("New {} partition", size_text(n.size))).strong());
            let cur = m.use_of(&key).cloned().unwrap_or(Use { mount: String::new(), format: None });
            let (mut mount, mut fs) = (cur.mount.clone(), cur.format.clone().unwrap_or_default());
            use_choice(ui, &format!("use-{key}"), &mut mount, &mut fs, true);
            if mount != cur.mount || Some(fs.clone()) != cur.format {
                m.set_use(&key, (!mount.is_empty()).then(|| Use { mount, format: Some(fs) }));
            }
            if ui.button("Remove this new partition").clicked() {
                let id = n.id.clone();
                m.create.retain(|c| c.id != id);
                m.set_use(&key, None);
            }
        }
        Some(Seg::Existing(p)) => {
            ui.label(RichText::new(format!("{}  ·  {}", p.path, content(p))).strong());
            if p.mounted {
                ui.label(RichText::new("In use by the running system: it cannot be changed").color(pal.amber));
            } else {
                let cur = m.use_of(&p.path).cloned().unwrap_or(Use { mount: String::new(), format: None });
                let (mut mount, mut fs) = (cur.mount.clone(), cur.format.clone().unwrap_or_default());
                let mut format = cur.format.is_some();
                use_choice(ui, &format!("use-{}", p.path), &mut mount, &mut fs, false);
                if !mount.is_empty() {
                    // / is always made new; the EFI partition and /home may be kept as they are (Windows' EFI partition, your files)
                    let must = mount == "/" || mount == "swap" && p.fstype != "swap";
                    if must {
                        format = true;
                    }
                    ui.add_enabled(!must, egui::Checkbox::new(&mut format, "Format it (erases what is on it)"));
                    if format && fs.is_empty() {
                        fs = fs_options(&mount).first().unwrap_or(&"").to_string();
                    }
                    if !format && mount == "/boot/efi" {
                        ui.label(RichText::new("Kept as it is: Labyrinth's boot menu is added beside what is there (Windows keeps its own)").color(pal.dim));
                    }
                }
                let new_use = (!mount.is_empty()).then(|| Use { mount: mount.clone(), format: format.then(|| fs.clone()) });
                if new_use.as_ref() != m.use_of(&p.path) {
                    m.set_use(&p.path, new_use);
                }
                if ui.button(RichText::new("Delete this partition").color(pal.red)).clicked() {
                    m.delete.push(p.number);
                    m.set_use(&p.path, None);
                    m.selected = None;
                }
            }
        }
        None => {
            ui.label(RichText::new("Choose a piece of the disk above to use it, delete it, or make a partition in free space.").color(pal.dim));
        }
    }
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let label = if m.new_table { "Keep the partition table" } else { "New partition table (erases the whole disk)" };
        if ui.button(label).clicked() {
            m.new_table = !m.new_table;
            m.delete.clear();
            m.create.clear();
            m.uses.clear();
            m.selected = None;
        }
        let deleted: Vec<String> = m.delete.iter().filter_map(|n| parts.iter().find(|p| p.number == *n).map(|p| p.path.clone())).collect();
        if !deleted.is_empty() && ui.button(format!("Undo deleting {}", deleted.join(", "))).clicked() {
            m.delete.clear();
        }
    });
    let _ = table;
}

/// "Use as" and, when it is made new, the file system.
fn use_choice(ui: &mut egui::Ui, id: &str, mount: &mut String, fs: &mut String, formatting: bool) {
    ui.horizontal(|ui| {
        ui.label("Use as");
        let shown = USES.iter().find(|(m, _, _)| m == mount).map(|(_, l, _)| *l).unwrap_or("Not used");
        egui::ComboBox::from_id_salt(id).selected_text(shown).width(260.0).show_ui(ui, |ui| {
            for (m, label, opts) in USES {
                if ui.selectable_label(*mount == m, label).clicked() {
                    *mount = m.to_string();
                    if !opts.contains(&fs.as_str()) {
                        *fs = opts.first().unwrap_or(&"").to_string();
                    }
                }
            }
        });
        let opts = fs_options(mount);
        if formatting && opts.len() > 1 {
            egui::ComboBox::from_id_salt((id, "fs")).selected_text(fs_name(fs)).show_ui(ui, |ui| {
                for o in opts {
                    if ui.selectable_label(fs == o, fs_name(o)).clicked() {
                        *fs = o.to_string();
                    }
                }
            });
        }
    });
    if !formatting && fs_options(mount).len() > 1 {
        ui.horizontal(|ui| {
            ui.label("File system if formatted");
            egui::ComboBox::from_id_salt((id, "fs2")).selected_text(fs_name(fs)).show_ui(ui, |ui| {
                for o in fs_options(mount) {
                    if ui.selectable_label(fs == o, fs_name(o)).clicked() {
                        *fs = o.to_string();
                    }
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn windows_disk() -> (u64, Vec<Part>) {
        let p = |n: u32, start: u64, size: u64, fs: &str, efi: bool| Part { path: format!("/dev/vda{n}"), number: n, start, size, fstype: fs.into(), label: String::new(), efi, mounted: false };
        (500 * GB, vec![p(1, MIB, 300 * MIB, "vfat", true), p(2, 301 * MIB, 299 * GB, "ntfs", false)])
    }

    #[test]
    fn the_disk_as_it_will_be_has_free_space_where_nothing_is() {
        let (size, parts) = windows_disk();
        let mut m = Manual::default();
        let segs = m.segments(size, &parts);
        assert_eq!(segs.len(), 3);
        assert!(matches!(segs[2], Seg::Free { .. }));
        m.delete.push(2);
        let segs = m.segments(size, &parts);
        assert_eq!(segs.len(), 2, "Windows' space joins the free space");
        m.new_table = true;
        assert_eq!(m.segments(size, &parts).len(), 1, "a new table: all of it is free");
    }

    #[test]
    fn beside_windows_with_its_efi_partition_kept() {
        let (size, parts) = windows_disk();
        let mut m = Manual::default();
        let Seg::Free { start, size: free } = m.segments(size, &parts)[2].clone() else { panic!() };
        assert!(m.problem(size, &parts, "gpt").unwrap().contains("/"));
        m.create.push(NewPart { id: "p1".into(), start, size: free });
        m.set_use("new:p1", Some(Use { mount: "/".into(), format: Some("btrfs".into()) }));
        assert!(m.problem(size, &parts, "gpt").unwrap().contains("EFI"));
        m.set_use("/dev/vda1", Some(Use { mount: "/boot/efi".into(), format: None }));
        assert_eq!(m.problem(size, &parts, "gpt"), None);
        let plan = m.plan(&parts);
        assert_eq!(plan["create"][0]["kind"], "linux");
        assert_eq!(plan["use"].as_array().unwrap().len(), 2);
        assert!(m.describe(size, &parts).iter().any(|l| l.contains("/dev/vda1 as /boot/efi, kept as it is")));
    }

    #[test]
    fn a_mount_point_belongs_to_one_partition() {
        let mut m = Manual::default();
        m.set_use("a", Some(Use { mount: "/".into(), format: Some("btrfs".into()) }));
        m.set_use("b", Some(Use { mount: "/".into(), format: Some("ext4".into()) }));
        assert_eq!(m.uses.len(), 1);
        assert_eq!(m.uses[0].0, "b");
        m.set_use("c", Some(Use { mount: "swap".into(), format: Some("swap".into()) }));
        m.set_use("d", Some(Use { mount: "swap".into(), format: Some("swap".into()) }));
        assert_eq!(m.uses.len(), 3, "swap may be on several");
    }

    #[test]
    fn small_roots_and_missing_tables_are_caught() {
        let (size, parts) = windows_disk();
        let mut m = Manual::default();
        let Seg::Free { start, .. } = m.segments(size, &parts)[2].clone() else { panic!() };
        m.create.push(NewPart { id: "p1".into(), start, size: 10 * GB });
        m.set_use("new:p1", Some(Use { mount: "/".into(), format: Some("btrfs".into()) }));
        assert!(m.problem(size, &parts, "gpt").unwrap().contains("at least"));
        assert!(Manual::default().problem(size, &[], "").unwrap().contains("partition table"));
    }
}
