// 各 provider CLI 的本地会话档案增量用量读取。
//
// kimi / claude / codex 的 ACP 适配器都不在 prompt 响应里上报 usage，但
// 各自的 CLI 会把用量写进本地档案：
//
// - kimi:   <KIMI_HOME|~/.kimi-code>/sessions/wd_<dir>_<hash>/
//           session_<acp会话id>/agents/<agent>/wire.jsonl（main + 子 agent
//           多文件，逐行 {"type":"usage.record","usage":{"inputOther":…,
//           "output":…,"inputCacheRead":…,"inputCacheCreation":…},
//           "usageScope":"turn"}）
// - claude: <CLAUDE_CONFIG_DIR|~/.claude>/projects/<cwd-slug>/
//           <acp会话id>.jsonl（cwd-slug = 工作目录把 / \ : 换成 -；每行
//           一条 JSON，assistant 条目的 message.usage 是该次 API 调用的
//           用量）。文件名对不上时回退到该 slug 目录下最新修改的 jsonl。
// - codex:  <CODEX_HOME|~/.codex>/sessions/<年>/<月>/<日>/
//           rollout-<时间戳>-<uuid>.jsonl（event_msg/token_count 条目，
//           取 last_token_usage 增量，不用累计的 total_token_usage）。
//           uuid 与 ACP 会话 id 的关系未验证：优先文件名以 sid 结尾，
//           回退最近修改的 rollout。
//
// 增量语义三家一致：locate 时已存在的内容对齐到 EOF（resume 的历史回合
// 不重复计数）；Started 时文件还不存在的（fresh 会话）挂起，首次读取时
// 再解析并从头读（当前回合的记录已在文件里）；只消费完整行，末尾半行
// 留给下轮；坏行跳过；总和为 0 返回 None。

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::acp::events::TurnUsage;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ArchiveKind {
    Kimi,
    Claude,
    Codex,
}

impl ArchiveKind {
    pub fn for_provider(provider: &str) -> Option<Self> {
        match provider.trim().to_lowercase().as_str() {
            "kimi" => Some(Self::Kimi),
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }
}

// ---- provider 数据根与文件定位 ----

fn env_or_home(env: &str, default_dir: &str) -> Option<PathBuf> {
    if let Some(h) = std::env::var_os(env) {
        if !h.is_empty() {
            return Some(PathBuf::from(h));
        }
    }
    dirs::home_dir().map(|h| h.join(default_dir))
}

pub fn kimi_home() -> Option<PathBuf> {
    env_or_home("KIMI_HOME", ".kimi-code")
}

pub fn claude_home() -> Option<PathBuf> {
    env_or_home("CLAUDE_CONFIG_DIR", ".claude")
}

pub fn codex_home() -> Option<PathBuf> {
    env_or_home("CODEX_HOME", ".codex")
}

/// kimi 两层扫描：sessions/*/session_<sid>/agents/*/wire.jsonl。
fn kimi_wire_files(home: &Path, acp_session_id: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    // kimi 的 ACP session id 自带 "session_" 前缀（如 session_5e5b…），
    // 磁盘目录名也是 session_<uuid>——已有前缀直接用，否则补上。
    let want = if acp_session_id.starts_with("session_") {
        acp_session_id.to_string()
    } else {
        format!("session_{acp_session_id}")
    };
    let Ok(wds) = std::fs::read_dir(home.join("sessions")) else {
        return out;
    };
    for wd in wds.flatten() {
        let Ok(agents) = std::fs::read_dir(wd.path().join(&want).join("agents")) else {
            continue;
        };
        for a in agents.flatten() {
            let p = a.path().join("wire.jsonl");
            if p.is_file() {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// claude cwd-slug：工作目录的 / \ : 全换成 -（如 C--workspace-Wardex-rust）。
fn cwd_slug(work_dir: &str) -> String {
    work_dir
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' => '-',
            c => c,
        })
        .collect()
}

/// 目录下最新修改的 *.jsonl（claude / codex 的回退定位）。
fn newest_jsonl(dir: &Path) -> Option<PathBuf> {
    let list = std::fs::read_dir(dir).ok()?;
    list.flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl") && p.is_file())
        .max_by_key(|p| {
            std::fs::metadata(p)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        })
}

/// claude 精确文件：projects/<slug>/<sid>.jsonl（不要求存在）。
fn claude_exact_file(home: &Path, acp_session_id: &str, work_dir: &str) -> PathBuf {
    home.join("projects")
        .join(cwd_slug(work_dir))
        .join(format!("{acp_session_id}.jsonl"))
}

/// codex 全部 rollout 文件：sessions/<y>/<m>/<d>/rollout-*.jsonl。
fn codex_rollout_files(home: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let sessions = home.join("sessions");
    let Ok(years) = std::fs::read_dir(&sessions) else {
        return out;
    };
    for y in years.flatten() {
        let Ok(months) = std::fs::read_dir(y.path()) else {
            continue;
        };
        for m in months.flatten() {
            let Ok(days) = std::fs::read_dir(m.path()) else {
                continue;
            };
            for d in days.flatten() {
                let Ok(files) = std::fs::read_dir(d.path()) else {
                    continue;
                };
                for f in files.flatten() {
                    let p = f.path();
                    let is_rollout = p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"));
                    if is_rollout && p.is_file() {
                        out.push(p);
                    }
                }
            }
        }
    }
    out
}

/// codex 精确文件：文件名（rollout-<ts>-<uuid>.jsonl）以 sid 结尾。
fn codex_exact_file(home: &Path, acp_session_id: &str) -> Option<PathBuf> {
    codex_rollout_files(home)
        .into_iter()
        .find(|p| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.ends_with(acp_session_id))
        })
}

fn codex_newest(home: &Path) -> Option<PathBuf> {
    codex_rollout_files(home).into_iter().max_by_key(|p| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    })
}

