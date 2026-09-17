// 账号页：卡片列表 + 串行批量签到实时进度 + 登录 + 导入
import React, { useEffect, useState } from "react";
import { toast } from "sonner";
import { Plus, ClipboardPaste, RefreshCw, Zap, Trash2, Loader2, KeyRound } from "lucide-react";
import { api } from "@/lib/tauri";
import type { AccountView, SigninProgress } from "@/lib/tauri";
import { useAccounts } from "@/stores";
import { Button, Card, Dialog, StatusBadge, Textarea, formatExpiry } from "@/components/ui";
import clsx from "clsx";

const STAGE_TEXT: Record<string, string> = {
  waiting: "等待中",
  refreshing: "刷新 token",
  checking: "查询状态",
  claiming: "签到中",
  querying: "查询积分",
  done: "完成",
};

export default function AccountPage() {
  const { accounts, loading, fetchAccounts, signinRunning, setSigninRunning, progress, loginWaiting, setLoginWaiting } =
    useAccounts();
  const [importOpen, setImportOpen] = useState(false);
  const [importText, setImportText] = useState("");
  const [deleteUid, setDeleteUid] = useState<string | null>(null);
  const [working, setWorking] = useState(false);

  useEffect(() => {
    fetchAccounts();
  }, [fetchAccounts]);

  const doSigninAll = async () => {
    if (signinRunning) {
      toast.warning("签到进行中");
      return;
    }
    setSigninRunning(true);
    try {
      const s = await api.signinAll();
      if (s.total === 0) {
        toast.info("没有账号");
      } else {
        toast.success(
          `签到完成：成功 ${s.ok} / 已签 ${s.already} / 禁用 ${s.disabled} / 失败 ${s.failed}` +
            (s.retryable_failed > 0 ? ` / 限流 ${s.retryable_failed}（稍后自动重试）` : ""),
        );
      }
    } catch (e) {
      toast.error(String(e));
    } finally {
      setSigninRunning(false);
      fetchAccounts();
    }
  };

  const doRefreshAll = async () => {
    setWorking(true);
    try {
      await api.refreshAll();
      toast.success("已刷新全部账号");
      fetchAccounts();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setWorking(false);
    }
  };

  const doSigninOne = async (uid: string) => {
    setSigninRunning(true);
    try {
      const s = await api.signinOne(uid);
      const a = accounts.find((x) => x.uid === uid);
      toast.success(`${a?.nickname ?? uid}：${summarize(s)}`);
    } catch (e) {
      toast.error(String(e));
    } finally {
      setSigninRunning(false);
      fetchAccounts();
    }
  };

  const doImport = async () => {
    try {
      const r = await api.importCredential(importText);
      toast.success(`已导入 ${r.nickname || r.uid}`);
      setImportOpen(false);
      setImportText("");
      fetchAccounts();
    } catch (e) {
      toast.error(String(e));
    }
  };

  const doDelete = async () => {
    if (!deleteUid) return;
    try {
      await api.deleteAccount(deleteUid);
      toast.success("已删除");
    } catch (e) {
      toast.error(String(e));
    } finally {
      setDeleteUid(null);
      fetchAccounts();
    }
  };

  const doCancelLogin = () => {
    api.cancelLogin();
    setLoginWaiting(null);
  };

  return (
    <div className="mx-auto max-w-3xl">
      {/* 工具栏 */}
      <div className="mb-4 flex flex-wrap gap-2">
        <Button onClick={() => api.startLogin().catch((e) => toast.error(String(e)))}>
          <Plus size={15} /> 添加账号
        </Button>
        <Button variant="outline" onClick={() => setImportOpen(true)}>
          <ClipboardPaste size={15} /> 导入凭证
        </Button>
        <div className="flex-1" />
        <Button onClick={doSigninAll} disabled={signinRunning || accounts.length === 0}>
          {signinRunning ? <Loader2 size={15} className="animate-spin" /> : <Zap size={15} />}
          全部签到
        </Button>
        <Button variant="outline" onClick={doRefreshAll} disabled={working || accounts.length === 0}>
          <RefreshCw size={15} className={clsx(working && "animate-spin")} /> 全部刷新
        </Button>
      </div>

      {/* 登录等待卡 */}
      {loginWaiting && loginWaiting.stage !== "done" && (
        <Card className="mb-4 border-[var(--accent)]">
          <div className="flex items-center gap-3">
            <Loader2 size={18} className="animate-spin text-[var(--accent)]" />
            <div className="flex-1">
              <div className="text-sm font-medium">等待浏览器授权…</div>
              <div className="text-xs text-[var(--text-dim)]">{loginWaiting.message}</div>
            </div>
            <Button variant="ghost" onClick={doCancelLogin}>
              取消
            </Button>
          </div>
        </Card>
      )}

      {/* 账号列表 */}
      {accounts.length === 0 && !loading ? (
        <Card className="py-12 text-center text-sm text-[var(--text-dim)]">
          <KeyRound size={32} className="mx-auto mb-3 text-[var(--border)]" />
          还没有账号，点击「添加账号」登录，或「导入凭证」粘贴 CLI 版的 auths/trae-*.json 内容。
        </Card>
      ) : (
        <div className="flex flex-col gap-3">
          {accounts.map((a) => (
            <AccountCard
              key={a.uid}
              account={a}
              progress={progress[a.uid]}
              busy={signinRunning}
              onSignin={() => doSigninOne(a.uid)}
              onRefresh={async () => {
                try {
                  await api.refreshAccount(a.uid);
                  toast.success("已刷新");
                  fetchAccounts();
                } catch (e) {
                  toast.error(String(e));
                }
              }}
              onDelete={() => setDeleteUid(a.uid)}
            />
          ))}
        </div>
      )}

      {/* 导入凭证弹窗 */}
      <Dialog open={importOpen} onOpenChange={setImportOpen} title="导入凭证 JSON" width={480}>
        <p className="mb-2 text-xs text-[var(--text-dim)]">
          粘贴 auths/trae-*.json 文件完整内容（兼容 CLI 版嵌套格式与扁平格式）。
        </p>
        <Textarea
          className="h-40 w-full font-mono text-xs"
          value={importText}
          onChange={(e: React.ChangeEvent<HTMLTextAreaElement>) => setImportText(e.target.value)}
          placeholder='{"auth": {...}, "account": {...}}'
        />
        <div className="mt-4 flex justify-end gap-2">
          <Button variant="ghost" onClick={() => setImportOpen(false)}>
            取消
          </Button>
          <Button onClick={doImport} disabled={!importText.trim()}>
            导入
          </Button>
        </div>
      </Dialog>

      {/* 删除确认 */}
      <Dialog open={deleteUid !== null} onOpenChange={(v) => !v && setDeleteUid(null)} title="删除账号">
        <p className="text-sm">
          确定删除账号 <b>{accounts.find((a) => a.uid === deleteUid)?.nickname || deleteUid}</b>{" "}
          吗？对应凭证文件将从数据目录移除。
        </p>
        <div className="mt-4 flex justify-end gap-2">
          <Button variant="ghost" onClick={() => setDeleteUid(null)}>
            取消
          </Button>
          <Button variant="danger" onClick={doDelete}>
            删除
          </Button>
        </div>
      </Dialog>
    </div>
  );
}

