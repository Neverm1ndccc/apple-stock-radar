import { Play, Square } from "lucide-react";
import { command } from "./api";
import { dateTime, reasonNames } from "./presentation";
import type { Snapshot } from "./types";

export function scanSummary({ settings, scanner }: Snapshot): string {
  if (scanner.stop_reason) {
    const reason = reasonNames[scanner.stop_reason] ?? scanner.stop_reason;
    return `已停扫：${reason}${scanner.resume_after ? `；最早可手动启动 ${dateTime(scanner.resume_after)}` : "；请先处理原因"}`;
  }
  if (!settings.scan_running)
    return scanner.running ? "已关闭，等待当前请求结束。" : "扫描已关闭。";
  if (settings.paused) return "扫描已暂停，请先恢复应用运行。";
  if (scanner.running) return "正在扫描当前批次…";
  return scanner.next_scan_at
    ? `已启动，下次扫描 ${dateTime(scanner.next_scan_at)}`
    : "已启动，等待可扫描的关注目标。";
}

export function ScanToggle({
  snapshot,
  busy,
  run,
}: {
  snapshot: Snapshot;
  busy: boolean;
  run: (action: () => Promise<unknown>, success?: string) => Promise<boolean>;
}) {
  const { settings, scanner, automatic_available: available } = snapshot;
  const waiting =
    !!scanner.resume_after &&
    new Date(scanner.resume_after).getTime() > Date.now();
  const startBlocked =
    !available ||
    settings.mode !== "apple" ||
    settings.paused ||
    waiting ||
    scanner.running;
  const hint = !available
    ? "请在设置中启用 Apple 查询"
    : settings.mode !== "apple"
      ? "请先切换到 Apple 模式"
      : settings.paused
        ? "请先恢复应用运行"
        : waiting
          ? `保护等待至 ${dateTime(scanner.resume_after)}，之后可手动启动`
          : scanner.running
            ? "请等待当前请求结束"
            : "按已保存的间隔启动定时扫描";
  return (
    <button
      className={`scan-toggle ${settings.scan_running ? "" : "primary"}`}
      disabled={busy || (!settings.scan_running && startBlocked)}
      title={settings.scan_running ? "关闭后不再发送后续扫描请求" : hint}
      onClick={() =>
        void run(
          () => command("set_scanning", { enabled: !settings.scan_running }),
          settings.scan_running ? "定时扫描已关闭" : "定时扫描已启动",
        )
      }
    >
      {settings.scan_running ? <Square size={15} /> : <Play size={15} />}
      {settings.scan_running ? "关闭扫描" : "启动扫描"}
    </button>
  );
}
