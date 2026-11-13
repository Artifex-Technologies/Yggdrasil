//! The time zones to choose from, consolidated: the tz database's ~400 zones people live in (zone.tab) become one row per
//! "friendly" zone, as Windows lists them, grouped and sorted by their offset from UTC. A friendly zone is every zone of one
//! part of the world that keeps the same clock all of the coming year (Denver, Edmonton and Boise are "Mountain Time (US &
//! Canada)"); its row names its main cities, and a search finds it by any city, country or zone name. What gets saved is
//! still one tz database name: the row's main city's, the city a search found, or the city nearest a click on the map.

use std::collections::BTreeMap;

/// One tz database zone (a line of zone.tab, or UTC).
#[derive(Clone, Debug, PartialEq)]
pub struct Zone {
    /// "America/Denver": what is saved (timedatectl set-timezone).
    pub id: String,
    /// "Denver" ("America/Argentina/Buenos_Aires" is "Buenos Aires").
    pub city: String,
    /// "United States" (iso3166.tab's name); empty for UTC.
    pub country: String,
    /// Where the city is (degrees east and north), for the map; None for UTC and a zone not in zone.tab.
    pub at: Option<(f32, f32)>,
    /// Seconds east of UTC in standard time (the least offset of the coming year, so without daylight saving time).
    pub standard: i32,
    /// Seconds east of UTC now.
    pub now: i32,
}

/// One row of the list: zones that keep the same clock, in one part of the world.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    /// "Mountain Time (US & Canada)"; for a group without a name of its own, its main cities ("Kathmandu").
    pub name: String,
    /// Whether `name` is a name of its own (the row then lists the cities under it; otherwise the countries).
    pub named: bool,
    /// Seconds east of UTC in standard time: the list's sections.
    pub standard: i32,
    /// Indexes into `Catalog::zones`, the main city first (big cities first, then by name).
    pub members: Vec<usize>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Catalog {
    pub zones: Vec<Zone>,
    /// By standard offset, then by name.
    pub groups: Vec<Group>,
    /// For each zone, its group.
    pub group_of: Vec<usize>,
}

/// Names of their own for the friendly zones (Windows' names where it has one). The first member of a group found here names it
/// and is its main city, so the order matters where groups merge (Mexico City before Regina: both keep -06:00 all year).
const NAMES: &[(&str, &str)] = &[
    ("Pacific/Pago_Pago", "Samoa (American), Midway"),
    ("Pacific/Honolulu", "Hawaii"),
    ("America/Adak", "Aleutian Islands"),
    ("America/Anchorage", "Alaska"),
    ("America/Los_Angeles", "Pacific Time (US & Canada)"),
    ("America/Tijuana", "Baja California"),
    ("America/Denver", "Mountain Time (US & Canada)"),
    ("America/Phoenix", "Arizona"),
    ("America/Chicago", "Central Time (US & Canada)"),
    ("America/Mexico_City", "Mexico, Central America"),
    ("America/New_York", "Eastern Time (US & Canada)"),
    ("America/Bogota", "Bogota, Lima, Quito"),
    ("America/Halifax", "Atlantic Time (Canada)"),
    ("America/Caracas", "Caracas, Caribbean"),
    ("America/St_Johns", "Newfoundland"),
    ("America/Sao_Paulo", "Brasilia, Buenos Aires"),
    ("America/Nuuk", "Greenland"),
    ("America/Noronha", "Mid-Atlantic"),
    ("Atlantic/Azores", "Azores"),
    ("Atlantic/Cape_Verde", "Cabo Verde"),
    ("Europe/London", "Western European Time (UK, Ireland, Portugal)"),
    ("Africa/Abidjan", "West Africa (Greenwich Mean Time)"),
    ("Europe/Berlin", "Central European Time"),
    ("Africa/Lagos", "West Central Africa"),
    ("Europe/Athens", "Eastern European Time"),
    ("Africa/Johannesburg", "Central and Southern Africa"),
    ("Europe/Moscow", "Moscow, Minsk, Istanbul"),
    ("Asia/Riyadh", "Arabia (Riyadh, Baghdad, Kuwait)"),
    ("Africa/Nairobi", "East Africa"),
    ("Asia/Dubai", "Gulf (Abu Dhabi, Muscat), Caucasus"),
    ("Asia/Karachi", "Pakistan, Central Asia"),
    ("Asia/Kolkata", "India, Sri Lanka"),
    ("Asia/Dhaka", "Bangladesh, Kazakhstan, Bhutan"),
    ("Asia/Bangkok", "Indochina (Bangkok, Hanoi, Jakarta)"),
    ("Asia/Shanghai", "China, Singapore, Philippines"),
    ("Australia/Perth", "Western Australia"),
    ("Asia/Tokyo", "Japan, Korea"),
    ("Australia/Adelaide", "Central Australia (Adelaide)"),
    ("Australia/Darwin", "Northern Territory (Darwin)"),
    ("Australia/Sydney", "Eastern Australia (Sydney, Melbourne)"),
    ("Australia/Brisbane", "Queensland (Brisbane)"),
    ("Australia/Lord_Howe", "Lord Howe Island"),
    ("Pacific/Noumea", "Solomon Islands, New Caledonia"),
    ("Pacific/Auckland", "New Zealand"),
    ("Pacific/Chatham", "Chatham Islands"),
    ("Pacific/Tongatapu", "Tonga, Samoa"),
    ("Pacific/Kiritimati", "Line Islands (Kiritimati)"),
];

