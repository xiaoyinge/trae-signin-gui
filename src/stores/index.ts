// zustand stores：账号 / 设置 / UI 状态
import { create } from "zustand";
import { toast } from "sonner";
import {
  api,
  onSigninProgress,
  onAccountsChanged,
  onLoginProgress,
  onLoginDone,
  onLoginFailed,
} from "@/lib/tauri";
import type {
  AccountView,
  Bootstrap,
  Settings,
  SigninProgress,
  LoginProgress,
  UnlistenFn,
} from "@/lib/tauri";
import { useEffect } from "react";

// ── UI store ──

export type Page = "account" | "history" | "settings";

interface UiState {
  page: Page;
  theme: Settings["theme"];
  setPage: (p: Page) => void;
  setTheme: (t: Settings["theme"]) => void;
}

export const useUi = create<UiState>((set) => ({
  page: "account",
  theme: "dark",
  setPage: (page) => set({ page }),
  setTheme: (theme) => set({ theme }),
}));

// ── 账号 store ──

interface AccountState {
  accounts: AccountView[];
  loading: boolean;
  signinRunning: boolean;
  /** 签到实时进度：uid → 最近一条 progress */
  progress: Record<string, SigninProgress>;
  loginWaiting: LoginProgress | null;
  fetchAccounts: () => Promise<void>;
  setSigninRunning: (v: boolean) => void;
  setLoginWaiting: (p: LoginProgress | null) => void;
}

export const useAccounts = create<AccountState>((set) => ({
  accounts: [],
  loading: false,
  signinRunning: false,
  progress: {},
  loginWaiting: null,
  fetchAccounts: async () => {
    set({ loading: true });
    try {
      const accounts = await api.listAccounts();
      set({ accounts });
    } finally {
      set({ loading: false });
    }
  },
  setSigninRunning: (v) => set({ signinRunning: v }),
  setLoginWaiting: (p) => set({ loginWaiting: p }),
}));

// ── 设置 store ──

interface SettingsState {
  settings: Settings | null;
  bootstrap: Bootstrap | null;
  version: string;
  load: () => Promise<void>;
  save: (s: Settings) => Promise<void>;
}

export const useSettings = create<SettingsState>((set) => ({
  settings: null,
  bootstrap: null,
  version: "",
  load: async () => {
    const bootstrap = await api.bootstrap();
    const version = await api.appVersion();
    let settings: Settings | null = null;
    if (!bootstrap.needs_setup) {
      settings = await api.getSettings();
      useUi.getState().setTheme(settings.theme);
    }
    set({ bootstrap, settings, version });
  },
  save: async (s) => {
    await api.setSettings(s);
    set({ settings: s });
    useUi.getState().setTheme(s.theme);
  },
}));

// ── 事件桥：在 App 挂载时注册一次 ──

export function useEventBridge() {
  useEffect(() => {
    const unlisteners: UnlistenFn[] = [];
    const push = async (p: Promise<UnlistenFn>) => {
      unlisteners.push(await p);
    };

    push(
      onSigninProgress((p) => {
        const cur = useAccounts.getState().progress;
        useAccounts.setState({ progress: { ...cur, [p.uid]: p } });
      }),
    );
    push(
      onAccountsChanged(() => {
        useAccounts.getState().fetchAccounts();
      }),
    );
    push(onLoginProgress((p) => useAccounts.getState().setLoginWaiting(p)));
    push(
      onLoginDone((p) => {
        useAccounts.getState().setLoginWaiting(null);
        useAccounts.getState().fetchAccounts();
        toast.success(`登录成功：${p.nickname || p.uid || ""}`);
      }),
    );
    push(
      onLoginFailed((p) => {
        if (p.stage !== "cancelled") {
          useAccounts.getState().setLoginWaiting(null);
          toast.error(`登录失败：${p.message}`);
        }
      }),
    );

    return () => {
      unlisteners.forEach((u) => u());
    };
  }, []);
}
