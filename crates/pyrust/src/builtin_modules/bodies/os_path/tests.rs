//! ntpath-semantics unit tests.
//!
//! pyrust's CI runs on Linux/WSL where the `os.path` parity fixture binds the
//! posix arm, so these exercise the `nt_*` helpers directly. Expected values
//! are taken verbatim from CPython 3.12's `ntpath`.

use super::normalize::{nt_expandvars, nt_isabs, nt_normpath, nt_relpath, posix_expandvars};
use super::split_join::{
    nt_commonpath, nt_join, nt_split, nt_splitdrive, nt_splitext, nt_splitroot, posix_commonpath,
};

fn sd(p: &str) -> (String, String) {
    nt_splitdrive(p)
}

fn s2(a: &str, b: &str) -> (String, String) {
    (a.to_string(), b.to_string())
}

#[test]
fn splitdrive_matches_cpython() {
    assert_eq!(sd("c:/x"), s2("c:", "/x"));
    assert_eq!(sd("//host/share/x"), s2("//host/share", "/x"));
    assert_eq!(sd("a/b"), s2("", "a/b"));
    assert_eq!(sd(r"C:\a"), s2("C:", r"\a"));
    assert_eq!(sd(r"\\?\UNC\srv\shr\x"), s2(r"\\?\UNC\srv\shr", r"\x"));
    assert_eq!(sd("C:"), s2("C:", ""));
    assert_eq!(sd(""), s2("", ""));
}