/// Big cities first (roughly by population): the order of a group's members after the one that names it.
const MAIN: &[&str] = &[
    "Asia/Shanghai",
    "Asia/Tokyo",
    "Asia/Kolkata",
    "America/Sao_Paulo",
    "America/Mexico_City",
    "Africa/Cairo",
    "Asia/Dhaka",
    "America/New_York",
    "Asia/Karachi",
    "America/Argentina/Buenos_Aires",
    "Europe/Istanbul",
    "Africa/Lagos",
    "Asia/Manila",
    "Africa/Kinshasa",
    "Europe/Moscow",
    "America/Bogota",
    "America/Lima",
    "Asia/Bangkok",
    "Asia/Seoul",
    "Asia/Jakarta",
    "Europe/London",
    "Asia/Tehran",
    "America/Chicago",
    "Asia/Ho_Chi_Minh",
    "Africa/Luanda",
    "Asia/Kuala_Lumpur",
    "Asia/Hong_Kong",
    "Asia/Riyadh",
    "Asia/Baghdad",
    "Asia/Singapore",
    "America/Santiago",
    "Africa/Khartoum",
    "Africa/Johannesburg",
    "Africa/Dar_es_Salaam",
    "America/Los_Angeles",
    "Europe/Madrid",
    "Africa/Nairobi",
    "America/Toronto",
    "Asia/Yangon",
    "Africa/Abidjan",
    "Europe/Berlin",
    "Africa/Algiers",
    "Africa/Casablanca",
    "Asia/Kabul",
    "Europe/Kyiv",
    "Africa/Addis_Ababa",
    "Asia/Taipei",
    "Europe/Rome",
    "Asia/Tashkent",
    "Europe/Paris",
    "America/Caracas",
    "Asia/Dubai",
    "America/Havana",
    "Asia/Almaty",
    "America/Guatemala",
    "Asia/Pyongyang",
    "Australia/Sydney",
    "Australia/Melbourne",
    "Europe/Bucharest",
    "Africa/Accra",
    "Europe/Minsk",
    "Europe/Vienna",
    "Europe/Warsaw",
    "Europe/Budapest",
    "America/Montevideo",
    "America/Phoenix",
    "America/Denver",
    "America/Edmonton",
    "America/Vancouver",
    "America/Halifax",
    "America/Anchorage",
    "Pacific/Honolulu",
    "Asia/Kathmandu",
    "Europe/Athens",
    "Europe/Helsinki",
    "Europe/Stockholm",
    "Europe/Amsterdam",
    "Europe/Brussels",
    "Europe/Prague",
    "Europe/Lisbon",
    "Europe/Dublin",
    "Asia/Jerusalem",
    "Asia/Beirut",
    "Asia/Colombo",
    "Australia/Brisbane",
    "Australia/Perth",
    "Australia/Adelaide",
    "Pacific/Auckland",
    "Asia/Vladivostok",
    "Asia/Novosibirsk",
    "Asia/Yekaterinburg",
    "Asia/Irkutsk",
    "America/Winnipeg",
    "America/Regina",
    "America/Panama",
    "America/Puerto_Rico",
    "America/Santo_Domingo",
    "America/La_Paz",
    "America/Asuncion",
];

