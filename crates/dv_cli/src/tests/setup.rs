//! `dv setup agents` (PLAN §10.5): the skill installed for each agent found in a stand-in home
//! directory (`--home`; never the real one), idempotently, repairing drift, and only with a yes.

use std::path::Path;

use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::*;
use crate::skill::SKILL_MD;

fn skill_in(dir: PathBuf) -> PathBuf {
    dir.join("skills").join("dv-review").join("SKILL.md")
}

fn claude_skill(home: &Path) -> PathBuf {
    skill_in(home.join(".claude"))
}

fn codex_skill(home: &Path) -> PathBuf {
    skill_in(home.join(".agents"))
}

fn setup(fixture: &Fixture, home: &Path, extra: &[&str]) -> Ran {
    let home = home.to_str().unwrap();
    let mut line = vec!["setup", "agents", "--home", home];
    line.extend_from_slice(extra);
    fixture.run(&line)
}

fn actions(json: &Value) -> Vec<(String, String)> {
    json["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|agent| {
            (
                agent["agent"].as_str().unwrap().to_string(),
                agent["action"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn pair(agent: &str, action: &str) -> (String, String) {
    (agent.to_string(), action.to_string())
}

#[test]
fn the_skill_is_installed_for_each_agent_found() {
    let fixture = Fixture::new();
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    std::fs::create_dir(home.join(".claude")).unwrap();

    let json = setup(&fixture, home, &["--yes", "--json"]).ok_json();
    assert_eq!(
        json["agents"],
        json!([
            {
                "agent": "claude-code",
                "path": claude_skill(home).display().to_string(),
                "action": "installed",
                "reason": null,
            },
            {
                "agent": "codex",
                "path": codex_skill(home).display().to_string(),
                "action": "skipped",
                "reason": format!("no {} here", home.join(".codex").display()),
            },
            {
                "agent": "cursor",
                "path": codex_skill(home).display().to_string(),
                "action": "skipped",
                "reason": format!("no {} here", home.join(".cursor").display()),
            },
        ])
    );
    assert_eq!(
        std::fs::read_to_string(claude_skill(home)).unwrap(),
        SKILL_MD
    );
    assert!(!codex_skill(home).exists());

    // Again: nothing to do, and nothing to ask.
    let json = setup(&fixture, home, &["--json"]).ok_json();
    assert_eq!(
        actions(&json),
        [
            pair("claude-code", "unchanged"),
            pair("codex", "skipped"),
            pair("cursor", "skipped"),
        ]
    );

    // A copy that drifted is repaired; Codex, once set up, gets its own.
    std::fs::write(claude_skill(home), "stale").unwrap();
    std::fs::create_dir(home.join(".codex")).unwrap();
    let ran = setup(&fixture, home, &["--yes"]);
    assert_eq!(ran.exit, Exit::Ok, "{}", ran.stderr);
    assert_eq!(
        ran.lines(),
        [
            format!("claude-code  updated  {}", claude_skill(home).display()),
            format!("codex  installed  {}", codex_skill(home).display()),
            format!(
                "cursor  skipped  (no {} here)",
                home.join(".cursor").display()
            ),
        ]
    );
    assert_eq!(
        std::fs::read_to_string(claude_skill(home)).unwrap(),
        SKILL_MD
    );
    assert_eq!(
        std::fs::read_to_string(codex_skill(home)).unwrap(),
        SKILL_MD
    );
    // No temporary file is left beside it.
    let names: Vec<String> = std::fs::read_dir(claude_skill(home).parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["SKILL.md"]);
}

#[test]
fn codex_home_says_where_codex_is() {
    let mut fixture = Fixture::new();
    let home = tempfile::tempdir().unwrap();
    let codex = tempfile::tempdir().unwrap();
    fixture.env.insert(
        "CODEX_HOME".into(),
        codex.path().to_string_lossy().into_owned(),
    );
    let json = setup(&fixture, home.path(), &["--yes", "--json"]).ok_json();
    assert_eq!(
        actions(&json),
        [
            pair("claude-code", "skipped"),
            pair("codex", "installed"),
            pair("cursor", "skipped"),
        ]
    );
    // The skill goes where Codex reads user skills, not into its state folder.
    assert!(codex_skill(home.path()).is_file());
    assert_eq!(std::fs::read_dir(codex.path()).unwrap().count(), 0);
}

#[test]
fn it_asks_first() {
    let fixture = Fixture::new();
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    std::fs::create_dir(home.join(".claude")).unwrap();
    let home_arg = home.to_str().unwrap();

    // Not on a terminal: the plan, then exit 2.
    let ran = setup(&fixture, home, &[]);
    assert_eq!(ran.exit, Exit::Usage);
    assert_eq!(
        ran.lines(),
        [
            format!("claude-code  install  {}", claude_skill(home).display()),
            format!("codex  skip  (no {} here)", home.join(".codex").display()),
            format!("cursor  skip  (no {} here)", home.join(".cursor").display()),
        ]
    );
    assert!(ran.stderr.contains("pass --yes"), "{}", ran.stderr);
    let error = setup(&fixture, home, &["--json"]).error(Exit::Usage, "usage");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .ends_with("pass --yes to go ahead")
    );
    assert!(!claude_skill(home).exists());

    // On a terminal: the plan and a question; no is no.
    let args = ["setup", "agents", "--home", home_arg];
    let terminal = |answer: &str| Input {
        stdin: answer.to_string(),
        terminal: true,
    };
    let ran = fixture.run_with(fixture.path(), &args, &terminal("n\n"));
    assert_eq!(ran.exit, Exit::Failure);
    assert!(
        ran.stdout.ends_with("Install the dv-review skill? [y/N] "),
        "{}",
        ran.stdout
    );
    assert_eq!(ran.stderr, "dv: nothing installed\n");
    assert!(!claude_skill(home).exists());
    let ran = fixture.run_with(fixture.path(), &args, &terminal("y\n"));
    assert_eq!(ran.exit, Exit::Ok, "{}", ran.stderr);
    assert!(
        ran.stdout.ends_with(&format!(
            "[y/N] claude-code  installed  {}\ncodex  skipped  (no {} here)\ncursor  skipped  \
             (no {} here)\n",
            claude_skill(home).display(),
            home.join(".codex").display(),
            home.join(".cursor").display()
        )),
        "{}",
        ran.stdout
    );
    assert!(claude_skill(home).is_file());
}

#[test]
fn cursor_shares_codexs_skill_file() {
    let fixture = Fixture::new();
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    std::fs::create_dir(home.join(".cursor")).unwrap();
    std::fs::create_dir(home.join(".codex")).unwrap();
    let json = setup(&fixture, home, &["--yes", "--json"]).ok_json();
    assert_eq!(
        actions(&json),
        [
            pair("claude-code", "skipped"),
            pair("codex", "installed"),
            pair("cursor", "installed"),
        ]
    );
    assert_eq!(json["agents"][1]["path"], json["agents"][2]["path"]);
    assert_eq!(
        std::fs::read_to_string(codex_skill(home)).unwrap(),
        SKILL_MD
    );
    // Cursor alone reads it there too.
    std::fs::remove_dir(home.join(".codex")).unwrap();
    std::fs::write(codex_skill(home), "an older dv's").unwrap();
    let json = setup(&fixture, home, &["--yes", "--json"]).ok_json();
    assert_eq!(json["agents"][2]["action"], "updated");
    assert_eq!(
        std::fs::read_to_string(codex_skill(home)).unwrap(),
        SKILL_MD
    );
}

#[test]
fn without_a_home_it_says_so() {
    let fixture = Fixture::new();
    let error = fixture
        .run(&["setup", "agents", "--yes", "--json"])
        .error(Exit::Failure, "io");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .starts_with("can't tell the home directory"),
        "{error}"
    );
}
