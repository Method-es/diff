use dv_core::review::{Author, ReviewId, ReviewStatus, ReviewSummary, ThreadCounts};
use dv_core::{Location, Target};
use gpui::{Entity, TestAppContext, VisualTestContext};
use pretty_assertions::assert_eq;

use super::items::{Entry, RepoSummary, build};
use super::*;
use crate::failure::Failure;
use crate::services::reviews::{LabelContext, RepoRow, ReviewRow, WorkingSummary};
use crate::shell::Shell;
use crate::test_support::{add_repo, fixture_at, open_shell, settle};

fn row(name: &str, path: &str) -> RepoRow {
    RepoRow {
        location: Location::Local(path.into()),
        name: name.to_string(),
        path: path.to_string(),
        loading: false,
        error: None,
        unreachable: None,
        toplevel: Some(path.to_string()),
        working: Some(WorkingSummary {
            files: 2,
            additions: 5,
            deletions: 1,
        }),
        reviews: Vec::new(),
        labels: LabelContext {
            head: Some("feat".into()),
            branch: Some("feat".into()),
            default_base: Some("main".into()),
        },
        last_used: 0,
    }
}

fn summary(target: Target, worktree: &str, status: ReviewStatus) -> ReviewSummary {
    ReviewSummary {
        id: ReviewId::generate(),
        target,
        worktree: worktree.to_string(),
        author: Author::reviewer("kyle"),
        created_at: 0,
        updated_at: 0,
        status,
        current_round: 2,
        threads: ThreadCounts {
            open: 3,
            addressed: 0,
            resolved: 1,
        },
        pr: None,
        warnings: 0,
    }
}

fn keys(listing: &items::Listing) -> Vec<&str> {
    listing.items.iter().map(|item| item.key.as_str()).collect()
}

#[test]
fn a_repo_offers_its_targets_beside_their_open_reviews() {
    let mut api = row("api-server", "D:\\code\\api-server");
    let working = summary(
        Target::Working,
        "D:\\code\\api-server",
        ReviewStatus::ChangesRequested,
    );
    let elsewhere = summary(Target::Working, "D:\\code\\api-wt", ReviewStatus::Open);
    let commit = summary(
        Target::Commit {
            rev: "abc1234".into(),
        },
        "D:\\code\\api-server",
        ReviewStatus::Ready,
    );
    let finished = summary(
        Target::Staged,
        "D:\\code\\api-server",
        ReviewStatus::Approved,
    );
    for summary in [&working, &elsewhere, &commit, &finished] {
        api.reviews.push(ReviewRow::Ok {
            summary: Box::new(summary.clone()),
            label: "label".into(),
        });
    }
    let listing = build(&[&api], "");
    assert_eq!(
        keys(&listing),
        [
            "api-server/working".to_string(),
            "api-server/staged".to_string(),
            "api-server/branch".to_string(),
            format!("api-server/review/{}", elsewhere.id),
            format!("api-server/review/{}", commit.id),
            "open_folder".to_string(),
        ]
    );
    // The working row stands for this worktree's open working review; the finished staged
    // review is offered as a new one.
    assert_eq!(
        listing.items[0].review().map(|badge| (
            &badge.id,
            badge.status,
            badge.round,
            badge.open_threads
        )),
        Some((&working.id, ReviewStatus::ChangesRequested, 2, 3))
    );
    assert_eq!(listing.items[1].review(), None);
    assert_eq!(listing.items[2].label(), "Branch feat vs main");
    assert_eq!(
        listing.repos[0].summary,
        RepoSummary::Changes(api.working.unwrap())
    );
    assert_eq!(listing.entries[0], Entry::Repo(0));
    assert_eq!(listing.entry_of(0), Some(1));
    assert_eq!(listing.entry_of(5), None, "Open folder… sits in the footer");
    assert_eq!(
        listing.request(0),
        Some((api.location.clone(), OpenRequest::Target(Target::Working)))
    );
    assert_eq!(
        listing.request(4),
        Some((api.location.clone(), OpenRequest::Review(commit.id.clone())))
    );
    assert_eq!(listing.request(5), None);
    assert_eq!(listing.review_row(&working.id), Some(0));
    assert_eq!(listing.first_of(&api.location), Some(0));
}

