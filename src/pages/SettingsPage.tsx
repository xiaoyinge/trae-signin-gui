// 设置页：数据与存储 / 定时与保活 / 通知 / 启动与托盘 / 外观 / 关于
import React, { useState } from "react";
import { toast } from "sonner";
import { FolderOpen, FolderInput, RefreshCw } from "lucide-react";
import { api } from "@/lib/tauri";
import type { Settings } from "@/lib/tauri";
import { useSettings } from "@/stores";
import { Button, Card, Input, Select, Switch } from "@/components/ui";

export default function SettingsPage() {
  const { settings, version, save, load } = useSettings();
  const [checking, setChecking] = useState(false);

  if (!settings) return null;
  const patch = (p: Partial<Settings>) => save({ ...settings, ...p });

  const doChangeDir = async () => {
    const picked = await api.chooseDataDir();
    if (!picked) return;
    try {
      await api.changeDataDir(picked);
      toast.success("数据目录已迁移");
      await load();
    } catch (e) {
      toast.error(String(e));
    }
  };

  const doCheckUpdate = async () => {
    setChecking(true);
    try {
      const r = await api.checkUpdate();
      if (r.configured) {
        toast.success(r.message);
      } else {
        toast.info(r.message);
      }
    } finally {
      setChecking(false);
    }
  };

  const doTestBark = async () => {
    if (!settings.bark_url.trim()) {
      toast.warning("请先填写 Bark URL");
      return;
    }
    try {
      await api.testBark(settings.bark_url);
      toast.success("测试推送已发送");
    } catch (e) {
      toast.error(String(e));
    }
  };

  return (
    <div className="mx-auto flex max-w-2xl flex-col gap-4">
      <h2 className="text-base font-semibold">设置</h2>

      <Section title="数据与存储">
        <Row label="数据目录">
          <div className="flex items-center gap-2">
            <span
              className="max-w-56 truncate rounded-md bg-[var(--bg)] px-2 py-1 text-xs text-[var(--text-dim)]"
              title={settings.data_dir}
            >
              {settings.data_dir || "-"}
            </span>
            <Button variant="outline" onClick={doChangeDir}>
              <FolderInput size={14} /> 更换并迁移
            </Button>
            <Button variant="ghost" onClick={() => api.openDataDir().catch((e) => toast.error(String(e)))}>
              <FolderOpen size={14} /> 打开
            </Button>
          </div>
        </Row>
        <Row label="每日定点签到" desc="HH:MM，到点自动签到全部未签账号">
          <Input
            type="time"
            className="w-32"
            value={settings.daily_time}
            onChange={(e) => patch({ daily_time: e.target.value })}
          />
        </Row>
        <Row label="周期检查" desc="每 N 分钟检查未签账号并补签">
          <div className="flex items-center gap-2">
            <Switch
              checked={settings.periodic_check_enabled}
              onCheckedChange={(v) => patch({ periodic_check_enabled: v })}
            />
            <Input
              type="number"
              min={1}
              max={720}
              className="w-20"
              value={settings.periodic_check_minutes}
              disabled={!settings.periodic_check_enabled}
              onChange={(e) =>
                patch({ periodic_check_minutes: Math.max(1, Number(e.target.value) || 30) })
              }
            />
            <span className="text-xs text-[var(--text-dim)]">分钟</span>
          </div>
        </Row>
        <Row label="每日 token 保活" desc="每天无条件刷新一次全部账号 token">
          <Switch
            checked={settings.keep_alive_daily}
            onCheckedChange={(v) => patch({ keep_alive_daily: v })}
          />
        </Row>
      </Section>

      <Section title="通知">
        <Row label="Windows 系统通知">
          <Switch checked={settings.notify_system} onCheckedChange={(v) => patch({ notify_system: v })} />
        </Row>
        <Row label="Bark 推送" desc="填入 https://api.day.app/你的Key">
          <div className="flex items-center gap-2">
            <Input
              className="w-64"
              placeholder="https://api.day.app/..."
              value={settings.bark_url}
              onChange={(e) => patch({ bark_url: e.target.value })}
            />
            <Button variant="outline" onClick={doTestBark}>
              测试推送
            </Button>
          </div>
        </Row>
      </Section>

      <Section title="启动与托盘">
        <Row label="开机自启" desc="开启后系统启动时静默进托盘">
          <Switch checked={settings.autostart} onCheckedChange={(v) => patch({ autostart: v })} />
        </Row>
        <Row label="关闭窗口" desc="点击关闭按钮时的行为">
          <Select
            value={settings.close_to_tray ? "tray" : "exit"}
            onValueChange={(v) => patch({ close_to_tray: v === "tray" })}
            options={[
              { value: "tray", label: "隐藏到托盘" },
              { value: "exit", label: "退出程序" },
            ]}
          />
        </Row>
      </Section>

      <Section title="外观">
        <Row label="主题">
          <Select
            value={settings.theme}
            onValueChange={(v) => patch({ theme: v })}
            options={[
              { value: "dark", label: "暗色" },
              { value: "light", label: "浅色" },
              { value: "system", label: "跟随系统" },
            ]}
          />
        </Row>
      </Section>

      <Section title="关于">
        <Row label="版本">
          <span className="text-sm text-[var(--text-dim)]">v{version}</span>
        </Row>
        <Row label="检查更新" desc="更新器为骨架占位，暂未接入发布源">
          <Button variant="outline" onClick={doCheckUpdate} disabled={checking}>
            <RefreshCw size={14} className={checking ? "animate-spin" : undefined} /> 检查更新
          </Button>
        </Row>
      </Section>
    </div>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <Card>
      <h3 className="mb-3 text-sm font-semibold text-[var(--text-dim)]">{title}</h3>
      <div className="flex flex-col gap-3">{children}</div>
    </Card>
  );
}

function Row({
  label,
  desc,
  children,
}: {
  label: string;
  desc?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between gap-4">
      <div>
        <div className="text-sm">{label}</div>
        {desc && <div className="text-xs text-[var(--text-dim)]">{desc}</div>}
      </div>
      {children}
    </div>
  );
}
