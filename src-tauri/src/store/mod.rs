// Persistence layer, field-compatible with the C++/Qt version so existing
// %AppData%/WarDex data (sessions/agents/projects/user_prefs) loads unchanged.
// Formats: sessions/<uuid>/meta.json + messages.jsonl, agents/index.json +
// agents/<id>.json, projects.json, user_prefs.json, todos.json, prompts.json,
// media/ paste cache.
// Ported from src/SessionStore.cpp / AgentStore.cpp / ProjectStore.cpp /
// UserPrefs.cpp / TodoStore.cpp / PromptStore.cpp / ClipboardHelper.cpp /
// AppPaths.cpp (see docs/data-formats.md for the authoritative spec).
//
// Layout: one file per store domain over the shared helpers in json.rs
// (atomic tmp+rename writes, tolerant reads, f64-tolerant timestamps,
// serde-flatten passthrough of unknown fields). Adding a new store domain =
// adding one file here.
//
// Concurrency: stores are plain structs requiring &mut for mutation (the
// session LRU needs it); the Tauri command layer wraps the registry in
// Mutexes. Nothing here holds global state — Paths carries the data root,
// which also keeps unit tests fully isolated.

pub mod agents;
pub mod browse;
pub mod json;
pub mod media;
pub mod paths;
pub mod prefs;
pub mod projects;
pub mod prompts;
pub mod sessions;
pub mod todos;
pub mod workspace;

pub use agents::{mask_key, Agent, AgentPatch, AgentStore, AgentsError};
pub use paths::{canonical_dir, Paths};
pub use prefs::{PanelLayoutEntry, PrefsError, UserPrefs};
pub use projects::{ProjectStore, ProjectsError, RecentEntry};
pub use prompts::{PromptRow, PromptStore, PromptsError};
pub use sessions::{
    AgentSnapshot, MessageRow, SearchEngine, SearchHit, SearchOutcome, SearchTarget,
    SessionIndexRow, SessionMeta, SessionStore, SessionsError,
};
pub use todos::{TodoRow, TodoStore, TodosError};

/// All singleton stores plus the startup initialization order
/// (architecture.md §4): layout first, then each domain loads tolerantly.
/// The Tauri layer constructs this once in `setup` and puts it behind
/// Mutexes.
pub struct StoreRegistry {
    pub paths: Paths,
    pub sessions: SessionStore,
    pub agents: AgentStore,
    pub projects: ProjectStore,
    pub prefs: UserPrefs,
    pub todos: TodoStore,
    pub prompts: PromptStore,
    pub search: SearchEngine,
}

impl StoreRegistry {
    pub fn init(paths: Paths) -> Self {
        // SessionStore::load performs ensure_layout (mkpath + media prune)
        // and discard_empty_sessions, matching the old startup order.
        let sessions = SessionStore::load(paths.clone());
        Self {
            agents: AgentStore::load(&paths),
            projects: ProjectStore::load(&paths),
            prefs: UserPrefs::load(&paths),
            todos: TodoStore::load(&paths),
            prompts: PromptStore::load(&paths),
            sessions,
            search: SearchEngine::new(),
            paths,
        }
    }

    /// Production registry using the dev/release data root discrimination.
    pub fn production() -> Self {
        Self::init(Paths::production())
    }
}
