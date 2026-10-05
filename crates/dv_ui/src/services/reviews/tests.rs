use std::path::Path;
use std::time::{Duration, Instant};

use dv_core::review::ReviewStatus;
use dv_test_support::FixtureRepo;
use gpui::TestAppContext;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::*;

fn object(value: Value) -> serde_json::Map<String, Value> {
    match value {
        Value::Object(object) => object,
        other => panic!("not an object: {other}"),
    }
}

fn local(path: &str) -> Location {
    Location::Local(path.into())
}

#[test]
fn known_repos_read_leniently_and_keep_what_they_dont_know() {
    let (file, problems) = KnownReposFile::from_object(object(json!({
        "v": 1,
        "repos": [
            {"location": {"local": "D:\\code\\api"}, "added_at": 5, "opened_at": 9, "pinned": true},
            {"location": {"wsl": {"distro": "Ubuntu", "path": "/home/kyle/web"}}, "added_at": 7},
            {"location": {"local": "D:\\code\\api"}, "added_at": 1},
            {"place": "somewhere else"},
        ],
        "window": {"x": 1},
    })))
    .unwrap();
    assert_eq!(problems.len(), 2, "{problems:?}");
    let repos: Vec<&KnownRepo> = file.repos().collect();
    assert_eq!(repos.len(), 2);
    assert_eq!(repos[0].location, local("D:\\code\\api"));
    assert_eq!(repos[0].opened_at, Some(9));
    assert_eq!(repos[0].last_used(), 9);
    assert_eq!(repos[1].last_used(), 7);
    let written: Value = serde_json::from_str(&file.to_json()).unwrap();
    assert_eq!(
        written,
        json!({
            "v": 1,
            "repos": [
                {"location": {"local": "D:\\code\\api"}, "added_at": 5, "opened_at": 9, "pinned": true},
                {"location": {"wsl": {"distro": "Ubuntu", "path": "/home/kyle/web"}}, "added_at": 7},
                {"place": "somewhere else"},
            ],
            "window": {"x": 1},
        })
    );
    // A newer version is read as far as it goes.
    let (file, problems) =
        KnownReposFile::from_object(object(json!({"v": 4, "repos": []}))).unwrap();
    assert_eq!(file.repos().count(), 0);
    assert!(problems[0].contains("newer dv"), "{problems:?}");
    // Repos that aren't a list make a file this version can't read.
    assert!(KnownReposFile::from_object(object(json!({"repos": {"a": 1}}))).is_err());
}

#[test]
fn known_repos_add_mark_and_remove() {
    let mut file = KnownReposFile::default();
    assert!(file.add(local("D:\\a"), 10));
    assert!(!file.add(local("D:\\a"), 11), "a location is known once");
    assert!(file.mark_opened(&local("D:\\a"), 20));
    assert!(!file.mark_opened(&local("D:\\b"), 20));
    assert_eq!(file.get(&local("D:\\a")).unwrap().last_used(), 20);
    let (again, problems) =
        KnownReposFile::from_object(object(serde_json::from_str(&file.to_json()).unwrap()))
            .unwrap();
    assert!(problems.is_empty());
    assert_eq!(again, file);
    assert!(file.remove(&local("D:\\a")));
    assert_eq!(file.repos().count(), 0);
}

#[test]
fn target_labels_name_what_is_compared() {
    let context = LabelContext {
        head: Some("main".into()),
        branch: Some("main".into()),
        default_base: Some("origin/main".into()),
    };
    let none = LabelContext::default();
    let branch = |base: Option<&str>| Target::Branch {
        branch: "feat".into(),
        base: base.map(str::to_string),
    };
    for (target, labels, label) in [
        (Target::Working, &context, "working tree vs main"),
        (Target::Working, &none, "working tree"),
        (Target::Staged, &context, "staged vs main"),
        (branch(None), &context, "feat vs origin/main"),
        (branch(Some("dev")), &context, "feat vs dev"),
        (branch(None), &none, "feat"),
        (
            Target::Commit {
                rev: "abc1234".into(),
            },
            &context,
            "commit abc1234",
        ),
        (
            Target::Range {
                from: "v1".into(),
                to: "v2".into(),
                three_dot: true,
            },
            &context,
            "v1...v2",
        ),
    ] {
        assert_eq!(target_label(&target, labels), label, "{target:?}");
    }
}

#[test]
fn the_branch_target_is_the_checked_out_branch_against_the_default() {
    let labels = |branch: Option<&str>, base: Option<&str>| LabelContext {
        head: branch.map(str::to_string),
        branch: branch.map(str::to_string),
        default_base: base.map(str::to_string),
    };
    assert_eq!(
        labels(Some("feat"), Some("main")).branch_target(),
        Some(Target::Branch {
            branch: "feat".into(),
            base: None
        })
    );
    // Unpushed work on main is worth a look against origin/main.
    assert!(
        labels(Some("main"), Some("origin/main"))
            .branch_target()
            .is_some()
    );
    assert_eq!(labels(Some("main"), Some("main")).branch_target(), None);
    assert_eq!(labels(None, Some("main")).branch_target(), None, "detached");
    assert_eq!(labels(Some("feat"), None).branch_target(), None);
}

