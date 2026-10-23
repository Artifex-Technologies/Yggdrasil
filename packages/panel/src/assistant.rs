//! The assistant the operating system provides, if any (an `assistant` integration file, see Yggdrasil's docs/INTEGRATIONS.md).
//! The panel owns the hold-to-talk key, the recording, Fylgja's listening and speaking animation and the answer card; the
//! assistant's programs do the rest. With no assistant installed, none of this is offered.

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

pub struct Assistant {
    pub name: String,
    ask: Vec<String>,
    speak: Option<Vec<String>>,
    warm: Option<Vec<String>>,
    runtime: Option<String>,
}

static ASSISTANT: OnceLock<Option<Assistant>> = OnceLock::new();

/// The installed assistant (read once).
pub fn get() -> Option<&'static Assistant> {
    ASSISTANT
        .get_or_init(|| {
            let i = yggdrasil_conf::integrations::first("assistant")?;
            Some(Assistant {
                name: i.get("name").unwrap_or("Assistant").to_string(),
                ask: i.command("ask")?,
                speak: i.command("speak"),
                warm: i.command("warm"),
                runtime: i.get("runtime").filter(|r| !r.contains('/') && !r.contains("..")).map(str::to_string),
            })
        })
        .as_ref()
}

/// The assistant's name, for text (or a neutral word when there is none).
pub fn name() -> &'static str {
    get().map(|a| a.name.as_str()).unwrap_or("The assistant")
}

fn command(v: &[String]) -> Command {
    let mut c = Command::new(&v[0]);
    c.args(&v[1..]);
    c
}

impl Assistant {
    /// Asks with a recording (added as the last argument).
    pub fn ask(&self, recording: &std::path::Path) -> Command {
        let mut c = command(&self.ask);
        c.arg(recording);
        c
    }
    pub fn speak(&self) -> Option<Command> {
        self.speak.as_deref().map(command)
    }
    pub fn warm(&self) -> Option<Command> {
        self.warm.as_deref().map(command)
    }
    /// Where it reports that it is speaking (speaking.pid) and keeps conversations (conv-ID.session).
    pub fn runtime_dir(&self) -> Option<PathBuf> {
        let r = self.runtime.as_ref()?;
        std::env::var_os("XDG_RUNTIME_DIR").map(|d| PathBuf::from(d).join(r))
    }
}
