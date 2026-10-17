//! More about an app than the catalog line: a longer description, screenshots, the licence and who makes it. It comes from AppStream, the
//! standard store metadata: the app's own file when it is installed (`appstreamcli dump`, nothing leaves the computer), or Flathub's public
//! AppStream service when the app is on Flathub (a request to flathub.org that says which app you looked at). Pictures are downloaded from
//! where the metadata points (Flathub's media server, the app's own repository) only when you have allowed it; they are kept in the cache.

use std::path::PathBuf;

use serde_json::{json, Value};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shot {
    pub url: String,
    pub caption: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Details {
    pub summary: String,
    pub description: String,
    pub screenshots: Vec<Shot>,
    pub homepage: String,
    pub license: String,
    pub developer: String,
    /// Where it came from: "this computer" or "Flathub".
    pub from: String,
}

fn cache_root() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache"))).unwrap_or_default().join("labyrinth/market")
}

fn settings_path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))).unwrap_or_default().join("labyrinth/market/settings.json")
}

/// Whether the user allowed fetching descriptions and pictures from the internet (None: not asked yet).
pub fn online_allowed() -> Option<bool> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(settings_path()).ok()?).ok()?;
    v.get("online_details").and_then(|b| b.as_bool())
}

pub fn set_online_allowed(on: bool) {
    let p = settings_path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, json!({"online_details": on}).to_string());
}

fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

/// Text of a node's `<p>` and `<li>` children: paragraphs separated by a blank line, list items with a bullet.
fn paragraphs(node: roxmltree::Node) -> String {
    let mut out: Vec<String> = vec![];
    for ch in node.children().filter(|c| c.is_element()) {
        match ch.tag_name().name() {
            "p" => out.push(ch.descendants().filter(|d| d.is_text()).map(|t| t.text().unwrap_or("")).collect::<Vec<_>>().join("").split_whitespace().collect::<Vec<_>>().join(" ")),
            "ul" | "ol" => {
                for li in ch.children().filter(|c| c.has_tag_name("li")) {
                    out.push(format!("\u{2022} {}", li.descendants().filter(|d| d.is_text()).map(|t| t.text().unwrap_or("")).collect::<Vec<_>>().join("").split_whitespace().collect::<Vec<_>>().join(" ")));
                }
            }
            _ => {}
        }
    }
    out.retain(|p| !p.is_empty());
    out.join("\n\n")
}

/// The first component of an AppStream XML document.
pub fn parse_xml(xml: &str, from: &str) -> Option<Details> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let comp = doc.descendants().find(|n| n.has_tag_name("component"))?;
    let text_of = |name: &str| comp.children().find(|c| c.has_tag_name(name)).and_then(|c| c.text()).unwrap_or("").trim().to_string();
    let description = comp.children().find(|c| c.has_tag_name("description")).map(paragraphs).unwrap_or_default();
    let homepage = comp.children().filter(|c| c.has_tag_name("url")).find(|c| c.attribute("type") == Some("homepage")).and_then(|c| c.text()).unwrap_or("").trim().to_string();
    let developer = comp.children().find(|c| c.has_tag_name("developer")).and_then(|d| d.children().find(|n| n.has_tag_name("name"))).and_then(|n| n.text()).or_else(|| comp.children().find(|c| c.has_tag_name("developer_name")).and_then(|c| c.text())).unwrap_or("").trim().to_string();
    let mut shots = vec![];
    if let Some(ss) = comp.children().find(|c| c.has_tag_name("screenshots")) {
        for sh in ss.children().filter(|c| c.has_tag_name("screenshot")) {
            let caption = sh.children().find(|c| c.has_tag_name("caption")).and_then(|c| c.text()).unwrap_or("").trim().to_string();
            // the biggest-looking source image is large; any thumbnail is fine for this window: prefer a thumbnail ~ 600-800 px wide, else the source
            let imgs: Vec<_> = sh.children().filter(|c| c.has_tag_name("image")).collect();
            let thumb = imgs.iter().filter(|i| i.attribute("type") == Some("thumbnail")).min_by_key(|i| (i.attribute("width").and_then(|w| w.parse::<i32>().ok()).unwrap_or(0) - 752).abs());
            let pick = thumb.or_else(|| imgs.iter().find(|i| i.attribute("type") != Some("thumbnail"))).or(imgs.first());
            if let Some(url) = pick.and_then(|i| i.text()).map(|t| t.trim().to_string()).filter(|u| u.starts_with("https://")) {
                shots.push(Shot { url, caption });
            }
        }
    }
    Some(Details { summary: text_of("summary"), description, screenshots: shots, homepage, license: text_of("project_license"), developer, from: from.to_string() })
}