/// "Denver"; "America/Argentina/Buenos_Aires" is "Buenos Aires".
pub fn city(id: &str) -> String {
    if id == "UTC" {
        return "Coordinated Universal Time".into();
    }
    id.rsplit('/').next().unwrap_or(id).replace('_', " ")
}

/// "UTC-07:00", "UTC+05:30", "UTC+00:00" (an ASCII minus: every font has it).
pub fn utc_offset(seconds: i32) -> String {
    let sign = if seconds < 0 { '-' } else { '+' };
    let m = seconds.unsigned_abs() / 60;
    format!("UTC{sign}{:02}:{:02}", m / 60, m % 60)
}

/// The part of the world a zone is in, for grouping: the tz database's area, with the Arctic counted as Europe and the Atlantic
/// islands with the continent they keep time with.
pub fn area(id: &str) -> &str {
    let (a, rest) = id.split_once('/').unwrap_or((id, ""));
    match (a, rest) {
        ("Arctic", _) => "Europe",
        ("Atlantic", "Bermuda" | "Stanley" | "South_Georgia") => "America",
        ("Atlantic", "Cape_Verde" | "St_Helena") => "Africa",
        ("Atlantic", _) => "Europe",
        _ => a,
    }
}

/// zone.tab's place: "+394421-1045903" (±DDMM[SS]±DDDMM[SS]) as degrees east and north.
pub fn parse_place(s: &str) -> Option<(f32, f32)> {
    let split = s[1..].find(['+', '-'])? + 1;
    let (lat, lon) = s.split_at(split);
    let deg = |v: &str, d: usize| -> Option<f32> {
        let (sign, digits) = (if v.starts_with('-') { -1.0 } else { 1.0 }, &v[1..]);
        if digits.len() < d + 2 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let whole: f32 = digits[..d].parse().ok()?;
        let min: f32 = digits[d..d + 2].parse().ok()?;
        let sec: f32 = if digits.len() >= d + 4 { digits[d + 2..d + 4].parse().ok()? } else { 0.0 };
        Some(sign * (whole + min / 60.0 + sec / 3600.0))
    };
    Some((deg(lon, 3)?, deg(lat, 2)?))
}

/// A line of zone.tab: the zone, its country's name, where its city is.
pub type Place = (String, String, Option<(f32, f32)>);

/// zone.tab and iso3166.tab (tzdata's own tables): (zone, country name, place) for the zones people live in, without
/// Antarctica's research stations.
pub fn parse_zone_tab(zone_tab: &str, iso3166: &str) -> Vec<Place> {
    let names: BTreeMap<&str, &str> =
        iso3166.lines().filter(|l| !l.starts_with('#')).filter_map(|l| l.split_once('\t')).map(|(c, n)| (c, n.trim())).collect();
    zone_tab
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let mut f = l.split('\t');
            let (cc, place, id) = (f.next()?, f.next()?, f.next()?.trim());
            (!id.starts_with("Antarctica/")).then(|| (id.to_string(), names.get(cc).unwrap_or(&cc).to_string(), parse_place(place)))
        })
        .collect()
}

