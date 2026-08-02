// SQL gate — defense layers 1/2 (PGAssistant `sql_gate` port, upgraded).
//
// Uses sqlparser's TOKENIZER (not a full parser) so arbitrary valid SQL is
// passed through untouched, while strings/comments/identifiers are recognised
// structurally — which is exactly what the text-heuristics of the original
// PRD had to fake. Fail-closed: anything the tokenizer cannot understand is
// handed to the keyword gate as a single statement and rejected in readonly
// mode if it does not start with a query keyword.
//
// Statements are SPLIT on top-level semicolons (paren aware) and their text
// is RECONSTRUCTED from the significant tokens (whitespace/comments dropped,
// a single space inserted between tokens — except around `.` so qualified
// names stay `schema.table`). The result is semantically identical, valid SQL.
//
// Layer 1 — whitelist: every statement must start SELECT/WITH/EXPLAIN/SHOW.
// Layer 2 — denylist: EXPLAIN ANALYZE, SELECT INTO, writable CTEs and a
//   catalogue of server-side hazard functions (pg_sleep, setval, dblink…).
// Layers 3/4 live in the driver (pg.rs): server readonly + statement timeout.

use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::tokenizer::{Token, Tokenizer};

fn tokenize(sql: &str) -> Vec<Token> {
    Tokenizer::new(&PostgreSqlDialect::default(), sql)
        .tokenize()
        .unwrap_or_default()
}

/// Significant tokens (all whitespace/comments removed).
fn significant(sql: &str) -> Vec<Token> {
    tokenize(sql)
        .into_iter()
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .collect()
}

/// Rejoin tokens into SQL text: a space between tokens except around `.`
/// (periods must hug so `public.orders` stays valid).
fn join_tokens(tokens: &[Token]) -> String {
    let mut out = String::new();
    let mut prev_period = false;
    for t in tokens {
        let is_period = matches!(t, Token::Period);
        if !out.is_empty() && !is_period && !prev_period {
            out.push(' ');
        }
        out.push_str(&t.to_string());
        prev_period = is_period;
    }
    out
}

/// Split `sql` on top-level semicolons (string/comment/paren aware). Pure
/// whitespace/comment input yields an empty list.
pub fn split_statements(sql: &str) -> Vec<String> {
    let tokens = tokenize(sql);
    if tokens.is_empty() {
        // Tokenizer hiccup: treat the whole input as one statement so the
        // caller's keyword gate can still reject it (fail closed).
        let t = sql.trim();
        return if t.is_empty() { Vec::new() } else { vec![t.to_string()] };
    }
    let mut out: Vec<String> = Vec::new();
    let mut cur: Vec<Token> = Vec::new();
    let mut depth: usize = 0;
    for t in tokens {
        match t {
            Token::Whitespace(_) => {}
            Token::SemiColon if depth == 0 => {
                if !cur.is_empty() {
                    out.push(join_tokens(&cur));
                    cur.clear();
                }
            }
            Token::LParen => {
                depth += 1;
                cur.push(t);
            }
            Token::RParen => {
                depth = depth.saturating_sub(1);
                cur.push(t);
            }
            other => cur.push(other),
        }
    }
    if !cur.is_empty() {
        out.push(join_tokens(&cur));
    }
    out
}

/// All Word tokens of a statement, lowercased (strings/comments excluded —
/// a string containing `pg_sleep` is NOT flagged).
fn words_of(sql: &str) -> Vec<String> {
    tokenize(sql)
        .into_iter()
        .filter_map(|t| match t {
            Token::Word(w) => Some(w.value.to_lowercase()),
            _ => None,
        })
        .collect()
}

/// First significant keyword (lowercased), or None when empty/unparseable.
pub fn first_keyword(sql: &str) -> Option<String> {
    words_of(sql).into_iter().next()
}

const QUERY_KEYWORDS: [&str; 4] = ["select", "with", "explain", "show"];

/// Layer 1 whitelist: does the statement start with a query keyword?
pub fn is_readonly_sql(sql: &str) -> bool {
    first_keyword(sql)
        .map(|k| QUERY_KEYWORDS.contains(&k.as_str()))
        .unwrap_or(false)
}

const HAZARD_FUNCTIONS: [&str; 20] = [
    "pg_sleep",
    "pg_terminate_backend",
    "pg_cancel_backend",
    "pg_reload_conf",
    "pg_rotate_logfile",
    "pg_read_file",
    "pg_read_binary_file",
    "pg_write_file",
    "pg_write_binary_file",
    "pg_ls_dir",
    "pg_stat_file",
    "pg_readlink",
    "pg_readdir",
    "pg_execute_server_program",
    "pg_signal_backend",
    "lo_import",
    "lo_export",
    "setval",
    "nextval",
    "pg_logdir_ls",
];

