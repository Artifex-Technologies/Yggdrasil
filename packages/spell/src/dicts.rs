//! The dictionaries: which are installed (Hunspell `.aff` + `.dic` pairs, one package per language), what each language is called,
//! and loading one. Old dictionaries are not UTF-8 (their `.aff` says `SET ISO8859-1`, `KOI8-R`, ...): they are converted with the
//! system's iconv when loaded.

use std::path::{Path, PathBuf};

/// Where dictionary packages put their files (Arch: /usr/share/hunspell), then the user's own.
pub fn dirs() -> Vec<PathBuf> {
    let mut d = vec![PathBuf::from("/usr/share/hunspell"), PathBuf::from("/usr/share/myspell/dicts")];
    d.push(yggdrasil_conf::dirs::data().join("spell").join("dicts"));
    d
}

/// An installed dictionary: its code (`en_US`, `de_AT`, `en_US-large`) and files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub code: String,
    pub aff: PathBuf,
    pub dic: PathBuf,
}

impl Installed {
    pub fn name(&self) -> String {
        name(&self.code)
    }
}

/// Every installed dictionary, sorted by its name; the first folder wins when two have the same code.
pub fn installed() -> Vec<Installed> {
    installed_in(&dirs())
}

pub fn installed_in(dirs: &[PathBuf]) -> Vec<Installed> {
    let mut out: Vec<Installed> = vec![];
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "dic") {
                let aff = p.with_extension("aff");
                let code = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
                // hyphenation and thesaurus files live beside them in some layouts
                if aff.is_file() && !code.starts_with("hyph_") && !code.starts_with("th_") && !out.iter().any(|i| i.code == code) {
                    out.push(Installed { code, aff, dic: p });
                }
            }
        }
    }
    out.sort_by_key(|i| i.name());
    out
}

pub fn find(code: &str) -> Option<Installed> {
    installed().into_iter().find(|i| i.code == code)
}

/// Loads a dictionary by its code.
pub fn load(code: &str) -> Result<spellbook::Dictionary, String> {
    let i = find(code).ok_or_else(|| format!("{} is not installed", name(code)))?;
    load_files(&i.aff, &i.dic)
}

pub fn load_files(aff: &Path, dic: &Path) -> Result<spellbook::Dictionary, String> {
    let aff_raw = std::fs::read(aff).map_err(|e| format!("{}: {e}", aff.display()))?;
    let dic_raw = std::fs::read(dic).map_err(|e| format!("{}: {e}", dic.display()))?;
    let enc = encoding(&aff_raw);
    let (aff_text, dic_text) = (to_utf8(&aff_raw, &enc)?, to_utf8(&dic_raw, &enc)?);
    // the dictionary is UTF-8 from here on, so its SET line must say so
    let aff_text: String =
        aff_text.lines().map(|l| if l.trim_start().starts_with("SET ") { "SET UTF-8" } else { l }).collect::<Vec<_>>().join("\n");
    spellbook::Dictionary::new(&aff_text, &dic_text).map_err(|e| format!("{}: {e}", dic.display()))
}

/// The `SET` line of an `.aff` file (UTF-8 when there is none).
fn encoding(aff: &[u8]) -> String {
    for line in aff.split(|b| *b == b'\n') {
        let l = String::from_utf8_lossy(line);
        if let Some(rest) = l.trim_start().strip_prefix("SET ") {
            return rest.trim().to_string();
        }
    }
    "UTF-8".into()
}

fn to_utf8(raw: &[u8], enc: &str) -> Result<String, String> {
    let e = enc.to_ascii_uppercase().replace('_', "-");
    if e == "UTF-8" || e == "UTF8" {
        return Ok(String::from_utf8_lossy(raw).into_owned());
    }
    // ISO8859-1 is ISO-8859-1 to iconv
    let e = if let Some(n) = e.strip_prefix("ISO8859-") { format!("ISO-8859-{n}") } else { e };
    if !e.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(format!("unknown encoding {enc}"));
    }
    use std::io::Write;
    let mut child = std::process::Command::new("iconv")
        .args(["-f", &e, "-t", "UTF-8"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|_| "iconv is not available".to_string())?;
    let mut stdin = child.stdin.take().ok_or("iconv")?;
    let raw = raw.to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&raw));
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let _ = writer.join();
    if !out.status.success() {
        return Err(format!("could not convert from {enc}"));
    }
    String::from_utf8(out.stdout).map_err(|_| format!("could not convert from {enc}"))
}

