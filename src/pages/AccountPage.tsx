// 账号页：卡片列表 + 串行批量签到实时进度 + 登录 + 导入
import React, { useEffect, useState } from "react";
import { toast } from "sonner";
import { Plus, ClipboardPaste, RefreshCw, Zap, Trash2, Loader2, KeyRound } from "lucide-react";
import { api } from "@/lib/tauri";
import type { AccountView, SigninProgress } from "@/lib/tauri";
import { useAccounts } from "@/stores";
import { Button, Card, Dialog, Input, StatusBadge, Textarea, formatExpiry } from "@/components/ui";
import clsx from "clsx";

// stage 仅三种实际 emit 的取值（L3：waiting/refreshing/claiming/querying 从未 emit，已删）
const STAGE_TEXT: Record<string, string> = {
  checking: "查询状态",
  skipped: "跳过",
  done: "完成",
};

export default function AccountPage() {
  const { accounts, loading, fetchAccounts, signinRunning, setSigninRunning, progress, loginWaiting, setLoginWaiting } =
    useAccounts();
  const [importOpen, setImportOpen] = useState(false);
  const [importText, setImportText] = useState("");
  const [deleteUid, setDeleteUid] = useState<string | null>(null);
  const [deviceIdUid, setDeviceIdUid] = useState<string | null>(null);
  const [deviceIdInput, setDeviceIdInput] = useState("");
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
      } else if (s.skipped_accounts === s.total) {
        toast.info("所有账号今日均已签或已禁用，无需签到");
      } else {
        toast.success(
          `签到完成：成功 ${s.ok} / 已签 ${s.already} / 禁用 ${s.disabled} / 失败 ${s.failed}` +
            (s.retryable_failed > 0 ? ` / 限流 ${s.retryable_failed}（稍后自动重试）` : "") +
            (s.skipped_accounts > 0 ? ` / 跳过 ${s.skipped_accounts}` : ""),
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
      if (s.retryable_failed > 0) {
        toast.info(
          "若为新账号且详情报 9074：上游已收紧设备校验，点账号卡 🔑「设置设备号」→「自动检测本机客户端」填入真号（一次性，见排障手册 3.9）",
        );
      }
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

  const doSetDeviceId = async () => {
    if (!deviceIdUid) return;
    try {
      await api.setDeviceId(deviceIdUid, deviceIdInput);
      toast.success("设备号已更新");
      setDeviceIdUid(null);
      setDeviceIdInput("");
      fetchAccounts();
    } catch (e) {
      toast.error(String(e));
    }
  };

  const doDetectDeviceId = async () => {
    try {
      const ids = await api.detectClientDeviceIds();
      if (ids.length === 0) {
        toast.error("未检测到本机 TRAE 客户端设备号（未装客户端或未登录过）");
      } else if (ids.length > 1) {
        toast.info(`检测到 ${ids.length} 个设备号，无法自动判定归属，请确认后手动填入：${ids.join("、")}`);
      } else {
        setDeviceIdInput(ids[0]);
        toast.success("已填入本机客户端设备号");
      }
    } catch (e) {
      toast.error(String(e));
    }
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
              onSetDeviceId={() => {
                setDeviceIdUid(a.uid);
                setDeviceIdInput("");
              }}
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
      {/* 设置设备号 */}
      <Dialog open={deviceIdUid !== null} onOpenChange={(v) => !v && setDeviceIdUid(null)} title="设置设备号">
        <p className="mb-2 text-sm">
          为 <b>{accounts.find((a) => a.uid === deviceIdUid)?.nickname || deviceIdUid}</b>{" "}
          填入真实设备号。
        </p>
        <p className="mb-2 text-xs text-[var(--text-dim)]">
          适用场景：新账号签到报 9074「参与用户太多」（上游已收紧设备校验，应用生成的号过不了首次绑定）。
          在<b>装过 TRAE 客户端并登录过该账号</b>的机器上，打开{" "}
          <code>%APPDATA%\Trae CN\User\globalStorage\storage.json</code>，找到{" "}
          <code>iCubeAuthInfo://icube-dc:</code> 开头的键名，冒号后的 16 位数字即设备号。
          注意一个号一天只能签一个账号。
        </p>
        <Input
          className="w-full font-mono"
          value={deviceIdInput}
          onChange={(e: React.ChangeEvent<HTMLInputElement>) => setDeviceIdInput(e.target.value)}
          placeholder="16 位纯数字设备号"
        />
        <div className="mt-4 flex items-center">
          <Button variant="ghost" onClick={doDetectDeviceId}>
            自动检测本机客户端
          </Button>
          <div className="flex-1" />
          <Button variant="ghost" onClick={() => setDeviceIdUid(null)}>
            取消
          </Button>
          <Button onClick={doSetDeviceId} disabled={!deviceIdInput.trim()}>
            保存
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
  onSetDeviceId,
}: {
  account: AccountView;
  progress?: SigninProgress;
  busy: boolean;
  onSignin: () => void;
  onRefresh: () => void;
  onDelete: () => void;
  onSetDeviceId: () => void;
}) {
  const running = progress && progress.stage !== "done" && progress.stage !== "skipped";
  // 一轮结束时若状态并非已签到类，标签不得写「完成」——读起来像签成功了
  const stageText = (() => {
    if (!progress) return "";
    const { stage, status } = progress;
    if (
      stage === "done" &&
      status !== "ok" &&
      status !== "already" &&
      status !== "disabled" &&
      // 刷新轮的 done 事件（未签）语义就是「刷新完成」，不算签到失败
      status !== "notsigned"
    ) {
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
          <Button variant="ghost" onClick={onSetDeviceId} title="设置设备号（客户端真号）">
            <KeyRound size={14} />
          </Button>
          <Button variant="danger" onClick={onDelete} title="删除">
            <Trash2 size={14} />
          </Button>
        </div>
      </div>
    </Card>
  );
}