/// The installed app's own AppStream data (no network).
pub fn local(appstream_id: &str) -> Option<Details> {
    if !safe_id(appstream_id) {
        return None;
    }
    let out = std::process::Command::new("appstreamcli").args(["dump", appstream_id]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let d = parse_xml(&String::from_utf8_lossy(&out.stdout), "this computer")?;
    (!d.description.is_empty() || !d.screenshots.is_empty()).then_some(d)
}

/// HTML from Flathub's description as plain paragraphs.
pub fn html_to_text(html: &str) -> String {
    let mut s = html.replace("</p>", "\n\n").replace("<li>", "\n\u{2022} ").replace("</li>", "").replace("<br>", "\n").replace("<br/>", "\n");
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.drain(..) {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    let out = out.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&nbsp;", " ");
    let paras: Vec<String> = out.split("\n\n").map(|p| p.lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")).filter(|p| !p.is_empty()).collect();
    paras.join("\n\n")
}

pub fn parse_flathub(json_text: &str) -> Option<Details> {
    let v: Value = serde_json::from_str(json_text).ok()?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let mut shots = vec![];
    for sh in v.get("screenshots").and_then(|x| x.as_array()).cloned().unwrap_or_default() {
        let sizes = sh.get("sizes").and_then(|x| x.as_array()).cloned().unwrap_or_default();
        let w = |z: &Value| z.get("width").and_then(|w| w.as_str().map(|s| s.parse::<i32>().unwrap_or(0)).or_else(|| w.as_i64().map(|n| n as i32))).unwrap_or(0);
        // the size closest to 750 px wide: sharp in this window without a huge download
        let pick = sizes.iter().min_by_key(|z| (w(z) - 752).abs());
        if let Some(url) = pick.and_then(|z| z.get("src")).and_then(|u| u.as_str()).filter(|u| u.starts_with("https://")) {
            shots.push(Shot { url: url.to_string(), caption: sh.get("caption").and_then(|c| c.as_str()).unwrap_or("").to_string() });
        }
    }
    let homepage = v.get("urls").and_then(|u| u.get("homepage")).and_then(|h| h.as_str()).unwrap_or("").to_string();
    // Flathub's own packaging notes ("This flatpak is community maintained ...") are about the Flatpak, not the app
    let description = html_to_text(&s("description")).split("\n\n").filter(|p| !p.to_lowercase().contains("this flatpak")).collect::<Vec<_>>().join("\n\n");
    Some(Details { summary: s("summary"), description, screenshots: shots, homepage, license: s("project_license"), developer: s("developer_name"), from: "Flathub".into() })
}

fn details_cache(id: &str) -> PathBuf {
    cache_root().join("details").join(format!("{id}.json"))
}

fn to_json(d: &Details) -> Value {
    json!({"summary": d.summary, "description": d.description, "homepage": d.homepage, "license": d.license, "developer": d.developer, "from": d.from, "shots": d.screenshots.iter().map(|s| json!({"url": s.url, "caption": s.caption})).collect::<Vec<_>>()})
}

fn from_json(v: &Value) -> Details {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    Details {
        summary: s("summary"),
        description: s("description"),
        homepage: s("homepage"),
        license: s("license"),
        developer: s("developer"),
        from: s("from"),
        screenshots: v.get("shots").and_then(|x| x.as_array()).map(|a| a.iter().map(|x| Shot { url: x.get("url").and_then(|u| u.as_str()).unwrap_or("").to_string(), caption: x.get("caption").and_then(|u| u.as_str()).unwrap_or("").to_string() }).collect()).unwrap_or_default(),
    }
}

/// Everything known about an entry: its own AppStream data if installed, else (when allowed) Flathub's; kept in the cache.
pub fn load(entry_id: &str, appstream: &str, flathub: &str, online: bool) -> Option<Details> {
    if safe_id(entry_id) {
        if let Ok(t) = std::fs::read_to_string(details_cache(entry_id)) {
            if let Ok(v) = serde_json::from_str::<Value>(&t) {
                return Some(from_json(&v));
            }
        }
    }
    let found = if !appstream.is_empty() { local(appstream) } else { None }.or_else(|| {
        if online && !flathub.is_empty() && safe_id(flathub) {
            let out = std::process::Command::new("curl").args(["-fsSL", "--proto", "=https", "--max-time", "30", "--max-filesize", "3000000", &format!("https://flathub.org/api/v2/appstream/{flathub}")]).output().ok()?;
            if out.status.success() { parse_flathub(&String::from_utf8_lossy(&out.stdout)) } else { None }
        } else {
            None
        }
    })?;
    // a "this computer" answer is cheap to get again and changes with the installed version; only the downloaded one is cached
    if found.from == "Flathub" && safe_id(entry_id) {
        let p = details_cache(entry_id);
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(p, to_json(&found).to_string());
    }
    Some(found)
}

/// Download a picture into the cache (HTTPS only, size-limited); the file's path.
pub fn fetch_image(url: &str) -> Option<PathBuf> {
    if !url.starts_with("https://") {
        return None;
    }
    let mut h: u64 = 1469598103934665603;
    for b in url.bytes() {
        h = (h ^ b as u64).wrapping_mul(1099511628211);
    }
    let ext = match url.rsplit('.').next().map(|e| e.split('?').next().unwrap_or("").to_lowercase()).as_deref() {
        Some("jpg") | Some("jpeg") => "jpg",
        Some("webp") => "webp",
        _ => "png",
    };
    let dir = cache_root().join("img");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{h:016x}.{ext}"));
    if path.is_file() {
        return Some(path);
    }
    let tmp = dir.join(format!("{h:016x}.part"));
    let ok = std::process::Command::new("curl").args(["-fsSL", "--proto", "=https", "--max-time", "40", "--max-filesize", "8000000", "-o", &tmp.to_string_lossy(), url]).status().map(|s| s.success()).unwrap_or(false);
    if ok && std::fs::rename(&tmp, &path).is_ok() {
        Some(path)
    } else {
        let _ = std::fs::remove_file(&tmp);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appstream_xml_is_read() {
        let xml = r#"<components><component type="desktop-application"><id>x</id><name>X</name><summary>Does x</summary>
          <description><p>First  paragraph.</p><ul><li>one</li><li>two</li></ul></description>
          <project_license>MIT</project_license><developer id="a"><name>Someone</name></developer>
          <url type="homepage">https://example.org</url>
          <screenshots><screenshot type="default"><caption>Main</caption><image type="source">https://e.org/big.png</image><image type="thumbnail" width="752" height="400">https://e.org/t.png</image></screenshot></screenshots>
          </component></components>"#;
        let d = parse_xml(xml, "this computer").unwrap();
        assert_eq!(d.summary, "Does x");
        assert_eq!(d.description, "First paragraph.\n\n\u{2022} one\n\n\u{2022} two");
        assert_eq!((d.license.as_str(), d.developer.as_str(), d.homepage.as_str()), ("MIT", "Someone", "https://example.org"));
        assert_eq!(d.screenshots, vec![Shot { url: "https://e.org/t.png".into(), caption: "Main".into() }]);
    }

    #[test]
    fn flathub_json_is_read() {
        let j = r#"{"summary":"S","description":"<p>Hello &amp; welcome</p><ul><li>a</li></ul>","project_license":"GPL-3.0","developer_name":"Dev","urls":{"homepage":"https://h.org"},
          "screenshots":[{"caption":"c","sizes":[{"width":"1920","src":"https://d/big.png"},{"width":"752","src":"https://d/mid.png"},{"width":"112","src":"https://d/small.png"}]}]}"#;
        let d = parse_flathub(j).unwrap();
        assert_eq!(d.description, "Hello & welcome\n\n\u{2022} a");
        assert_eq!(d.screenshots[0].url, "https://d/mid.png");
        assert_eq!(d.from, "Flathub");
    }

    #[test]
    fn only_https_pictures_are_fetched() {
        assert!(fetch_image("http://example.org/a.png").is_none());
        assert!(!safe_id("../x"));
    }
}
