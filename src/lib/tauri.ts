// Tauri invoke / event 封装 + 共享类型
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { UnlistenFn } from "@tauri-apps/api/event";

// ── 类型（与 Rust 侧 serde 结构对应） ──

export type CheckinStatus = "ok" | "already" | "disabled" | "failed" | "unknown";

export interface AccountView {
  uid: string;
  nickname: string;
  credits: number | null;
  status: CheckinStatus;
  expires_at: number | null;
  need_relogin: boolean;
  refresh_failed: boolean;
  last_run_ts: number | null;
}

export interface Settings {
  data_dir: string;
  daily_time: string;
  periodic_check_enabled: boolean;
  periodic_check_minutes: number;
  keep_alive_daily: boolean;
  notify_system: boolean;
  bark_url: string;
  close_to_tray: boolean;
  autostart: boolean;
  theme: "dark" | "light" | "system";
}

export interface Bootstrap {
  needs_setup: boolean;
  suggested_dir: string;
  data_dir: string | null;
}

export interface HistoryEntry {
  ts: number;
  uid: string;
  nickname: string;
  status: CheckinStatus;
  credits: number | null;
  message: string;
}

export interface SigninProgress {
  uid: string;
  nickname: string;
  stage: string;
  status: CheckinStatus | null;
  message: string;
  credits: number | null;
}

export interface SigninSummary {
  total: number;
  ok: number;
  already: number;
  disabled: number;
  failed: number;
  /** 本轮无进展、稍后会自动重试的账号数（上游限流/网络类） */
  retryable_failed: number;
  /** true = 另一轮占用互斥，本轮未执行 */
  skipped: boolean;
}

export interface LoginProgress {
  stage: string;
  message: string;
  uid?: string;
  nickname?: string;
}

// ── 命令封装 ──

export const api = {
  bootstrap: () => invoke<Bootstrap>("bootstrap"),
  chooseDataDir: () => invoke<string | null>("choose_data_dir"),
  initDataDir: (path: string) => invoke<Settings>("init_data_dir", { path }),
  changeDataDir: (path: string) => invoke<Settings>("change_data_dir", { path }),
  openDataDir: () => invoke<void>("open_data_dir"),
  openExternal: (url: string) => invoke<void>("open_external", { url }),
  getSettings: () => invoke<Settings>("get_settings"),
  setSettings: (settings: Settings) => invoke<void>("set_settings", { settings }),
  checkUpdate: () => invoke<{ configured: boolean; message: string }>("check_update"),
  appVersion: () => invoke<string>("app_version"),
  listAccounts: () => invoke<AccountView[]>("list_accounts"),
  importCredential: (json: string) =>
    invoke<{ uid: string; nickname: string }>("import_credential", { json }),
  deleteAccount: (uid: string) => invoke<boolean>("delete_account", { uid }),
  startLogin: () => invoke<number>("start_login"),
  cancelLogin: () => invoke<void>("cancel_login"),
  signinAll: () => invoke<SigninSummary>("signin_all"),
  signinOne: (uid: string) => invoke<SigninSummary>("signin_one", { uid }),
  refreshAll: () => invoke<void>("refresh_all"),
  refreshAccount: (uid: string) => invoke<void>("refresh_account", { uid }),
  getHistory: (limit?: number) => invoke<HistoryEntry[]>("get_history", { limit: limit ?? 200 }),
  testBark: (url: string) => invoke<void>("test_bark", { url }),
};

// ── 事件订阅 ──

export const onSigninProgress = (cb: (p: SigninProgress) => void) =>
  listen<SigninProgress>("signin://progress", (e) => cb(e.payload));
export const onSigninDone = (cb: (s: SigninSummary) => void) =>
  listen<SigninSummary>("signin://done", (e) => cb(e.payload));
export const onLoginProgress = (cb: (p: LoginProgress) => void) =>
  listen<LoginProgress>("login://progress", (e) => cb(e.payload));
export const onLoginDone = (cb: (p: LoginProgress) => void) =>
  listen<LoginProgress>("login://done", (e) => cb(e.payload));
export const onLoginFailed = (cb: (p: LoginProgress) => void) =>
  listen<LoginProgress>("login://failed", (e) => cb(e.payload));
export const onAccountsChanged = (cb: () => void) =>
  listen("accounts://changed", () => cb());
export const onTrayStatus = (
  cb: (s: { signed: number; total: number; red_dot: boolean }) => void,
) => listen<{ signed: number; total: number; red_dot: boolean }>("tray://status", (e) => cb(e.payload));

export type { UnlistenFn };