/// Layer 2 denylist: known attack/abuse patterns. Returns a Chinese message
/// describing the first violation, or None when the statement is clean.
pub fn readonly_danger(sql: &str) -> Option<String> {
    let tokens = tokenize(sql);
    if tokens.is_empty() {
        return Some("无法解析的 SQL".to_string());
    }
    let words: Vec<String> = tokens
        .iter()
        .filter_map(|t| match t {
            Token::Word(w) => Some(w.value.to_lowercase()),
            _ => None,
        })
        .collect();

    // EXPLAIN ANALYZE actually executes the statement.
    if words.first().map(String::as_str) == Some("explain")
        && words.get(1).map(String::as_str) == Some("analyze")
    {
        return Some("EXPLAIN ANALYZE 会真实执行语句，已拦截".to_string());
    }

    // SELECT ... INTO creates a table (a write).
    for (i, w) in words.iter().enumerate() {
        if w == "select" {
            for later in &words[i + 1..] {
                if later == "into" {
                    return Some("SELECT INTO 是建表写操作，已拦截".to_string());
                }
                if later == "from" {
                    break;
                }
            }
        }
    }

    // WITH ... (INSERT/UPDATE/DELETE/MERGE …) — a writable CTE.
    if words.first().map(String::as_str) == Some("with") {
        let mut depth: usize = 0;
        for t in tokens {
            match t {
                Token::LParen => depth += 1,
                Token::RParen => depth = depth.saturating_sub(1),
                Token::Word(w) => {
                    let w = w.value.to_lowercase();
                    if depth == 1
                        && matches!(w.as_str(), "insert" | "update" | "delete" | "merge")
                    {
                        return Some(format!("可写 CTE（WITH 内含 {w}）已拦截"));
                    }
                }
                _ => {}
            }
        }
    }

    // Server-side hazard functions (also matches the dblink* family).
    for w in &words {
        if w.starts_with("dblink") {
            return Some(format!("高危函数 {w} 已拦截"));
        }
        if HAZARD_FUNCTIONS.contains(&w.as_str()) {
            return Some(format!("高危函数 {w} 已拦截"));
        }
    }

    None
}

/// Auto-append `LIMIT {limit}` to a SELECT/WITH that has no explicit LIMIT.
/// Returns the new text and whether a limit was appended.
pub fn apply_default_limit(stmt: &str, limit: u32) -> (String, bool) {
    let words = words_of(stmt);
    let Some(first) = words.first() else {
        return (stmt.to_string(), false);
    };
    if first != "select" && first != "with" {
        return (stmt.to_string(), false);
    }
    if words.iter().any(|w| w == "limit") {
        return (stmt.to_string(), false);
    }
    (format!("{}\nLIMIT {limit}", stmt.trim_end()), true)
}

/// Full readonly gate for a batch: whitelist + denylist per statement.
/// In `allow_write` mode nothing is rejected (the write confirm dialog is the
/// caller's concern); returned statements are the gate-approved split.
pub fn check_batch(sql: &str, allow_write: bool) -> Result<Vec<String>, String> {
    if sql.trim().is_empty() {
        return Err("SQL 为空".to_string());
    }
    let stmts = split_statements(sql);
    if stmts.is_empty() {
        return Err("SQL 为空（仅包含注释）".to_string());
    }
    let mut out = Vec::with_capacity(stmts.len());
    for s in stmts {
        if !allow_write {
            if !is_readonly_sql(&s) {
                return Err(format!(
                    "只读模式拒绝：语句必须以 SELECT / WITH / EXPLAIN / SHOW 开头\n{}",
                    preview(&s)
                ));
            }
            if let Some(msg) = readonly_danger(&s) {
                return Err(format!("只读模式拒绝：{msg}\n{}", preview(&s)));
            }
        }
        out.push(s);
    }
    Ok(out)
}

/// Gate + auto `LIMIT 10`. This is what commands/MCP feed to the driver.
pub fn prepare_batch(sql: &str, allow_write: bool) -> Result<Vec<String>, String> {
    Ok(check_batch(sql, allow_write)?
        .into_iter()
        .map(|s| apply_default_limit(&s, 10).0)
        .collect())
}