fn services(cx: &mut TestAppContext, data_dir: &Path) -> (Entity<Locations>, Entity<Reviews>) {
    let locations = cx.new(|_| Locations::new());
    let reviews = cx.new(|cx| Reviews::new(locations.clone(), data_dir, cx));
    (locations, reviews)
}

fn settle(cx: &mut TestAppContext, reviews: &Entity<Reviews>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        cx.run_until_parked();
        if reviews.read_with(cx, |reviews, cx| reviews.is_settled(cx)) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the reviews service never settled"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn fixture() -> FixtureRepo {
    let repo = FixtureRepo::new();
    repo.write("src/a.ts", "export const a = 1;\n");
    repo.write("src/b.ts", "export const b = 1;\n");
    repo.commit("base");
    repo.write("src/a.ts", "export const a = 2;\nexport const z = 0;\n");
    repo.write("notes.md", "# notes\n");
    repo
}

#[gpui::test]
async fn a_repo_is_added_listed_and_saved(cx: &mut TestAppContext) {
    let data = tempfile::tempdir().unwrap();
    let repo = fixture();
    let (locations, reviews) = services(cx, data.path());
    locations.update(cx, |locations, _| {
        locations.register_env(repo.path().to_path_buf(), repo.env_overlay())
    });
    let added = reviews
        .update(cx, |reviews, cx| {
            reviews.add_repo(Location::Local(repo.path().join("src")), cx)
        })
        .await
        .expect("a repo");
    assert_eq!(
        added,
        Location::Local(repo.path().to_path_buf()),
        "its root is known"
    );
    settle(cx, &reviews);
    reviews.read_with(cx, |reviews, _| {
        let rows = reviews.rows();
        assert_eq!(rows.len(), 1);
        let row = rows[0];
        assert!(!row.loading);
        assert_eq!(row.error, None);
        assert_eq!(row.name, "repo");
        assert_eq!(
            row.working,
            Some(WorkingSummary {
                files: 2,
                additions: 3,
                deletions: 1,
            })
        );
        assert!(row.reviews.is_empty());
        assert_eq!(row.labels.head.as_deref(), Some("main"));
    });

    let opened = reviews
        .update(cx, |reviews, cx| {
            reviews.open_review(added.clone(), Target::Working, cx)
        })
        .await
        .expect("a review");
    assert!(opened.created);
    assert_eq!(
        opened.reviewer,
        Author::reviewer(dv_test_support::AUTHOR_NAME)
    );
    // The labels come with the review, so its session names the target in full at once.
    assert_eq!(opened.labels.head.as_deref(), Some("main"));
    let resumed = reviews
        .update(cx, |reviews, cx| {
            reviews.open_review(added.clone(), Target::Working, cx)
        })
        .await
        .unwrap();
    assert!(!resumed.created);
    assert_eq!(resumed.loaded.review.id, opened.loaded.review.id);
    // A review asked for by id loads as it is.
    let by_id = reviews
        .update(cx, |reviews, cx| {
            reviews.open(
                added.clone(),
                OpenRequest::Review(opened.loaded.review.id.clone()),
                cx,
            )
        })
        .await
        .unwrap();
    assert!(!by_id.created);
    assert_eq!(by_id.loaded.review.id, opened.loaded.review.id);
    settle(cx, &reviews);
    reviews.read_with(cx, |reviews, _| {
        let row = reviews.row(&added).unwrap();
        assert_eq!(row.unreachable, None);
        assert_eq!(
            row.toplevel.as_deref(),
            Some(repo.path().to_str().unwrap()),
            "the worktree its reviews record"
        );
        assert_eq!(
            row.open_review_of(&Target::Working)
                .map(|summary| &summary.id),
            Some(&opened.loaded.review.id)
        );
        assert_eq!(row.open_review_of(&Target::Staged), None);
        assert_eq!(row.reviews.len(), 1);
        let ReviewRow::Ok { summary, label } = &row.reviews[0] else {
            panic!("a readable review");
        };
        assert_eq!(summary.status, ReviewStatus::Open);
        assert_eq!(label, "working tree vs main");
        assert!(reviews.known().get(&added).unwrap().opened_at.is_some());
    });

    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(data.path().join(KNOWN_REPOS_FILE)).unwrap())
            .unwrap();
    assert_eq!(saved["v"], 1);
    assert_eq!(saved["repos"].as_array().unwrap().len(), 1);
    assert_eq!(
        saved["repos"][0]["location"],
        serde_json::to_value(&added).unwrap()
    );

    // A new service reads them back.
    let (_, again) = services(cx, data.path());
    again.read_with(cx, |again, _| {
        assert_eq!(again.rows().len(), 1);
        assert_eq!(again.rows()[0].location, added);
    });
}

