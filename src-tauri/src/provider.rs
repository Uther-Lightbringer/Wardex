// Provider registry: a pure data table (design-principles.md red line C3).
// Every kimi/claude/codex/opencode/custom difference — acpArgs, env names,
// clearEnvs, modeMap, the claude relay-key special case — lives ONLY here;
// the protocol and chat layers never branch on `provider == ...`. Supporting
// a new CLI = adding one entry to REGISTRY.
//
// Ported from src/ProviderRegistry.h/.cpp of the C++/Qt codebase; the
// authoritative spec is docs/providers-and-cli.md §1.

use serde::Serialize;

/// One record = everything WarDex needs to run a CLI as an ACP agent over
/// stdio. Slices instead of Vecs so records can live in a `static` — this
/// table is read-only for the process lifetime.
pub struct ProviderSpec {
    /// Stable lower-case id stored in agents/sessions data.
    pub id: &'static str,
    /// UI display name.
    pub display_name: &'static str,
    /// Executable looked up on PATH when agent.cliPath is empty ("" for
    /// custom: the user's cliPath is mandatory there).
    pub default_command: &'static str,
    /// Args that put the CLI into ACP stdio mode.
    pub acp_args: &'static [&'static str],
    /// Env vars receiving agent.apiKey (all get the same value).
    pub api_key_envs: &'static [&'static str],
    /// Env vars receiving agent.baseUrl.
    pub base_url_envs: &'static [&'static str],
    /// Env vars stripped from the child environment before launch
    /// (anti-nesting guards; see claude entry below).
    pub clear_envs: &'static [&'static str],
    /// apiKey additionally lands in this Bearer-style env var, unless the key
    /// starts with official_key_prefix. Empty = no such behavior. Rationale
    /// (providers-and-cli.md §1.2): relay keys need `Authorization: Bearer`
    /// (ANTHROPIC_AUTH_TOKEN); official sk-ant-… keys must stay on the
    /// primary header only (x-api-key, fed by ANTHROPIC_API_KEY).
    pub bearer_token_env: &'static str,
    /// See bearer_token_env. Empty = every key counts as a relay key.
    pub official_key_prefix: &'static str,
    /// Expected Base URL shape, shown in the config UI.
    pub base_url_hint: &'static str,
    /// WarDex permission-mode id -> provider mode id. Unmapped modes pass
    /// through unchanged (identity).
    pub mode_map: &'static [(&'static str, &'static str)],
    /// Shown in the config page.
    pub install_hint: &'static str,
    /// Reserved field from the old registry; fixed true for all entries.
    pub chat_capable: bool,
}

