//! What a report leaves out: the names that make a person or their home recognisable (the accounts' names and full names, the
//! computer's name, saved networks' and paired devices' names, found on this computer), and anything shaped like a network
//! address (IPv4, IPv6, a hardware address), an e-mail address or a serial number. The device's make, model and parts stay:
//! they are what a fix needs.

/// The names to take out, each with its stand-in, longest first (so a home folder's path goes before the account's name).
#[derive(Debug, Default, Clone)]
pub struct Personal {
    words: Vec<(String, &'static str)>,
}

impl Personal {
    /// Takes `word` out wherever it stands as a word of its own (letters or digits on neither side), in any case. A name of
    /// one or two letters is kept: it would come out of ordinary words and numbers.
    pub fn add(&mut self, word: &str, stand_in: &'static str) {
        let w = word.trim();
        if w.chars().count() < 3 || self.words.iter().any(|(x, _)| x.eq_ignore_ascii_case(w)) {
            return;
        }
        self.words.push((w.to_string(), stand_in));
        self.words.sort_by_key(|a| std::cmp::Reverse(a.0.len()));
    }
}

/// `text` with the names in `p` and every address, e-mail address and serial number replaced by a stand-in.
pub fn scrub(text: &str, p: &Personal) -> String {
    // addresses first, whole (an e-mail address with the account's name in it goes as one)
    let out = tokens(text, mac, "<mac>");
    let out = tokens(&out, ipv6, "<ip>");
    let out = tokens(&out, ipv4, "<ip>");
    let out = tokens(&out, email, "<email>");
    let mut out = serials(&out);
    for (w, stand_in) in &p.words {
        out = replace_word(&out, w, stand_in);
    }
    out
}

fn word_char(c: char) -> bool {
    c.is_alphanumeric()
}

/// Every whole-word `word` in `text` (ASCII case ignored) replaced by `stand_in`.
fn replace_word(text: &str, word: &str, stand_in: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let needle = word.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut from = 0;
    while let Some(i) = lower[from..].find(&needle).map(|i| i + from) {
        let end = i + needle.len();
        let before = text[..i].chars().next_back().is_some_and(word_char);
        let after = text[end..].chars().next().is_some_and(word_char);
        if !before && !after {
            out.push_str(&text[last..i]);
            out.push_str(stand_in);
            last = end;
        }
        from = end;
    }
    out.push_str(&text[last..]);
    out
}

/// `text` with every run of characters that `is` accepts (a run of the characters an address can hold, with no letter, digit
/// or underscore on either side) replaced by `stand_in`; an address's /prefix (192.168.1.5/24) goes with it. A path is cut at
/// its slashes, so an address in a file's path goes too (/var/lib/bluetooth/<mac>/...).
fn tokens(text: &str, is: fn(&str) -> bool, stand_in: &str) -> String {
    let piece = |c: char| c.is_alphanumeric() || matches!(c, ':' | '.' | '-' | '@' | '_' | '+' | '%');
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        let start = rest.find(piece).unwrap_or(rest.len());
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest.find(|c: char| !piece(c)).unwrap_or(rest.len());
        let tok = &rest[..end];
        // an address ends a sentence with a full stop, a colon or a dash now and then: those stay
        let core = tok.trim_end_matches(['.', ':', '-']);
        let mut end = end;
        if is(core) {
            out.push_str(stand_in);
            out.push_str(&tok[core.len()..]);
            // the prefix's length: "/24", "/64"
            if core.len() == tok.len() {
                if let Some(after) = rest[end..].strip_prefix('/') {
                    let digits = after.len() - after.trim_start_matches(|c: char| c.is_ascii_digit()).len();
                    if (1..=3).contains(&digits) && !after[digits..].starts_with(|c: char| c.is_alphanumeric()) {
                        end += 1 + digits;
                    }
                }
            }
        } else {
            out.push_str(tok);
        }
        rest = &rest[end..];
    }
    out
}

/// A hardware (MAC) address: six pairs of hexadecimal digits split by ':' or '-'.
fn mac(t: &str) -> bool {
    let parts: Vec<&str> = t.split([':', '-']).collect();
    parts.len() == 6 && parts.iter().all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// An IPv4 address (four numbers up to 255), maybe with a /prefix; this computer's own loopback and 0.0.0.0 stay.
fn ipv4(t: &str) -> bool {
    let addr = t.split('/').next().unwrap_or(t);
    let parts: Vec<&str> = addr.split('.').collect();
    parts.len() == 4
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 3 && p.chars().all(|c| c.is_ascii_digit()) && p.parse::<u16>().is_ok_and(|n| n <= 255))
        && !addr.starts_with("127.")
        && addr != "0.0.0.0"
}

/// An IPv6 address: hexadecimal groups split by ':', with "::" or seven colons (a clock's 12:34:56 is neither); ::1 stays.
fn ipv6(t: &str) -> bool {
    let addr = t.split(['/', '%']).next().unwrap_or(t);
    let colons = addr.matches(':').count();
    let groups_ok = addr.split(':').all(|g| g.len() <= 4 && g.chars().all(|c| c.is_ascii_hexdigit()) || g.contains('.') && ipv4(g));
    groups_ok && addr.chars().any(|c| c.is_ascii_hexdigit()) && (addr.contains("::") || colons == 7) && colons >= 2 && addr != "::1"
}

