// 签到历史页：时间倒序列表 + 按账号筛选（默认最近 200 条）
import { useEffect, useMemo, useState } from "react";
import { api } from "@/lib/tauri";
import type { HistoryEntry } from "@/lib/tauri";
import { Card, Select, StatusBadge, formatTime } from "@/components/ui";
import { Loader2 } from "lucide-react";

export default function HistoryPage() {
  const [entries, setEntries] = useState<HistoryEntry[]>([]);
  const [uidFilter, setUidFilter] = useState<string>("all");
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    api
      .getHistory(200)
      .then(setEntries)
      .catch(() => setEntries([]))
      .finally(() => setLoading(false));
  }, []);

  const accountOptions = useMemo(() => {
    const seen = new Map<string, string>();
    for (const e of entries) {
      if (!seen.has(e.uid)) seen.set(e.uid, e.nickname || e.uid);
    }
    return [
      { value: "all", label: "全部账号" },
      ...Array.from(seen.entries()).map(([uid, name]) => ({ value: uid, label: name })),
    ];
  }, [entries]);

  const filtered = useMemo(
    () => (uidFilter === "all" ? entries : entries.filter((e) => e.uid === uidFilter)),
    [entries, uidFilter],
  );

  return (
    <div className="mx-auto max-w-3xl">
      <div className="mb-4 flex items-center gap-3">
        <h2 className="text-base font-semibold">签到历史</h2>
        <span className="text-xs text-[var(--text-dim)]">最近 200 条</span>
        <div className="flex-1" />
        {loading && <Loader2 size={15} className="animate-spin text-[var(--text-dim)]" />}
        <Select value={uidFilter} onValueChange={setUidFilter} options={accountOptions} />
      </div>

      {filtered.length === 0 ? (
        <Card className="py-12 text-center text-sm text-[var(--text-dim)]">暂无记录</Card>
      ) : (
        <div className="flex flex-col gap-2">
          {filtered.map((e, i) => (
            <Card key={`${e.ts}-${e.uid}-${i}`} className="py-3">
              <div className="flex items-center gap-3">
                <span className="w-32 shrink-0 text-xs text-[var(--text-dim)]">
                  {formatTime(e.ts)}
                </span>
                <span className="w-28 shrink-0 truncate text-sm font-medium">
                  {e.nickname || e.uid}
                </span>
                <StatusBadge status={e.status} />
                {e.credits !== null && (
                  <span className="text-xs text-[var(--text-dim)]">积分 {e.credits}</span>
                )}
                <span className="flex-1 truncate text-right text-xs text-[var(--text-dim)]" title={e.message}>
                  {e.message}
                </span>
              </div>
            </Card>
          ))}
        </div>
      )}
    </div>
  );
}
