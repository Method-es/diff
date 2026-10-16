//! The language servers dv knows, one module each, registered in [`ALL`] (docs/PLAN.md §18.4).
//! Several can serve one file; hover answers stack in this list's order, and definition and
//! references take the first server that answers with anything.

pub mod css;
pub mod tailwindcss;
pub mod typescript;

use crate::spec::LanguageServerSpec;

/// Every spec, in the order their answers are used: a `.tsx` hovers TypeScript, then Tailwind
/// CSS; a stylesheet Tailwind CSS, then CSS.
pub static ALL: &[&LanguageServerSpec] = &[&typescript::SPEC, &tailwindcss::SPEC, &css::SPEC];

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
    fn each_file_gets_its_servers_in_order() {
        let ids = |rel: &str| {
            for_path(rel)
                .map(|(spec, language)| (spec.id, language))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids("src/a.ts"),
            [("typescript", "typescript"), ("tailwindcss", "typescript")]
        );
        assert_eq!(
            ids("src/App.TSX"),
            [
                ("typescript", "typescriptreact"),
                ("tailwindcss", "typescriptreact")
            ]
        );
        assert_eq!(
            ids("x.cjs"),
            [("typescript", "javascript"), ("tailwindcss", "javascript")]
        );
        assert_eq!(
            ids("x.jsx"),
            [
                ("typescript", "javascriptreact"),
                ("tailwindcss", "javascriptreact")
            ]
        );
        assert_eq!(
            ids("src/styles/app.css"),
            [("tailwindcss", "css"), ("css", "css")]
        );
        assert_eq!(ids("a.scss"), [("tailwindcss", "scss"), ("css", "scss")]);
        assert_eq!(ids("index.html"), [("tailwindcss", "html")]);
        assert!(ids("README.md").is_empty());
        assert!(ids("Makefile").is_empty());
        assert!(ids("src.ts/readme").is_empty());
        assert_eq!(
            by_id("typescript").map(|spec| spec.name),
            Some("TypeScript")
        );
        assert_eq!(
            by_id("tailwindcss").map(|spec| spec.name),
            Some("Tailwind CSS")
        );
        assert!(by_id("cobol").is_none());
    }
}
