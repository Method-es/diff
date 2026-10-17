//! `dv.exe`: the console binary (docs/PLAN.md §13.1): the CLI, `dv backend --stdio`, and `dv`
//! / `dv open`, which start the app detached. Everything is in the crate's library.

fn main() -> std::process::ExitCode {
    dv::main(dv::Entry::Console)
}