/// Registration order is the UI list order: kimi, claude, codex, opencode, custom.
pub static REGISTRY: &[ProviderSpec] = &[
    ProviderSpec {
        id: "kimi",
        display_name: "Kimi CLI",
        default_command: "kimi",
        acp_args: &["acp"],
        api_key_envs: &["KIMI_API_KEY", "OPENAI_API_KEY"],
        base_url_envs: &["KIMI_BASE_URL", "OPENAI_BASE_URL"],
        clear_envs: &[],
        bearer_token_env: "",
        official_key_prefix: "",
        base_url_hint: "OpenAI 兼容端点，通常以 /v1 结尾，如 https://api.kimi.com/coding/v1；留空使用本机登录态",
        mode_map: &[],
        install_hint: "安装见 https://www.kimi.com/code",
        chat_capable: true,
    },
    // Claude Code speaks ACP through Zed's adapter, which reuses the local
    // `claude /login` credentials when no ANTHROPIC_API_KEY is provided.
    ProviderSpec {
        id: "claude",
        display_name: "Claude Code",
        default_command: "claude-code-acp",
        acp_args: &[], // adapter speaks ACP directly, no subcommand
        api_key_envs: &["ANTHROPIC_API_KEY"],
        base_url_envs: &["ANTHROPIC_BASE_URL"],
        // The adapter refuses to run "inside another Claude Code session" —
        // strip the session markers a parent claude/WarDex launch leaks in.
        clear_envs: &[
            "CLAUDECODE",
            "CLAUDE_CODE_ENTRYPOINT",
            "CLAUDE_CODE_SSE_PORT",
        ],
        bearer_token_env: "ANTHROPIC_AUTH_TOKEN",
        official_key_prefix: "sk-ant-",
        base_url_hint: "Anthropic 格式根地址（走 /v1/messages 协议），如 https://api.anthropic.com 或中转的 Anthropic 兼容地址，结尾不要带 /v1/messages",
        mode_map: &[("auto", "acceptEdits"), ("yolo", "bypassPermissions")],
        install_hint: "npm i -g @zed-industries/claude-code-acp；API Key 留空则使用 claude /login 的本地凭据",
        chat_capable: true,
    },
    ProviderSpec {
        id: "codex",
        display_name: "Codex CLI",
        default_command: "codex-acp",
        acp_args: &[],
        api_key_envs: &["OPENAI_API_KEY"],
        base_url_envs: &["OPENAI_BASE_URL"],
        clear_envs: &[],
        bearer_token_env: "",
        official_key_prefix: "",
        base_url_hint: "OpenAI 兼容端点，以 /v1 结尾，如 https://api.openai.com/v1；仅支持旧式 chat 接口的中转需在 ~/.codex/config.toml 配 wire_api=\"chat\"",
        mode_map: &[],
        install_hint: "npm i -g @zed-industries/codex-acp；API Key 留空则使用 codex login 的本地凭据",
        chat_capable: true,
    },
    // opencode speaks ACP natively via its `acp` subcommand — no adapter.
    // Credentials fall back to `opencode auth login` (auth.json) when no key
    // is injected; OPENCODE_API_KEY covers both Zen providers (zen/v1 and
    // zen/go/v1). There is no base-URL env convention, so baseUrlEnvs is
    // empty — custom endpoints belong in opencode.json.
    ProviderSpec {
        id: "opencode",
        display_name: "OpenCode",
        default_command: "opencode",
        acp_args: &["acp"],
        api_key_envs: &["OPENCODE_API_KEY"],
        base_url_envs: &[],
        clear_envs: &[],
        bearer_token_env: "",
        official_key_prefix: "",
        base_url_hint: "此栏不注入环境变量，留空即可；自定义端点请在 opencode.json 配置 provider",
        mode_map: &[],
        install_hint: "npm i -g opencode-ai；API Key 留空则使用 opencode auth login 的本地凭据",
        chat_capable: true,
    },
    // Escape hatch: any ACP-speaking CLI without touching code — the user
    // supplies the command (cliPath) and the args (extraArgs, which ARE the
    // full arg list here since acp_args is empty).
    ProviderSpec {
        id: "custom",
        display_name: "自定义 (ACP)",
        default_command: "",
        acp_args: &[],
        api_key_envs: &["OPENAI_API_KEY"],
        base_url_envs: &["OPENAI_BASE_URL"],
        clear_envs: &[],
        bearer_token_env: "",
        official_key_prefix: "",
        base_url_hint: "按该 CLI 文档要求的根地址填写（注入 OPENAI_BASE_URL）",
        mode_map: &[],
        install_hint: "填写 CLI 路径与进入 ACP 模式的参数（如 acp 或 --experimental-acp）",
        chat_capable: true,
    },
];

/// spec(): trim + lowercase, exact match. None for unknown ids.
pub fn spec(id: &str) -> Option<&'static ProviderSpec> {
    let key = id.trim().to_lowercase();
    REGISTRY.iter().find(|s| s.id == key)
}

/// ids(): the four ids in registration order.
pub fn ids() -> impl Iterator<Item = &'static str> {
    REGISTRY.iter().map(|s| s.id)
}

/// chatCapable(): spec exists and is chat_capable.
pub fn chat_capable(id: &str) -> bool {
    spec(id).is_some_and(|s| s.chat_capable)
}

/// mapMode(): translate a WarDex mode (default/plan/auto/yolo) for the given
/// provider. Unmapped modes and unknown providers pass through unchanged.
pub fn map_mode<'m>(id: &str, mode: &'m str) -> &'m str {
    match spec(id) {
        Some(s) => s
            .mode_map
            .iter()
            .find(|(from, _)| *from == mode)
            .map(|(_, to)| *to)
            .unwrap_or(mode),
        None => mode,
    }
}

/// UI-friendly view of a spec (specMap in the old code), serialized with the
/// same camelCase keys the QML QVariantMap had; acpArgs joined with spaces.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpecView {
    pub id: &'static str,
    pub display_name: &'static str,
    pub default_command: &'static str,
    pub acp_args: String,
    pub install_hint: &'static str,
    pub base_url_hint: &'static str,
    pub chat_capable: bool,
}

