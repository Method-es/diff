use super::*;

/// Each line as `<kind><old>:<new> <text>`, e.g. `-3: gone`, `+:4 new`, ` 1:1 same`.
fn render(file: &PatchFile) -> Vec<String> {
    file.lines
        .iter()
        .map(|line| {
            let kind = match line.kind {
                LineKind::Context => ' ',
                LineKind::Removed => '-',
                LineKind::Added => '+',
            };
            let no = |n: Option<u32>| n.map(|n| n.to_string()).unwrap_or_default();
            format!(
                "{kind}{}:{} {}",
                no(line.old_no),
                no(line.new_no),
                line.text
            )
        })
        .collect()
}

fn parse_one(text: &str) -> PatchFile {
    let mut files = parse(text.as_bytes()).unwrap();
    assert_eq!(files.len(), 1, "{files:?}");
    files.remove(0)
}

#[test]
fn a_modified_file_with_full_context() {
    let file = parse_one(
        "diff --git a/src/lib.rs b/src/lib.rs\n\
         index 1111111..2222222 100644\n\
         --- a/src/lib.rs\n\
         +++ b/src/lib.rs\n\
         @@ -1,4 +1,4 @@ fn section_heading()\n\
         \x20one\n\
         -two\n\
         +TWO\n\
         \x20three\n\
         \x20four\n",
    );
    assert_eq!(file.new_name(), Some("src/lib.rs"));
    assert_eq!(file.old_name(), Some("src/lib.rs"));
    assert_eq!(
        render(&file),
        [" 1:1 one", "-2: two", "+:2 TWO", " 3:3 three", " 4:4 four"]
    );
    assert!(file.old_eof_newline && file.new_eof_newline);
    assert!(!file.binary && !file.crlf && !file.combined);
}

#[test]
fn added_and_deleted_files_number_only_their_side() {
    let files = parse(
        b"diff --git a/new.txt b/new.txt\n\
          new file mode 100644\n\
          index 0000000..1111111\n\
          --- /dev/null\n\
          +++ b/new.txt\n\
          @@ -0,0 +1,2 @@\n\
          +a\n\
          +b\n\
          diff --git a/old.txt b/old.txt\n\
          deleted file mode 100644\n\
          index 1111111..0000000\n\
          --- a/old.txt\n\
          +++ /dev/null\n\
          @@ -1 +0,0 @@\n\
          -gone\n",
    )
    .unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].new_name(), Some("new.txt"));
    assert_eq!(
        files[0].old_name(),
        Some("new.txt"),
        "falls back to the header"
    );
    assert_eq!(render(&files[0]), ["+:1 a", "+:2 b"]);
    assert_eq!(
        files[1].new_name(),
        Some("old.txt"),
        "falls back to the header"
    );
    assert_eq!(render(&files[1]), ["-1: gone"]);
}

#[test]
fn no_newline_markers_apply_to_the_line_before_them() {
    // Old side ends without a newline; the new side adds one.
    let file = parse_one(
        "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n a\n-b\n\\ No newline at end of file\n+b\n",
    );
    assert_eq!(render(&file), [" 1:1 a", "-2: b", "+:2 b"]);
    assert!(!file.old_eof_newline);
    assert!(file.new_eof_newline);

    // New side loses its newline.
    let file = parse_one(
        "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-b\n+b\n\\ No newline at end of file\n",
    );
    assert!(file.old_eof_newline);
    assert!(!file.new_eof_newline);

    // Both sides end without one on the same unchanged last line.
    let file = parse_one(
        "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n-a\n+A\n z\n\\ No newline at end of file\n",
    );
    assert!(!file.old_eof_newline && !file.new_eof_newline);
    assert_eq!(render(&file), ["-1: a", "+:1 A", " 2:2 z"]);
}

#[test]
fn hunk_bodies_are_read_by_count_not_by_appearance() {
    // A reviewed `.patch` file: its removed and added lines look like headers.
    let file = parse_one(
        "diff --git a/x.patch b/x.patch\n\
         --- a/x.patch\n\
         +++ b/x.patch\n\
         @@ -1,3 +1,3 @@\n\
         --- a/inner\n\
         +++ b/inner\n\
         \x20diff --git a/y b/y\n\
         -@@ -1 +1 @@\n\
         +@@ -2 +2 @@\n",
    );
    assert_eq!(
        render(&file),
        [
            "-1: -- a/inner",
            "+:1 ++ b/inner",
            " 2:2 diff --git a/y b/y",
            "-3: @@ -1 +1 @@",
            "+:3 @@ -2 +2 @@"
        ]
    );
    assert_eq!(file.new_name(), Some("x.patch"));
}

