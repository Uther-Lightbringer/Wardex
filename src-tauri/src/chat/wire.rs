// kimi CLI wire.jsonl 增量用量读取。
//
// kimi 的 ACP 适配器不在 prompt 响应里上报 usage，但 CLI 会把每个会话的
// 用量写进自己的会话档案：
//   <kimi_home>/sessions/wd_<dir>_<hash>/session_<acp会话id>/agents/<agent>/wire.jsonl
// （kimi_home = 环境变量 KIMI_HOME，默认 ~/.kimi-code；agent 有 main 和
// 子 agent 如 agent-0）。每行一个 JSON，用量记录形如：
//   {"type":"usage.record","usage":{"inputOther":20042,"output":320,
//    "inputCacheRead":2560,"inputCacheCreation":0},"usageScope":"turn","time":…}
//
// WireUsageReader 在会话就绪（Started）时定位现有文件并跳过其当前内容
// （历史回合的用量不属于本次运行），之后每回合结束做增量读取：新出现的
// 文件（子 agent 中途才创建）从头读，已知文件从上次偏移继续，所有 agent
// 的记录求和成一条 TurnUsage。

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::acp::events::TurnUsage;

/// kimi 数据根：KIMI_HOME 环境变量优先，否则 ~/.kimi-code。
pub fn kimi_home() -> Option<PathBuf> {
    if let Some(h) = std::env::var_os("KIMI_HOME") {
        if !h.is_empty() {
            return Some(PathBuf::from(h));
        }
    }
    dirs::home_dir().map(|h| h.join(".kimi-code"))
}