impl Catalog {
    /// The catalog from zone.tab's lines (`parse_zone_tab`), with each zone's offsets over the coming year from `offsets` (now
    /// first, then samples spread over the year; None for a zone the tz database does not have). UTC is always there, and so is
    /// `current` (the zone in use) even when it is an alias such as "US/Mountain".
    pub fn build(places: &[Place], current: &str, offsets: impl Fn(&str) -> Option<Vec<i32>>) -> Catalog {
        let mut zones = vec![];
        let mut signatures = vec![];
        let extra = (!current.is_empty() && current != "UTC" && !places.iter().any(|(id, ..)| id == current))
            .then(|| (current.to_string(), String::new(), None));
        for (id, country, at) in places.iter().cloned().chain(std::iter::once(("UTC".to_string(), String::new(), None))).chain(extra) {
            let Some(o) = offsets(&id).filter(|o| !o.is_empty()) else { continue };
            zones.push(Zone { city: city(&id), id, country, at, standard: *o.iter().min().unwrap_or(&0), now: o[0] });
            signatures.push(o);
        }
        // one group per part of the world and clock over the year
        let mut by_key: BTreeMap<(String, Vec<i32>), Vec<usize>> = BTreeMap::new();
        for (i, z) in zones.iter().enumerate() {
            let a = if z.id == "UTC" { "UTC".to_string() } else { area(&z.id).to_string() };
            by_key.entry((a, signatures[i].clone())).or_default().push(i);
        }
        let rank = |id: &str| -> usize {
            NAMES
                .iter()
                .position(|(n, _)| *n == id)
                .or_else(|| MAIN.iter().position(|m| *m == id).map(|p| NAMES.len() + p))
                .unwrap_or(usize::MAX)
        };
        let mut groups: Vec<Group> = by_key
            .into_values()
            .map(|mut members| {
                members.sort_by(|&a, &b| rank(&zones[a].id).cmp(&rank(&zones[b].id)).then_with(|| zones[a].city.cmp(&zones[b].city)));
                let named = NAMES.iter().find(|(id, _)| members.iter().any(|&m| zones[m].id == *id));
                if let Some((id, _)) = named {
                    // the zone that names the group is its main city
                    let p = members.iter().position(|&m| zones[m].id == *id).unwrap_or(0);
                    let m = members.remove(p);
                    members.insert(0, m);
                }
                let first = &zones[members[0]];
                let name = match (first.id.as_str(), named) {
                    ("UTC", _) => "Coordinated Universal Time".to_string(),
                    (_, Some((_, n))) => n.to_string(),
                    _ => members.iter().take(3).map(|&m| zones[m].city.as_str()).collect::<Vec<_>>().join(", "),
                };
                Group { name, named: named.is_some() || first.id == "UTC", standard: first.standard, members }
            })
            .collect();
        groups.sort_by(|a, b| a.standard.cmp(&b.standard).then_with(|| a.name.cmp(&b.name)));
        let mut group_of = vec![0; zones.len()];
        for (g, gr) in groups.iter().enumerate() {
            for &m in &gr.members {
                group_of[m] = g;
            }
        }
        Catalog { zones, groups, group_of }
    }

    /// The system's catalog: /usr/share/zoneinfo's zone.tab and iso3166.tab, each zone's offsets from the tz database now and
    /// every 15 days for a year.
    pub fn system(current: &str) -> Catalog {
        let read = |f: &str| std::fs::read_to_string(format!("/usr/share/zoneinfo/{f}")).unwrap_or_default();
        let places = parse_zone_tab(&read("zone.tab"), &read("iso3166.tab"));
        let now = jiff::Timestamp::now();
        let instants: Vec<jiff::Timestamp> =
            (0..25).filter_map(|k| now.checked_add(jiff::SignedDuration::from_hours(24 * 15 * k)).ok()).collect();
        Catalog::build(&places, current, |id| {
            let tz = jiff::tz::TimeZone::get(id).ok()?;
            Some(instants.iter().map(|t| tz.to_offset(*t).seconds()).collect())
        })
    }

    pub fn index(&self, id: &str) -> Option<usize> {
        self.zones.iter().position(|z| z.id == id)
    }

    /// The zone the map picks at a point: the nearest city among those whose standard offset is `offset` (the zone band the
    /// point is in), else the nearest city of all.
    pub fn nearest(&self, lon: f32, lat: f32, offset: Option<i32>) -> Option<usize> {
        let dist = |z: &Zone| {
            let (x, y) = z.at?;
            let mut dl = (x - lon).abs() % 360.0;
            if dl > 180.0 {
                dl = 360.0 - dl;
            }
            let dx = dl * ((y + lat) / 2.0).to_radians().cos();
            let dy = y - lat;
            Some(dx * dx + dy * dy)
        };
        let best = |only: Option<i32>| {
            self.zones
                .iter()
                .enumerate()
                .filter(|(_, z)| only.is_none_or(|o| z.standard == o))
                .filter_map(|(i, z)| dist(z).map(|d| (i, d)))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
        };
        offset.and_then(|o| best(Some(o))).or_else(|| best(None))
    }