#[test]
fn several_hunks_keep_their_own_numbering() {
    let file = parse_one(
        "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -2,2 +2,2 @@\n-b\n+B\n c\n@@ -10 +10,2 @@\n j\n+k\n",
    );
    assert_eq!(
        render(&file),
        ["-2: b", "+:2 B", " 3:3 c", " 10:10 j", "+:11 k"]
    );
}

#[test]
fn crlf_endings_are_stripped_and_reported() {
    let file =
        parse_one("diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n a\r\n-b\r\n+c\r\n");
    assert!(file.crlf);
    assert_eq!(render(&file), [" 1:1 a", "-2: b", "+:2 c"]);
    // A carriage return inside a line stays.
    let file = parse_one("diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\rb\n+a\rc\n");
    assert!(!file.crlf);
    assert_eq!(file.lines[0].text, "a\rb");
}

#[test]
fn an_empty_line_in_a_hunk_is_an_empty_context_line() {
    // `diff.suppressBlankEmpty=true` drops the space of an empty context line.
    let file = parse_one("diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n a\n\n-b\n+c\n");
    assert_eq!(render(&file), [" 1:1 a", " 2:2 ", "-3: b", "+:3 c"]);
}

#[test]
fn binary_mode_only_and_empty_files_have_no_lines() {
    let files = parse(
        b"diff --git a/img.png b/img.png\n\
          index 1111111..2222222 100644\n\
          Binary files a/img.png and b/img.png differ\n\
          diff --git a/run.sh b/run.sh\n\
          old mode 100644\n\
          new mode 100755\n\
          diff --git a/empty b/empty\n\
          new file mode 100644\n\
          index 0000000..e69de29\n",
    )
    .unwrap();
    assert_eq!(files.len(), 3);
    assert!(files[0].binary);
    assert_eq!(files[0].new_name(), Some("img.png"));
    assert!(!files[1].binary && files[1].lines.is_empty());
    assert_eq!(files[1].new_name(), Some("run.sh"));
    assert!(files[2].lines.is_empty());
    assert_eq!(files[2].new_name(), Some("empty"));
}

#[test]
fn renames_take_their_names_from_the_rename_lines() {
    let file = parse_one(
        "diff --git a/old name.txt b/new name.txt\n\
         similarity index 90%\n\
         rename from old name.txt\n\
         rename to new name.txt\n\
         index 1111111..2222222 100644\n\
         --- a/old name.txt\t\n\
         +++ b/new name.txt\t\n\
         @@ -1 +1 @@\n\
         -a\n\
         +b\n",
    );
    assert_eq!(file.old_name(), Some("old name.txt"));
    assert_eq!(file.new_name(), Some("new name.txt"));
    // A pure rename has no ---/+++ lines and no hunks.
    let file = parse_one("diff --git a/a b/b\nsimilarity index 100%\nrename from a\nrename to b\n");
    assert_eq!((file.old_name(), file.new_name()), (Some("a"), Some("b")));
    let file = parse_one("diff --git a/a b/c\nsimilarity index 100%\ncopy from a\ncopy to c\n");
    assert_eq!((file.old_name(), file.new_name()), (Some("a"), Some("c")));
}

#[test]
fn names_with_spaces_take_the_trailing_tab_off() {
    let file = parse_one(
        "diff --git a/sp ace.txt b/sp ace.txt\n--- a/sp ace.txt\t\n+++ b/sp ace.txt\t\n@@ -1 +1 @@\n-a\n+b\n",
    );
    assert_eq!(file.new_name(), Some("sp ace.txt"));
    assert_eq!(file.old_name(), Some("sp ace.txt"));
    // Without ---/+++ lines (a mode change), the header's two equal halves name it.
    let file =
        parse_one("diff --git a/sp ace.txt b/sp ace.txt\nold mode 100644\nnew mode 100755\n");
    assert_eq!(file.new_name(), Some("sp ace.txt"));
    assert_eq!(
        header_names(b"a/x b/y"),
        None,
        "a rename needs its rename lines"
    );
}