pub fn spec_view(id: &str) -> Option<SpecView> {
    spec(id).map(|s| SpecView {
        id: s.id,
        display_name: s.display_name,
        default_command: s.default_command,
        acp_args: s.acp_args.join(" "),
        install_hint: s.install_hint,
        base_url_hint: s.base_url_hint,
        chat_capable: s.chat_capable,
    })
}

/// One env override for the child process. `None` value = DELETE the variable
/// (the ACP transport's "null = remove from child env" convention, see
/// acp-protocol.md §2.2 and ChatController.cpp:862-864).
pub type EnvOverrides = Vec<(String, Option<String>)>;

/// Build env overrides from an agent's credentials, in the old ensureAcp
/// order (ChatController.cpp:845-865):
///   apiKeyEnvs -> bearerTokenEnv special case -> baseUrlEnvs -> clearEnvs.
///
/// `apply_clear_envs` distinguishes the two call sites: session start
/// (ensureAcp, phase 1c acp module) passes true; testAgent passes false —
/// the old testAgent deliberately did NOT clear the nesting guards
/// (AgentStore.cpp:263-279 vs ChatController.cpp:862-864), a discrepancy kept
/// verbatim per docs/providers-and-cli.md §4.2.
pub fn env_overrides(
    spec: &ProviderSpec,
    api_key: &str,
    base_url: &str,
    apply_clear_envs: bool,
) -> EnvOverrides {
    let mut out = EnvOverrides::new();
    if !api_key.is_empty() {
        for name in spec.api_key_envs {
            out.push((name.to_string(), Some(api_key.to_string())));
        }
        // Relay keys additionally go out Bearer-style; official-prefix keys
        // must not (they stay on the primary header only).
        if !spec.bearer_token_env.is_empty()
            && (spec.official_key_prefix.is_empty()
                || !api_key.starts_with(spec.official_key_prefix))
        {
            out.push((
                spec.bearer_token_env.to_string(),
                Some(api_key.to_string()),
            ));
        }
    }
    if !base_url.is_empty() {
        for name in spec.base_url_envs {
            out.push((name.to_string(), Some(base_url.to_string())));
        }
    }
    if apply_clear_envs {
        for name in spec.clear_envs {
            out.push((name.to_string(), None));
        }
    }
    out
}

/// command resolution (ChatController.cpp:867-869): agent cliPath trimmed,
/// else the provider defaultCommand. May still be empty (custom provider
/// without a cliPath) — the ACP transport rejects that at spawn time
/// ("未配置 CLI 命令"). testAgent has its own extra fallback to "kimi"
/// (AgentStore.cpp:281-285) and does not use this helper for that last step.
pub fn resolve_command(spec: Option<&ProviderSpec>, cli_path: &str) -> String {
    let cli = cli_path.trim();
    if !cli.is_empty() {
        return cli.to_string();
    }
    spec.map(|s| s.default_command.to_string())
        .unwrap_or_default()
}

/// args = acpArgs + extraArgs split shell-style (quotes group words, like
/// QProcess::splitCommand). For the custom provider acpArgs is empty, so
/// extraArgs IS the whole arg list (ChatController.cpp:870-875).
pub fn resolve_args(spec: Option<&ProviderSpec>, extra_args: &str) -> Vec<String> {
    let mut args: Vec<String> = match spec {
        Some(s) => s.acp_args.iter().map(|a| a.to_string()).collect(),
        None => Vec::new(),
    };
    args.extend(split_extra_args(extra_args));
    args
}

