//! The language servers dv knows, one module each, registered in [`ALL`] (docs/PLAN.md §18.4).
//! Several can serve one file; hover answers stack in this list's order, and definition and
//! references take the first server that answers with anything.

pub mod typescript;

use crate::spec::LanguageServerSpec;

/// Every spec, in the order their answers are used.
pub static ALL: &[&LanguageServerSpec] = &[&typescript::SPEC];

/// The spec with this id.
pub fn by_id(id: &str) -> Option<&'static LanguageServerSpec> {
    ALL.iter().copied().find(|spec| spec.id == id)
}

/// The specs that serve the worktree file `rel`, each with the language id it opens it with.
pub fn for_path(rel: &str) -> impl Iterator<Item = (&'static LanguageServerSpec, &'static str)> {
    ALL.iter()
        .copied()
        .filter_map(move |spec| spec.language_id(rel).map(|language| (spec, language)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typescript_serves_its_extensions() {
        let ids = |rel: &str| {
            for_path(rel)
                .map(|(spec, language)| (spec.id, language))
                .collect::<Vec<_>>()
        };
        assert_eq!(ids("src/a.ts"), [("typescript", "typescript")]);
        assert_eq!(ids("src/App.TSX"), [("typescript", "typescriptreact")]);
        assert_eq!(ids("x.cjs"), [("typescript", "javascript")]);
        assert_eq!(ids("x.jsx"), [("typescript", "javascriptreact")]);
        assert!(ids("README.md").is_empty());
        assert!(ids("Makefile").is_empty());
        assert!(ids("src.ts/readme").is_empty());
        assert_eq!(
            by_id("typescript").map(|spec| spec.name),
            Some("TypeScript")
        );
        assert!(by_id("cobol").is_none());
    }
}