    /// The rows a search shows, in the list's order: each with the member it found (the main city when the group's own name, its
    /// offset or its main city matched; otherwise the first city or country that did). An empty search shows every row.
    pub fn search(&self, q: &str) -> Vec<(usize, usize)> {
        let q = norm(q);
        self.groups
            .iter()
            .enumerate()
            .filter_map(|(g, gr)| {
                if q.is_empty() {
                    return Some((g, gr.members[0]));
                }
                let whole = norm(&gr.name).contains(&q) || offset_words(gr.standard).iter().any(|w| w.starts_with(&q));
                let member = gr.members.iter().copied().find(|&m| {
                    let z = &self.zones[m];
                    norm(&z.city).contains(&q)
                        || norm(&z.country).contains(&q)
                        || norm(&z.id).contains(&q)
                        || offset_words(z.now).iter().any(|w| w.starts_with(&q))
                });
                match (whole, member) {
                    (true, _) => Some((g, gr.members[0])),
                    (false, Some(m)) => Some((g, m)),
                    _ => None,
                }
            })
            .collect()
    }

    /// The line for a zone in Windows' style: "(UTC-07:00) Mountain Time (US & Canada): Denver".
    pub fn label(&self, id: &str) -> String {
        let Some(i) = self.index(id) else { return id.to_string() };
        let (z, g) = (&self.zones[i], &self.groups[self.group_of[i]]);
        if z.id == "UTC" {
            return "(UTC) Coordinated Universal Time".into();
        }
        if g.named {
            format!("({}) {}: {}", utc_offset(g.standard), g.name, z.city)
        } else if z.country.is_empty() {
            format!("({}) {}", utc_offset(g.standard), z.city)
        } else {
            format!("({}) {}, {}", utc_offset(g.standard), z.city, z.country)
        }
    }

    /// What a row says under its name: the cities (with `found` first, the one a search matched), or for a group named after
    /// its cities, the countries. At most `n`, then "and 4 more".
    pub fn row_detail(&self, g: usize, found: usize, n: usize) -> String {
        let gr = &self.groups[g];
        if self.zones[gr.members[0]].id == "UTC" {
            return "No daylight saving time".into();
        }
        let order: Vec<usize> = std::iter::once(found).chain(gr.members.iter().copied().filter(|&m| m != found)).collect();
        let list = |cities: bool| {
            let mut items: Vec<String> = vec![];
            for &m in &order {
                let z = &self.zones[m];
                let s = if cities { z.city.clone() } else { z.country.clone() };
                // a row named after its one city ("Azores") names its country instead
                if !s.is_empty() && s != gr.name && !items.contains(&s) {
                    items.push(s);
                }
            }
            items
        };
        let mut items = if gr.named { list(true) } else { list(false) };
        if items.is_empty() {
            items = list(false);
        }
        let more = items.len().saturating_sub(n);
        items.truncate(n);
        let mut s = items.join(", ");
        if more > 0 {
            s.push_str(&format!(" and {more} more"));
        }
        s
    }
}

/// Lower case, "_" and "/" as spaces, and the accents of Latin letters dropped ("São Paulo" matches "sao paulo").
pub fn norm(s: &str) -> String {
    s.trim()
        .chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            '_' | '/' => ' ',
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
            'ç' => 'c',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ñ' => 'n',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ý' | 'ÿ' => 'y',
            c => c,
        })
        .collect()
}