/// 两层扫描：sessions/*/session_<sid>/agents/*/wire.jsonl。
pub fn wire_files(kimi_home: &Path, acp_session_id: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let want = format!("session_{acp_session_id}");
    let Ok(wds) = std::fs::read_dir(kimi_home.join("sessions")) else {
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

/// 单条 usage.record 的拆分计数（input 三分量不提前合并，便于求和缓存量）。
#[derive(Debug, Default, PartialEq)]
struct RecordSum {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
}

fn get_u64(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// 解析一行；非 usage.record、非 turn 粒度、坏行都返回 None。
fn parse_record(line: &str) -> Option<RecordSum> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("type").and_then(Value::as_str) != Some("usage.record") {
        return None;
    }
    // 只累计 turn 粒度；scope 缺失按 turn 处理（容错）。
    match v.get("usageScope").and_then(Value::as_str) {
        Some(s) if s != "turn" => return None,
        _ => {}
    }
    let u = v.get("usage")?;
    let cache_read = get_u64(u, "inputCacheRead");
    let cache_write = get_u64(u, "inputCacheCreation");
    let input = get_u64(u, "inputOther") + cache_read + cache_write;
    Some(RecordSum {
        input,
        output: get_u64(u, "output"),
        cache_read,
        cache_write,
    })
}

/// 从 offset 读到文件尾，只消费完整行（末尾半行留给下次，写方可能正在
///  flush）。文件被截断/重建（比 offset 还短）时从头重读。
fn read_records_from(path: &Path, offset: &mut u64) -> Vec<RecordSum> {
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
        if let Some(r) = parse_record(line) {
            out.push(r);
        }
    }
    *offset += complete as u64;
    out
}

/// 每会话一个：对已定位文件记录字节偏移，增量求和。
#[derive(Debug, Default)]
pub struct WireUsageReader {
    /// wire.jsonl 路径 → 下次读取起点。
    offsets: BTreeMap<PathBuf, u64>,
}

impl WireUsageReader {
    /// 定位现有 wire 文件并跳过其当前内容（resume 的历史回合不计入本次
    /// 运行）。此时文件可能还不存在（fresh 会话）——后续 read_new 会
    /// 重新扫描并把新文件从头读起。
    pub fn locate(kimi_home: &Path, acp_session_id: &str) -> Self {
        let mut r = Self::default();
        for p in wire_files(kimi_home, acp_session_id) {
            let len = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            r.offsets.insert(p, len);
        }
        r
    }

    /// 读取各文件的新增部分并求和。没有新记录或总和为 0 返回 None。
    pub fn read_new(&mut self, kimi_home: &Path, acp_session_id: &str) -> Option<TurnUsage> {
        // 中途出现的文件（子 agent）从头读。
        for p in wire_files(kimi_home, acp_session_id) {
            self.offsets.entry(p).or_insert(0);
        }
        let mut sum = RecordSum::default();
        for (p, off) in self.offsets.iter_mut() {
            for r in read_records_from(p, off) {
                sum.input += r.input;
                sum.output += r.output;
                sum.cache_read += r.cache_read;
                sum.cache_write += r.cache_write;
            }
        }
        let total = sum.input + sum.output;
        if total == 0 {
            return None;
        }
        Some(TurnUsage {
            input_tokens: sum.input,
            output_tokens: sum.output,
            total_tokens: total,
            cached_read_tokens: (sum.cache_read > 0).then_some(sum.cache_read),
            cached_write_tokens: (sum.cache_write > 0).then_some(sum.cache_write),
            thought_tokens: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage_line(input_other: u64, output: u64, cache_read: u64, cache_write: u64) -> String {
        format!(
            "{{\"type\":\"usage.record\",\"model\":\"__kimi_env_model__\",\"usage\":{{\"inputOther\":{input_other},\"output\":{output},\"inputCacheRead\":{cache_read},\"inputCacheCreation\":{cache_write}}},\"usageScope\":\"turn\",\"time\":1785571131291}}"
        )
    }

    /// 造 <tmp>/sessions/wd_proj_ab12/session_sid1/agents/<agent>/wire.jsonl。
    fn wire_path(root: &Path, sid: &str, agent: &str) -> PathBuf {
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
    fn incremental_read_sums_and_advances_offset() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let main = wire_path(home, "sid1", "main");
        // 历史内容：locate 跳过。
        std::fs::write(&main, format!("{}\n", usage_line(100, 10, 0, 0))).unwrap();

        let mut reader = WireUsageReader::locate(home, "sid1");
        assert_eq!(reader.read_new(home, "sid1"), None, "history skipped");

        // 第一回合：main 追加一条（含一条坏行和一条非 usage 行），
        // 子 agent agent-0 新文件出现。
        let mut f = std::fs::OpenOptions::new().append(true).open(&main).unwrap();
        use std::io::Write as _;
        writeln!(f, "not json at all").unwrap();
        writeln!(f, "{{\"type\":\"session.start\"}}").unwrap();
        writeln!(f, "{}", usage_line(20042, 320, 2560, 0)).unwrap();
        drop(f);
        let sub = wire_path(home, "sid1", "agent-0");
        std::fs::write(&sub, format!("{}\n", usage_line(1000, 50, 0, 40))).unwrap();

        let u = reader.read_new(home, "sid1").expect("usage");
        assert_eq!(u.input_tokens, 20042 + 2560 + 1000 + 40);
        assert_eq!(u.output_tokens, 370);
        assert_eq!(u.total_tokens, u.input_tokens + 370);
        assert_eq!(u.cached_read_tokens, Some(2560));
        assert_eq!(u.cached_write_tokens, Some(40));
        assert_eq!(u.thought_tokens, None);

        // 第二次读：没有新增 → None（偏移已推进）。
        assert_eq!(reader.read_new(home, "sid1"), None);

        // 末尾半行不消费，补全后下轮读到。
        let mut f = std::fs::OpenOptions::new().append(true).open(&main).unwrap();
        write!(f, "{}", usage_line(7, 3, 0, 0)).unwrap(); // 无换行
        drop(f);
        assert_eq!(reader.read_new(home, "sid1"), None, "partial line held");
        let mut f = std::fs::OpenOptions::new().append(true).open(&main).unwrap();
        writeln!(f).unwrap();
        drop(f);
        let u = reader.read_new(home, "sid1").expect("usage");
        assert_eq!(u.total_tokens, 10);
    }

    #[test]
    fn missing_files_yield_none() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let mut reader = WireUsageReader::locate(home, "nope");
        assert_eq!(reader.read_new(home, "nope"), None);
    }

    #[test]
    fn non_turn_scope_ignored() {
        let line = "{\"type\":\"usage.record\",\"usage\":{\"inputOther\":5,\"output\":1},\"usageScope\":\"session\"}";
        assert_eq!(parse_record(line), None);
        let line = "{\"type\":\"usage.record\",\"usage\":{\"inputOther\":5,\"output\":1}}";
        assert_eq!(
            parse_record(line),
            Some(RecordSum {
                input: 5,
                output: 1,
                cache_read: 0,
                cache_write: 0
            })
        );
    }
}