/// Does the batch contain write / DDL statements? (frontend confirm dialog)
pub fn contains_write(stmts: &[String]) -> bool {
    const WRITE: [&str; 9] = [
        "insert", "update", "delete", "create", "drop", "alter", "truncate", "merge", "copy",
    ];
    stmts.iter()
        .any(|s| first_keyword(s).map(|k| WRITE.contains(&k.as_str())).unwrap_or(false))
}

/// Extract the SQL from an AI reply: prefer the first ```sql block, else None.
pub fn extract_sql(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    if let Some(start) = lower.find("```sql") {
        let after = &text[start + 6..];
        let end = after.find("```").unwrap_or(after.len());
        let sql = after[..end].trim();
        return if sql.is_empty() { None } else { Some(sql.to_string()) };
    }
    None
}

fn preview(s: &str) -> String {
    let one = s.lines().next().unwrap_or(s).trim();
    if one.chars().count() > 60 {
        one.chars().take(60).collect::<String>() + "…"
    } else {
        one.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_basic() {
        let parts = split_statements("SELECT 1; SELECT 2;");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], "SELECT 1");
        assert_eq!(parts[1], "SELECT 2");
    }

    #[test]
    fn split_string_semicolon() {
        // Semicolon inside a string must not split.
        let parts = split_statements("SELECT 'a;b' AS x;");
        assert_eq!(parts.len(), 1);
    }

    #[test]
    fn split_keeps_qualified_names() {
        let parts = split_statements("SELECT public.orders.id FROM public.orders;");
        assert_eq!(parts.len(), 1);
        assert!(parts[0].contains("public.orders.id"), "{}", parts[0]);
    }

    #[test]
    fn split_strips_comments() {
        let parts = split_statements("-- hello\nSELECT 1;");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0], "SELECT 1");
    }

    #[test]
    fn pure_comment_is_empty() {
        let parts = split_statements("-- just a comment\n/* and another */");
        assert!(parts.is_empty());
    }

    #[test]
    fn readonly_whitelist() {
        assert!(is_readonly_sql("SELECT * FROM t"));
        assert!(is_readonly_sql("-- c\nWITH x AS (SELECT 1) SELECT * FROM x"));
        assert!(is_readonly_sql("EXPLAIN SELECT 1"));
        assert!(!is_readonly_sql("DELETE FROM t"));
        assert!(!is_readonly_sql("INSERT INTO t VALUES (1)"));
        assert!(!is_readonly_sql("CREATE TABLE t (id int)"));
    }

    #[test]
    fn danger_explain_analyze() {
        assert!(readonly_danger("EXPLAIN ANALYZE DELETE FROM t").is_some());
    }

    #[test]
    fn danger_select_into() {
        assert!(readonly_danger("SELECT * INTO b FROM a").is_some());
    }

    #[test]
    fn danger_writable_cte() {
        assert!(readonly_danger("WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d").is_some());
        assert!(readonly_danger("WITH c AS (SELECT 1) SELECT * FROM c").is_none());
    }

    #[test]
    fn danger_functions() {
        assert!(readonly_danger("SELECT pg_sleep(3600)").is_some());
        assert!(readonly_danger("SELECT setval('s', 1)").is_some());
        assert!(readonly_danger("SELECT * FROM dblink('x','SELECT 1') t(a int)").is_some());
        // A string containing a hazard word must NOT be flagged.
        assert!(readonly_danger("SELECT 'pg_sleep' AS x").is_none());
    }

    #[test]
    fn default_limit_applied() {
        assert_eq!(apply_default_limit("SELECT * FROM t", 10).1, true);
        assert_eq!(apply_default_limit("SELECT * FROM t LIMIT 5", 10).1, false);
        assert_eq!(apply_default_limit("SHOW ALL", 10).1, false);
    }

    #[test]
    fn batch_gates() {
        assert!(prepare_batch("SELECT 1; DELETE FROM t", false).is_err());
        assert!(prepare_batch("SELECT 1", false).is_ok());
        assert!(prepare_batch("DELETE FROM t", true).is_ok());
        assert_eq!(
            prepare_batch("-- only comment", false).unwrap_err(),
            "SQL 为空（仅包含注释）"
        );
    }

    #[test]
    fn write_detection() {
        assert!(contains_write(&["INSERT INTO t VALUES (1)".to_string()]));
        assert!(!contains_write(&["SELECT 1".to_string()]));
    }

    #[test]
    fn extract_code_block() {
        assert_eq!(
            extract_sql("好的：\n```sql\nSELECT 1\n```\n还有别的").unwrap().trim(),
            "SELECT 1"
        );
        assert_eq!(extract_sql("没有代码块"), None);
    }
}