function summarize(s: { ok: number; already: number; disabled: number; failed: number }) {
  if (s.ok) return "签到成功";
  if (s.already) return "今日已签到";
  if (s.disabled) return "签到功能未启用";
  return "签到失败";
}

function AccountCard({
  account,
  progress,
  busy,
  onSignin,
  onRefresh,
  onDelete,
}: {
  account: AccountView;
  progress?: SigninProgress;
  busy: boolean;
  onSignin: () => void;
  onRefresh: () => void;
  onDelete: () => void;
}) {
  const running = progress && progress.stage !== "done";
  // 一轮结束时若状态并非已签到类，标签不得写「完成」——读起来像签成功了
  const stageText = (() => {
    if (!progress) return "";
    const { stage, status } = progress;
    if (stage === "done" && status !== "ok" && status !== "already" && status !== "disabled") {
      return "结束";
    }
    return STAGE_TEXT[stage] ?? stage;
  })();
  return (
    <Card>
      <div className="flex items-start gap-3">
        <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full bg-[var(--accent-soft)] text-sm font-bold text-[var(--accent)]">
          {(account.nickname || account.uid).slice(0, 1).toUpperCase()}
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-medium">{account.nickname || "未命名账号"}</span>
            <StatusBadge status={account.status} />
            {account.need_relogin && <span className="badge badge-failed">需重新登录</span>}
            {account.refresh_failed && <span className="badge badge-unknown">刷新失败(可重试)</span>}
            {account.credits !== null && (
              <span className="text-xs text-[var(--text-dim)]">积分 {account.credits}</span>
            )}
          </div>
          <div className="mt-1 text-xs text-[var(--text-dim)]">
            <span title={account.uid}>UID {account.uid.length > 14 ? account.uid.slice(0, 14) + "…" : account.uid}</span>
            <span className="mx-2">·</span>
            token 至 {formatExpiry(account.expires_at)}
          </div>
          {/* 实时进度 */}
          {progress && (
            <div className="mt-2 text-xs text-[var(--accent)]">
              {running && <Loader2 size={11} className="mr-1 inline animate-spin" />}
              {stageText}
              {progress.message ? `：${progress.message}` : ""}
              {progress.credits !== null && progress.stage === "done" ? `（积分 ${progress.credits}）` : ""}
            </div>
          )}
        </div>
        <div className="flex shrink-0 gap-1">
          <Button variant="ghost" onClick={onRefresh} title="刷新状态与积分">
            <RefreshCw size={14} />
          </Button>
          <Button variant="ghost" onClick={onSignin} disabled={busy} title="签到">
            <Zap size={14} />
          </Button>
          <Button variant="danger" onClick={onDelete} title="删除">
            <Trash2 size={14} />
          </Button>
        </div>
      </div>
    </Card>
  );
}