/// A language package offered in Settings: the package that installs it and what the language is called.
pub struct Package {
    pub package: &'static str,
    pub name: &'static str,
}

/// The dictionary packages in Arch's repositories (checked 2026-11-03; `pacman -Ssq '^hunspell-'`). Regional variants of one
/// language come in one package (hunspell-de has de_DE, de_AT, de_CH ...); Spanish is one package per country.
pub const PACKAGES: &[Package] = &[
    Package { package: "hunspell-en_us", name: "English (United States)" },
    Package { package: "hunspell-en_gb", name: "English (United Kingdom)" },
    Package { package: "hunspell-en_ca", name: "English (Canada)" },
    Package { package: "hunspell-en_au", name: "English (Australia)" },
    Package { package: "hunspell-de", name: "German" },
    Package { package: "hunspell-el", name: "Greek" },
    Package { package: "hunspell-es_any", name: "Spanish (all countries)" },
    Package { package: "hunspell-es_es", name: "Spanish (Spain)" },
    Package { package: "hunspell-es_mx", name: "Spanish (Mexico)" },
    Package { package: "hunspell-es_ar", name: "Spanish (Argentina)" },
    Package { package: "hunspell-es_co", name: "Spanish (Colombia)" },
    Package { package: "hunspell-es_cl", name: "Spanish (Chile)" },
    Package { package: "hunspell-es_pe", name: "Spanish (Peru)" },
    Package { package: "hunspell-es_ve", name: "Spanish (Venezuela)" },
    Package { package: "hunspell-fr-comprehensive", name: "French" },
    Package { package: "hunspell-fr-classical", name: "French (classical spelling)" },
    Package { package: "hunspell-fr-revised", name: "French (1990 reform)" },
    Package { package: "hunspell-he", name: "Hebrew" },
    Package { package: "hunspell-hu", name: "Hungarian" },
    Package { package: "hunspell-it", name: "Italian" },
    Package { package: "hunspell-nl", name: "Dutch" },
    Package { package: "hunspell-pl", name: "Polish" },
    Package { package: "hunspell-ro", name: "Romanian" },
    Package { package: "hunspell-ru", name: "Russian" },
];

/// Whether a package is installed (pacman's own database).
pub fn package_installed(package: &str) -> bool {
    std::path::Path::new("/var/lib/pacman/local")
        .read_dir()
        .map(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().rsplitn(3, '-').nth(2) == Some(package)))
        .unwrap_or(false)
}

const LANGS: &[(&str, &str)] = &[
    ("en", "English"),
    ("de", "German"),
    ("el", "Greek"),
    ("es", "Spanish"),
    ("fr", "French"),
    ("he", "Hebrew"),
    ("hu", "Hungarian"),
    ("it", "Italian"),
    ("nl", "Dutch"),
    ("pl", "Polish"),
    ("ro", "Romanian"),
    ("ru", "Russian"),
    ("pt", "Portuguese"),
    ("sv", "Swedish"),
    ("da", "Danish"),
    ("nb", "Norwegian (Bokmål)"),
    ("nn", "Norwegian (Nynorsk)"),
    ("fi", "Finnish"),
    ("cs", "Czech"),
    ("sk", "Slovak"),
    ("uk", "Ukrainian"),
    ("tr", "Turkish"),
    ("ca", "Catalan"),
    ("gl", "Galician"),
    ("eu", "Basque"),
    ("hr", "Croatian"),
    ("sl", "Slovenian"),
    ("bg", "Bulgarian"),
    ("lt", "Lithuanian"),
    ("lv", "Latvian"),
    ("et", "Estonian"),
    ("ga", "Irish"),
    ("cy", "Welsh"),
    ("la", "Latin"),
];

