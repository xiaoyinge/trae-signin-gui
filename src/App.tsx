// App 布局：侧边栏三页 + 主题应用 + 首次启动引导
import React, { useEffect } from "react";
import { toast } from "sonner";
import { User, History, Settings as SettingsIcon, CircleCheck } from "lucide-react";
import { useUi, useSettings, useEventBridge } from "@/stores";
import type { Page } from "@/stores";
import AccountPage from "@/pages/AccountPage";
import HistoryPage from "@/pages/HistoryPage";
import SettingsPage from "@/pages/SettingsPage";
import { Card, Button } from "@/components/ui";
import { api } from "@/lib/tauri";

const NAV: { key: Page; label: string; icon: React.ReactNode }[] = [
  { key: "account", label: "账号", icon: <User size={18} /> },
  { key: "history", label: "签到历史", icon: <History size={18} /> },
  { key: "settings", label: "设置", icon: <SettingsIcon size={18} /> },
];

export default function App() {
  const page = useUi((s) => s.page);
  const setPage = useUi((s) => s.setPage);
  const theme = useUi((s) => s.theme);
  const { bootstrap, load } = useSettings();
  useEventBridge();

  useEffect(() => {
    load();
  }, [load]);

  // 主题应用
  useEffect(() => {
    const root = document.documentElement;
    if (theme === "system") {
      const dark = window.matchMedia("(prefers-color-scheme: dark)").matches;
      root.classList.toggle("light", !dark);
    } else {
      root.classList.toggle("light", theme === "light");
    }
  }, [theme]);

  if (!bootstrap) {
    return null; // bootstrap 加载中
  }

  const needsSetup = bootstrap.needs_setup;

  return (
    <div className="flex h-full">
      {/* 侧边栏 */}
      <nav className="flex w-16 flex-col items-center gap-1 border-r border-[var(--border)] bg-[var(--bg-soft)] py-4">
        <div className="mb-4 flex h-9 w-9 items-center justify-center rounded-xl bg-[var(--accent)] text-sm font-bold text-white">
          T
        </div>
        {NAV.map((n) => (
          <button
            key={n.key}
            onClick={() => setPage(n.key)}
            title={n.label}
            className={`flex h-10 w-10 cursor-pointer items-center justify-center rounded-lg transition-colors ${
              page === n.key
                ? "bg-[var(--accent-soft)] text-[var(--accent)]"
                : "text-[var(--text-dim)] hover:bg-[var(--bg-hover)] hover:text-[var(--text)]"
            }`}
          >
            {n.icon}
          </button>
        ))}
      </nav>

      {/* 主内容 */}
      <main className="flex-1 overflow-y-auto p-5">
        {needsSetup ? (
          <SetupGuide />
        ) : (
          <>
            {page === "account" && <AccountPage />}
            {page === "history" && <HistoryPage />}
            {page === "settings" && <SettingsPage />}
          </>
        )}
      </main>
    </div>
  );
}

/** 首次启动空状态引导卡（不做向导） */
function SetupGuide() {
  const { bootstrap, load } = useSettings();
  const suggested = bootstrap?.suggested_dir ?? "";

  const pickAndInit = async () => {
    const picked = await api.chooseDataDir();
    const dir = picked ?? suggested; // 取消则用建议目录
    if (!dir) return;
    try {
      await api.initDataDir(dir);
      toast.success("数据目录已就绪");
      await load();
    } catch (e) {
      toast.error(String(e));
    }
  };

  return (
    <div className="mx-auto max-w-xl pt-16">
      <Card className="p-8 text-center">
        <CircleCheck size={44} className="mx-auto mb-4 text-[var(--accent)]" />
        <h1 className="mb-2 text-xl font-semibold">欢迎使用 TRAE 签到</h1>
        <p className="mb-6 text-sm text-[var(--text-dim)]">
          凭证以 <code className="text-[var(--accent)]">auths/trae-*.json</code> 形式存放在你选择的数据目录中，
          与 CLI 版、GitHub Actions secrets 互通。
        </p>
        <div className="flex flex-col items-center gap-3">
          <Button className="w-56" onClick={pickAndInit}>
            1. 选择数据目录（默认 exe 同级）
          </Button>
          <p className="text-xs text-[var(--text-dim)]">
            完成后即可在「账号」页登录第一个账号或导入凭证。
          </p>
        </div>
        <div className="mt-6 border-t border-[var(--border)] pt-4 text-xs text-[var(--text-dim)]">
          建议目录：<span className="text-[var(--text)]">{suggested || "-"}</span>
        </div>
      </Card>
    </div>
  );
}
