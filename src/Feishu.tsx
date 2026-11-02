import { useEffect, useState } from "react";
import { Send } from "lucide-react";
import { command } from "./api";
import { errorText } from "./presentation";
import type { Settings, Target } from "./types";

type Run = (
  action: () => Promise<unknown>,
  success?: string,
) => Promise<boolean>;

export function TargetFeishu({
  target,
  settings,
  busy,
  run,
}: {
  target: Target;
  settings: Settings;
  busy: boolean;
  run: Run;
}) {
  return (
    <div className="target-feishu">
      <label>
        <input
          type="checkbox"
          checked={target.feishu_enabled}
          disabled={busy}
          aria-label={`飞书提醒 · ${target.product.model} · ${target.store.name} · ${target.product.color}`}
          onChange={() =>
            void run(() =>
              command("set_target_feishu", {
                id: target.id,
                enabled: !target.feishu_enabled,
              }),
            )
          }
        />
        飞书有货提醒
      </label>
      <button
        disabled={
          busy ||
          !target.feishu_enabled ||
          !target.enabled ||
          !settings.feishu_enabled ||
          settings.paused ||
          target.source === "mock"
        }
        title={
          !settings.feishu_enabled
            ? "请先在设置中配置并开启飞书通知"
            : "发送该关注的当前状态，不会重新查询库存"
        }
        onClick={() =>
          void run(
            () => command("send_target_feishu", { id: target.id }),
            "飞书已确认接收该关注的当前状态",
          )
        }
      >
        <Send size={13} />
        发送到飞书
      </button>
    </div>
  );
}

export function FeishuSettings({
  settings,
  busy,
  run,
}: {
  settings: Settings;
  busy: boolean;
  run: Run;
}) {
  const [configured, setConfigured] = useState(false);
  const [webhook, setWebhook] = useState("");
  const [secret, setSecret] = useState("");
  const [error, setError] = useState("");
  useEffect(() => {
    let active = true;
    command<boolean>("feishu_configured")
      .then((value) => {
        if (active) setConfigured(value);
      })
      .catch((e) => {
        if (active) setError(errorText(e));
      });
    return () => {
      active = false;
    };
  }, []);
  return (
    <section className="panel settings-panel">
      <div className="section-title">
        <Send size={19} />
        <h2>飞书群机器人</h2>
      </div>
      {error && (
        <p className="danger-text" role="alert">
          {error}
        </p>
      )}
      <p className="panel-caption">
        在飞书群添加「自定义机器人」，将 Webhook
        保存于下方。可以创建只有自己的群。若机器人启用关键词校验，请添加“库存雷达”；若启用签名校验，请填写签名密钥。
      </p>
      {!configured && (
        <div className="banner">
          请在下方保存本机配置，或从旧钥匙串导入。升级不会自动读取钥匙串；导入可能需要最后一次系统授权。配置完成前不会自动投递飞书消息。
        </div>
      )}
      <div className="setting-row">
        <div>
          <strong>飞书通知</strong>
          <p>
            开启后，仅为选中的关注发送有货变化提醒。模拟数据不发送；关闭会取消尚未发送的飞书提醒。
          </p>
        </div>
        <button
          role="switch"
          aria-label="飞书通知"
          aria-checked={settings.feishu_enabled}
          className={`toggle ${settings.feishu_enabled ? "on" : ""}`}
          disabled={busy || (!configured && !settings.feishu_enabled)}
          onClick={() =>
            void run(() =>
              command("set_feishu_enabled", {
                enabled: !settings.feishu_enabled,
              }),
            )
          }
        >
          <span />
        </button>
      </div>
      <div className="bark-config">
        <label>
          飞书 Webhook{" "}
          {configured && <span className="green-text">· 已保存在本机</span>}
          <input
            aria-label="飞书 Webhook"
            type="password"
            autoComplete="off"
            spellCheck={false}
            value={webhook}
            placeholder="https://open.feishu.cn/open-apis/bot/v2/hook/…"
            onChange={(e) => setWebhook(e.target.value)}
          />
        </label>
        <label>
          签名密钥（机器人未启用签名时留空）
          <input
            aria-label="飞书签名密钥"
            type="password"
            autoComplete="off"
            spellCheck={false}
            value={secret}
            onChange={(e) => setSecret(e.target.value)}
          />
        </label>
        <div className="button-row">
          <button
            disabled={busy || !webhook.trim()}
            onClick={() => {
              const values = { webhook, secret };
              void run(async () => {
                await command("set_feishu", values);
                setConfigured(true);
                setWebhook("");
                setSecret("");
              }, "飞书本机配置已保存，请测试后开启飞书通知");
            }}
          >
            保存到本机
          </button>
          {!configured && (
            <button
              disabled={busy}
              onClick={() =>
                void run(async () => {
                  await command("import_feishu_keychain");
                  setConfigured(true);
                }, "旧配置已导入本机，请开启飞书通知")
              }
            >
              从旧钥匙串导入（一次）
            </button>
          )}
          <button
            disabled={busy || !configured}
            onClick={() =>
              void run(
                () => command("test_notification", { channel: "feishu" }),
                "飞书已确认接收测试消息",
              )
            }
          >
            测试飞书消息
          </button>
          <button
            className="danger-text"
            disabled={busy || !configured}
            onClick={() =>
              void run(async () => {
                await command("remove_feishu");
                setConfigured(false);
              }, "飞书配置已移除")
            }
          >
            移除飞书配置
          </button>
        </div>
        <p>
          Webhook
          和签名密钥保存在本机应用私有文件中，仅当前账户可读（未加密），每次发送不再访问钥匙串。配置不回显、不写入日志、SQLite或备份。更换或导入机器人后自动关闭全局通知，请重新开启。旧钥匙串配置保留，移除按钮只清除本机配置。到「我的关注」勾选具体目标的“飞书有货提醒”，也可手动发送当前状态。
        </p>
      </div>
    </section>
  );
}
