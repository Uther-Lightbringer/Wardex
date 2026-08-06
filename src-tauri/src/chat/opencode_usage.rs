// opencode 本地用量补源（kimi/claude/codex 的 wire.rs 档案之外的第四条
// 路，只对 provider == "opencode" 启用）。
//
// 背景：opencode 的 ACP prompt result 只报回合最后一次 LLM 调用的 usage
// （packages/opencode/src/acp/service.ts 的 promptResponse 取 session.prompt
// 返回的最后一条 assistant 消息），工具循环回合里前面 N-1 次调用的消耗
// 全部漏掉；usage_update 通知又只带 used/size/cost（无 usage 字段），
// 解析不到。而 opencode 把会话持久化在 SQLite：
//
// - 数据目录 = Global.Path.data（Windows %LOCALAPPDATA%\opencode，Unix
//   XDG_DATA_HOME/opencode）
// - db 文件名：OPENCODE_DB 环境变量 > opencode.db（稳定版）>
//   opencode-<channel>.db（local/dev 等）
// - session 表带整会话累计 token 列（tokens_input/output/reasoning/
//   cache_read/cache_write，迁移 20260510033149_session_usage），每条
//   assistant 消息落库即累加
//
// 所以：
// - 回合增量（OpencodeReader）：Started 时读一次累计值作基线，turnFinished
//   再读一次取差值 = 本回合全部 LLM 调用的消耗（比 prompt result 准）。
// - 回填（read_archive_full）：message 表逐条取 assistant 消息的 tokens 与
//   time_created（一条消息 = 一次 LLM 调用，粒度与 claude 档案一致）。
//
// 只读连接（WAL 由运行中的 agent 持有）；任何失败返回 None，回退到
// prompt result 路径，绝不影响回合本身。

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::acp::events::TurnUsage;
use crate::chat::wire::ArchiveRecord;

/// 数据目录：与 opencode Global.Path.data 一致（xdg-data/opencode）。
pub fn opencode_data_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("opencode"))
}

/// 候选 db 文件（优先级）：OPENCODE_DB 显式指定 > 稳定版 opencode.db >
/// 其他 channel 的 opencode-<channel>.db（local/dev）。
fn db_candidates(data_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(v) = std::env::var("OPENCODE_DB") {
        if !v.is_empty() {
            let p = if Path::new(&v).is_absolute() {
                PathBuf::from(v)
            } else {
                data_dir.join(v)
            };
            out.push(p);
        }
    }
    for name in ["opencode.db", "opencode-local.db", "opencode-dev.db"] {
        out.push(data_dir.join(name));
    }
    out
}

/// session 表的整会话累计 token 五元组 (input, output, reasoning,
/// cache_read, cache_write)；行不存在或查询失败返回 None。
fn session_totals(conn: &Connection, session_id: &str) -> Option<(u64, u64, u64, u64, u64)> {
    conn.query_row(
        "SELECT tokens_input, tokens_output, tokens_reasoning, tokens_cache_read, tokens_cache_write
         FROM session WHERE id = ?1",
        [session_id],
        |row| {
            let g = |i: usize| row.get::<_, i64>(i).unwrap_or(0).max(0) as u64;
            Ok((g(0), g(1), g(2), g(3), g(4)))
        },
    )
    .ok()
}

/// 拆分的五元组 → TurnUsage。input 口径与 wire.rs 一致：含缓存读写
/// （opencode 自己的 buildUsage 的 total 也含 reasoning + 缓存）。
fn usage_from_totals(input: u64, output: u64, reasoning: u64, cache_read: u64, cache_write: u64) -> TurnUsage {
    TurnUsage {
        input_tokens: input + cache_read + cache_write,
        output_tokens: output,
        total_tokens: input + output + reasoning + cache_read + cache_write,
        cached_read_tokens: (cache_read > 0).then_some(cache_read),
        cached_write_tokens: (cache_write > 0).then_some(cache_write),
        thought_tokens: (reasoning > 0).then_some(reasoning),
    }
}

/// message.data 的 assistant 条目 → 单次调用的用量；非 assistant 或无
/// tokens 返回 None。
fn parse_message_data(data: &str) -> Option<TurnUsage> {
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    if v.get("role").and_then(serde_json::Value::as_str) != Some("assistant") {
        return None;
    }
    let t = v.get("tokens")?;
    let get = |k: &str| t.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0);
    let input = get("input");
    let output = get("output");
    let reasoning = get("reasoning");
    let (cache_read, cache_write) = t
        .get("cache")
        .map(|c| {
            (
                c.get("read").and_then(serde_json::Value::as_u64).unwrap_or(0),
                c.get("write").and_then(serde_json::Value::as_u64).unwrap_or(0),
            )
        })
        .unwrap_or((0, 0));
    if input + output + reasoning + cache_read + cache_write == 0 {
        return None;
    }
    Some(usage_from_totals(input, output, reasoning, cache_read, cache_write))
}

