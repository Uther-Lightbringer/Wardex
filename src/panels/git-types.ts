// Shared git DTOs for the version-control panel and the commit dialog
// (mirrors src-tauri inspect/git.rs serde output).

export interface GitCommit {
  hash: string;
  short: string;
  author: string;
  date: string;
  subject: string;
}

export interface GitStatusEntry {
  path: string;
  index: string; // staged column (' ' = clean, '?' = untracked)
  worktree: string; // unstaged column
  orig_path: string; // renames only
}

export interface DiffLine {
  kind: 'meta' | 'hunk' | 'add' | 'del' | 'ctx' | 'eof';
  old_lineno: number | null;
  new_lineno: number | null;
  text: string;
}

export interface FileDiff {
  path: string;
  binary: boolean;
  lines: DiffLine[];
}

export interface GitDiff {
  files: FileDiff[];
  truncated: boolean;
}
