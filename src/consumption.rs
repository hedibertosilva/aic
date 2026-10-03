//! Two compact counters, not a turn ledger. Deleting UI replies cannot refund usage.
use std::collections::HashMap;
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use crate::chat::Usage;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Total {
    pub usage: Usage,
    pub turns: u64,
    pub missing_usage: u64,
    /// Per-field coverage, so a partial cost isn't presented as a full total.
    pub reports: HashMap<String, u64>,
}

#[derive(Clone, Copy)]
pub struct SessionCounter<'a> { pub id: &'a str, pub resumed: bool }

pub fn init(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS chat_consumption (
        conversation TEXT NOT NULL, agent TEXT NOT NULL, total_json TEXT NOT NULL,
        PRIMARY KEY (conversation, agent));
        CREATE TABLE IF NOT EXISTS chat_usage_baselines (
        conversation TEXT NOT NULL, agent TEXT NOT NULL, session TEXT NOT NULL,
        usage_json TEXT NOT NULL, PRIMARY KEY (conversation, agent, session));")?;
    Ok(())
}

pub fn load(db: &Connection, conversation: &str) -> Result<HashMap<String, Total>> {
    let mut statement = db.prepare("SELECT agent, total_json FROM chat_consumption WHERE conversation = ?1")?;
    let mut totals = HashMap::new();
    for row in statement.query_map([conversation], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (agent, raw) = row?;
        totals.insert(agent, serde_json::from_str(&raw)?);
    }
    Ok(totals)
}

/// Baselines apply to cumulative native-session results: agy's and Codex's
/// (`exec resume` reports session totals, checked with Codex 0.160.0). Claude's
/// one-process-per-print results are per invocation.
pub fn delta(current: &Usage, previous: Option<&Usage>) -> Usage {
    let Some(previous) = previous else { return current.clone() };
    // A decreasing counter means the native counter reset. Count this run,
    // rather than clamping a new session's usage to zero.
    if counter_reset(current, previous) { return current.clone(); }
    let sub = |a: Option<u64>, b: Option<u64>| a.map(|n| n.saturating_sub(b.unwrap_or(0)));
    Usage {
        input_tokens: sub(current.input_tokens, previous.input_tokens),
        output_tokens: sub(current.output_tokens, previous.output_tokens),
        cache_read_tokens: sub(current.cache_read_tokens, previous.cache_read_tokens),
        cache_write_tokens: sub(current.cache_write_tokens, previous.cache_write_tokens),
        cost_usd: current.cost_usd.map(|n| (n - previous.cost_usd.unwrap_or(0.0)).max(0.0)),
        num_turns: sub(current.num_turns, previous.num_turns),
    }
}

fn counter_reset(current: &Usage, previous: &Usage) -> bool {
    [(current.input_tokens, previous.input_tokens), (current.output_tokens, previous.output_tokens),
        (current.cache_read_tokens, previous.cache_read_tokens), (current.cache_write_tokens, previous.cache_write_tokens)]
        .iter().any(|(a, b)| matches!((a, b), (Some(a), Some(b)) if a < b))
        || matches!((current.num_turns, previous.num_turns), (Some(a), Some(b)) if a <= b)
        || matches!((current.cost_usd, previous.cost_usd), (Some(a), Some(b)) if a < b)
}