#[test]
fn quoted_names_are_unquoted() {
    let file = parse_one(
        "diff --git \"a/q\\\"uo\\te\\\\x.txt\" \"b/q\\\"uo\\te\\\\x.txt\"\n\
         --- \"a/q\\\"uo\\te\\\\x.txt\"\n\
         +++ \"b/q\\\"uo\\te\\\\x.txt\"\n\
         @@ -1 +1 @@\n-a\n+b\n",
    );
    assert_eq!(file.new_name(), Some("q\"uo\te\\x.txt"));
    // A quoted second name only.
    assert_eq!(
        header_names(b"a/plain \"b/new\\nline\""),
        Some(("plain".to_string(), "new\nline".to_string()))
    );
    // A quoted first name only.
    assert_eq!(
        header_names(b"\"a/tab\\there\" b/plain"),
        Some(("tab\there".to_string(), "plain".to_string()))
    );
    // Octal escapes are raw bytes (UTF-8 with core.quotepath=on).
    assert_eq!(unquote(b"\"\\303\\274.txt\"").unwrap(), "ü.txt".as_bytes());
    assert_eq!(unquote(b"plain").unwrap(), b"plain");
    for bad in [
        &b"\"unterminated"[..],
        b"\"bad\\q\"",
        b"\"\\39\"",
        b"\"x\" trailing",
    ] {
        assert_eq!(unquote(bad), None, "{:?}", String::from_utf8_lossy(bad));
    }
}

#[test]
fn a_combined_diff_is_marked_and_skipped() {
    let files = parse(
        b"diff --cc conflict.txt\n\
          index 1111111,2222222..0000000\n\
          --- a/conflict.txt\n\
          +++ b/conflict.txt\n\
          @@@ -1,1 -1,1 +1,5 @@@\n\
          ++<<<<<<< HEAD\n\
          \x20+ours\n\
          ++=======\n\
          + theirs\n\
          ++>>>>>>> side\n\
          diff --git a/next.txt b/next.txt\n\
          --- a/next.txt\n\
          +++ b/next.txt\n\
          @@ -1 +1 @@\n\
          -x\n\
          +y\n",
    )
    .unwrap();
    assert_eq!(files.len(), 2);
    assert!(files[0].combined);
    assert_eq!(files[0].new_name(), Some("conflict.txt"));
    assert_eq!(render(&files[1]), ["-1: x", "+:1 y"]);
}

#[test]
fn malformed_hunks_are_errors_not_panics() {
    for bad in [
        "diff --git a/f b/f\n@@ -1,2 +1,2 @@\n a\n",
        "diff --git a/f b/f\n@@ -1 +1 @@\n a\n b\n-c\n",
        "diff --git a/f b/f\n@@ -x +1 @@\n a\n",
        "diff --git a/f b/f\n@@ -1 +1 @@\n?what\n",
        "diff --git a/f b/f\n@@ -0,0 +1 @@\n\\ No newline at end of file\n+a\n",
    ] {
        assert!(parse(bad.as_bytes()).is_err(), "{bad:?}");
    }
    assert_eq!(parse(b"").unwrap(), vec![]);
    assert_eq!(parse(b"warning: something\n").unwrap(), vec![]);
}

#[test]
fn hunk_headers_parse_with_and_without_counts() {
    assert_eq!(hunk_header(b"@@ -1,3 +1,4 @@"), Some((1, 3, 1, 4)));
    assert_eq!(hunk_header(b"@@ -5 +7 @@ fn x()"), Some((5, 1, 7, 1)));
    assert_eq!(hunk_header(b"@@ -0,0 +1 @@"), Some((0, 0, 1, 1)));
    // The section heading is a line of the file, in any encoding.
    assert_eq!(
        hunk_header(b"@@ -1,2 +1,3 @@ caf\xe9()"),
        Some((1, 2, 1, 3))
    );
    // Numbers at the edge of the type don't overflow.
    let file = parse_one("diff --git a/f b/f\n@@ -4294967295,0 +4294967295 @@\n+x\n");
    assert_eq!(file.lines[0].new_no, Some(u32::MAX));
    assert_eq!(hunk_header(b"@@ -1 +1"), None);
    assert_eq!(hunk_header(b"@@@ -1 -1 +1 @@@"), None);
}

#[test]
fn non_utf8_content_is_decoded_lossily() {
    let file =
        parse_one_bytes(b"diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-caf\xe9\n+cafe\n");
    assert_eq!(file.lines[0].text, "caf\u{fffd}");
}

fn parse_one_bytes(bytes: &[u8]) -> PatchFile {
    let mut files = parse(bytes).unwrap();
    assert_eq!(files.len(), 1);
    files.remove(0)
}