// ---- 行解析 ----

/// 单条用量记录的拆分计数（total 在解析时定案：档案给了用档案的，否则
/// input+output）。request_id / ts 只为 claude 去重和回填取时间戳服务。
#[derive(Debug, Default, PartialEq)]
struct RecordSum {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    thought: u64,
    total: u64,
    /// claude 顶层 requestId：同一次 API 调用拆出的多条 assistant 行共享，
    /// 去重时每组只计 output 最大的那行。
    request_id: Option<String>,
    /// 记录自带时间戳（ms；0 = 档案没给）。
    ts: i64,
}

fn get_u64(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// ISO-8601 时间串 → epoch ms；解析失败返回 0。
fn iso_to_ms(s: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|d| d.timestamp_millis())
        .unwrap_or(0)
}

impl RecordSum {
    fn into_usage(&self) -> TurnUsage {
        TurnUsage {
            input_tokens: self.input,
            output_tokens: self.output,
            total_tokens: self.total,
            cached_read_tokens: (self.cache_read > 0).then_some(self.cache_read),
            cached_write_tokens: (self.cache_write > 0).then_some(self.cache_write),
            thought_tokens: (self.thought > 0).then_some(self.thought),
        }
    }
}

/// kimi usage.record 行；非 turn 粒度（session 累计等）跳过，scope 缺失
/// 按 turn 容错。
fn parse_kimi(line: &str) -> Option<RecordSum> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("type").and_then(Value::as_str) != Some("usage.record") {
        return None;
    }
    match v.get("usageScope").and_then(Value::as_str) {
        Some(s) if s != "turn" => return None,
        _ => {}
    }
    let u = v.get("usage")?;
    let cache_read = get_u64(u, "inputCacheRead");
    let cache_write = get_u64(u, "inputCacheCreation");
    let input = get_u64(u, "inputOther") + cache_read + cache_write;
    let output = get_u64(u, "output");
    Some(RecordSum {
        input,
        output,
        cache_read,
        cache_write,
        thought: 0,
        total: input + output,
        request_id: None,
        ts: v.get("time").and_then(Value::as_i64).unwrap_or(0),
    })
}

/// claude assistant 条目的 message.usage（每条 = 一次 API 调用；同一调用
/// 的多行共享顶层 requestId，由 read_new / 回填按 requestId 去重）。
fn parse_claude(line: &str) -> Option<RecordSum> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("type").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let u = v.get("message")?.get("usage")?;
    let cache_write = get_u64(u, "cache_creation_input_tokens");
    let cache_read = get_u64(u, "cache_read_input_tokens");
    let input = get_u64(u, "input_tokens") + cache_write + cache_read;
    let output = get_u64(u, "output_tokens");
    Some(RecordSum {
        input,
        output,
        cache_read,
        cache_write,
        thought: 0,
        total: input + output,
        request_id: v
            .get("requestId")
            .and_then(Value::as_str)
            .map(str::to_string),
        ts: v
            .get("timestamp")
            .and_then(Value::as_str)
            .map(iso_to_ms)
            .unwrap_or(0),
    })
}

