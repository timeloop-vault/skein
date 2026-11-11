use super::*;

fn mkdir(root: &Path, rel: &str) {
    std::fs::create_dir_all(root.join(rel)).unwrap();
}

fn list(root: &Path) -> ProfileListDto {
    list_profiles_in(Some(root), &HookInputs::default())
}

fn names(l: &ProfileListDto) -> Vec<&str> {
    l.profiles.iter().map(|p| p.name.as_str()).collect()
}

#[test]
fn normal_root_has_no_note() {
    let d = tempfile::tempdir().unwrap();
    mkdir(d.path(), "profiles/work");
    let l = list(d.path());
    assert_eq!(l.root_note, None);
    assert_eq!(names(&l), ["skein", "default", "work"]);
}

#[test]
fn root_named_profiles_suggests_its_parent() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("Profiles");
    mkdir(&root, "work");
    let l = list(&root);
    let note = l.root_note.unwrap();
    assert_eq!(note.kind, RootNoteKind::LooksLikeProfilesDir);
    assert_eq!(
        note.suggested_root.as_deref(),
        Some(d.path().to_string_lossy().as_ref())
    );
    // A missing `.../profiles` root gets the same hint.
    let missing = d.path().join("nope").join("profiles");
    let note = list(&missing).root_note.unwrap();
    assert_eq!(note.kind, RootNoteKind::LooksLikeProfilesDir);
    assert!(note.suggested_root.is_some());
}

#[test]
fn real_root_that_happens_to_be_named_profiles_is_fine() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("profiles");
    mkdir(&root, "profiles/work");
    assert_eq!(list(&root).root_note, None);
}

#[test]
fn root_without_profiles_dir_is_informational() {
    let d = tempfile::tempdir().unwrap();
    let l = list(d.path());
    assert_eq!(
        l.root_note,
        Some(RootNoteDto {
            kind: RootNoteKind::NoProfilesDir,
            suggested_root: None
        })
    );
    assert_eq!(l.error, None);
}

#[test]
fn missing_root_is_reported() {
    let d = tempfile::tempdir().unwrap();
    let l = list(&d.path().join("gone"));
    assert_eq!(
        l.root_note.as_ref().map(|n| n.kind),
        Some(RootNoteKind::Missing)
    );
    assert_eq!(names(&l), ["skein", "default"]);
}

#[test]
fn skipped_entries_are_not_listed() {
    let d = tempfile::tempdir().unwrap();
    mkdir(d.path(), "profiles/Bad Name");
    mkdir(d.path(), "profiles/tmp");
    mkdir(d.path(), "profiles/ok");
    std::fs::write(d.path().join("profiles/file"), "x").unwrap();
    let l = list(d.path());
    assert_eq!(names(&l), ["skein", "default", "ok"]);
    assert_eq!(l.root_note, None);
}

#[test]
fn root_note_serializes_camel_case() {
    let note = RootNoteDto {
        kind: RootNoteKind::LooksLikeProfilesDir,
        suggested_root: Some("/p".to_string()),
    };
    assert_eq!(
        serde_json::to_string(&note).unwrap(),
        r#"{"kind":"looksLikeProfilesDir","suggestedRoot":"/p"}"#
    );
}
