//! ntpath and posixpath semantics unit tests.
//!
//! Expected values are taken verbatim from CPython 3.12's `ntpath` and `posixpath`.

use crate::builtin_modules::genericpath::ops::splitext_str;
use crate::builtin_modules::ntpath::ops::{
    nt_commonpath_str, nt_expandvars_str, nt_isabs_str, nt_join_str, nt_normpath_str,
    nt_relpath_str, nt_split_str, nt_splitdrive_str, nt_splitroot_str,
};
use crate::builtin_modules::posixpath::ops::{
    posix_commonpath_str, posix_expandvars_str, posix_splitroot_str,
};

fn sd(p: &str) -> (String, String) {
    nt_splitdrive_str(p)
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
    assert_eq!(nt_split_str(r"C:\a\b"), s2(r"C:\a", "b"));
    assert_eq!(nt_split_str("C:/a/b"), s2("C:/a", "b"));
    assert_eq!(nt_split_str("a/b"), s2("a", "b"));
    assert_eq!(nt_split_str(r"C:\"), s2(r"C:\", ""));
    assert_eq!(
        nt_split_str(r"\\host\share\x\y"),
        s2(r"\\host\share\x", "y")
    );
    assert_eq!(nt_split_str(r"\\host\share"), s2(r"\\host\share", ""));
    assert_eq!(nt_split_str(""), s2("", ""));
    assert_eq!(nt_split_str("/usr/bin"), s2("/usr", "bin"));
    assert_eq!(nt_split_str(r"a\b\"), s2(r"a\b", ""));
}

#[test]
fn isabs_matches_cpython() {
    assert!(nt_isabs_str(r"C:\x"));
    assert!(nt_isabs_str("/x"));
    assert!(nt_isabs_str(r"\\x"));
    assert!(!nt_isabs_str("C:x"));
    assert!(!nt_isabs_str("x"));
    assert!(nt_isabs_str("C:/"));
    assert!(nt_isabs_str("/"));
    assert!(nt_isabs_str(r"\\host\share"));
}

#[test]
fn join_matches_cpython() {
    let j = |parts: &[&str]| nt_join_str(&parts.iter().map(|s| s.to_string()).collect::<Vec<_>>());
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
    assert_eq!(nt_normpath_str("a/b"), r"a\b");
    assert_eq!(nt_normpath_str(r"a\b"), r"a\b");
    assert_eq!(nt_normpath_str("C:/a/../b"), r"C:\b");
    assert_eq!(nt_normpath_str("//host/share/a/../b"), r"\\host\share\b");
    assert_eq!(nt_normpath_str("C:/../x"), r"C:\x");
    assert_eq!(nt_normpath_str(""), ".");
    assert_eq!(nt_normpath_str("."), ".");
    assert_eq!(nt_normpath_str("C://a//b"), r"C:\a\b");
    assert_eq!(nt_normpath_str(r"C:\..\x"), r"C:\x");
    assert_eq!(nt_normpath_str("foo/../.."), "..");
    assert_eq!(nt_normpath_str("/a/b"), r"\a\b");
    assert_eq!(nt_normpath_str("a/./b/"), r"a\b");
}

#[test]
fn splitext_matches_cpython() {
    let nt_ext = |p: &str| splitext_str(p, '\\', Some('/'), '.');
    assert_eq!(nt_ext(r"C:\a\b.txt"), s2(r"C:\a\b", ".txt"));
    assert_eq!(nt_ext("a.b.c"), s2("a.b", ".c"));
    assert_eq!(nt_ext(".bashrc"), s2(".bashrc", ""));
    assert_eq!(nt_ext("/x/.y"), s2("/x/.y", ""));
    assert_eq!(nt_ext("a/b"), s2("a/b", ""));
    assert_eq!(nt_ext("foo."), s2("foo", "."));
    assert_eq!(nt_ext(r"C:\a.b\c"), s2(r"C:\a.b\c", ""));
}

#[test]
fn dirname_basename_match_cpython() {
    assert_eq!(nt_split_str(r"C:\a\b").0, r"C:\a");
    assert_eq!(nt_split_str(r"C:\a\b").1, "b");
    assert_eq!(nt_split_str(r"C:\a\").1, "");
    assert_eq!(nt_split_str(r"C:\a\").0, r"C:\a");
}

#[test]
fn relpath_matches_cpython() {
    assert_eq!(nt_relpath_str(r"C:\a\b\c", r"C:\a\d").unwrap(), r"..\b\c");
    assert_eq!(nt_relpath_str(r"C:\a\b", r"C:\a\b").unwrap(), ".");
    assert_eq!(nt_relpath_str(r"C:\a\b\c", r"C:\a").unwrap(), r"b\c");
    let err = nt_relpath_str(r"C:\a", r"D:\b").unwrap_err();
    assert!(err.class_name_is("ValueError"));
}

#[test]
fn splitroot_matches_cpython() {
    assert_eq!(
        nt_splitroot_str("//server/share/"),
        ("//server/share".into(), "/".into(), "".into())
    );
    assert_eq!(
        nt_splitroot_str("C:/Users/Barney"),
        ("C:".into(), "/".into(), "Users/Barney".into())
    );
    assert_eq!(
        nt_splitroot_str("C:///spam///ham"),
        ("C:".into(), "/".into(), "//spam///ham".into())
    );
    assert_eq!(
        nt_splitroot_str("Windows/notepad"),
        ("".into(), "".into(), "Windows/notepad".into())
    );
    assert_eq!(
        posix_splitroot_str("//a/b"),
        ("".into(), "//".into(), "a/b".into())
    );
    assert_eq!(
        posix_splitroot_str("///a/b"),
        ("".into(), "/".into(), "//a/b".into())
    );
}

#[test]
fn commonpath_matches_cpython() {
    let s = |xs: &[&str]| xs.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert_eq!(
        posix_commonpath_str(&s(&["/usr/lib", "/usr/local/lib"])).unwrap(),
        "/usr"
    );
    assert_eq!(posix_commonpath_str(&s(&["/a/b", "/c/d"])).unwrap(), "/");
    assert_eq!(
        posix_commonpath_str(&s(&["a/b/c", "a/b/d"])).unwrap(),
        "a/b"
    );
    assert_eq!(posix_commonpath_str(&s(&["a/b", "c/d"])).unwrap(), "");
    assert!(
        posix_commonpath_str(&s(&["/a", "a"]))
            .unwrap_err()
            .class_name_is("ValueError")
    );
    assert!(
        posix_commonpath_str(&s(&[]))
            .unwrap_err()
            .class_name_is("ValueError")
    );

    assert_eq!(
        nt_commonpath_str(&s(&[r"C:\a\b", "c:/a/c"])).unwrap(),
        r"C:\a"
    );
    assert_eq!(nt_commonpath_str(&s(&[r"C:\a", r"C:\b"])).unwrap(), r"C:\");
    assert_eq!(nt_commonpath_str(&s(&["a/b/c", r"a\b\d"])).unwrap(), r"a\b");
    assert!(
        nt_commonpath_str(&s(&[r"C:\a", r"D:\a"]))
            .unwrap_err()
            .class_name_is("ValueError")
    );
    assert!(
        nt_commonpath_str(&s(&[r"C:\a", "a"]))
            .unwrap_err()
            .class_name_is("ValueError")
    );
}

#[test]
fn expandvars_preserves_unknown_vars() {
    assert_eq!(
        posix_expandvars_str("$PYRUST_NO_SUCH_ENV_3206/${PYRUST_NO_SUCH_ENV_3206}"),
        "$PYRUST_NO_SUCH_ENV_3206/${PYRUST_NO_SUCH_ENV_3206}"
    );
    assert_eq!(
        nt_expandvars_str("%PYRUST_NO_SUCH_ENV_3206%/$PYRUST_NO_SUCH_ENV_3206/%%"),
        "%PYRUST_NO_SUCH_ENV_3206%/$PYRUST_NO_SUCH_ENV_3206/%"
    );
    assert_eq!(nt_expandvars_str("'%PATH%'"), "'%PATH%'");
}