/// codex event_msg/token_count 的 last_token_usage（每次调用的增量；
/// 累计值 total_token_usage 和 rate_limits 都不接）。
fn parse_codex(line: &str) -> Option<RecordSum> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("type").and_then(Value::as_str) != Some("event_msg") {
        return None;
    }
    let payload = v.get("payload")?;
    if payload.get("type").and_then(Value::as_str) != Some("token_count") {
        return None;
    }
    let u = payload.get("info")?.get("last_token_usage")?;
    if u.is_null() {
        return None;
    }
    let input = get_u64(u, "input_tokens");
    let output = get_u64(u, "output_tokens");
    let thought = get_u64(u, "reasoning_output_tokens");
    let total = match get_u64(u, "total_tokens") {
        0 => input + output,
        t => t,
    };
    Some(RecordSum {
        input,
        output,
        cache_read: get_u64(u, "cached_input_tokens"),
        cache_write: 0,
        thought,
        total,
        request_id: None,
        ts: v
            .get("timestamp")
            .and_then(Value::as_str)
            .map(iso_to_ms)
            .unwrap_or(0),
    })
}

fn parse_line(kind: ArchiveKind, line: &str) -> Option<RecordSum> {
    match kind {
        ArchiveKind::Kimi => parse_kimi(line),
        ArchiveKind::Claude => parse_claude(line),
        ArchiveKind::Codex => parse_codex(line),
    }
}

/// claude 去重：同一 requestId 的多行（thinking/text 块各一条）只计
/// output 最大的那行（该次调用的最终完整用量），input/cache 用同一行的
/// 值；没有 requestId 的行各计各的。保持首次出现顺序。
fn dedup_by_request_id(records: Vec<RecordSum>) -> Vec<RecordSum> {
    let mut out: Vec<RecordSum> = Vec::new();
    let mut by_id: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for r in records {
        match &r.request_id {
            Some(id) => {
                if let Some(&i) = by_id.get(id) {
                    if r.output > out[i].output {
                        out[i] = r;
                    }
                } else {
                    by_id.insert(id.clone(), out.len());
                    out.push(r);
                }
            }
            None => out.push(r),
        }
    }
    out
}

/// 档案文件选择（locate 与回填共用）：kimi 全量扫描；claude/codex 精确
/// 优先、回退最新。
fn archive_files(kind: ArchiveKind, home: &Path, acp_session_id: &str, work_dir: &str) -> Vec<PathBuf> {
    match kind {
        ArchiveKind::Kimi => kimi_wire_files(home, acp_session_id),
        ArchiveKind::Claude => {
            let exact = claude_exact_file(home, acp_session_id, work_dir);
            if exact.is_file() {
                vec![exact]
            } else {
                exact.parent().and_then(newest_jsonl).into_iter().collect()
            }
        }
        ArchiveKind::Codex => codex_exact_file(home, acp_session_id)
            .or_else(|| codex_newest(home))
            .into_iter()
            .collect(),
    }
}

