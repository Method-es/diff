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
fn a_wsl_location_has_no_backend_yet() {
    let api = wsl("Ubuntu", "/home/kyle/api");
    let err = api.open_backend().err().expect("WSL must not open");
    assert!(
        matches!(&err, BackendError::Unavailable { location, .. } if location == "Ubuntu:/home/kyle/api"),
        "{err:?}"
    );
    assert!(err.to_string().contains("Phase 4"), "{err}");
}