#[test]
fn rows_say_why_a_repo_has_nothing_to_offer() {
    let mut wsl = row("web", "Ubuntu:/home/kyle/web");
    wsl.location = Location::Wsl {
        distro: "Ubuntu".into(),
        path: "/home/kyle/web".into(),
    };
    wsl.unreachable = Some("WSL support arrives later".into());
    wsl.working = None;
    let mut clean = row("clean", "D:\\clean");
    clean.working = Some(WorkingSummary::default());
    // On the default branch there is no branch to compare.
    clean.labels.branch = Some("main".into());
    let mut loading = row("new", "D:\\new");
    loading.working = None;
    // Its `.git` gone since it was opened: the read error, and no rows, not even its reviews.
    let mut broken = row("broken", "D:\\broken");
    broken.working = None;
    broken.error = Some(Failure::new("git status failed"));
    broken.reviews.push(ReviewRow::Ok {
        summary: Box::new(summary(
            Target::Commit { rev: "abc".into() },
            "D:\\broken",
            ReviewStatus::Open,
        )),
        label: "abc".into(),
    });

    let listing = build(&[&wsl, &clean, &loading, &broken], "");
    let summaries: Vec<&RepoSummary> = listing.repos.iter().map(|repo| &repo.summary).collect();
    assert_eq!(
        summaries,
        [
            &RepoSummary::Unreachable("WSL support arrives later".into()),
            &RepoSummary::Clean,
            &RepoSummary::Loading,
            &RepoSummary::Error("git status failed".into()),
        ]
    );
    assert_eq!(listing.repos[0].distro.as_deref(), Some("Ubuntu"));
    assert_eq!(
        listing.first_of(&wsl.location),
        None,
        "nothing to open there"
    );
    assert_eq!(
        listing.first_of(&broken.location),
        None,
        "nothing to open in a repo that can't be read"
    );
    assert!(super::items::target_offers(&broken).is_empty());
    assert!(
        listing.first_of(&loading.location).is_some(),
        "still loading"
    );
    assert!(
        !keys(&listing).contains(&"clean/branch"),
        "{:?}",
        keys(&listing)
    );
    assert!(keys(&listing).contains(&"clean/staged"));
}

#[test]
fn the_filter_keeps_rows_holding_every_word() {
    let api = row("api-server", "D:\\code\\api-server");
    let web = row("web", "D:\\code\\web");
    let listing = build(&[&api, &web], "API stag");
    assert_eq!(keys(&listing), ["api-server/staged", "open_folder"]);
    let listing = build(&[&api, &web], "web");
    assert_eq!(
        keys(&listing),
        ["web/working", "web/staged", "web/branch", "open_folder"]
    );
    let listing = build(&[&api, &web], "nothing like it");
    assert!(listing.entries.is_empty());
    assert_eq!(keys(&listing), ["open_folder"], "Open folder… always stays");
    // Repos that share a name are told apart by their paths.
    let twin = row("api-server", "D:\\other\\api-server");
    let listing = build(&[&api, &twin], "staged");
    assert_eq!(
        keys(&listing),
        [
            "D:\\code\\api-server/staged",
            "D:\\other\\api-server/staged",
            "open_folder"
        ]
    );
}