/// An e-mail address (its domain ending in letters); a systemd unit's name (user@1000.service, getty@tty1.service) and a
/// screen's mode (3440x1440@174.96) are not one.
fn email(t: &str) -> bool {
    const UNITS: [&str; 10] = [".service", ".socket", ".target", ".timer", ".mount", ".slice", ".scope", ".path", ".device", ".swap"];
    let Some((user, domain)) = t.split_once('@') else { return false };
    !user.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && domain.chars().all(|c| c.is_alphanumeric() || matches!(c, '.' | '-'))
        && domain.rsplit('.').next().is_some_and(|tld| tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic()))
        && !UNITS.iter().any(|u| t.ends_with(u))
}

/// The value after "serial", "serial number", "SerialNumber", "serial_no" and the like, followed by ':' or '='.
fn serials(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut from = 0;
    while let Some(i) = lower[from..].find("serial").map(|i| i + from) {
        let mut j = i + "serial".len();
        let rest = &lower[j..];
        for tail in ["_number", " number", "number", "_no", " no", "no", "#"] {
            if rest.starts_with(tail) {
                j += tail.len();
                break;
            }
        }
        let after = &text[j..];
        let gap = after.len() - after.trim_start_matches([' ', '\t']).len();
        let sep = after[gap..].chars().next();
        if matches!(sep, Some(':') | Some('=')) {
            let v_start = j + gap + 1;
            let v = &text[v_start..];
            let lead = v.len() - v.trim_start_matches([' ', '\t', '"', '\'']).len();
            let value_start = v_start + lead;
            let value_end = text[value_start..]
                .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ';' | ')'))
                .map_or(text.len(), |e| value_start + e);
            if value_end > value_start {
                out.push_str(&text[last..value_start]);
                out.push_str("<serial>");
                last = value_end;
            }
            from = value_end.max(j);
        } else {
            from = j;
        }
    }
    out.push_str(&text[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me() -> Personal {
        let mut p = Personal::default();
        p.add("sam", "<user>");
        p.add("Sam Sample", "<name>");
        p.add("example-pc", "<computer>");
        p.add("Cosy Home 5G", "<network>");
        p.add("jo", "<user>");
        p
    }

    #[test]
    fn names_go_where_they_stand_alone_and_nowhere_else() {
        let t = "Nov 26 example-pc kernel: /home/sam/Games ran as sam (Sam Sample); EXAMPLE-PC joined Cosy Home 5G";
        assert_eq!(scrub(t, &me()), "Nov 26 <computer> kernel: /home/<user>/Games ran as <user> (<name>); <computer> joined <network>");
        // a name inside another word stays: "sam" in "samba", "Samsung", "balsamic"
        assert_eq!(scrub("samba Samsung balsamic", &me()), "samba Samsung balsamic");
        // a two-letter name is kept everywhere (it would come out of "journal", "join")
        assert_eq!(scrub("jo joined the journal", &me()), "jo joined the journal");
    }

    #[test]
    fn addresses_go_and_clocks_versions_and_code_stay() {
        let t = "wlan0: 192.168.1.23/24 gw 192.168.1.1 dev aa:bb:cc:dd:ee:ff v6 fe80::1c2b:3cff:fe4d:5e6f%wlan0 and 2001:db8::7.";
        assert_eq!(scrub(t, &me()), "wlan0: <ip> gw <ip> dev <mac> v6 <ip> and <ip>.");
        let keep = "at 20:39:12 kernel 7.3.5-2 mesa 26.0.1 on 127.0.0.1 and ::1, 0.0.0.0; smithay::backend::drm std::vec 1.2.3.4.5";
        assert_eq!(scrub(keep, &me()), keep);
        assert_eq!(scrub("full form 2001:0db8:0000:0000:0000:ff00:0042:8329 end", &me()), "full form <ip> end");
        assert_eq!(
            scrub("Unable to load key file from /var/lib/bluetooth/A1:B2:C3:D4:E5:F6/identity", &me()),
            "Unable to load key file from /var/lib/bluetooth/<mac>/identity",
            "an address in a path"
        );
        assert_eq!(scrub("route 10.0.0.0/8 via fe80::1/64; file /srv/10.0.0.5/24x", &me()), "route <ip> via <ip>; file /srv/<ip>/24x");
    }

    #[test]
    fn mail_addresses_go_and_unit_names_stay() {
        assert_eq!(scrub("from sam.sample+x@example.com today", &me()), "from <email> today");
        let units = "user@1000.service getty@tty1.service systemd-fsck@dev-disk.service";
        assert_eq!(scrub(units, &me()), units);
        let modes = "drm: Mode 0: 3440x1440@174.96Hz (preferred), 2560x1440@59.95";
        assert_eq!(scrub(modes, &me()), modes, "a screen's mode is no e-mail address");
    }

    #[test]
    fn serial_numbers_go() {
        let t = "usb 1-2: SerialNumber: ABC123XYZ\nSerial Number = 9F8E7D, model X\nserial_no:\"QX42\" next";
        assert_eq!(scrub(t, &me()), "usb 1-2: SerialNumber: <serial>\nSerial Number = <serial>, model X\nserial_no:\"<serial>\" next");
        assert_eq!(scrub("serial console on ttyS0", &me()), "serial console on ttyS0", "no value: nothing taken");
    }
}