/// Shell-rule split of the user's extraArgs text. On a parse error (e.g. an
/// unclosed quote) fall back to plain whitespace splitting — the old
/// QProcess::splitCommand was similarly lenient in practice.
pub fn split_extra_args(text: &str) -> Vec<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    shell_words::split(trimmed)
        .unwrap_or_else(|_| trimmed.split_whitespace().map(str::to_string).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_five_records_in_order() {
        let ids: Vec<&str> = ids().collect();
        assert_eq!(ids, ["kimi", "claude", "codex", "opencode", "custom"]);
        // Every record: fixed chat_capable, non-empty display/hints.
        for s in REGISTRY {
            assert!(s.chat_capable, "{} chat_capable", s.id);
            assert!(!s.display_name.is_empty());
            assert!(!s.install_hint.is_empty());
            assert!(!s.base_url_hint.is_empty());
        }
    }

    #[test]
    fn registry_field_values_match_spec_table() {
        let kimi = spec("kimi").expect("kimi");
        assert_eq!(kimi.display_name, "Kimi CLI");
        assert_eq!(kimi.default_command, "kimi");
        assert_eq!(kimi.acp_args, &["acp"]);
        assert_eq!(kimi.api_key_envs, &["KIMI_API_KEY", "OPENAI_API_KEY"]);
        assert_eq!(kimi.base_url_envs, &["KIMI_BASE_URL", "OPENAI_BASE_URL"]);
        assert!(kimi.clear_envs.is_empty());
        assert_eq!(kimi.bearer_token_env, "");
        assert_eq!(kimi.official_key_prefix, "");
        assert!(kimi.mode_map.is_empty());

        let claude = spec("claude").expect("claude");
        assert_eq!(claude.display_name, "Claude Code");
        assert_eq!(claude.default_command, "claude-code-acp");
        assert!(claude.acp_args.is_empty());
        assert_eq!(claude.api_key_envs, &["ANTHROPIC_API_KEY"]);
        assert_eq!(claude.base_url_envs, &["ANTHROPIC_BASE_URL"]);
        assert_eq!(
            claude.clear_envs,
            &["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_SSE_PORT"]
        );
        assert_eq!(claude.bearer_token_env, "ANTHROPIC_AUTH_TOKEN");
        assert_eq!(claude.official_key_prefix, "sk-ant-");
        assert_eq!(
            claude.mode_map,
            &[("auto", "acceptEdits"), ("yolo", "bypassPermissions")]
        );

        let codex = spec("codex").expect("codex");
        assert_eq!(codex.display_name, "Codex CLI");
        assert_eq!(codex.default_command, "codex-acp");
        assert!(codex.acp_args.is_empty());
        assert_eq!(codex.api_key_envs, &["OPENAI_API_KEY"]);
        assert_eq!(codex.base_url_envs, &["OPENAI_BASE_URL"]);
        assert!(codex.clear_envs.is_empty());

        let opencode = spec("opencode").expect("opencode");
        assert_eq!(opencode.display_name, "OpenCode");
        assert_eq!(opencode.default_command, "opencode");
        assert_eq!(opencode.acp_args, &["acp"]);
        assert_eq!(opencode.api_key_envs, &["OPENCODE_API_KEY"]);
        assert!(opencode.base_url_envs.is_empty());
        assert!(opencode.clear_envs.is_empty());

        let custom = spec("custom").expect("custom");
        assert_eq!(custom.display_name, "自定义 (ACP)");
        assert_eq!(custom.default_command, "");
        assert!(custom.acp_args.is_empty());
        assert_eq!(custom.api_key_envs, &["OPENAI_API_KEY"]);
        assert_eq!(custom.base_url_envs, &["OPENAI_BASE_URL"]);
    }

    #[test]
    fn spec_lookup_trims_and_lowercases() {
        assert!(spec("  KIMI ").is_some());
        assert!(spec("Claude").is_some());
        assert!(spec("nope").is_none());
        assert!(spec("").is_none());
    }

    #[test]
    fn map_mode_translates_and_passes_through() {
        assert_eq!(map_mode("claude", "auto"), "acceptEdits");
        assert_eq!(map_mode("claude", "yolo"), "bypassPermissions");
        // Unmapped modes pass through unchanged (identity).
        assert_eq!(map_mode("claude", "default"), "default");
        assert_eq!(map_mode("claude", "plan"), "plan");
        // Providers with an empty map are fully identity.
        assert_eq!(map_mode("kimi", "yolo"), "yolo");
        // Unknown provider: identity too.
        assert_eq!(map_mode("nope", "auto"), "auto");
    }

    #[test]
    fn spec_view_joins_acp_args() {
        let kimi = spec_view("kimi").expect("kimi view");
        assert_eq!(kimi.acp_args, "acp");
        let claude = spec_view("claude").expect("claude view");
        assert_eq!(claude.acp_args, "");
        assert!(spec_view("nope").is_none());
        let json = serde_json::to_value(&kimi).expect("serialize");
        assert_eq!(json["displayName"], "Kimi CLI");
        assert_eq!(json["defaultCommand"], "kimi");
        assert_eq!(json["chatCapable"], true);
    }

    #[test]
    fn env_overrides_kimi_injects_all_key_and_url_vars() {
        let kimi = spec("kimi").expect("kimi");
        let env = env_overrides(kimi, "sk-key", "https://x/v1", true);
        assert_eq!(
            env,
            vec![
                ("KIMI_API_KEY".to_string(), Some("sk-key".to_string())),
                ("OPENAI_API_KEY".to_string(), Some("sk-key".to_string())),
                ("KIMI_BASE_URL".to_string(), Some("https://x/v1".to_string())),
                ("OPENAI_BASE_URL".to_string(), Some("https://x/v1".to_string())),
            ]
        );
    }

    #[test]
    fn env_overrides_empty_credentials_inject_nothing() {
        let kimi = spec("kimi").expect("kimi");
        assert!(env_overrides(kimi, "", "", true).is_empty());
        let claude = spec("claude").expect("claude");
        // clearEnvs are applied even without credentials (session start).
        let env = env_overrides(claude, "", "", true);
        assert_eq!(env.len(), 3);
        assert!(env.iter().all(|(_, v)| v.is_none()));
    }

    #[test]
    fn env_overrides_clear_envs_delete_semantics_and_test_mode() {
        let claude = spec("claude").expect("claude");
        let env = env_overrides(claude, "", "", true);
        assert_eq!(
            env,
            vec![
                ("CLAUDECODE".to_string(), None),
                ("CLAUDE_CODE_ENTRYPOINT".to_string(), None),
                ("CLAUDE_CODE_SSE_PORT".to_string(), None),
            ]
        );
        // testAgent mode: clearEnvs NOT applied (old AgentStore behavior).
        assert!(env_overrides(claude, "", "", false).is_empty());
    }

    #[test]
    fn env_overrides_claude_relay_key_special_case() {
        let claude = spec("claude").expect("claude");
        // Relay key: goes to BOTH the primary env and ANTHROPIC_AUTH_TOKEN.
        let env = env_overrides(claude, "relay-key-123", "", false);
        assert_eq!(
            env,
            vec![
                ("ANTHROPIC_API_KEY".to_string(), Some("relay-key-123".to_string())),
                (
                    "ANTHROPIC_AUTH_TOKEN".to_string(),
                    Some("relay-key-123".to_string())
                ),
            ]
        );
        // Official sk-ant- key: primary header only, never Bearer.
        let env = env_overrides(claude, "sk-ant-official", "", false);
        assert_eq!(
            env,
            vec![(
                "ANTHROPIC_API_KEY".to_string(),
                Some("sk-ant-official".to_string())
            )]
        );
        // Ordering for a fully populated session start: keys -> bearer ->
        // baseUrl -> clearEnvs.
        let env = env_overrides(claude, "relay", "https://a", true);
        let names: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            names,
            [
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_AUTH_TOKEN",
                "ANTHROPIC_BASE_URL",
                "CLAUDECODE",
                "CLAUDE_CODE_ENTRYPOINT",
                "CLAUDE_CODE_SSE_PORT"
            ]
        );
    }

    #[test]
    fn resolve_command_chain() {
        let kimi = spec("kimi");
        assert_eq!(resolve_command(kimi, "  C:\\bin\\kimi.exe "), "C:\\bin\\kimi.exe");
        assert_eq!(resolve_command(kimi, "  "), "kimi");
        // custom without cliPath stays empty (rejected later at spawn).
        assert_eq!(resolve_command(spec("custom"), ""), "");
        assert_eq!(resolve_command(None, ""), "");
    }

    #[test]
    fn resolve_args_appends_split_extra_args() {
        let kimi = spec("kimi");
        assert_eq!(resolve_args(kimi, ""), vec!["acp"]);
        assert_eq!(
            resolve_args(kimi, "--flag \"two words\" plain"),
            vec!["acp", "--flag", "two words", "plain"]
        );
        // custom: extraArgs ARE the args.
        assert_eq!(
            resolve_args(spec("custom"), "--experimental-acp"),
            vec!["--experimental-acp"]
        );
        // Unclosed quote falls back to whitespace split.
        assert_eq!(split_extra_args("a \"b"), vec!["a", "\"b"]);
    }
}