#[test]
fn split_matches_cpython() {
    assert_eq!(nt_split(r"C:\a\b"), s2(r"C:\a", "b"));
    assert_eq!(nt_split("C:/a/b"), s2("C:/a", "b"));
    assert_eq!(nt_split("a/b"), s2("a", "b"));
    assert_eq!(nt_split(r"C:\"), s2(r"C:\", ""));
    assert_eq!(nt_split(r"\\host\share\x\y"), s2(r"\\host\share\x", "y"));
    assert_eq!(nt_split(r"\\host\share"), s2(r"\\host\share", ""));
    assert_eq!(nt_split(""), s2("", ""));
    assert_eq!(nt_split("/usr/bin"), s2("/usr", "bin"));
    assert_eq!(nt_split(r"a\b\"), s2(r"a\b", ""));
}

#[test]
fn isabs_matches_cpython() {
    assert!(nt_isabs(r"C:\x"));
    assert!(nt_isabs("/x")); // legacy bug: bare separator is "absolute"
    assert!(nt_isabs(r"\\x"));
    assert!(!nt_isabs("C:x"));
    assert!(!nt_isabs("x"));
    assert!(nt_isabs("C:/"));
    assert!(nt_isabs("/"));
    assert!(nt_isabs(r"\\host\share"));
}

#[test]
fn join_matches_cpython() {
    let j = |parts: &[&str]| nt_join(&parts.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    assert_eq!(j(&[r"C:\a", "b"]), r"C:\a\b");
    assert_eq!(j(&["a", r"C:\b"]), r"C:\b");
    assert_eq!(j(&["C:", "b"]), "C:b");
    assert_eq!(j(&[r"C:\", "b"]), r"C:\b");
    assert_eq!(j(&["c:/a", "d:/b"]), "d:/b");
    assert_eq!(j(&["/a", "b"]), r"/a\b");
    assert_eq!(j(&["a", "/b"]), "/b");
    assert_eq!(j(&["a", "b", "c"]), r"a\b\c");
    assert_eq!(j(&["//host/share", "a"]), r"//host/share\a");
}

#[test]
fn normpath_matches_cpython() {
    assert_eq!(nt_normpath("a/b"), r"a\b");
    assert_eq!(nt_normpath(r"a\b"), r"a\b");
    assert_eq!(nt_normpath("C:/a/../b"), r"C:\b");
    assert_eq!(nt_normpath("//host/share/a/../b"), r"\\host\share\b");
    assert_eq!(nt_normpath("C:/../x"), r"C:\x");
    assert_eq!(nt_normpath(""), ".");
    assert_eq!(nt_normpath("."), ".");
    assert_eq!(nt_normpath("C://a//b"), r"C:\a\b");
    assert_eq!(nt_normpath(r"C:\..\x"), r"C:\x");
    assert_eq!(nt_normpath("foo/../.."), "..");
    assert_eq!(nt_normpath("/a/b"), r"\a\b");
    assert_eq!(nt_normpath("a/./b/"), r"a\b");
}

#[test]
fn splitext_matches_cpython() {
    assert_eq!(nt_splitext(r"C:\a\b.txt"), s2(r"C:\a\b", ".txt"));
    assert_eq!(nt_splitext("a.b.c"), s2("a.b", ".c"));
    assert_eq!(nt_splitext(".bashrc"), s2(".bashrc", ""));
    assert_eq!(nt_splitext("/x/.y"), s2("/x/.y", ""));
    assert_eq!(nt_splitext("a/b"), s2("a/b", ""));
    assert_eq!(nt_splitext("foo."), s2("foo", "."));
    assert_eq!(nt_splitext(r"C:\a.b\c"), s2(r"C:\a.b\c", ""));
}

#[test]
fn dirname_basename_match_cpython() {
    // dirname == split[0], basename == split[1].
    assert_eq!(nt_split(r"C:\a\b").0, r"C:\a");
    assert_eq!(nt_split(r"C:\a\b").1, "b");
    assert_eq!(nt_split(r"C:\a\").1, "");
    assert_eq!(nt_split(r"C:\a\").0, r"C:\a");
}

#[test]
fn relpath_matches_cpython() {
    // Absolute inputs so the host cwd does not enter the computation.
    assert_eq!(nt_relpath(r"C:\a\b\c", r"C:\a\d").unwrap(), r"..\b\c");
    assert_eq!(nt_relpath(r"C:\a\b", r"C:\a\b").unwrap(), ".");
    assert_eq!(nt_relpath(r"C:\a\b\c", r"C:\a").unwrap(), r"b\c");
    let err = nt_relpath(r"C:\a", r"D:\b").unwrap_err();
    assert!(err.class_name_is("ValueError"));
}

#[test]
fn splitroot_matches_cpython() {
    let r = |p: &str| {
        let (d, r, t) = nt_splitroot(p);
        (d, r, t)
    };
    assert_eq!(
        r("//server/share/"),
        ("//server/share".into(), "/".into(), "".into())
    );
    assert_eq!(
        r("C:/Users/Barney"),
        ("C:".into(), "/".into(), "Users/Barney".into())
    );
    assert_eq!(
        r("C:///spam///ham"),
        ("C:".into(), "/".into(), "//spam///ham".into())
    );
    assert_eq!(
        r("Windows/notepad"),
        ("".into(), "".into(), "Windows/notepad".into())
    );
}

#[test]
fn commonpath_matches_cpython() {
    let s = |xs: &[&str]| xs.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert_eq!(
        posix_commonpath(&s(&["/usr/lib", "/usr/local/lib"])).unwrap(),
        "/usr"
    );
    assert_eq!(posix_commonpath(&s(&["/a/b", "/c/d"])).unwrap(), "/");
    assert_eq!(posix_commonpath(&s(&["a/b/c", "a/b/d"])).unwrap(), "a/b");
    assert_eq!(posix_commonpath(&s(&["a/b", "c/d"])).unwrap(), "");
    assert!(
        posix_commonpath(&s(&["/a", "a"]))
            .unwrap_err()
            .class_name_is("ValueError")
    );
    assert!(
        posix_commonpath(&s(&[]))
            .unwrap_err()
            .class_name_is("ValueError")
    );

    assert_eq!(nt_commonpath(&s(&[r"C:\a\b", "c:/a/c"])).unwrap(), r"C:\a");
    assert_eq!(nt_commonpath(&s(&[r"C:\a", r"C:\b"])).unwrap(), r"C:\");
    assert_eq!(nt_commonpath(&s(&["a/b/c", r"a\b\d"])).unwrap(), r"a\b");
    assert!(
        nt_commonpath(&s(&[r"C:\a", r"D:\a"]))
            .unwrap_err()
            .class_name_is("ValueError")
    );
    assert!(
        nt_commonpath(&s(&[r"C:\a", "a"]))
            .unwrap_err()
            .class_name_is("ValueError")
    );
}

#[test]
fn expandvars_preserves_unknown_vars() {
    assert_eq!(
        posix_expandvars("$PYRUST_NO_SUCH_ENV_3206/${PYRUST_NO_SUCH_ENV_3206}"),
        "$PYRUST_NO_SUCH_ENV_3206/${PYRUST_NO_SUCH_ENV_3206}"
    );
    assert_eq!(
        nt_expandvars("%PYRUST_NO_SUCH_ENV_3206%/$PYRUST_NO_SUCH_ENV_3206/%%"),
        "%PYRUST_NO_SUCH_ENV_3206%/$PYRUST_NO_SUCH_ENV_3206/%"
    );
    assert_eq!(nt_expandvars("'%PATH%'"), "'%PATH%'");
}
