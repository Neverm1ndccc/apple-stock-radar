import { useEffect, useState } from "react";
import { Play, RefreshCw, Radar } from "lucide-react";
import { command } from "./api";
import {
  dateTime,
  deliveryNames,
  reasonNames,
  regionName,
} from "./presentation";
import type { Settings, Snapshot } from "./types";
import { ScanToggle } from "./ScanToggle";

export function ScannerSettings({
  snapshot,
  busy,
  save,
  run,
}: {
  snapshot: Snapshot;
  busy: boolean;
  save: (patch: Partial<Settings>) => Promise<boolean>;
  run: (action: () => Promise<unknown>, success?: string) => Promise<boolean>;
}) {
  const { settings, scanner, automatic_available: available } = snapshot;
  const [interval, setInterval] = useState(
    String(settings.scan_interval_seconds),
  );
  useEffect(() => {
    setInterval(String(settings.scan_interval_seconds));
  }, [settings.scan_interval_seconds]);
  const seconds = Number(interval);
  const valid =
    interval.trim() !== "" &&
    Number.isInteger(seconds) &&
    seconds >= 5 &&
    seconds <= 86400;
  const effective = Math.max(
    settings.scan_interval_seconds,
    scanner.minimum_interval_seconds ?? 0,
  );
  const roundSeconds =
    scanner.batch_count * Math.max(effective, scanner.budget_refill_seconds);
  return (
    <section className="panel settings-panel scanner-panel">
      <div className="section-title">
        <Radar size={19} />
        <h2>Apple 商店定时扫描</h2>
        <span className="muted">
          {scanner.access_kind === "personal"
            ? "个人非公开接口查询"
            : available
              ? "来源许可已登记"
              : "尚未启用查询"}
        </span>
      </div>
      <p className="panel-caption">
        查询中国大陆与香港 Apple 商店的到店取货状态。按门店合并最多 20 个
        SKU。优先查询最久未更新的目标；响应包含其他关注门店的精确 SKU
        时复用结果，减少重复请求。设置值是目标间隔，实际还受累计预算约束。
      </p>
      <div className="setting-row">
        <div>
          <strong>个人使用非公开库存接口</strong>
          <p>本机用户允许个人查询，不代表 Apple 授权；遇拒绝或限流立即停扫。</p>
        </div>
        <button
          role="switch"
          aria-label="个人使用非公开库存接口"
          aria-checked={settings.apple_personal_access}
          className={`toggle ${settings.apple_personal_access ? "on" : ""}`}
          disabled={busy}
          onClick={() =>
            void save({
              apple_personal_access: !settings.apple_personal_access,
            })
          }
        >
          <span />
        </button>
      </div>
      {!available && (
        <div className="banner">
          尚未允许个人查询，也没有已登记的来源许可。可以先保存扫描间隔，开启个人查询后再选择
          Apple 模式并启用后台扫描。
        </div>
      )}
      {available && scanner.stop_reason && (
        <div className="notice error" role="alert">
          扫描已停止：{reasonNames[scanner.stop_reason] ?? scanner.stop_reason}
          。来源不会自动重试，请处理原因后明确重新开启。
          {scanner.resume_after &&
            ` 最早允许手动恢复：${dateTime(scanner.resume_after)}；此时间不代表服务端已解除阻断。`}
        </div>
      )}
      <div className="banner">
        应用累计预算：最多暂存 {scanner.budget_capacity} 次请求额度，每{" "}
        {scanner.budget_refill_seconds} 秒恢复 1 次，当前可用{" "}
        {scanner.budget_remaining}{" "}
        次。额度用完后自动等待，立即扫描、重启及恢复备份均不能重置预算。此预算不是
        Apple 公布的配额。
      </div>
      <div className="setting-row">
        <div>
          <strong>低频连接保护</strong>
          <p>
            开启后每批至少间隔 180
            秒，立即扫描也遵守此间隔；保留已保存的目标间隔。降低长期请求压力，但不能保证
            Apple 不再拒绝。
          </p>
        </div>
        <button
          role="switch"
          aria-label="低频连接保护"
          aria-checked={settings.scan_connection_protection}
          className={`toggle ${settings.scan_connection_protection ? "on" : ""}`}
          disabled={busy}
          onClick={() =>
            void save({
              scan_connection_protection: !settings.scan_connection_protection,
            })
          }
        >
          <span />
        </button>
      </div>
      <div className="setting-row">
        <div>
          <strong>停扫时通知飞书</strong>
          <p>
            拒绝、限流或响应异常导致停扫时，发送一次来源故障消息。沿用已配置的飞书机器人与全局开关；重启不补发旧故障。
          </p>
        </div>
        <button
          role="switch"
          aria-label="停扫时通知飞书"
          aria-checked={settings.scan_failure_notifications}
          className={`toggle ${settings.scan_failure_notifications ? "on" : ""}`}
          disabled={busy}
          onClick={() =>
            void save({
              scan_failure_notifications: !settings.scan_failure_notifications,
            })
          }
        >
          <span />
        </button>
      </div>
      {roundSeconds > settings.stale_minutes * 60 && (
        <div className="banner">
          若响应仅覆盖当前门店，按当前 {scanner.batch_count} 批次完整覆盖约需{" "}
          {Math.ceil(roundSeconds / 60)} 分钟，超过记录有效时间{" "}
          {settings.stale_minutes} 分钟。
          部分记录可能过期；减少关注门店可缩短周期。响应复用可能缩短实际覆盖时间。
        </div>
      )}
      <div className="setting-row">
        <div>
          <strong>扫描间隔</strong>
          <p>
            5–86400
            秒。按上次请求完成时间计算；来源最低间隔与失败退避优先，不补发休眠期间积压的请求。
          </p>
        </div>
        <div className="scan-interval">
          <input
            aria-label="扫描间隔秒数"
            type="number"
            min={5}
            max={86400}
            step={1}
            value={interval}
            disabled={busy}
            onChange={(e) => setInterval(e.target.value)}
          />
          <span>秒</span>
          <button
            disabled={
              busy || !valid || seconds === settings.scan_interval_seconds
            }
            onClick={() => void save({ scan_interval_seconds: seconds })}
          >
            保存间隔
          </button>
        </div>
      </div>
      <div className="scan-presets">
        {[30, 60, 300, 600, 1800, 3600].map((value) => (
          <button
            key={value}
            disabled={busy}
            onClick={() => setInterval(String(value))}
          >
            {value < 60 ? `${value} 秒` : `${value / 60} 分钟`}
          </button>
        ))}
      </div>
      {!valid && (
        <p className="danger-text panel-caption" role="alert">
          请输入 5–86400 之间的整数秒数。
        </p>
      )}
      <div className="setting-row">
        <div>
          <strong>后台定时扫描</strong>
          <p>
            {settings.scan_running
              ? "已开启；全局暂停或切换来源后不请求。"
              : "关闭；开启后在 Mac 在线且唤醒时持续扫描。"}
          </p>
        </div>
        <ScanToggle snapshot={snapshot} busy={busy} run={run} />
      </div>
      <div className="button-row">
        <button
          disabled={busy || settings.mode === "apple"}
          onClick={() => void save({ mode: "apple" })}
        >
          <Play size={14} />
          切换到 Apple 记录
        </button>
        <button
          disabled={
            busy ||
            !available ||
            settings.paused ||
            settings.mode !== "apple" ||
            scanner.running ||
            !!scanner.stop_reason
          }
          onClick={() =>
            void run(
              () => command("scan_now"),
              "扫描批次已结束，请查看结果与来源状态",
            )
          }
        >
          <RefreshCw size={14} />
          {scanner.running ? "扫描中…" : "立即扫描一批"}
        </button>
      </div>
      <dl>
        <dt>已保存间隔</dt>
        <dd>
          {settings.scan_interval_seconds} 秒
          {scanner.minimum_interval_seconds !== null &&
            ` · ${scanner.access_kind === "personal" ? "应用保护间隔" : "来源下限"} ${scanner.minimum_interval_seconds} 秒 · 有效间隔至少 ${effective} 秒`}
        </dd>
        <dt>查询地区</dt>
        <dd>
          {scanner.allowed_locales.length
            ? scanner.allowed_locales.map(regionName).join(" / ")
            : "尚未登记"}
        </dd>
        <dt>最近扫描</dt>
        <dd>{dateTime(scanner.last_started_at)}</dd>
        <dt>最近请求成功</dt>
        <dd>{dateTime(scanner.last_success_at)}</dd>
        <dt>最近 HTTP 结果</dt>
        <dd>
          {scanner.last_http_status
            ? `HTTP ${scanner.last_http_status}`
            : "尚无响应"}
        </dd>
        <dt>协商协议 / 耗时</dt>
        <dd>
          {settings.scan_last_http_version ?? "尚无记录"}
          {settings.scan_last_duration_ms !== null &&
            ` · ${settings.scan_last_duration_ms} ms`}
        </dd>
        <dt>逐店完整覆盖估算</dt>
        <dd>
          {scanner.batch_count
            ? `约 ${Math.ceil(roundSeconds / 60)} 分钟；复用响应可缩短`
            : "尚无启用目标"}
        </dd>
        <dt>最近停扫提醒</dt>
        <dd>
          {settings.scan_alert
            ? `${dateTime(settings.scan_alert.at)} · ${deliveryNames[settings.scan_alert.status] ?? settings.scan_alert.status}${settings.scan_alert.error ? ` · ${settings.scan_alert.error}` : ""}`
            : "尚无故障提醒"}
        </dd>
        <dt>下次扫描</dt>
        <dd>
          {scanner.next_scan_at
            ? dateTime(scanner.next_scan_at)
            : scanner.running
              ? "当前批次执行中"
              : "未计划"}
        </dd>
        <dt>预算恢复</dt>
        <dd>
          {scanner.budget_ready_at
            ? dateTime(scanner.budget_ready_at)
            : "当前有可用额度"}
        </dd>
        <dt>保护等待至</dt>
        <dd>{scanner.resume_after ? dateTime(scanner.resume_after) : "—"}</dd>
        <dt>最近批次</dt>
        <dd>
          {scanner.checked_targets} 个目标 · 当前 {scanner.batch_count} 个批次
        </dd>
        <dt>连续失败</dt>
        <dd>{scanner.consecutive_failures} 次</dd>
        <dt>停止原因</dt>
        <dd>
          {scanner.stop_reason
            ? (reasonNames[scanner.stop_reason] ?? scanner.stop_reason)
            : "—"}
        </dd>
      </dl>
      <p className="panel-caption">
        401 / 403 / 429 /
        541、拒绝页面或响应结构异常会停用自动扫描。拒绝后的保护冷却依次为
        5、10、20、30 分钟，并遵守
        Retry-After；临时服务失败和网络超时使用退避。冷却结束也不自动探测，需人工明确恢复。
      </p>
    </section>
  );
}