/// The ways a search may name an offset: "utc-07:00", "utc-07", "utc-7", "gmt-7", "-07:00", "-7".
fn offset_words(seconds: i32) -> Vec<String> {
    let sign = if seconds < 0 { '-' } else { '+' };
    let m = seconds.unsigned_abs() / 60;
    let (h, mm) = (m / 60, m % 60);
    let mut v = vec![];
    for p in ["utc", "gmt", ""] {
        v.push(format!("{p}{sign}{h:02}:{mm:02}"));
        if mm == 0 {
            v.push(format!("{p}{sign}{h:02}"));
            v.push(format!("{p}{sign}{h}"));
        } else {
            v.push(format!("{p}{sign}{h}:{mm:02}"));
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: i32 = 3600;

    /// A small world: offsets now and over a year (summer in the second half), for zones of three continents.
    fn sample() -> Catalog {
        let zone_tab = "# comment\n\
            US\t+394421-1045903\tAmerica/Denver\tMountain (most areas)\n\
            US\t+433649-1161209\tAmerica/Boise\tMountain - ID (south)\n\
            CA\t+5333-11328\tAmerica/Edmonton\tMountain - AB\n\
            US\t+332654-1120424\tAmerica/Phoenix\tMST - AZ (most areas)\n\
            US\t+415100-0873900\tAmerica/Chicago\tCentral (most areas)\n\
            GB\t+513030-0000731\tEurope/London\n\
            DE\t+5230+01322\tEurope/Berlin\n\
            FR\t+4852+00220\tEurope/Paris\n\
            NO\t+5955+01045\tEurope/Oslo\n\
            AR\t-3436-05827\tAmerica/Argentina/Buenos_Aires\tBuenos Aires (BA, CF)\n\
            BR\t-2332-04637\tAmerica/Sao_Paulo\n\
            IN\t+2232+08822\tAsia/Kolkata\n\
            NP\t+2743+08519\tAsia/Kathmandu\n\
            AQ\t-7750+16636\tAntarctica/McMurdo\n";
        let iso = "# comment\nUS\tUnited States\nCA\tCanada\nGB\tBritain (UK)\nDE\tGermany\nFR\tFrance\nNO\tNorway\nAR\tArgentina\n\
                   BR\tBrazil\nIN\tIndia\nNP\tNepal\nAQ\tAntarctica\n";
        let places = parse_zone_tab(zone_tab, iso);
        Catalog::build(&places, "Europe/London", |id| {
            let (std, dst) = match id {
                "America/Denver" | "America/Boise" | "America/Edmonton" | "US/Mountain" => (-7 * H, true),
                "America/Phoenix" => (-7 * H, false),
                "America/Chicago" => (-6 * H, true),
                "Europe/London" => (0, true),
                "UTC" => (0, false),
                "Europe/Berlin" | "Europe/Paris" | "Europe/Oslo" => (H, true),
                "America/Argentina/Buenos_Aires" | "America/Sao_Paulo" => (-3 * H, false),
                "Asia/Kolkata" => (5 * H + 1800, false),
                "Asia/Kathmandu" => (5 * H + 2700, false),
                _ => return None,
            };
            Some((0..24).map(|k| if dst && k >= 12 { std + H } else { std }).collect())
        })
    }

    fn names(c: &Catalog) -> Vec<&str> {
        c.groups.iter().map(|g| g.name.as_str()).collect()
    }

    #[test]
    fn zones_that_keep_the_same_clock_in_one_part_of_the_world_are_one_row() {
        let c = sample();
        assert_eq!(c.zones.len(), 14, "13 places (no Antarctica) and UTC");
        assert_eq!(
            names(&c),
            [
                "Arizona",
                "Mountain Time (US & Canada)",
                "Central Time (US & Canada)",
                "Brasilia, Buenos Aires",
                "Coordinated Universal Time",
                "Western European Time (UK, Ireland, Portugal)",
                "Central European Time",
                "India, Sri Lanka",
                "Kathmandu",
            ],
            "sorted by standard offset, then by name"
        );
        let mountain = &c.groups[1];
        assert_eq!(mountain.standard, -7 * H);
        let cities: Vec<&str> = mountain.members.iter().map(|&m| c.zones[m].city.as_str()).collect();
        assert_eq!(cities, ["Denver", "Edmonton", "Boise"], "the naming city first, then big cities, then by name");
        assert_eq!(c.group_of[c.index("America/Boise").unwrap()], 1);
        // Arizona keeps -07:00 all year: not Mountain Time
        assert_eq!(c.group_of[c.index("America/Phoenix").unwrap()], 0);
        assert_eq!(c.groups[7].standard, 5 * H + 1800);
        assert!(!c.groups[8].named && c.groups[8].name == "Kathmandu");
    }

    #[test]
    fn the_zone_in_use_is_listed_even_as_an_alias() {
        let places = parse_zone_tab("US\t+394421-1045903\tAmerica/Denver\n", "US\tUnited States\n");
        let c = Catalog::build(&places, "US/Mountain", |id| match id {
            "America/Denver" | "US/Mountain" => Some(vec![-7 * H; 3]),
            "UTC" => Some(vec![0; 3]),
            _ => None,
        });
        assert!(c.index("US/Mountain").is_some());
        assert_eq!(c.label("US/Mountain"), "(UTC-07:00) Mountain");
        assert_eq!(c.label("America/Denver"), "(UTC-07:00) Mountain Time (US & Canada): Denver");
        assert_eq!(c.label("UTC"), "(UTC) Coordinated Universal Time");
        assert_eq!(c.label("Mars/Base"), "Mars/Base", "unknown: as it is");
    }

    #[test]
    fn places_and_offsets_read_as_written() {
        assert_eq!(utc_offset(-7 * H), "UTC-07:00");
        assert_eq!(utc_offset(5 * H + 1800), "UTC+05:30");
        assert_eq!(utc_offset(0), "UTC+00:00");
        let (lon, lat) = parse_place("+394421-1045903").unwrap();
        assert!((lat - 39.739).abs() < 0.01 && (lon + 104.984).abs() < 0.01);
        let (lon, lat) = parse_place("-3436-05827").unwrap();
        assert!((lat + 34.6).abs() < 0.01 && (lon + 58.45).abs() < 0.01);
        assert_eq!(parse_place("+12"), None);
        assert_eq!(city("America/Argentina/Buenos_Aires"), "Buenos Aires");
        assert_eq!(area("Atlantic/Bermuda"), "America");
        assert_eq!(area("Atlantic/Canary"), "Europe");
        assert_eq!(area("Arctic/Longyearbyen"), "Europe");
    }

    #[test]
    fn search_finds_a_row_by_city_country_zone_name_or_offset() {
        let c = sample();
        let found = |q: &str| c.search(q).into_iter().map(|(g, m)| (c.groups[g].name.as_str(), c.zones[m].id.as_str())).collect::<Vec<_>>();
        assert_eq!(c.search("").len(), c.groups.len(), "nothing typed: every row");
        assert_eq!(found("edmonton"), [("Mountain Time (US & Canada)", "America/Edmonton")], "a city: that city is what is chosen");
        assert_eq!(
            found("Canada"),
            [("Mountain Time (US & Canada)", "America/Denver"), ("Central Time (US & Canada)", "America/Chicago")],
            "the row's own name: its main city"
        );
        assert_eq!(found("norway"), [("Central European Time", "Europe/Oslo")], "a country");
        assert_eq!(found("america/boise"), [("Mountain Time (US & Canada)", "America/Boise")], "a zone name");
        assert_eq!(found("sao paulo"), [("Brasilia, Buenos Aires", "America/Sao_Paulo")]);
        assert_eq!(found("São Paulo"), [("Brasilia, Buenos Aires", "America/Sao_Paulo")], "accents do not matter");
        assert_eq!(found("UTC+05:30"), [("India, Sri Lanka", "Asia/Kolkata")]);
        assert_eq!(
            found("utc-7"),
            [("Arizona", "America/Phoenix"), ("Mountain Time (US & Canada)", "America/Denver")],
            "an offset: every row of it"
        );
        assert!(found("atlantis").is_empty());
    }

    #[test]
    fn a_row_lists_its_cities_or_countries() {
        let c = sample();
        assert_eq!(c.row_detail(1, c.groups[1].members[0], 2), "Denver, Edmonton and 1 more");
        assert_eq!(c.row_detail(1, c.index("America/Boise").unwrap(), 5), "Boise, Denver, Edmonton", "the found city first");
        let k = c.groups.iter().position(|g| g.name == "Kathmandu").unwrap();
        assert_eq!(c.row_detail(k, c.groups[k].members[0], 3), "Nepal");
        let utc = c.groups.iter().position(|g| g.name == "Coordinated Universal Time").unwrap();
        assert_eq!(c.row_detail(utc, c.groups[utc].members[0], 3), "No daylight saving time");
    }

    #[test]
    fn the_map_picks_the_nearest_city_of_the_band_clicked() {
        let c = sample();
        let id = |i: Option<usize>| i.map(|i| c.zones[i].id.as_str());
        // western Kansas: nearer Denver, but in the -06:00 band, so Chicago
        assert_eq!(id(c.nearest(-101.0, 38.5, Some(-6 * H))), Some("America/Chicago"));
        assert_eq!(id(c.nearest(-101.0, 38.5, None)), Some("America/Denver"), "no band known: the nearest city");
        // near Edmonton
        assert_eq!(id(c.nearest(-113.0, 52.0, Some(-7 * H))), Some("America/Edmonton"));
        // a band without a city of it (an ocean): the nearest city of all
        assert_eq!(id(c.nearest(-30.0, 45.0, Some(-2 * H))), Some("Europe/London"));
        // across the date line: from 179 east, Boise (116 west) is nearer than Kolkata (88 east)
        assert_eq!(id(c.nearest(179.0, 25.0, None)), Some("America/Boise"));
        // UTC has no place on the map
        assert_ne!(id(c.nearest(0.0, 0.0, Some(0))), Some("UTC"));
    }
}
