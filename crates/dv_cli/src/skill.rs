//! The `dv-review` agent skill (docs/PLAN.md §10.5): the `SKILL.md` that teaches a coding agent
//! to answer a review through the CLI, embedded in the binary so `dv setup agents` installs
//! exactly the text this build's commands match (a bare `dv` inside a WSL distro has no data
//! files beside it).

/// The skill's name: its frontmatter `name` and the folder it is installed in.
pub const SKILL_NAME: &str = "dv-review";

/// The skill, as installed (`crates/dv_cli/skill/SKILL.md`).
pub const SKILL_MD: &str = include_str!("../skill/SKILL.md");
