use super::*;
use std::fs;
use tempfile::TempDir;

const ROOM: &str = "r1";

struct Fx {
    dir: TempDir,
    cwd: String,
    db: Database,
}

impl Fx {
    /// A git repo whose initial commit holds `.gitignore` (`ignore`) and
    /// `tracked.txt`.
    fn git(ignore: &str) -> Self {
        let fx = Self::bare();
        let repo = git2::Repository::init(fx.dir.path()).unwrap();
        fx.write(".gitignore", ignore);
        fx.write("tracked.txt", "one\n");
        commit(&repo);
        fx
    }

    /// Not a repo at all.
    fn bare() -> Self {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let db = Database::open(&dir.path().join("skein.db")).unwrap();
        Self { dir, cwd, db }
    }

    fn write(&self, rel: &str, text: &str) {
        let p = abs_path(&self.cwd, rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    /// Absolute paths, the shape `event.path` arrives in.
    fn reported(&self, names: &[&str]) -> Vec<String> {
        names
            .iter()
            .map(|r| abs_path(&self.cwd, r).to_string_lossy().to_string())
            .collect()
    }

    fn paths(&self) -> Vec<String> {
        self.db
            .review_baselines_for_room(ROOM)
            .unwrap()
            .into_iter()
            .map(|b| b.path)
            .collect()
    }
}

fn commit(repo: &git2::Repository) {
    let mut index = repo.index().unwrap();
    index
        .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = git2::Signature::now("t", "t@example.com").unwrap();
    let parents = repo
        .head()
        .ok()
        .and_then(|h| h.peel_to_commit().ok())
        .map(|c| vec![c])
        .unwrap_or_default();
    let strs: Vec<&git2::Commit> = parents.iter().collect();
    repo.commit(Some("HEAD"), &sig, &sig, "c", &tree, &strs)
        .unwrap();
}

fn keys(n: usize, prefix: &str) -> Vec<String> {
    (0..n).map(|i| format!("{prefix}{i}.o")).collect()
}

// ── filter_batch with a fake query ────────────────────────────

#[test]
fn directory_decision_is_asked_once_for_a_thousand_paths() {
    let mut dir_queries: Vec<String> = Vec::new();
    let mut file_queries = 0usize;
    let mut q = |p: &str| -> Result<bool, String> {
        if p.ends_with('/') {
            dir_queries.push(p.to_owned());
            Ok(p == "out/")
        } else {
            file_queries += 1;
            Ok(false)
        }
    };
    let mut cache = IgnoreCache::new();
    let f = filter_batch(
        "/w",
        keys(1000, "out/a/"),
        &mut cache,
        Some(&mut q as &mut IgnoreQuery<'_>),
    );
    assert_eq!(f.survivors, Vec::<String>::new());
    assert_eq!(f.ignored, 1000);
    assert_eq!(dir_queries, ["out/"]);
    assert_eq!(file_queries, 0);
}

#[test]
fn query_errors_keep_the_path_and_directory_errors_are_not_cached() {
    let mut cache = IgnoreCache::new();
    let mut fail = |_: &str| -> Result<bool, String> { Err("boom".into()) };
    let f = filter_batch(
        "/w",
        vec!["out/a.o".to_owned(), "x.txt".to_owned()],
        &mut cache,
        Some(&mut fail as &mut IgnoreQuery<'_>),
    );
    assert_eq!(f.survivors, ["out/a.o", "x.txt"]);
    assert_eq!(f.ignored, 0);
    assert!(cache.dirs.is_empty(), "an error must not be remembered");

    // A later healthy query still gets to decide.
    let mut ok = |p: &str| -> Result<bool, String> { Ok(p == "out/") };
    assert!(cache.is_ignored("out/a.o", &mut ok));
    assert_eq!(cache.dirs.get("out"), Some(&true));
}

#[test]
fn git_paths_are_dropped_and_counted() {
    let mut cache = IgnoreCache::new();
    let mut never = |_: &str| -> Result<bool, String> { Ok(false) };
    let f = filter_batch(
        "/w",
        vec![
            ".git/HEAD".to_owned(),
            ".git/objects/ab/cd".to_owned(),
            "src/a.rs".to_owned(),
            "src/a.rs".to_owned(), // duplicate
        ],
        &mut cache,
        Some(&mut never as &mut IgnoreQuery<'_>),
    );
    assert_eq!(f.under_git, 2);
    assert_eq!(f.survivors, ["src/a.rs"]);
}

// ── real repos ────────────────────────────────────────────────

#[test]
fn an_all_ignored_batch_inserts_nothing() {
    let fx = Fx::git("out/\n");
    let names: Vec<String> = (0..20).map(|i| format!("out/debug/f{i}.o")).collect();
    for r in &names {
        fx.write(r, "x");
    }
    let strs: Vec<&str> = names.iter().map(String::as_str).collect();

    let repo = skein_git::Repo::open(Path::new(&fx.cwd)).unwrap();
    let mut q = |p: &str| repo.is_path_ignored(p).map_err(|e| e.to_string());
    let mut cache = IgnoreCache::new();
    let f = filter_batch(
        &fx.cwd,
        fx.reported(&strs),
        &mut cache,
        Some(&mut q as &mut IgnoreQuery<'_>),
    );
    assert_eq!(f.survivors, Vec::<String>::new());
    assert_eq!(f.ignored, 20);

    let mut d = Discovery::new(ROOM, &fx.cwd);
    assert_eq!(d.discover(&fx.db, fx.reported(&strs)), 0);
    assert_eq!(fx.paths(), Vec::<String>::new());
}

#[test]
fn a_mixed_batch_baselines_only_the_new_unignored_paths() {
    let fx = Fx::git("target/\n*.log\n");
    let repo = git2::Repository::open(&fx.cwd).unwrap();
    fx.write("pre.txt", "base\n");
    commit(&repo);

    // An existing baseline, via a first discover.
    fx.write("pre.txt", "edited once\n");
    let mut d = Discovery::new(ROOM, &fx.cwd);
    assert_eq!(d.discover(&fx.db, fx.reported(&["pre.txt"])), 1);
    let before = fx.db.review_baseline(ROOM, "pre.txt").unwrap().unwrap();
    assert_eq!(before.content.as_deref(), Some("base\n"));

    fx.write("pre.txt", "edited twice\n");
    fx.write("tracked.txt", "two\n");
    fx.write("fresh.txt", "new\n");
    fx.write("target/x.o", "x");
    fx.write("build.log", "x");

    let n = d.discover(
        &fx.db,
        fx.reported(&[
            "target/x.o",
            "build.log",
            ".git/HEAD",
            "pre.txt",
            "tracked.txt",
            "fresh.txt",
        ]),
    );
    assert_eq!(n, 2);
    assert_eq!(fx.paths(), ["fresh.txt", "pre.txt", "tracked.txt"]);
    let after = fx.db.review_baseline(ROOM, "pre.txt").unwrap().unwrap();
    assert_eq!(after.kind, before.kind);
    assert_eq!(after.content, before.content);
    assert_eq!(after.captured_ms, before.captured_ms);
}

#[test]
fn editing_gitignore_invalidates_the_cached_decisions() {
    let fx = Fx::git("out/\n");
    let mut d = Discovery::new(ROOM, &fx.cwd);
    fx.write("out/x", "1");
    assert_eq!(d.discover(&fx.db, fx.reported(&["out/x"])), 0);
    assert_eq!(fx.paths(), Vec::<String>::new());

    fx.write(".gitignore", "other/\n");
    // Without the .gitignore in the batch the stale decision stands
    // (until the TTL), which is the documented contract.
    assert_eq!(d.discover(&fx.db, fx.reported(&["out/x"])), 0);
    assert_eq!(fx.paths(), Vec::<String>::new());

    assert!(d.discover(&fx.db, fx.reported(&[".gitignore", "out/x"])) >= 1);
    assert!(fx.paths().contains(&"out/x".to_owned()));
}

#[test]
fn a_deleted_ignored_directory_is_still_ignored() {
    let fx = Fx::git("gen/\n");
    fx.write("gen/x", "1");
    fs::remove_dir_all(fx.dir.path().join("gen")).unwrap();
    let mut d = Discovery::new(ROOM, &fx.cwd);
    assert_eq!(d.discover(&fx.db, fx.reported(&["gen/x", "gen"])), 0);
    assert_eq!(fx.paths(), Vec::<String>::new());
}

#[test]
fn a_non_git_room_still_baselines_new_files() {
    let fx = Fx::bare();
    fx.write("a.txt", "hi\n");
    let mut d = Discovery::new(ROOM, &fx.cwd);
    assert_eq!(d.discover(&fx.db, fx.reported(&["a.txt"])), 1);
    assert_eq!(fx.paths(), ["a.txt"]);
}

#[test]
fn five_thousand_ignored_paths_are_cheap() {
    let fx = Fx::git("out/\n");
    let names = keys(5000, "out/debug/deps/f");
    let strs: Vec<&str> = names.iter().map(String::as_str).collect();
    let reported = fx.reported(&strs);

    let mut d = Discovery::new(ROOM, &fx.cwd);
    let t = Instant::now();
    let n = d.discover(&fx.db, reported);
    let new_ms = t.elapsed().as_millis();
    assert_eq!(n, 0);
    assert_eq!(fx.paths(), Vec::<String>::new());
    assert!(new_ms < 5000, "took {new_ms} ms");

    let repo = skein_git::Repo::open(Path::new(&fx.cwd)).unwrap();
    let t = Instant::now();
    let ignored = names
        .iter()
        .filter(|r| repo.is_path_ignored(r).unwrap())
        .count();
    let naive_ms = t.elapsed().as_millis();
    assert_eq!(ignored, 5000);
    eprintln!(
        "discovery timing: batched {new_ms} ms, naive per-path is_path_ignored {naive_ms} ms (5000 paths)"
    );
}

// ── default build-output dirs and the per-batch cap (#658) ────

#[test]
fn default_ignored_dirs_drop_without_a_git_query() {
    let mut cache = IgnoreCache::new();
    let f = filter_batch(
        "/w",
        [
            "target/debug/x.o",
            "crates/a/node_modules/p/i.js",
            "Build/out.txt",
            ".venv/lib/x.py",
            "src/build.rs",
            "src/target.txt",
            "docs/building/x.md",
            "src/build",
        ]
        .map(str::to_owned),
        &mut cache,
        None,
    );
    assert_eq!(f.default_ignored, 4);
    assert_eq!(f.ignored, 0);
    assert_eq!(
        f.survivors,
        [
            "src/build.rs",
            "src/target.txt",
            "docs/building/x.md",
            "src/build"
        ]
    );
}

#[test]
fn a_non_git_room_skips_build_output() {
    let fx = Fx::bare();
    fx.write("target/debug/x.o", "x");
    fx.write("node_modules/p/i.js", "x");
    fx.write("keep.txt", "k");
    let mut d = Discovery::new(ROOM, &fx.cwd);
    let n = d.discover(
        &fx.db,
        fx.reported(&["target/debug/x.o", "node_modules/p/i.js", "keep.txt"]),
    );
    assert_eq!(n, 1);
    assert_eq!(fx.paths(), ["keep.txt"]);
}

#[test]
fn the_cap_stops_work_on_the_excess() {
    let fx = Fx::bare();
    let names = ["a.txt", "b.txt", "c.txt"];
    for n in names {
        fx.write(n, "x");
    }
    let survivors: Vec<String> = names.iter().map(|s| (*s).to_owned()).collect();
    let (out, already, _) = capture_survivors(&fx.db, ROOM, &fx.cwd, None, &survivors, 2);
    assert_eq!(out.inserted, 2);
    assert_eq!(out.capped, 1);
    assert_eq!(already, 0);
    assert_eq!(fx.paths(), ["a.txt", "b.txt"]);
}

#[test]
fn catch_up_is_not_capped_where_a_live_batch_is() {
    let fx = Fx::git("");
    let names = ["a.txt", "b.txt", "c.txt"];
    for n in names {
        fx.write(n, "x");
    }
    let mut d = Discovery::new(ROOM, &fx.cwd);
    d.ensure_repo();
    let out = d.run(&fx.db, fx.reported(&names[..2]), true, 1);
    assert_eq!((out.inserted, out.capped), (1, 1));
    assert_eq!(d.catch_up(&fx.db), 2);
    assert_eq!(fx.paths(), ["a.txt", "b.txt", "c.txt"]);
}

#[test]
fn a_gitignore_inside_build_output_does_not_clear_the_cache() {
    let mut cache = IgnoreCache::new();
    cache.dirs.insert("out".to_owned(), true);
    let batch = |k: &str| vec![format!("/w/{k}")];
    filter_batch("/w", batch("node_modules/p/.gitignore"), &mut cache, None);
    assert_eq!(cache.dirs.get("out"), Some(&true));
    filter_batch("/w", batch("sub/.gitignore"), &mut cache, None);
    assert!(cache.dirs.is_empty());
}

#[test]
fn a_reported_path_gone_from_disk_and_head_gets_no_row() {
    let fx = Fx::git("");
    let mut d = Discovery::new(ROOM, &fx.cwd);
    assert_eq!(d.discover(&fx.db, fx.reported(&["ghost.txt"])), 0);
    assert_eq!(fx.paths(), Vec::<String>::new());
}

#[test]
fn a_tracked_file_deleted_from_disk_is_baselined() {
    let fx = Fx::git("");
    fs::remove_file(fx.dir.path().join("tracked.txt")).unwrap();
    let mut d = Discovery::new(ROOM, &fx.cwd);
    assert_eq!(d.discover(&fx.db, fx.reported(&["tracked.txt"])), 1);
    let b = fx.db.review_baseline(ROOM, "tracked.txt").unwrap().unwrap();
    assert_eq!(b.content.as_deref(), Some("one\n"));
}