/// 每会话一个增量读取器：Started 时记累计基线，之后每次 read_new 取差值
/// = 本回合新增的整会话消耗。session 表无 usage 迁移列（旧版 opencode）时
/// 差值恒为 0，此时退到 message 表按本回合新增 assistant 消息求和——
/// 一条 assistant 消息 = 一次 LLM 调用，同样是整回合全量（含工具循环）。
#[derive(Debug)]
pub struct OpencodeReader {
    data_dir: PathBuf,
    session_id: String,
    /// 已定位的 db 文件。
    db: Option<PathBuf>,
    /// 已定位时的整会话累计值（差值基准；Some 即迁移列有效）。
    baseline: Option<(u64, u64, u64, u64, u64)>,
    /// 已定位时该会话 message 表最大 rowid（message 求和兜底的基线）。
    baseline_msg_rowid: Option<i64>,
}

impl OpencodeReader {
    /// Started 时调用：定位含该会话的 db 并读取累计基线（resume 的
    /// 历史回合不计入）。
    pub fn locate(data_dir: PathBuf, session_id: &str) -> Self {
        let mut r = Self {
            data_dir,
            session_id: session_id.to_string(),
            db: None,
            baseline: None,
            baseline_msg_rowid: None,
        };
        r.resolve();
        r
    }

    /// 定位含该会话的 db；会话行还不存在时记下首个存在的 db 等 read_new。
    /// 命中即同时记 message 基线（迁移列缺失时 message 兜底用；历史
    /// 消息在 locate 时就被排除，只算本回合新增）。
    fn resolve(&mut self) -> Option<PathBuf> {
        if self.db.is_some() && (self.baseline.is_some() || self.baseline_msg_rowid.is_some()) {
            return self.db.clone();
        }
        for p in db_candidates(&self.data_dir) {
            if !p.is_file() {
                continue;
            }
            let Ok(conn) = Connection::open_with_flags(&p, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
                continue;
            };
            if let Some(t) = session_totals(&conn, &self.session_id) {
                self.db = Some(p.clone());
                self.baseline = Some(t);
                self.baseline_msg_rowid = Some(msg_max_rowid(&conn, &self.session_id));
                return self.db.clone();
            }
            let max = msg_max_rowid(&conn, &self.session_id);
            if max > 0 {
                self.db = Some(p.clone());
                self.baseline_msg_rowid = Some(max);
                return self.db.clone();
            }
        }
        self.db
            .get_or_insert_with(|| {
                db_candidates(&self.data_dir)
                    .into_iter()
                    .find(|p| p.is_file())
                    .unwrap_or_else(|| self.data_dir.join("opencode.db"))
            })
            .clone()
            .into()
    }