const REGIONS: &[(&str, &str)] = &[
    ("US", "United States"),
    ("GB", "United Kingdom"),
    ("CA", "Canada"),
    ("AU", "Australia"),
    ("NZ", "New Zealand"),
    ("IE", "Ireland"),
    ("ZA", "South Africa"),
    ("IN", "India"),
    ("DE", "Germany"),
    ("AT", "Austria"),
    ("CH", "Switzerland"),
    ("LI", "Liechtenstein"),
    ("LU", "Luxembourg"),
    ("BE", "Belgium"),
    ("FR", "France"),
    ("ES", "Spain"),
    ("MX", "Mexico"),
    ("AR", "Argentina"),
    ("CO", "Colombia"),
    ("CL", "Chile"),
    ("PE", "Peru"),
    ("VE", "Venezuela"),
    ("BO", "Bolivia"),
    ("CR", "Costa Rica"),
    ("CU", "Cuba"),
    ("DO", "Dominican Republic"),
    ("EC", "Ecuador"),
    ("GT", "Guatemala"),
    ("HN", "Honduras"),
    ("NI", "Nicaragua"),
    ("PA", "Panama"),
    ("PR", "Puerto Rico"),
    ("PY", "Paraguay"),
    ("SV", "El Salvador"),
    ("UY", "Uruguay"),
    ("IT", "Italy"),
    ("NL", "Netherlands"),
    ("PT", "Portugal"),
    ("BR", "Brazil"),
    ("RU", "Russia"),
    ("IL", "Israel"),
    ("HU", "Hungary"),
    ("PL", "Poland"),
    ("RO", "Romania"),
    ("GR", "Greece"),
];

/// A readable name for a dictionary code: `de_AT` is "German (Austria)", `en_US-large` "English (United States, more words)".
pub fn name(code: &str) -> String {
    let (base, extra) = code.split_once('-').map(|(b, x)| (b, Some(x))).unwrap_or((code, None));
    let (lang, region) = base.split_once('_').map(|(l, r)| (l, Some(r))).unwrap_or((base, None));
    let lang_name = LANGS.iter().find(|(c, _)| *c == lang).map(|(_, n)| n.to_string()).unwrap_or_else(|| lang.to_string());
    let mut parts = vec![];
    if let Some(r) = region {
        parts.push(REGIONS.iter().find(|(c, _)| *c == r).map(|(_, n)| n.to_string()).unwrap_or_else(|| r.to_string()));
    }
    match extra {
        Some("large") => parts.push("more words".into()),
        Some(x) => parts.push(x.to_string()),
        None => {}
    }
    if parts.is_empty() {
        lang_name
    } else {
        format!("{lang_name} ({})", parts.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(name("de_AT"), "German (Austria)");
        assert_eq!(name("en_US-large"), "English (United States, more words)");
        assert_eq!(name("fr"), "French");
        assert_eq!(name("xx_YY"), "xx (YY)");
    }

    #[test]
    fn finds_pairs_and_skips_halves_and_hyphenation() {
        let d = std::env::temp_dir().join(format!("ygg-spell-dicts-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        for f in ["en_US.aff", "en_US.dic", "de_DE.dic", "hyph_en_US.dic", "hyph_en_US.aff"] {
            std::fs::write(d.join(f), "").unwrap();
        }
        let got: Vec<String> = installed_in(&[d.clone()]).into_iter().map(|i| i.code).collect();
        assert_eq!(got, ["en_US"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn converts_an_old_encoding() {
        // a tiny Latin-1 dictionary: "café" with é as the single byte 0xE9
        let d = std::env::temp_dir().join(format!("ygg-spell-latin1-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("xx.aff"), b"SET ISO8859-1\n").unwrap();
        std::fs::write(d.join("xx.dic"), b"1\ncaf\xe9\n").unwrap();
        let dict = load_files(&d.join("xx.aff"), &d.join("xx.dic")).unwrap();
        assert!(dict.check("café"));
        assert!(!dict.check("cafe"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
