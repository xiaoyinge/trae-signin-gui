// 基础 UI 组件（Radix 封装，风格参考 workbuddy-switch 的暗色卡片风）
import React from "react";
import * as SwitchPrimitive from "@radix-ui/react-switch";
import * as DialogPrimitive from "@radix-ui/react-dialog";
import * as SelectPrimitive from "@radix-ui/react-select";
import { Check, ChevronDown, X } from "lucide-react";
import type { CheckinStatus } from "@/lib/tauri";
import clsx from "clsx";

// ── Button ──

type ButtonVariant = "default" | "ghost" | "danger" | "outline";

export function Button({
  variant = "default",
  className,
  ...props
}: React.ButtonHTMLAttributes<HTMLButtonElement> & { variant?: ButtonVariant }) {
  const base =
    "inline-flex items-center justify-center gap-1.5 rounded-lg px-3 py-1.5 text-sm font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer";
  const styles: Record<ButtonVariant, string> = {
    default: "bg-[var(--accent)] text-white hover:opacity-85",
    ghost: "text-[var(--text-dim)] hover:bg-[var(--bg-hover)] hover:text-[var(--text)]",
    danger: "bg-transparent text-[var(--danger)] hover:bg-[rgba(239,68,68,0.12)]",
    outline:
      "border border-[var(--border)] text-[var(--text)] hover:bg-[var(--bg-hover)]",
  };
  return <button className={clsx(base, styles[variant], className)} {...props} />;
}

// ── Card ──

export function Card({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={clsx(
        "rounded-xl border border-[var(--border)] bg-[var(--bg-soft)] p-4",
        className,
      )}
      {...props}
    />
  );
}

// ── 状态徽章 ──

export const STATUS_META: Record<CheckinStatus, { label: string; cls: string }> = {
  ok: { label: "已签", cls: "badge-ok" },
  already: { label: "已签(重复)", cls: "badge-already" },
  disabled: { label: "禁用", cls: "badge-disabled" },
  failed: { label: "失败", cls: "badge-failed" },
  unknown: { label: "未知", cls: "badge-unknown" },
};

export function StatusBadge({ status }: { status: CheckinStatus }) {
  const meta = STATUS_META[status] ?? STATUS_META.unknown;
  return <span className={clsx("badge", meta.cls)}>{meta.label}</span>;
}

// ── Input ──

export function Input({ className, ...props }: React.InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      className={clsx(
        "rounded-lg border border-[var(--border)] bg-[var(--bg)] px-3 py-1.5 text-sm text-[var(--text)] outline-none focus:border-[var(--accent)]",
        className,
      )}
      {...props}
    />
  );
}

// ── Textarea ──

export function Textarea({
  className,
  ...props
}: React.TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return (
    <textarea
      className={clsx(
        "rounded-lg border border-[var(--border)] bg-[var(--bg)] px-3 py-2 text-sm text-[var(--text)] outline-none focus:border-[var(--accent)]",
        className,
      )}
      {...props}
    />
  );
}

// ── Switch ──

export function Switch({
  checked,
  onCheckedChange,
}: {
  checked: boolean;
  onCheckedChange: (v: boolean) => void;
}) {
  return (
    <SwitchPrimitive.Root
      checked={checked}
      onCheckedChange={onCheckedChange}
      className={clsx(
        "relative h-5 w-9 cursor-pointer rounded-full transition-colors",
        checked ? "bg-[var(--accent)]" : "bg-[var(--border)]",
      )}
    >
      <SwitchPrimitive.Thumb className="block h-4 w-4 translate-x-0.5 rounded-full bg-white transition-transform data-[state=checked]:translate-x-[18px]" />
    </SwitchPrimitive.Root>
  );
}

// ── Dialog ──

export function Dialog({
  open,
  onOpenChange,
  title,
  children,
  width,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  title: string;
  children: React.ReactNode;
  width?: number;
}) {
  return (
    <DialogPrimitive.Root open={open} onOpenChange={onOpenChange}>
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay className="fixed inset-0 bg-black/50" />
        <DialogPrimitive.Content
          className="fixed left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2 rounded-xl border border-[var(--border)] bg-[var(--bg-soft)] p-5 shadow-xl focus:outline-none"
          style={{ width: width ?? 420 }}
        >
          <div className="mb-3 flex items-center justify-between">
            <DialogPrimitive.Title className="text-base font-semibold">
              {title}
            </DialogPrimitive.Title>
            <DialogPrimitive.Close asChild>
              <button className="text-[var(--text-dim)] hover:text-[var(--text)] cursor-pointer">
                <X size={16} />
              </button>
            </DialogPrimitive.Close>
          </div>
          {children}
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}

// ── Select ──

export function Select<T extends string>({
  value,
  onValueChange,
  options,
  className,
}: {
  value: T;
  onValueChange: (v: T) => void;
  options: { value: T; label: string }[];
  className?: string;
}) {
  return (
    <SelectPrimitive.Root value={value} onValueChange={onValueChange}>
      <SelectPrimitive.Trigger
        className={clsx(
          "inline-flex items-center justify-between gap-2 rounded-lg border border-[var(--border)] bg-[var(--bg)] px-3 py-1.5 text-sm outline-none focus:border-[var(--accent)] cursor-pointer min-w-28",
          className,
        )}
      >
        <SelectPrimitive.Value />
        <SelectPrimitive.Icon>
          <ChevronDown size={14} />
        </SelectPrimitive.Icon>
      </SelectPrimitive.Trigger>
      <SelectPrimitive.Portal>
        <SelectPrimitive.Content
          position="popper"
          className="z-50 rounded-lg border border-[var(--border)] bg-[var(--bg-soft)] shadow-lg"
        >
          <SelectPrimitive.Viewport className="p-1">
            {options.map((o) => (
              <SelectPrimitive.Item
                key={o.value}
                value={o.value}
                className="flex cursor-pointer items-center justify-between rounded-md px-2.5 py-1.5 text-sm outline-none data-[highlighted]:bg-[var(--bg-hover)]"
              >
                <SelectPrimitive.ItemText>{o.label}</SelectPrimitive.ItemText>
                <SelectPrimitive.ItemIndicator>
                  <Check size={13} />
                </SelectPrimitive.ItemIndicator>
              </SelectPrimitive.Item>
            ))}
          </SelectPrimitive.Viewport>
        </SelectPrimitive.Content>
      </SelectPrimitive.Portal>
    </SelectPrimitive.Root>
  );
}

// ── 工具 ──

export function formatExpiry(ts: number | null): string {
  if (!ts) return "-";
  const d = new Date(ts * 1000);
  const now = Date.now() / 1000;
  const days = Math.floor((ts - now) / 86400);
  const dateStr = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
  if (days < 0) return `${dateStr}（已过期）`;
  if (days <= 3) return `${dateStr}（${days}天后到期）`;
  return dateStr;
}

export function formatTime(ts: number): string {
  const d = new Date(ts * 1000);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")} ${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}