    /// 读取本回合新增的用量；无新增（或读失败）返回 None。
    pub fn read_new(&mut self) -> Option<TurnUsage> {
        let p = self.resolve()?;
        let conn = Connection::open_with_flags(&p, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
        let cur = session_totals(&conn, &self.session_id);
        match (self.baseline, cur) {
            // 迁移列有效：累计差值 = 本回合全量（含工具循环）。
            (Some(base), Some(cur)) => {
                let diff = (
                    cur.0.saturating_sub(base.0),
                    cur.1.saturating_sub(base.1),
                    cur.2.saturating_sub(base.2),
                    cur.3.saturating_sub(base.3),
                    cur.4.saturating_sub(base.4),
                );
                self.baseline = Some(cur);
                if diff.0 + diff.1 + diff.2 + diff.3 + diff.4 == 0 {
                    return None;
                }
                return Some(usage_from_totals(diff.0, diff.1, diff.2, diff.3, diff.4));
            }
            // totals 行异常消失：弃用 totals 基线，落 message 兜底。
            (Some(_), None) => {
                self.baseline = None;
                self.baseline_msg_rowid = Some(msg_max_rowid(&conn, &self.session_id));
                return None;
            }
            // 定位时会话行还不存在（极端时序）：首个可见窗口当基线，
            // 本回合不回补，下回合正常差值。
            (None, Some(cur)) => {
                self.baseline = Some(cur);
                return None;
            }
            // 迁移列缺失（旧版 opencode 无 session_usage 列）：message 表
            // 按新增 assistant 行求和兜底。
            (None, None) => self.sum_new_messages(&conn),
        }
    }

    /// message 表兜底：rowid > 基线的 assistant 消息逐条求和（一条消息 =
    /// 一次 LLM 调用，含工具循环与思考）。零新增时基线不推进，避免
    /// user 行先落盘、assistant 行后落盘时被窗口跳过。
    fn sum_new_messages(&mut self, conn: &Connection) -> Option<TurnUsage> {
        let cur_rowid = msg_max_rowid(conn, &self.session_id);
        let base = self.baseline_msg_rowid;
        let mut sum = TurnUsage::default();
        let mut found = false;
        let Ok(mut stmt) = conn.prepare(
            "SELECT data FROM message WHERE session_id = ?1 AND rowid > ?2",
        ) else {
            return None;
        };
        let Ok(rows) = stmt.query_map(
            rusqlite::params![self.session_id, base.unwrap_or(0)],
            |row| row.get::<_, String>(0),
        ) else {
            return None;
        };
        for row in rows {
            let Ok(data) = row else { continue };
            if let Some(u) = parse_message_data(&data) {
                sum.input_tokens += u.input_tokens;
                sum.output_tokens += u.output_tokens;
                sum.total_tokens += u.total_tokens;
                if let Some(c) = u.cached_read_tokens {
                    *sum.cached_read_tokens.get_or_insert(0) += c;
                }
                if let Some(c) = u.cached_write_tokens {
                    *sum.cached_write_tokens.get_or_insert(0) += c;
                }
                if let Some(c) = u.thought_tokens {
                    *sum.thought_tokens.get_or_insert(0) += c;
                }
                found = true;
            }
        }
        if found {
            self.baseline_msg_rowid = Some(cur_rowid);
            return Some(sum);
        }
        if base.is_none() {
            // 首个可见窗口：当前行数为基线，本回合不回补（极端时序）。
            self.baseline_msg_rowid = Some(cur_rowid);
        }
        None
    }
}

/// 该会话 message 表最大 rowid（0 = 无行）。
fn msg_max_rowid(conn: &Connection, session_id: &str) -> i64 {
    conn.query_row(
        "SELECT COALESCE(MAX(rowid), 0) FROM message WHERE session_id = ?1",
        [session_id],
        |row| row.get(0),
    )
    .unwrap_or(0)
}

/// 回填用全量解析：message 表逐条取 assistant 消息（一条 = 一次 LLM
/// 调用），ts 用消息自带 time_created（ms）。返回 (命中 db 数, 记录)。
pub fn read_archive_full(data_dir: &Path, session_id: &str) -> (usize, Vec<ArchiveRecord>) {
    for p in db_candidates(data_dir) {
        if !p.is_file() {
            continue;
        }
        let Ok(conn) = Connection::open_with_flags(&p, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
            continue;
        };
        let Ok(mut stmt) = conn.prepare(
            "SELECT time_created, data FROM message WHERE session_id = ?1 ORDER BY time_created, id",
        ) else {
            continue;
        };
        let Ok(rows) = stmt.query_map([session_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        }) else {
            continue;
        };
        let mut records = Vec::new();
        for row in rows {
            let Ok((ts, data)) = row else { continue };
            if let Some(usage) = parse_message_data(&data) {
                records.push(ArchiveRecord { ts, usage });
            }
        }
        if !records.is_empty() {
            return (1, records);
        }
    }
    (0, Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 建一个和 opencode 迁移后 schema 兼容的 db（只建用到的最小表）。
    fn make_db(dir: &Path) -> Connection {
        let conn = Connection::open(dir.join("opencode.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (
                id TEXT PRIMARY KEY,
                tokens_input INTEGER NOT NULL DEFAULT 0,
                tokens_output INTEGER NOT NULL DEFAULT 0,
                tokens_reasoning INTEGER NOT NULL DEFAULT 0,
                tokens_cache_read INTEGER NOT NULL DEFAULT 0,
                tokens_cache_write INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                data TEXT NOT NULL
            );",
        )
        .unwrap();
        conn
    }

    fn set_totals(conn: &Connection, sid: &str, t: (u64, u64, u64, u64, u64)) {
        conn.execute(
            "UPDATE session SET tokens_input=?1, tokens_output=?2, tokens_reasoning=?3,
             tokens_cache_read=?4, tokens_cache_write=?5 WHERE id=?6",
            rusqlite::params![t.0 as i64, t.1 as i64, t.2 as i64, t.3 as i64, t.4 as i64, sid],
        )
        .unwrap();
    }

    #[test]
    fn incremental_diff_counts_only_the_turn() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = make_db(tmp.path());
        conn.execute(
            "INSERT INTO session (id, tokens_input, tokens_output, tokens_reasoning,
             tokens_cache_read, tokens_cache_write) VALUES ('sid1', 1000, 200, 10, 0, 0)",
            [],
        )
        .unwrap();

        // Started：历史累计 = 基线。
        let mut reader = OpencodeReader::locate(tmp.path().to_path_buf(), "sid1");
        assert_eq!(reader.read_new(), None, "no new consumption yet");

        // 回合内新增 300 input / 60 output / 15 reasoning / 80 cache read。
        set_totals(
            &conn,
            "sid1",
            (1300, 260, 25, 80, 0),
        );
        let u = reader.read_new().expect("usage");
        assert_eq!(u.input_tokens, 300 + 80, "input 含缓存读写");
        assert_eq!(u.output_tokens, 60);
        assert_eq!(u.total_tokens, 300 + 60 + 15 + 80);
        assert_eq!(u.cached_read_tokens, Some(80));
        assert_eq!(u.cached_write_tokens, None);
        assert_eq!(u.thought_tokens, Some(15));

        // 第二次读：没新增 → None（基线已推进）。
        assert_eq!(reader.read_new(), None);
    }