/// 从 offset 读到文件尾，只消费完整行（末尾半行留给下次，写方可能正在
/// flush）。文件被截断/重建（比 offset 还短）时从头重读。
fn read_records_from(kind: ArchiveKind, path: &Path, offset: &mut u64) -> Vec<RecordSum> {
    let mut out = Vec::new();
    let Ok(mut f) = std::fs::File::open(path) else {
        return out;
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if len < *offset {
        *offset = 0;
    }
    if f.seek(SeekFrom::Start(*offset)).is_err() {
        return out;
    }
    let mut buf = String::new();
    if f.read_to_string(&mut buf).is_err() {
        return out;
    }
    let complete = match buf.rfind('\n') {
        Some(i) => i + 1,
        None => 0,
    };
    for line in buf[..complete].lines() {
        if let Some(r) = parse_line(kind, line) {
            out.push(r);
        }
    }
    *offset += complete as u64;
    out
}

fn file_len(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

// ---- 读取器 ----

/// 每会话一个档案读取器。kimi 跟踪多文件偏移并每轮重扫（子 agent 中途
/// 出现）；claude/codex 只跟踪单文件，Started 时定位不到则挂起
/// （resolved=false），首次 read_new 再解析——此时当前回合的记录已在
/// 文件里，从头读才不会丢。
#[derive(Debug)]
pub struct ArchiveReader {
    kind: ArchiveKind,
    home: PathBuf,
    session_id: String,
    work_dir: String,
    /// 档案路径 → 下次读取起点。
    offsets: BTreeMap<PathBuf, u64>,
    resolved: bool,
}

impl ArchiveReader {
    /// Started 时调用。已存在的文件对齐到 EOF（历史不计入）；定位不到
    /// （fresh 会话）挂起，等 read_new 解析。
    pub fn locate(
        kind: ArchiveKind,
        home: PathBuf,
        acp_session_id: &str,
        work_dir: &str,
    ) -> Self {
        let mut r = Self {
            kind,
            home,
            session_id: acp_session_id.to_string(),
            work_dir: work_dir.to_string(),
            offsets: BTreeMap::new(),
            resolved: false,
        };
        match kind {
            ArchiveKind::Kimi => {
                for p in kimi_wire_files(&r.home, &r.session_id) {
                    r.offsets.insert(p.clone(), file_len(&p));
                }
                r.resolved = true; // kimi 每轮重扫，无需挂起态
            }
            ArchiveKind::Claude => {
                let exact = claude_exact_file(&r.home, &r.session_id, &r.work_dir);
                if exact.is_file() {
                    r.offsets.insert(exact.clone(), file_len(&exact));
                    r.resolved = true;
                }
            }
            ArchiveKind::Codex => {
                if let Some(exact) = codex_exact_file(&r.home, &r.session_id) {
                    r.offsets.insert(exact.clone(), file_len(&exact));
                    r.resolved = true;
                }
            }
        }
        r
    }

    /// 挂起态的首次解析：精确文件从头读（当前会话新建的）；都没有才用
    /// 回退（claude：slug 目录最新 jsonl；codex：最新 rollout），同样
    /// 从头读——fresh 会话回退命中的就是当前文件。
    fn try_resolve(&mut self) {
        let found: Option<PathBuf> = match self.kind {
            ArchiveKind::Kimi => None,
            ArchiveKind::Claude => {
                let exact = claude_exact_file(&self.home, &self.session_id, &self.work_dir);
                if exact.is_file() {
                    Some(exact)
                } else {
                    exact.parent().and_then(newest_jsonl)
                }
            }
            ArchiveKind::Codex => codex_exact_file(&self.home, &self.session_id)
                .or_else(|| codex_newest(&self.home)),
        };
        if let Some(p) = found {
            self.offsets.insert(p, 0);
            self.resolved = true;
        }
    }

    /// 读取新增部分并求和。没有新记录或总和为 0 返回 None。
    pub fn read_new(&mut self) -> Option<TurnUsage> {
        if !self.resolved {
            self.try_resolve();
        }
        if self.kind == ArchiveKind::Kimi {
            // 中途出现的文件（子 agent）从头读。
            for p in kimi_wire_files(&self.home, &self.session_id) {
                self.offsets.entry(p).or_insert(0);
            }
        }
        let mut records = Vec::new();
        for (p, off) in self.offsets.iter_mut() {
            records.extend(read_records_from(self.kind, p, off));
        }
        if self.kind == ArchiveKind::Claude {
            records = dedup_by_request_id(records);
        }
        let mut sum = RecordSum::default();
        for r in &records {
            sum.input += r.input;
            sum.output += r.output;
            sum.cache_read += r.cache_read;
            sum.cache_write += r.cache_write;
            sum.thought += r.thought;
            sum.total += r.total;
        }
        if sum.total == 0 {
            return None;
        }
        Some(sum.into_usage())
    }
}

/// 回填用的一条档案用量（逐条，未聚合）。
#[derive(Debug, Clone)]
pub struct ArchiveRecord {
    /// 记录自带时间戳（ms；0 = 档案没给）。
    pub ts: i64,
    pub usage: TurnUsage,
}

/// 回填用全量解析：从 offset 0 读完整文件（不 EOF 对齐，与 ArchiveReader
/// 的增量状态完全无关），返回 (命中档案数, 逐条记录)。claude 同样按
/// requestId 去重。
pub fn read_archive_full(
    kind: ArchiveKind,
    home: &Path,
    acp_session_id: &str,
    work_dir: &str,
) -> (usize, Vec<ArchiveRecord>) {
    let files = archive_files(kind, home, acp_session_id, work_dir);
    let mut records = Vec::new();
    for p in &files {
        let mut off = 0u64;
        records.extend(read_records_from(kind, p, &mut off));
    }
    if kind == ArchiveKind::Claude {
        records = dedup_by_request_id(records);
    }
    let out = records
        .iter()
        .map(|r| ArchiveRecord {
            ts: r.ts,
            usage: r.into_usage(),
        })
        .collect();
    (files.len(), out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- kimi（原 wire.rs 用例，逻辑不变）----

    fn kimi_line(input_other: u64, output: u64, cache_read: u64, cache_write: u64) -> String {
        format!(
            "{{\"type\":\"usage.record\",\"model\":\"__kimi_env_model__\",\"usage\":{{\"inputOther\":{input_other},\"output\":{output},\"inputCacheRead\":{cache_read},\"inputCacheCreation\":{cache_write}}},\"usageScope\":\"turn\",\"time\":1785571131291}}"
        )
    }

    fn kimi_wire_path(root: &Path, sid: &str, agent: &str) -> PathBuf {
        let dir = root
            .join("sessions")
            .join("wd_proj_ab12")
            .join(format!("session_{sid}"))
            .join("agents")
            .join(agent);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("wire.jsonl")
    }

    #[test]
    fn kimi_incremental_read_sums_and_advances_offset() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let main = kimi_wire_path(home, "sid1", "main");
        // 历史内容：locate 跳过。
        std::fs::write(&main, format!("{}\n", kimi_line(100, 10, 0, 0))).unwrap();

        let mut reader = ArchiveReader::locate(ArchiveKind::Kimi, home.to_path_buf(), "sid1", "");
        assert_eq!(reader.read_new(), None, "history skipped");

        // 第一回合：main 追加一条（含一条坏行和一条非 usage 行），
        // 子 agent agent-0 新文件出现。
        let mut f = std::fs::OpenOptions::new().append(true).open(&main).unwrap();
        use std::io::Write as _;
        writeln!(f, "not json at all").unwrap();
        writeln!(f, "{{\"type\":\"session.start\"}}").unwrap();
        writeln!(f, "{}", kimi_line(20042, 320, 2560, 0)).unwrap();
        drop(f);
        let sub = kimi_wire_path(home, "sid1", "agent-0");
        std::fs::write(&sub, format!("{}\n", kimi_line(1000, 50, 0, 40))).unwrap();

        let u = reader.read_new().expect("usage");
        assert_eq!(u.input_tokens, 20042 + 2560 + 1000 + 40);
        assert_eq!(u.output_tokens, 370);
        assert_eq!(u.total_tokens, u.input_tokens + 370);
        assert_eq!(u.cached_read_tokens, Some(2560));
        assert_eq!(u.cached_write_tokens, Some(40));
        assert_eq!(u.thought_tokens, None);

        // 第二次读：没有新增 → None（偏移已推进）。
        assert_eq!(reader.read_new(), None);

        // 末尾半行不消费，补全后下轮读到。
        let mut f = std::fs::OpenOptions::new().append(true).open(&main).unwrap();
        write!(f, "{}", kimi_line(7, 3, 0, 0)).unwrap(); // 无换行
        drop(f);
        assert_eq!(reader.read_new(), None, "partial line held");
        let mut f = std::fs::OpenOptions::new().append(true).open(&main).unwrap();
        writeln!(f).unwrap();
        drop(f);
        let u = reader.read_new().expect("usage");
        assert_eq!(u.total_tokens, 10);
    }

    #[test]
    fn kimi_missing_files_yield_none() {
        let tmp = tempfile::tempdir().unwrap();
        let mut reader =
            ArchiveReader::locate(ArchiveKind::Kimi, tmp.path().to_path_buf(), "nope", "");
        assert_eq!(reader.read_new(), None);
    }

    #[test]
    fn kimi_prefixed_session_id_locates_dir() {
        // 真实环境里 kimi 的 ACP session id 自带 "session_" 前缀，
        // 磁盘目录也是 session_<uuid>——不能再拼一次前缀。
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let main = kimi_wire_path(home, "5e5b6987", "main");
        std::fs::write(&main, format!("{}\n", kimi_line(10, 5, 0, 0))).unwrap();
        let mut reader = ArchiveReader::locate(
            ArchiveKind::Kimi,
            home.to_path_buf(),
            "session_5e5b6987",
            "",
        );
        assert_eq!(reader.read_new(), None, "history skipped (aligned to EOF)");
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new().append(true).open(&main).unwrap();
        writeln!(f, "{}", kimi_line(100, 20, 0, 0)).unwrap();
        drop(f);
        let u = reader.read_new().expect("usage");
        assert_eq!(u.input_tokens, 100);
        assert_eq!(u.output_tokens, 20);
    }

    // ---- claude ----

    fn claude_line(input: u64, output: u64, cache_write: u64, cache_read: u64) -> String {
        format!(
            "{{\"type\":\"assistant\",\"message\":{{\"role\":\"assistant\",\"usage\":{{\"input_tokens\":{input},\"cache_creation_input_tokens\":{cache_write},\"cache_read_input_tokens\":{cache_read},\"output_tokens\":{output}}}}}}}"
        )
    }

    #[test]
    fn claude_slug_and_exact_file() {
        assert_eq!(cwd_slug("C:/workspace/Wardex-rust"), "C--workspace-Wardex-rust");
        assert_eq!(cwd_slug("C:\\a\\b"), "C--a-b");
    }

    #[test]
    fn claude_pending_resolve_reads_from_start_then_incremental() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let dir = home.join("projects").join("C--workspace-Wardex-rust");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("sid42.jsonl");

        // Started：文件还不存在 → 挂起。
        let mut reader =
            ArchiveReader::locate(ArchiveKind::Claude, home.clone(), "sid42", "C:/workspace/Wardex-rust");
        assert!(!reader.resolved);

        // 第一回合结束：文件出现，含一条 assistant 用量 + 一条坏行 +
        // 一条 user 行；从头读到。
        std::fs::write(
            &file,
            format!(
                "{{\"type\":\"user\",\"message\":{{}}}}\n{bad}\n{}\n",
                claude_line(18762, 149, 0, 0),
                bad = "not json"
            ),
        )
        .unwrap();
        let u = reader.read_new().expect("usage");
        assert_eq!(u.input_tokens, 18762);
        assert_eq!(u.output_tokens, 149);
        assert_eq!(u.total_tokens, 18911);
        assert_eq!(u.cached_read_tokens, None);
        assert!(reader.resolved);

        // 追加第二条（带缓存），增量只求新增。
        let mut f = std::fs::OpenOptions::new().append(true).open(&file).unwrap();
        use std::io::Write as _;
        writeln!(f, "{}", claude_line(100, 10, 5, 7)).unwrap();
        drop(f);
        let u = reader.read_new().expect("usage");
        assert_eq!(u.input_tokens, 100 + 5 + 7);
        assert_eq!(u.output_tokens, 10);
        assert_eq!(u.cached_read_tokens, Some(7));
        assert_eq!(u.cached_write_tokens, Some(5));
        assert_eq!(reader.read_new(), None);
    }

    fn claude_line_req(req: Option<&str>, input: u64, output: u64) -> String {
        let req = match req {
            Some(r) => format!("\"requestId\":\"{r}\","),
            None => String::new(),
        };
        format!(
            "{{\"type\":\"assistant\",\"timestamp\":\"2026-07-31T10:00:00.000Z\",{req}\"message\":{{\"role\":\"assistant\",\"usage\":{{\"input_tokens\":{input},\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":0,\"output_tokens\":{output}}}}}}}"
        )
    }

    #[test]
    fn claude_dedups_same_request_id() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let dir = home.join("projects").join("C--p");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("sid5.jsonl");
        // Started 时文件尚不存在 → 挂起；回合结束后文件出现，从头读。
        let mut reader = ArchiveReader::locate(ArchiveKind::Claude, home, "sid5", "C:/p");
        // 同一 requestId 三行（thinking/text 块拆分，output 不同）只计
        // 一次，取 output 最大那行；另一个 requestId 和无 requestId 的行
        // 各计各的。
        std::fs::write(
            &file,
            [
                claude_line_req(Some("r1"), 100, 5),
                claude_line_req(Some("r1"), 18762, 149),
                claude_line_req(Some("r1"), 300, 60),
                claude_line_req(Some("r2"), 20, 10),
                claude_line_req(None, 3, 7),
                String::new(),
            ]
            .join("\n"),
        )
        .unwrap();

        let u = reader.read_new().expect("usage");
        assert_eq!(u.input_tokens, 18762 + 20 + 3);
        assert_eq!(u.output_tokens, 149 + 10 + 7);
        assert_eq!(u.total_tokens, u.input_tokens + u.output_tokens);
    }

    #[test]
    fn claude_fallback_newest_when_name_mismatches() {        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let dir = home.join("projects").join("C--p");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("other.jsonl"), format!("{}\n", claude_line(3, 1, 0, 0))).unwrap();

        let mut reader =
            ArchiveReader::locate(ArchiveKind::Claude, home, "missing-sid", "C:/p");
        let u = reader.read_new().expect("fallback");
        assert_eq!(u.total_tokens, 4);
    }

    #[test]
    fn claude_resumed_history_skipped_at_locate() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let dir = home.join("projects").join("C--p");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("sid9.jsonl");
        std::fs::write(&file, format!("{}\n", claude_line(500, 50, 0, 0))).unwrap();

        // resume：locate 时文件已存在 → 历史跳过；追加的才算。
        let mut reader = ArchiveReader::locate(ArchiveKind::Claude, home, "sid9", "C:/p");
        assert!(reader.resolved);
        assert_eq!(reader.read_new(), None);
        let mut f = std::fs::OpenOptions::new().append(true).open(&file).unwrap();
        use std::io::Write as _;
        writeln!(f, "{}", claude_line(8, 2, 0, 0)).unwrap();
        drop(f);
        assert_eq!(reader.read_new().expect("usage").total_tokens, 10);
    }

    // ---- codex ----

    fn codex_line(input: u64, output: u64, cached: u64, reasoning: u64, total: u64) -> String {
        format!(
            "{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"last_token_usage\":{{\"input_tokens\":{input},\"cached_input_tokens\":{cached},\"output_tokens\":{output},\"reasoning_output_tokens\":{reasoning},\"total_tokens\":{total}}},\"total_token_usage\":{{\"input_tokens\":999999,\"output_tokens\":999999,\"total_tokens\":999999}}}}}}}}"
        )
    }

    fn codex_rollout_path(root: &Path, name: &str) -> PathBuf {
        let dir = root.join("sessions").join("2026").join("08").join("01");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn codex_exact_match_and_incremental() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let file = codex_rollout_path(&home, "rollout-2026-08-01T07-00-00-sid777.jsonl");

        let mut reader = ArchiveReader::locate(ArchiveKind::Codex, home, "sid777", "");
        assert!(!reader.resolved, "file not there yet at Started");

        // 第一回合：token_count 两条 + 一条无关事件。total 用档案给的
        // 12135 这类值直接求和（含 reasoning）。
        std::fs::write(
            &file,
            format!(
                "{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"agent_message\"}}}}\n{}\n{}\n",
                codex_line(12094, 41, 0, 29, 12135),
                codex_line(100, 10, 4, 3, 110),
            ),
        )
        .unwrap();
        let u = reader.read_new().expect("usage");
        assert_eq!(u.input_tokens, 12094 + 100);
        assert_eq!(u.output_tokens, 51);
        assert_eq!(u.total_tokens, 12135 + 110, "archive totals summed");
        assert_eq!(u.cached_read_tokens, Some(4));
        assert_eq!(u.thought_tokens, Some(29 + 3));
        assert_eq!(reader.read_new(), None);
    }

    #[test]
    fn codex_fallback_newest_rollout() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let f1 = codex_rollout_path(&home, "rollout-2026-08-01T06-00-00-aaaa.jsonl");
        std::fs::write(&f1, format!("{}\n", codex_line(5, 1, 0, 0, 6))).unwrap();

        let mut reader = ArchiveReader::locate(ArchiveKind::Codex, home, "unknown-sid", "");
        let u = reader.read_new().expect("fallback");
        assert_eq!(u.total_tokens, 6);
    }

    #[test]
    fn codex_ignores_total_token_usage_and_rate_limits() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let file = codex_rollout_path(&home, "rollout-t-sid1.jsonl");
        // last_token_usage 为 null 的 token_count 不计。
        std::fs::write(
            &file,
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":null,\"rate_limits\":{\"primary\":{}}}}}\n",
        )
        .unwrap();
        let mut reader = ArchiveReader::locate(ArchiveKind::Codex, home, "sid1", "");
        assert_eq!(reader.read_new(), None);
    }
}
