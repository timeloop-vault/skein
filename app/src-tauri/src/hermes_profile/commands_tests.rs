use super::*;

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn find<'a>(l: &'a ProfileListDto, name: &str) -> &'a ProfileDto {
    l.profiles.iter().find(|p| p.name == name).unwrap()
}

#[test]
fn lists_skein_default_and_valid_profile_dirs() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(r, "config.yaml", "model: x\n");
    write(
        r,
        "profiles/work/config.yaml",
        "mcp_servers:\n  a:\n    url: u\n",
    );
    write(r, "profiles/Bad Name/config.yaml", "model: x\n");
    write(r, "profiles/root/config.yaml", "model: x\n");
    write(r, "profiles/bare/note.txt", "x");
    let l = list_profiles_in(Some(r));
    assert_eq!(l.error, None);
    let names: Vec<_> = l.profiles.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["skein", "default", "bare", "work"]);
    let skein = find(&l, "skein");
    assert!(!skein.exists);
    assert_eq!(skein.skein_entry, SkeinEntry::NoConfig);
    assert_eq!(find(&l, "default").skein_entry, SkeinEntry::Absent);
    assert_eq!(find(&l, "work").skein_entry, SkeinEntry::Absent);
    assert_eq!(find(&l, "bare").skein_entry, SkeinEntry::NoConfig);
}

#[test]
fn present_and_refused_states() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(r, "profiles/a/config.yaml", "model: x\n");
    inject_in(r, "a").unwrap();
    write(r, "profiles/b/config.yaml", "a: 1\n---\nb: 2\n");
    let l = list_profiles_in(Some(r));
    assert_eq!(find(&l, "a").skein_entry, SkeinEntry::Present);
    let b = find(&l, "b");
    assert_eq!(b.skein_entry, SkeinEntry::Refused);
    assert!(b.reason.is_some());
}

#[test]
fn no_root_lists_only_skein() {
    let l = list_profiles_in(None);
    assert_eq!(l.root, None);
    assert_eq!(l.profiles.len(), 1);
    assert_eq!(l.profiles[0].name, "skein");
    assert!(!l.profiles[0].exists);
}

#[test]
fn unreadable_profiles_dir_reports_error_but_keeps_defaults() {
    let d = tempfile::tempdir().unwrap();
    // `profiles` is a file, so read_dir fails with something other than NotFound.
    write(d.path(), "profiles", "x");
    let l = list_profiles_in(Some(d.path()));
    assert!(l.error.is_some());
    assert_eq!(l.profiles.len(), 2);
}

#[test]
fn preview_then_inject_then_already_present() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(r, "profiles/work/config.yaml", "model: x\n");
    let p = preview_in(r, "work").unwrap();
    assert!(p.config_path.ends_with("config.yaml"));
    assert!(p.lines.iter().any(|l| l.contains("skein")));
    assert_eq!(inject_in(r, "work").unwrap(), "added");
    assert_eq!(inject_in(r, "work").unwrap(), "alreadyPresent");
    assert_eq!(preview_in(r, "work").unwrap_err(), "already present");
}

#[test]
fn invalid_name_and_missing_config_are_errors() {
    let d = tempfile::tempdir().unwrap();
    assert!(preview_in(d.path(), "../x").is_err());
    assert!(inject_in(d.path(), "sudo").is_err());
    assert!(
        preview_in(d.path(), "nope")
            .unwrap_err()
            .contains("not found")
    );
    assert!(inject_in(d.path(), "nope").is_err());
}
