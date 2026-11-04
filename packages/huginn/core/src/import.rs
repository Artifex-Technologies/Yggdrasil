//! Bringing subscriptions from elsewhere: NewPipe's export (Settings > Content > Export subscriptions: a JSON file), FreeTube's and
//! OpenTubeX's profiles file (`profiles.db`: one JSON object per line, each with its subscriptions), and Google Takeout's
//! `subscriptions.csv`. Only YouTube channels with a valid id are taken; names and pictures are refreshed at the first refresh.

use serde_json::Value;

use crate::model::{valid_channel_id, Channel};

/// The channel id in a YouTube channel URL (`https://www.youtube.com/channel/UC...`), or the id itself.
pub fn channel_id_in(s: &str) -> Option<String> {
    let s = s.trim();
    let id = s.rsplit_once("/channel/").map(|(_, r)| r).unwrap_or(s);
    let id: String = id.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    valid_channel_id(&id).then_some(id)
}

/// Every subscription the file holds, whatever of the three it is; channels appear once.
pub fn subscriptions(text: &str) -> Vec<Channel> {
    let mut out: Vec<Channel> = vec![];
    let mut push = |id: Option<String>, name: &str, avatar: Option<&str>| {
        if let Some(id) = id {
            if !out.iter().any(|c| c.id == id) {
                out.push(Channel { id, name: name.trim().to_string(), avatar: avatar.filter(|a| a.starts_with("https://")).map(str::to_string), followers: None });
            }
        }
    };
    let t = text.trim_start_matches('\u{feff}').trim();
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        // NewPipe: {"subscriptions":[{"service_id":0,"url":"https://www.youtube.com/channel/UC...","name":"..."}]}
        for s in v.get("subscriptions").and_then(Value::as_array).into_iter().flatten() {
            if s.get("service_id").and_then(Value::as_u64).unwrap_or(0) == 0 {
                push(s.get("url").and_then(Value::as_str).and_then(channel_id_in), s.get("name").and_then(Value::as_str).unwrap_or(""), None);
            }
        }
        return out;
    }
    if t.lines().next().is_some_and(|l| l.trim_start().starts_with('{')) {
        // FreeTube / OpenTubeX profiles.db: {"name":"All Channels","subscriptions":[{"id":"UC...","name":"...","thumbnail":"..."}],...}
        for line in t.lines() {
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
            for s in v.get("subscriptions").and_then(Value::as_array).into_iter().flatten() {
                push(s.get("id").and_then(Value::as_str).and_then(channel_id_in), s.get("name").and_then(Value::as_str).unwrap_or(""), s.get("thumbnail").and_then(Value::as_str));
            }
        }
        return out;
    }
    // Takeout: "Channel Id,Channel Url,Channel Title" then one channel per line
    for line in t.lines().skip_while(|l| l.to_lowercase().starts_with("channel id")) {
        let mut cols = line.splitn(3, ',');
        let (id, url, name) = (cols.next().unwrap_or(""), cols.next().unwrap_or(""), cols.next().unwrap_or(""));
        push(channel_id_in(id).or_else(|| channel_id_in(url)), name.trim_matches('"'), None);
    }
    out
}

/// NewPipe's own export format, so subscriptions can go back to the phone.
pub fn to_newpipe(subs: &[Channel]) -> String {
    let list: Vec<Value> = subs.iter().map(|c| serde_json::json!({"service_id": 0, "url": format!("https://www.youtube.com/channel/{}", c.id), "name": c.name})).collect();
    serde_json::to_string_pretty(&serde_json::json!({"app_version": "0.27.0", "app_version_int": 1000, "subscriptions": list})).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "UC0vBXGSyV14uvJ4hECDOl0Q";

    #[test]
    fn newpipe() {
        let t = format!(r#"{{"app_version":"0.27.2","app_version_int":1001,"subscriptions":[
            {{"service_id":0,"url":"https://www.youtube.com/channel/{ID}","name":"Techquickie"}},
            {{"service_id":1,"url":"https://soundcloud.com/x","name":"SoundCloud one"}},
            {{"service_id":0,"url":"https://www.youtube.com/channel/{ID}","name":"twice"}},
            {{"service_id":0,"url":"https://evil.example/--exec","name":"bad"}}]}}"#);
        let s = subscriptions(&t);
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].id.as_str(), s[0].name.as_str()), (ID, "Techquickie"));
        assert_eq!(subscriptions(&to_newpipe(&s)), s, "round trip");
    }

    #[test]
    fn freetube_and_opentubex() {
        let t = format!("{{\"name\":\"All Channels\",\"subscriptions\":[{{\"id\":\"{ID}\",\"name\":\"Techquickie\",\"thumbnail\":\"https://yt3.ggpht.com/a\"}}],\"_id\":\"allChannels\"}}\n{{\"name\":\"Tech\",\"subscriptions\":[{{\"id\":\"{ID}\",\"name\":\"Techquickie\"}}]}}\n");
        let s = subscriptions(&t);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].avatar.as_deref(), Some("https://yt3.ggpht.com/a"));
    }

    #[test]
    fn takeout_csv() {
        let t = format!("Channel Id,Channel Url,Channel Title\n{ID},http://www.youtube.com/channel/{ID},Techquickie\nnot,a,channel\n");
        let s = subscriptions(&t);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].name, "Techquickie");
    }
}