#[gpui::test]
fn a_dropped_add_still_adds_and_settles(cx: &mut TestAppContext) {
    let data = tempfile::tempdir().unwrap();
    let repo = fixture();
    let (locations, reviews) = services(cx, data.path());
    locations.update(cx, |locations, _| {
        locations.register_env(repo.path().to_path_buf(), repo.env_overlay())
    });
    let location = Location::Local(repo.path().to_path_buf());
    let dropped = reviews.update(cx, |reviews, cx| reviews.add_repo(location.clone(), cx));
    drop(dropped);
    settle(cx, &reviews);
    reviews.read_with(cx, |reviews, _| {
        let row = reviews.row(&location).expect("added");
        assert!(row.working.is_some());
    });
}

#[gpui::test]
async fn an_unreadable_review_log_is_a_row_of_its_own(cx: &mut TestAppContext) {
    let data = tempfile::tempdir().unwrap();
    let repo = fixture();
    let (locations, reviews) = services(cx, data.path());
    locations.update(cx, |locations, _| {
        locations.register_env(repo.path().to_path_buf(), repo.env_overlay())
    });
    let location = Location::Local(repo.path().to_path_buf());
    let opened = reviews
        .update(cx, |reviews, cx| {
            reviews.open_review(location.clone(), Target::Staged, cx)
        })
        .await
        .unwrap();
    let broken = dv_core::review::ReviewId::generate();
    std::fs::write(
        repo.path().join(format!(".git/dv/reviews/{broken}.jsonl")),
        "not json\n",
    )
    .unwrap();
    reviews.update(cx, |reviews, cx| reviews.refresh(location.clone(), cx));
    settle(cx, &reviews);
    reviews.read_with(cx, |reviews, _| {
        let row = reviews.row(&location).unwrap();
        assert_eq!(row.error, None, "one bad log doesn't fail the repo");
        let ids: Vec<&ReviewId> = row.reviews.iter().map(ReviewRow::id).collect();
        assert!(ids.contains(&&opened.loaded.review.id));
        let bad = row
            .reviews
            .iter()
            .find(|review| review.id() == &broken)
            .expect("the broken log is listed");
        assert!(matches!(bad, ReviewRow::Err { .. }), "{bad:?}");
    });
}

#[gpui::test]
async fn wsl_and_missing_repos_fail_typed_without_panicking(cx: &mut TestAppContext) {
    let data = tempfile::tempdir().unwrap();
    let (_, reviews) = services(cx, data.path());
    let wsl = Location::Wsl {
        distro: "Ubuntu".into(),
        path: "/home/kyle/api".into(),
    };
    // Added as given, so its row says why it can't be shown.
    let added = reviews
        .update(cx, |reviews, cx| reviews.add_repo(wsl.clone(), cx))
        .await
        .unwrap();
    assert_eq!(added, wsl);
    settle(cx, &reviews);
    reviews.read_with(cx, |reviews, _| {
        let row = reviews.row(&wsl).unwrap();
        let error = row.error.as_ref().expect("unavailable");
        assert!(error.message.contains("can't be opened"), "{error:?}");
        assert_eq!(
            row.unreachable.as_deref(),
            Some("WSL support arrives later"),
            "the few words a switcher row shows"
        );
        assert_eq!(row.toplevel, None);
        assert_eq!(row.working, None);
        assert_eq!(row.name, "api");
    });
    let failure = reviews
        .update(cx, |reviews, cx| {
            reviews.open_review(wsl, Target::Working, cx)
        })
        .await
        .err()
        .expect("no WSL backend yet");
    assert!(failure.message.contains("can't be opened"), "{failure:?}");

    let missing = Location::Local(data.path().join("nowhere"));
    let failure = reviews
        .update(cx, |reviews, cx| reviews.add_repo(missing.clone(), cx))
        .await
        .expect_err("a missing path isn't added");
    assert!(failure.message.contains("no such directory"), "{failure:?}");
    reviews.read_with(cx, |reviews, _| assert!(reviews.row(&missing).is_none()));
}

#[gpui::test]
async fn a_file_dv_cant_read_is_never_saved_over(cx: &mut TestAppContext) {
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join(KNOWN_REPOS_FILE);
    for original in ["[\"a list\"]\n", "{\"repos\": \"text\"}\n"] {
        std::fs::write(&path, original).unwrap();
        let (_, reviews) = services(cx, data.path());
        let wsl = Location::Wsl {
            distro: "Ubuntu".into(),
            path: "/srv/x".into(),
        };
        reviews
            .update(cx, |reviews, cx| reviews.add_repo(wsl, cx))
            .await
            .unwrap();
        settle(cx, &reviews);
        reviews.read_with(cx, |reviews, cx| {
            assert_eq!(reviews.rows().len(), 1, "known for this session");
            assert!(
                reviews
                    .save_error(cx)
                    .is_some_and(|error| error.contains("left as it is")),
                "{:?}",
                reviews.save_error(cx)
            );
        });
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }
}