/// The server calls this once per actual invocation, even on disconnect/failure.
/// Updating the aggregate and its cumulative baseline is one SQLite transaction.
pub fn record(db: &mut Connection, conversation: &str, agent: &str, session: Option<SessionCounter<'_>>, cumulative: bool, usage: Option<&Usage>) -> Result<Option<Usage>> {
    let transaction = db.transaction()?;
    let raw: Option<String> = transaction.query_row(
        "SELECT total_json FROM chat_consumption WHERE conversation=?1 AND agent=?2",
        params![conversation, agent], |r| r.get(0)).optional()?;
    let mut total: Total = raw.map(|s| serde_json::from_str(&s)).transpose()?.unwrap_or_default();
    let previous: Option<Usage> = if cumulative && session.is_some() {
        transaction.query_row("SELECT usage_json FROM chat_usage_baselines WHERE conversation=?1 AND agent=?2 AND session=?3",
            params![conversation, agent, session.map(|s| s.id)], |r| r.get::<_, String>(0)).optional()?
            .map(|s| serde_json::from_str(&s)).transpose()?
    } else { None };
    // A legacy resumed session has no baseline: its cumulative total includes
    // earlier, unattributed usage. Start a baseline, without charging history.
    let baseline_only = cumulative && previous.is_none() && session.is_some_and(|s| s.resumed)
        && usage.is_some_and(|u| u.num_turns != Some(1));
    let observed = usage.filter(|_| !baseline_only).map(|u| if cumulative { delta(u, previous.as_ref()) } else { u.clone() });
    total.turns += 1;
    if let Some(usage) = &observed {
        macro_rules! add {
            ($field:ident) => { if let Some(value) = usage.$field {
                total.usage.$field = Some(total.usage.$field.unwrap_or_default() + value);
                *total.reports.entry(stringify!($field).into()).or_default() += 1;
            } };
        }
        add!(input_tokens); add!(output_tokens); add!(cache_read_tokens);
        add!(cache_write_tokens); add!(cost_usd); add!(num_turns);
    }
    if observed.as_ref().is_none_or(|u| u.input_tokens.is_none() && u.output_tokens.is_none() && u.cost_usd.is_none()) {
        total.missing_usage += 1;
    }
    transaction.execute("INSERT INTO chat_consumption VALUES (?1,?2,?3)
        ON CONFLICT(conversation,agent) DO UPDATE SET total_json=excluded.total_json",
        params![conversation, agent, serde_json::to_string(&total)?])?;
    if cumulative && session.is_some() && usage.is_some() {
        let current = usage.unwrap();
        let previous = previous.filter(|p| !counter_reset(current, p)).unwrap_or_default();
        // Providers may omit a field on an intermediate result. Keep its
        // last baseline so its next cumulative reading isn't counted twice.
        let baseline = Usage {
            input_tokens: current.input_tokens.or(previous.input_tokens),
            output_tokens: current.output_tokens.or(previous.output_tokens),
            cache_read_tokens: current.cache_read_tokens.or(previous.cache_read_tokens),
            cache_write_tokens: current.cache_write_tokens.or(previous.cache_write_tokens),
            cost_usd: current.cost_usd.or(previous.cost_usd),
            num_turns: current.num_turns.or(previous.num_turns),
        };
        transaction.execute("INSERT INTO chat_usage_baselines VALUES (?1,?2,?3,?4)
            ON CONFLICT(conversation,agent,session) DO UPDATE SET usage_json=excluded.usage_json",
            params![conversation, agent, session.map(|s| s.id), serde_json::to_string(&baseline)?])?;
    }
    transaction.commit()?;
    Ok(observed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn counters_survive_deleted_replies_and_are_separate_for_agents_and_copies() {
        let mut db = Connection::open_in_memory().unwrap(); init(&db).unwrap();
        let usage = Usage { input_tokens: Some(100), output_tokens: Some(10), ..Default::default() };
        record(&mut db, "conversation", "codex", None, false, Some(&usage)).unwrap();
        record(&mut db, "conversation", "codex", None, false, None).unwrap();
        record(&mut db, "conversation", "claude", None, false, Some(&usage)).unwrap();
        let totals = load(&db, "conversation").unwrap();
        assert_eq!(totals["codex"].usage.input_tokens, Some(100));
        assert_eq!(totals["codex"].usage.cost_usd, None);
        assert_eq!(totals["codex"].missing_usage, 1);
        assert_eq!(totals["claude"].turns, 1);
        assert!(load(&db, "copy").unwrap().is_empty());
    }
    #[test]
    fn cumulative_results_count_deltas_and_reset_without_double_counting() {
        let mut db = Connection::open_in_memory().unwrap(); init(&db).unwrap();
        for (tokens, turns) in [(100,1),(150,2),(20,1)] {
            let usage = Usage { input_tokens: Some(tokens), num_turns: Some(turns), ..Default::default() };
            record(&mut db,"c","gemini",Some(SessionCounter { id:"native",resumed:false }),true,Some(&usage)).unwrap();
        }
        assert_eq!(load(&db,"c").unwrap()["gemini"].usage.input_tokens,Some(170));
    }
    #[test]
    fn totals_and_baselines_survive_sqlite_reopen() {
        let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("chat.sqlite3");
        { let mut db = Connection::open(&path).unwrap(); init(&db).unwrap();
          record(&mut db,"c","gemini",Some(SessionCounter { id:"s",resumed:false }),true,Some(&Usage { input_tokens:Some(100), ..Default::default() })).unwrap(); }
        let mut db = Connection::open(&path).unwrap(); init(&db).unwrap();
        record(&mut db,"c","gemini",Some(SessionCounter { id:"s",resumed:true }),true,Some(&Usage { input_tokens:Some(150), ..Default::default() })).unwrap();
        assert_eq!(load(&db,"c").unwrap()["gemini"].usage.input_tokens,Some(150));
    }
    #[test]
    fn omitted_fields_keep_their_baseline_until_a_counter_reset() {
        let mut db = Connection::open_in_memory().unwrap(); init(&db).unwrap();
        let session = Some(SessionCounter { id:"s", resumed:false });
        for (input, cost, turns) in [(100,Some(1.0),1),(150,None,2),(200,Some(1.5),3),(10,None,1),(20,Some(0.1),2)] {
            record(&mut db,"c","gemini",session,true,Some(&Usage { input_tokens:Some(input),cost_usd:cost,num_turns:Some(turns),..Default::default() })).unwrap();
        }
        let total = &load(&db,"c").unwrap()["gemini"];
        assert_eq!(total.usage.input_tokens,Some(220));
        assert_eq!(total.usage.cost_usd,Some(1.6));
        assert_eq!(total.reports["cost_usd"],3);
    }
    #[test]
    fn per_process_turn_counts_do_not_subtract_previous_print_usage() {
        let old = Usage { input_tokens:Some(100),num_turns:Some(1),..Default::default() };
        let new = Usage { input_tokens:Some(120),num_turns:Some(1),..Default::default() };
        assert_eq!(delta(&new,Some(&old)).input_tokens,Some(120));
    }
    #[test]
    fn legacy_cumulative_history_is_not_charged_to_the_current_conversation() {
        let mut db = Connection::open_in_memory().unwrap(); init(&db).unwrap();
        let session = Some(SessionCounter { id:"legacy", resumed:true });
        record(&mut db,"c","gemini",session,true,Some(&Usage { input_tokens:Some(1000),num_turns:Some(5),..Default::default() })).unwrap();
        record(&mut db,"c","gemini",session,true,Some(&Usage { input_tokens:Some(1100),num_turns:Some(6),..Default::default() })).unwrap();
        let totals = load(&db,"c").unwrap();
        assert_eq!(totals["gemini"].usage.input_tokens,Some(100));
        assert_eq!(totals["gemini"].missing_usage,1);
    }
    #[test]
    fn codex_resume_counts_only_the_new_turn() {
        // Real `turn.completed` totals from two turns of one Codex session,
        // after Usage::from_codex splits cached input out.
        let mut db = Connection::open_in_memory().unwrap(); init(&db).unwrap();
        let turn = |input, cached, output| Usage { input_tokens:Some(input-cached), cache_read_tokens:Some(cached), output_tokens:Some(output), ..Default::default() };
        record(&mut db,"c","codex",Some(SessionCounter { id:"t",resumed:false }),true,Some(&turn(14172,12288,5))).unwrap();
        let second = record(&mut db,"c","codex",Some(SessionCounter { id:"t",resumed:true }),true,Some(&turn(28368,24576,10))).unwrap().unwrap();
        assert_eq!((second.input_tokens,second.cache_read_tokens,second.output_tokens),(Some(1908),Some(12288),Some(5)));
        let total = &load(&db,"c").unwrap()["codex"];
        assert_eq!((total.usage.input_tokens,total.usage.cache_read_tokens,total.usage.output_tokens),(Some(3792),Some(24576),Some(10)));
    }
}
