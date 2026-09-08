//! Smoke test against the real stores on this machine: list every
//! Claude Code session with its per-model usage and Claude's own cost
//! total, then the most recent opencode root sessions with their
//! subagent trees.
//!
//! ```sh
//! cargo run -p skein-harness --example sessions
//! ```

use skein_harness::{claude, home_dir, opencode};

fn main() {
    let Some(home) = home_dir() else {
        eprintln!("no home dir");
        return;
    };

    println!("== Claude Code ({})", claude::projects_dir(&home).display());
    let mut sessions = claude::list_sessions(&home);
    sessions.sort_by(|a, b| a.path.cmp(&b.path));
    let mut grand_total = 0.0;
    for s in &sessions {
        let Ok(rows) = claude::rows(&s.path) else {
            continue;
        };
        let summary = claude::summarize(rows);
        let cost = summary
            .cost_state
            .as_ref()
            .map_or(0.0, |c| c.total_cost_usd);
        grand_total += cost;
        let subagents = s.subagent_files().len();
        println!(
            "{}  {:<38} ${:>8.2}  subagents={subagents}",
            s.project_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?"),
            s.session_id,
            cost
        );
        for (model, usage) in &summary.usage_by_model {
            println!(
                "    {model:<24} responses={:>4}  in={:>7} cache_w={:>9} cache_r={:>11} out={:>7}",
                summary.responses_by_model.get(model).copied().unwrap_or(0),
                usage.input,
                usage.cache_creation,
                usage.cache_read,
                usage.output
            );
        }
    }
    println!(
        "sessions={}  total reported by Claude=${grand_total:.2}\n",
        sessions.len()
    );

    let db = opencode::db_path(&home);
    println!("== opencode ({})", db.display());
    let conn = match opencode::open_read_only(&db) {
        Ok(c) => c,
        Err(e) => {
            println!("not readable: {e}");
            return;
        }
    };
    let roots = opencode::root_sessions(&conn, None).unwrap_or_default();
    for root in roots.iter().take(5) {
        let tree = opencode::session_tree(&conn, &root.id).unwrap_or_default();
        for (depth, s) in tree {
            println!(
                "{}{:<14} {:<40} ${:>7.3} cache_r={:>9} {}",
                "  ".repeat(depth),
                s.agent.as_deref().unwrap_or("-"),
                s.model_ref().unwrap_or_default(),
                s.cost,
                s.tokens.cache_read,
                s.title.chars().take(40).collect::<String>()
            );
        }
    }
    let msgs = opencode::assistant_messages(&conn, None, None).unwrap_or_default();
    println!("roots={}  assistant messages={}", roots.len(), msgs.len());
}
