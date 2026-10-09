use super::*;

fn wsl(distro: &str, path: &str) -> Location {
    Location::Wsl {
        distro: distro.to_string(),
        path: path.to_string(),
    }
}

fn local(path: &str) -> Location {
    Location::Local(PathBuf::from(path))
}

#[test]
fn path_arguments_parse_into_locations() {
    let cases: &[(&str, Location)] = &[
        (
            r"\\wsl.localhost\Ubuntu\home\kyle\api",
            wsl("Ubuntu", "/home/kyle/api"),
        ),
        (r"\\WSL.LocalHost\Ubuntu\home", wsl("Ubuntu", "/home")),
        (r"\\wsl$\Debian\srv\x", wsl("Debian", "/srv/x")),
        (r"\\WSL$\Debian\srv", wsl("Debian", "/srv")),
        (
            "//wsl.localhost/Ubuntu/home/kyle",
            wsl("Ubuntu", "/home/kyle"),
        ),
        ("//wsl$/Ubuntu-24.04/x", wsl("Ubuntu-24.04", "/x")),
        (r"\/wsl.localhost/Ubuntu\a/b\\c\", wsl("Ubuntu", "/a/b/c")),
        (r"\\wsl.localhost\Ubuntu", wsl("Ubuntu", "/")),
        (r"\\wsl.localhost\Ubuntu\", wsl("Ubuntu", "/")),
        (r"\\wsl$\Ubuntu\a\.\b\..\c", wsl("Ubuntu", "/a/b/../c")),
        (r"\\?\UNC\wsl.localhost\Ubuntu\home", wsl("Ubuntu", "/home")),
        (r"\\server\share\repo", local(r"\\server\share\repo")),
        (
            r"\\wsl.localhostx\Ubuntu\home",
            local(r"\\wsl.localhostx\Ubuntu\home"),
        ),
        (r"\\wsl\Ubuntu\home", local(r"\\wsl\Ubuntu\home")),
        (r"D:\code\api", local(r"D:\code\api")),
        ("./relative/dir", local("./relative/dir")),
        ("/home/kyle/api", local("/home/kyle/api")),
        ("wsl.localhost", local("wsl.localhost")),
    ];
    for (arg, expected) in cases {
        assert_eq!(Location::parse(arg).as_ref(), Ok(expected), "{arg}");
    }
}

#[test]
fn bad_path_arguments_are_typed_errors() {
    assert_eq!(Location::parse(""), Err(LocationError::Empty));
    for arg in [
        r"\\wsl.localhost\",
        r"\\wsl.localhost",
        "//wsl$/",
        r"\\wsl$\\x",
    ] {
        assert_eq!(
            Location::parse(arg),
            Err(LocationError::MissingDistro {
                arg: arg.to_string()
            }),
            "{arg}"
        );
    }
    for (arg, distro) in [
        (r"\\wsl.localhost\-d\x", "-d"),
        (r"\\wsl.localhost\a:b\x", "a:b"),
        (r"\\wsl.localhost\my distro\x", "my distro"),
    ] {
        assert_eq!(
            Location::parse(arg),
            Err(LocationError::InvalidDistro {
                distro: distro.to_string()
            }),
            "{arg}"
        );
    }
}

#[test]
fn wsl_arguments_parse_into_locations() {
    let cases: &[(&str, Location)] = &[
        ("Ubuntu:/home/kyle/api", wsl("Ubuntu", "/home/kyle/api")),
        ("Ubuntu:/home//kyle/api/", wsl("Ubuntu", "/home/kyle/api")),
        ("Ubuntu-24.04:/", wsl("Ubuntu-24.04", "/")),
        ("Debian:/srv/a:b", wsl("Debian", "/srv/a:b")),
    ];
    for (arg, expected) in cases {
        assert_eq!(Location::parse_wsl(arg).as_ref(), Ok(expected), "{arg}");
    }
    for arg in ["Ubuntu", "Ubuntu:home/kyle", ":/home", "Ubuntu:", ""] {
        assert_eq!(
            Location::parse_wsl(arg),
            Err(LocationError::InvalidWslArg {
                arg: arg.to_string()
            }),
            "{arg}"
        );
    }
    assert_eq!(
        Location::parse_wsl("Ub/untu:/x"),
        Err(LocationError::InvalidDistro {
            distro: "Ub/untu".to_string()
        })
    );
}

#[test]
fn display_and_short_names() {
    let api = wsl("Ubuntu", "/home/kyle/api");
    assert_eq!(api.display_name(), "Ubuntu:/home/kyle/api");
    assert_eq!(api.to_string(), "Ubuntu:/home/kyle/api");
    assert_eq!(api.short_name(), "api");
    assert_eq!(wsl("Ubuntu", "/").short_name(), "Ubuntu");

    let path = std::path::Path::new("code").join("api");
    let local_api = Location::Local(path.clone());
    assert_eq!(local_api.display_name(), path.display().to_string());
    assert_eq!(local_api.short_name(), "api");
    assert_eq!(local(".").short_name(), ".");
}

#[test]
fn serde_shape_is_externally_tagged_snake_case() {
    let api = wsl("Ubuntu", "/home/kyle/api");
    let json = serde_json::to_string(&api).unwrap();
    assert_eq!(
        json,
        r#"{"wsl":{"distro":"Ubuntu","path":"/home/kyle/api"}}"#
    );
    assert_eq!(serde_json::from_str::<Location>(&json).unwrap(), api);

    let local_api = local("/srv/api");
    let json = serde_json::to_string(&local_api).unwrap();
    assert_eq!(json, r#"{"local":"/srv/api"}"#);
    assert_eq!(serde_json::from_str::<Location>(&json).unwrap(), local_api);
}

#[test]
fn a_wsl_location_is_spelled_for_windows_through_its_share() {
    assert_eq!(
        wsl("Ubuntu", "/home/kyle/api").unc_path().as_deref(),
        Some(r"\\wsl.localhost\Ubuntu\home\kyle\api")
    );
    assert_eq!(
        wsl("Ubuntu-24.04", "/").unc_path().as_deref(),
        Some(r"\\wsl.localhost\Ubuntu-24.04\")
    );
    assert_eq!(local(r"D:\code\api").unc_path(), None);
    // The spelling parses back into the same location.
    let api = wsl("Debian", "/srv/my repo");
    assert_eq!(Location::parse(&api.unc_path().unwrap()), Ok(api));
}

#[test]
fn a_file_is_spelled_as_this_machine_opens_it() {
    // A WSL file through the distro's share, never `/home/…` read as a path on this machine.
    let api = wsl("Ubuntu", "/home/kyle/api");
    let share = PathBuf::from(r"\\wsl.localhost\Ubuntu\home\kyle\api");
    assert_eq!(
        api.file_path("src/routes/users.ts"),
        share.join("src").join("routes").join("users.ts")
    );
    if cfg!(windows) {
        assert_eq!(
            api.file_path("src/main.rs"),
            PathBuf::from(r"\\wsl.localhost\Ubuntu\home\kyle\api\src\main.rs")
        );
        assert_eq!(
            Location::parse(&api.file_path("src/main.rs").to_string_lossy()),
            Ok(wsl("Ubuntu", "/home/kyle/api/src/main.rs"))
        );
    }
    let root = std::env::temp_dir().join("api");
    assert_eq!(
        Location::Local(root.clone()).file_path("src/main.rs"),
        root.join("src").join("main.rs")
    );
    assert_eq!(Location::Local(root.clone()).file_path(""), root);
}

#[test]
fn only_a_local_repository_on_windows_or_macos_ignores_case() {
    assert_eq!(
        local(r"D:\code\api").folds_case(),
        cfg!(any(windows, target_os = "macos"))
    );
    assert!(!wsl("Ubuntu", "/home/kyle/api").folds_case());
}

#[test]
fn a_wsl_distro_names_the_same_place_in_any_case() {
    let api = wsl("Ubuntu", "/home/kyle/api");
    assert!(api.same_place(&wsl("ubuntu", "/home/kyle/api")));
    assert!(api.same_place(&api));
    // The path is the distro's Linux file system's: its case counts.
    assert!(!api.same_place(&wsl("Ubuntu", "/home/kyle/API")));
    assert!(!api.same_place(&wsl("Debian", "/home/kyle/api")));
    assert!(!api.same_place(&local(r"\wsl.localhost\Ubuntu\home\kyle\api")));
    assert!(local(r"D:\code\api").same_place(&local(r"D:\code\api")));
}

#[test]
fn a_windows_drive_is_spelled_as_wsl_mounts_it_and_back() {
    for (windows, mounted) in [
        (r"D:\Software\diff", "/mnt/d/Software/diff"),
        ("D:/Software/diff", "/mnt/d/Software/diff"),
        (r"d:\x\", "/mnt/d/x"),
        (r"C:\a\.\b\..\c", "/mnt/c/a/b/../c"),
        ("C:", "/mnt/c"),
        (r"C:\", "/mnt/c"),
        (r"e:\my repo $x", "/mnt/e/my repo $x"),
    ] {
        assert_eq!(mount_path(windows).as_deref(), Some(mounted), "{windows}");
    }
    for not_a_drive in [
        "",
        "D",
        "D:code",
        "DD:\\x",
        "1:\\x",
        "/mnt/d/x",
        "relative/path",
        r"\\server\share",
        r"\\wsl.localhost\Ubuntu\x",
        "é:\\x",
    ] {
        assert_eq!(mount_path(not_a_drive), None, "{not_a_drive}");
    }

    for (mounted, windows) in [
        ("/mnt/d/Software/diff", r"D:\Software\diff"),
        ("/mnt/c", r"C:\"),
        ("/mnt/c/", r"C:\"),
        ("/mnt/e/my repo $x", r"E:\my repo $x"),
    ] {
        assert_eq!(drive_path(mounted).as_deref(), Some(windows), "{mounted}");
        // And back.
        assert_eq!(
            drive_path(&mount_path(windows).unwrap()).as_deref(),
            Some(windows)
        );
    }
    for elsewhere in ["/mnt", "/mnt/wsl/x", "/mnt/dd/x", "/home/kyle", "D:\\x"] {
        assert_eq!(drive_path(elsewhere), None, "{elsewhere}");
    }
}