fn switcher(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Option<Entity<Switcher>> {
    shell.read_with(cx, |shell, _| shell.switcher().cloned())
}

fn selected(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Option<String> {
    let switcher = switcher(shell, cx)?;
    switcher.read_with(cx, |switcher, _| {
        switcher.selected_key().map(str::to_string)
    })
}

#[gpui::test]
fn ctrl_o_filters_moves_and_opens_by_keyboard(cx: &mut TestAppContext) {
    let (_dir, shell, cx) = open_shell(cx);
    let repos = tempfile::tempdir().unwrap();
    let api = fixture_at(&repos.path().join("api-server"));
    let web = fixture_at(&repos.path().join("web"));
    add_repo(&api, &shell, cx);
    add_repo(&web, &shell, cx);
    let api_name = "api-server";

    cx.simulate_keystrokes("ctrl-o");
    settle(cx, &shell, "the rows", |_, _| true);
    let view = switcher(&shell, cx).expect("Ctrl O opens the switcher");
    assert!(cx.update(|window, cx| view.read(cx).is_focused(window, cx)));
    let first = selected(&shell, cx).unwrap();
    assert!(first.ends_with("/working"), "{first}");

    // ↓ moves, ↑ wraps back past the top to "Open folder…".
    cx.simulate_keystrokes("down");
    assert!(selected(&shell, cx).unwrap().ends_with("/staged"));
    cx.simulate_keystrokes("up up");
    assert_eq!(selected(&shell, cx).as_deref(), Some("open_folder"));

    // Typing filters to one repository's staged row.
    cx.simulate_input(&format!("{api_name} staged"));
    settle(cx, &shell, "the filter", |_, _| true);
    view.read_with(cx, |view, _| {
        let keys: Vec<&str> = view
            .listing()
            .items
            .iter()
            .map(|item| item.key.as_str())
            .collect();
        assert_eq!(keys, [format!("{api_name}/staged").as_str(), "open_folder"]);
        assert_eq!(view.query(), format!("{api_name} staged"));
    });
    assert_eq!(selected(&shell, cx), Some(format!("{api_name}/staged")));

    // Enter opens the staged review of that repository and closes the switcher.
    cx.simulate_keystrokes("enter");
    settle(cx, &shell, "the review", |shell, _| {
        shell.session().is_some()
    });
    shell.read_with(cx, |shell, cx| {
        assert!(shell.switcher().is_none());
        let session = shell.session().unwrap().read(cx);
        assert_eq!(session.review().target, Target::Staged);
        assert_eq!(
            session.backend().repo().location,
            Location::Local(api.path().to_path_buf())
        );
    });

    // Opened again, it selects the open review's row; Esc closes it and the keyboard goes
    // back to the shell.
    cx.simulate_keystrokes("ctrl-o");
    settle(cx, &shell, "the rows", |_, _| true);
    assert_eq!(selected(&shell, cx), Some(format!("{api_name}/staged")));
    cx.simulate_keystrokes("escape");
    assert!(switcher(&shell, cx).is_none());
    let focus = shell.read_with(cx, |shell, cx| shell.focus_handle(cx));
    assert!(cx.update(|window, _| focus.is_focused(window)));
}

#[gpui::test]
fn open_selects_the_named_repo_and_open_folder_adds_one(cx: &mut TestAppContext) {
    let (_dir, shell, cx) = open_shell(cx);
    let repos = tempfile::tempdir().unwrap();
    let api = fixture_at(&repos.path().join("api-server"));
    let web = fixture_at(&repos.path().join("web"));
    add_repo(&api, &shell, cx);
    add_repo(&web, &shell, cx);
    let web_location = Location::Local(web.path().to_path_buf());
    let web_name = "web";
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.open_switcher(Some(&web_location), window, cx)
        })
    });
    settle(cx, &shell, "the rows", |_, _| true);
    assert_eq!(selected(&shell, cx), Some(format!("{web_name}/working")));

    // "Open folder…" goes through the scripted picker; the folder's repo is added and
    // selected.
    let third = fixture_at(&repos.path().join("third"));
    crate::test_support::register(&third, cx);
    cx.update(|_, cx| crate::system::queue_answer(Some(third.path().to_path_buf()), cx))
        .unwrap();
    let view = switcher(&shell, cx).unwrap();
    view.update(cx, |view, cx| assert!(view.select_key("open_folder", cx)));
    cx.simulate_keystrokes("enter");
    settle(cx, &shell, "the new repo", |shell, cx| {
        shell.switcher().is_some_and(|switcher| {
            switcher
                .read(cx)
                .selected_key()
                .is_some_and(|key| key == "third/working")
        })
    });
    let (requests, _) = cx.update(|_, cx| crate::system::requests(cx));
    assert_eq!(
        requests.last().map(crate::system::SystemRequest::kind),
        Some("pick_folder")
    );
    let known = cx.update(|_, cx| {
        crate::ActiveServices::services(cx)
            .reviews
            .read(cx)
            .rows()
            .len()
    });
    assert_eq!(known, 3);
}

#[gpui::test]
fn the_source_picker_switches_the_target_by_keyboard(cx: &mut TestAppContext) {
    let (_dir, shell, cx) = open_shell(cx);
    let repos = tempfile::tempdir().unwrap();
    let api = fixture_at(&repos.path().join("api-server"));
    api.branch("feat");
    api.checkout("feat");
    add_repo(&api, &shell, cx);
    let location = Location::Local(api.path().to_path_buf());
    let task = cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.open(
                location.clone(),
                OpenRequest::Target(Target::Working),
                window,
                cx,
            )
        })
    });
    cx.run_until_parked();
    settle(cx, &shell, "the review", |shell, _| {
        shell.session().is_some()
    });
    let working = cx.foreground_executor().block_test(task).unwrap();

    cx.update(|window, cx| shell.update(cx, |shell, cx| shell.open_source_picker(window, cx)));
    settle(cx, &shell, "the repo's row", |_, _| true);
    let picker = shell.read_with(cx, |shell, _| shell.source_picker().cloned().unwrap());
    picker.read_with(cx, |picker, _| {
        let offers: Vec<(&str, &str, Option<&ReviewId>)> = picker
            .offers()
            .iter()
            .map(|offer| {
                (
                    offer.kind.key(),
                    offer.label.as_str(),
                    offer.review.as_ref().map(|badge| &badge.id),
                )
            })
            .collect();
        assert_eq!(
            offers,
            [
                ("working", "Uncommitted changes", Some(&working.review)),
                ("staged", "Staged changes", None),
                ("branch", "Branch feat vs main", None),
            ]
        );
        assert_eq!(
            picker.selected().map(|offer| offer.kind.key()),
            Some("working")
        );
    });
    cx.simulate_keystrokes("down down down");
    picker.read_with(cx, |picker, _| {
        assert_eq!(
            picker.selected().map(|offer| offer.kind.key()),
            Some("working"),
            "wraps"
        )
    });
    cx.simulate_keystrokes("up enter");
    settle(cx, &shell, "the branch review", |shell, cx| {
        shell.session().is_some_and(|session| {
            matches!(session.read(cx).review().target, Target::Branch { .. })
        })
    });
    shell.read_with(cx, |shell, cx| {
        assert!(shell.source_picker().is_none());
        assert_eq!(
            shell.session().unwrap().read(cx).target_label(),
            "feat vs main"
        );
    });
}
