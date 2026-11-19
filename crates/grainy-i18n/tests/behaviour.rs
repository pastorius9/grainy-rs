//! Behaviour that needs no oracle: the language switch, the compiler rules and the column split.

use grainy_i18n::compile::{compile_sources, CompileError};
use grainy_i18n::shortcuts::{column_split, GROUPS, TABLE};
use grainy_i18n::{language, set_language, tr, tr_with, try_tr_with, Language};

fn source(text: &str) -> Vec<(String, String)> {
    vec![("en.txt".to_string(), text.to_string())]
}

// Global language state: one test only touches it.
#[test]
fn language_switch_and_tr() {
    assert_eq!(language(), Language::Korean);
    assert_eq!(tr("설치 확인"), "설치 확인");
    set_language(Language::English);
    assert_eq!(language(), Language::English);
    assert_eq!(tr("설치…"), "Install…");
    assert_eq!(tr("table has no such text"), "table has no such text");
    assert_eq!(tr_with("table has no {0}", &["x"]), "table has no x");
    assert_eq!(grainy_i18n::tr_with!("n={0} m={1}", 3, "b"), "n=3 m=b");
    // An error falls back to the unformatted text; try_tr_with shows it.
    assert_eq!(tr_with("needs {0}", &[]), "needs {0}");
    assert!(try_tr_with("needs {0} {1}", &["a"]).is_err());
    set_language(Language::Korean);
    assert_eq!(language(), Language::Korean);
    assert_eq!(Language::parse("en"), Ok(Language::English));
    assert!(Language::parse("fr").is_err());
}

#[test]
fn compiler_rules() {
    let t = compile_sources(&source("a\\nb => c\\nd\n\n   \nx {0} => y {0}\r\nx2 => y2")).unwrap();
    assert_eq!(t["a\nb"], "c\nd");
    assert_eq!(t["x {0}"], "y {0}");
    assert_eq!(t.len(), 3);
    // The same pair twice is fine; a different translation is not.
    assert!(compile_sources(&source("a => b\na => b")).is_ok());
    assert!(matches!(compile_sources(&source("a => b\na => c")), Err(CompileError::Duplicate { line: 2, .. })));
    assert!(matches!(compile_sources(&source("a {0} => b")), Err(CompileError::ChangedPlaceholders { .. })));
    assert!(matches!(compile_sources(&source("no separator")), Err(CompileError::NoSeparator { .. })));
}

#[test]
fn column_split_balances_rows() {
    let sizes: Vec<usize> = GROUPS.iter().map(|g| TABLE.iter().filter(|e| e.group == *g).count()).collect();
    assert_eq!(sizes, [11, 11, 5, 7, 3]);
    assert_eq!(column_split(&sizes), 2); // 22 / 15
}