    #[test]
    fn fresh_session_zero_baseline_counts_first_turn() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = make_db(tmp.path());
        // session/new 落库时累计列全 0。
        conn.execute(
            "INSERT INTO session (id) VALUES ('sid2')",
            [],
        )
        .unwrap();
        let mut reader = OpencodeReader::locate(tmp.path().to_path_buf(), "sid2");
        set_totals(&conn, "sid2", (500, 100, 5, 0, 0));
        let u = reader.read_new().expect("usage");
        assert_eq!(u.input_tokens, 500);
        assert_eq!(u.total_tokens, 500 + 100 + 5);
    }

    #[test]
    fn locate_falls_back_when_session_row_missing_then_recovers() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = make_db(tmp.path());
        // Started 时会话行还没落库：记下 db 文件，不回补。
        let mut reader = OpencodeReader::locate(tmp.path().to_path_buf(), "late");
        assert_eq!(reader.read_new(), None);
        // 行出现后：当前累计当基线，后续差值正常。
        conn.execute("INSERT INTO session (id) VALUES ('late')", []).unwrap();
        assert_eq!(reader.read_new(), None, "first sighting = baseline");
        set_totals(&conn, "late", (300, 60, 0, 0, 0));
        let u = reader.read_new().expect("usage");
        assert_eq!(u.input_tokens, 300);
        assert_eq!(u.output_tokens, 60);
    }

    #[test]
    fn missing_db_yields_none() {
        let tmp = tempfile::tempdir().unwrap();
        let mut reader = OpencodeReader::locate(tmp.path().to_path_buf(), "nope");
        assert_eq!(reader.read_new(), None);
    }

    #[test]
    fn backfill_reads_per_message_records() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = make_db(tmp.path());
        let ins = |id: &str, sid: &str, ts: i64, data: &str| {
            conn.execute(
                "INSERT INTO message (id, session_id, time_created, data) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![id, sid, ts, data],
            )
            .unwrap();
        };
        ins("m1", "sid1", 1000, r#"{"role":"user"}"#);
        ins("m2", "sid1", 2000, r#"{"role":"assistant"}"#);
        ins(
            "m3",
            "sid1",
            3000,
            r#"{"role":"assistant","tokens":{"input":100,"output":20,"cache":{"read":4,"write":5},"reasoning":3}}"#,
        );
        ins(
            "m4",
            "sid1",
            4000,
            r#"{"role":"assistant","tokens":{"input":50,"output":10}}"#,
        );
        ins(
            "m5",
            "sid2",
            5000,
            r#"{"role":"assistant"}"#,
        );
        let (files, recs) = read_archive_full(tmp.path(), "sid1");
        assert_eq!(files, 1);
        assert_eq!(recs.len(), 2, "user 行和无 tokens 行跳过");
        assert_eq!(recs[0].ts, 3000);
        assert_eq!(recs[0].usage.input_tokens, 100 + 4 + 5);
        assert_eq!(recs[0].usage.total_tokens, 100 + 20 + 3 + 4 + 5);
        assert_eq!(recs[0].usage.cached_read_tokens, Some(4));
        assert_eq!(recs[0].usage.cached_write_tokens, Some(5));
        assert_eq!(recs[1].ts, 4000);
        assert_eq!(recs[1].usage.input_tokens, 50);
        assert_eq!(recs[1].usage.thought_tokens, None);
        // 别的会话不串。
        assert_eq!(read_archive_full(tmp.path(), "sid2").1.len(), 0);
        // 目录不存在 → 0 文件。
        assert_eq!(read_archive_full(&tmp.path().join("none"), "sid1").0, 0);
    }

    #[test]
    fn parse_message_data_tolerates_shapes() {
        assert_eq!(parse_message_data("{\"role\":\"user\"}"), None);
        assert_eq!(parse_message_data("not json"), None);
        assert_eq!(
            parse_message_data("{\"role\":\"assistant\",\"tokens\":{\"input\":0,\"output\":0}}"),
            None,
            "全零不计"
        );
        assert_eq!(parse_message_data("{\"role\":\"assistant\"}"), None);
    }

    /// 旧版 opencode（无 session_usage 迁移列）：session 表只有 id。
    fn make_legacy_db(dir: &Path) -> Connection {
        let conn = Connection::open(dir.join("opencode.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY);
             CREATE TABLE message (
                 id TEXT PRIMARY KEY,
                 session_id TEXT NOT NULL,
                 time_created INTEGER NOT NULL,
                 data TEXT NOT NULL
             );",
        )
        .unwrap();
        conn
    }

    #[test]
    fn message_fallback_sums_new_assistant_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = make_legacy_db(tmp.path());
        conn.execute("INSERT INTO session (id) VALUES ('sid9')", []).unwrap();
        let ins = |id: &str, sid: &str, ts: i64, data: &str| {
            conn.execute(
                "INSERT INTO message (id, session_id, time_created, data) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![id, sid, ts, data],
            )
            .unwrap();
        };
        // 历史消息（resume）：locate 时就作为基线，不计入。
        ins("h1", "sid9", 1000, r#"{"role":"assistant","tokens":{"input":500,"output":50}}"#);
        let mut reader = OpencodeReader::locate(tmp.path().to_path_buf(), "sid9");
        assert_eq!(reader.read_new(), None, "历史不计入");
        // 本回合新增：两条 assistant（工具循环两次调用）+ 思考 + 一条 user。
        ins("u1", "sid9", 2000, r#"{"role":"user"}"#);
        ins(
            "a1",
            "sid9",
            3000,
            r#"{"role":"assistant","tokens":{"input":100,"output":20,"reasoning":5,"cache":{"read":4,"write":3}}}"#,
        );
        ins("a2", "sid9", 4000, r#"{"role":"assistant","tokens":{"input":60,"output":10}}"#);
        let u = reader.read_new().expect("usage");
        assert_eq!(u.input_tokens, 100 + 4 + 3 + 60, "含缓存读写");
        assert_eq!(u.output_tokens, 30);
        assert_eq!(u.total_tokens, 100 + 20 + 5 + 4 + 3 + 60 + 10);
        assert_eq!(u.thought_tokens, Some(5));
        // 第二次读：无新增 → None。
        assert_eq!(reader.read_new(), None);
    }

    #[test]
    fn message_fallback_waits_for_late_assistant_row() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = make_legacy_db(tmp.path());
        conn.execute("INSERT INTO session (id) VALUES ('sid10')", []).unwrap();
        let ins = |id: &str, sid: &str, ts: i64, data: &str| {
            conn.execute(
                "INSERT INTO message (id, session_id, time_created, data) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![id, sid, ts, data],
            )
            .unwrap();
        };
        let mut reader = OpencodeReader::locate(tmp.path().to_path_buf(), "sid10");
        // 首次可见：只有 user 行（无 tokens）→ 建基线，不计。
        ins("u1", "sid10", 1000, r#"{"role":"user"}"#);
        assert_eq!(reader.read_new(), None);
        // 同一回合内 user 行之后才落的 assistant 行不能丢。
        ins("a1", "sid10", 2000, r#"{"role":"assistant","tokens":{"input":80,"output":9}}"#);
        let u = reader.read_new().expect("usage");
        assert_eq!(u.input_tokens, 80);
        assert_eq!(u.output_tokens, 9);
        assert_eq!(reader.read_new(), None);
    }
}
