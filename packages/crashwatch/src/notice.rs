//! What the notices say, and the assistant they may offer. With no assistant installed (no `assistant` integration file, or one
//! without a `question` command), a crash notice names the program and the signal and offers nothing else.

use crate::core::program_name;
use yggdrasil_conf::integrations::Integration;

/// The assistant that can be asked about a crash: its name, the command that answers a typed question (the question is added as
/// the last argument; it prints the answer) and, optionally, how to ask about a crash (`crash_question`, with `{program}` and
/// `{signal}` in it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assistant {
    pub name: String,
    pub question: Vec<String>,
    pub crash_question: Option<String>,
}

/// The installed assistant, from the integration files (the same one the panel uses: the first of kind `assistant`).
pub fn assistant_from(list: &[Integration]) -> Option<Assistant> {
    let i = list.iter().find(|i| i.kind == "assistant")?;
    Some(Assistant {
        name: i.get("name").unwrap_or("The assistant").to_string(),
        question: i.command("question")?,
        crash_question: i.get("crash_question").map(str::to_string),
    })
}

/// A notification, as org.freedesktop.Notifications.Notify takes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub app: String,
    pub icon: &'static str,
    pub summary: String,
    pub body: String,
    /// (key, label) pairs
    pub actions: Vec<(String, String)>,
    /// milliseconds; 0 stays until closed, -1 is the server's choice
    pub timeout: i32,
}

/// The key of the "Ask ..." action.
pub const ASK: &str = "ask";

/// The notice about a crash.
pub fn crash_notice(exe: &str, signal: &str, assistant: Option<&Assistant>) -> Notice {
    let program = program_name(exe);
    match assistant {
        Some(a) => Notice {
            app: a.name.clone(),
            icon: "dialog-warning",
            summary: format!("{program} crashed"),
            body: format!("{exe} ({signal}). Want {} to take a look?", a.name),
            actions: vec![(ASK.to_string(), format!("Ask {}", a.name))],
            timeout: 30_000,
        },
        None => Notice {
            app: "Crash watch".to_string(),
            icon: "dialog-warning",
            summary: format!("{program} crashed"),
            body: format!("{exe} ({signal})."),
            actions: vec![],
            timeout: 30_000,
        },
    }
}

/// Shown at once when "Ask" is clicked, so the click visibly did something; the answer replaces it.
pub fn looking_notice(program: &str, a: &Assistant) -> Notice {
    Notice {
        app: a.name.clone(),
        icon: "dialog-information",
        summary: format!("{} is looking at the {program} crash", a.name),
        body: format!("{}'s answer shows here in a moment.", a.name),
        actions: vec![],
        timeout: -1,
    }
}

/// The answer; it stays until it is read and closed (it answers a question you asked).
pub fn answer_notice(program: &str, a: &Assistant, text: &str) -> Notice {
    Notice {
        app: a.name.clone(),
        icon: "dialog-information",
        summary: format!("About the {program} crash"),
        body: text.chars().take(900).collect(),
        actions: vec![],
        timeout: 0,
    }
}

/// What the assistant is asked.
pub fn question(program: &str, signal: &str, a: &Assistant) -> String {
    match &a.crash_question {
        Some(t) => t.replace("{program}", program).replace("{signal}", signal),
        None => format!(
            "The program {program} just crashed ({signal}). Look into it: say in plain words what the system recorded, which package owns it, \
             what you cannot tell, and one or two things I could try (as guesses, with how to check them). Do not change anything."
        ),
    }
}

/// The text of the answer: what it printed, else what it printed as an error, else a line saying there was nothing.
pub fn answer_text(stdout: &str, stderr: &str, name: &str) -> String {
    let t = if stdout.trim().is_empty() { stderr.trim() } else { stdout.trim() };
    if t.is_empty() {
        format!("{name} had nothing to say about it.")
    } else {
        t.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn integ(name: &str, text: &str) -> Integration {
        Integration::parse(Path::new(name), text).unwrap()
    }

    fn sarina() -> Integration {
        integ(
            "/usr/share/yggdrasil/integrations/sarina.conf",
            "kind = assistant\nname = Sarina\nask = /usr/local/bin/labyrinth-sarina-ask\nquestion = /usr/local/bin/labyrinth-ask-sarina --voice\n\
             crash_question = The program {program} just crashed ({signal}). Look into it: call crash_detail on it. Do not change anything.\n",
        )
    }

    #[test]
    fn ask_is_offered_only_with_an_assistant_that_answers_questions() {
        let others = vec![integ("/x/weather.conf", "kind = bar-item\nname = Weather\ncommand = /usr/bin/w\n")];
        assert_eq!(assistant_from(&others), None, "no assistant installed");
        let n = crash_notice("/usr/bin/game", "SIGSEGV", None);
        assert!(n.actions.is_empty());
        assert_eq!((n.app.as_str(), n.summary.as_str(), n.body.as_str()), ("Crash watch", "game crashed", "/usr/bin/game (SIGSEGV)."));
        assert!(!n.body.contains("Sarina") && !n.body.contains("look"));

        let mut with = others.clone();
        with.push(sarina());
        let a = assistant_from(&with).unwrap();
        assert_eq!(a.question, vec!["/usr/local/bin/labyrinth-ask-sarina".to_string(), "--voice".to_string()]);
        let n = crash_notice("/usr/bin/game", "SIGSEGV", Some(&a));
        assert_eq!(n.actions, vec![("ask".to_string(), "Ask Sarina".to_string())]);
        assert_eq!(n.app, "Sarina");
        assert_eq!(n.summary, "game crashed");
        assert_eq!(n.body, "/usr/bin/game (SIGSEGV). Want Sarina to take a look?");
        assert_eq!((n.icon, n.timeout), ("dialog-warning", 30_000));

        // an assistant that only takes recordings (an older file without `question`), or one with a relative command: no action
        let voice_only = integ("/x/sarina.conf", "kind = assistant\nname = Sarina\nask = /usr/local/bin/labyrinth-sarina-ask\n");
        assert_eq!(assistant_from(&[voice_only]), None);
        let relative = integ("/x/sarina.conf", "kind = assistant\nname = Sarina\nquestion = labyrinth-ask-sarina --voice\n");
        assert_eq!(assistant_from(&[relative]), None);
    }

    #[test]
    fn the_question_and_the_answer() {
        let a = assistant_from(&[sarina()]).unwrap();
        assert_eq!(
            question("game", "SIGSEGV", &a),
            "The program game just crashed (SIGSEGV). Look into it: call crash_detail on it. Do not change anything."
        );
        let plain = Assistant { name: "Ada".into(), question: vec!["/opt/ask".into()], crash_question: None };
        let q = question("game", "SIGABRT", &plain);
        assert!(q.starts_with("The program game just crashed (SIGABRT). Look into it: say in plain words"), "{q}");
        assert!(q.ends_with("Do not change anything."));

        let l = looking_notice("game", &a);
        assert_eq!((l.summary.as_str(), l.body.as_str()), ("Sarina is looking at the game crash", "Sarina's answer shows here in a moment."));
        assert_eq!(answer_text("  It ran out of memory.\n", "warning", "Sarina"), "It ran out of memory.");
        assert_eq!(answer_text(" \n", "sarina: model server not running\n", "Sarina"), "sarina: model server not running");
        assert_eq!(answer_text("", "", "Sarina"), "Sarina had nothing to say about it.");
        let n = answer_notice("game", &a, &"é".repeat(2000));
        assert_eq!(n.body.chars().count(), 900);
        assert_eq!((n.summary.as_str(), n.timeout), ("About the game crash", 0));
    }
}
