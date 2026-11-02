import type { TargetBatchAction } from "./types";

export const batchActions: { action: TargetBatchAction; label: string }[] = [
  { action: "enable_feishu", label: "开启飞书提醒" },
  { action: "disable_feishu", label: "关闭飞书提醒" },
  { action: "pause", label: "暂停关注" },
  { action: "resume", label: "恢复关注" },
  { action: "high_priority", label: "设为高优先" },
  { action: "normal_priority", label: "恢复普通优先级" },
];

export function TargetBatchToolbar({
  selected,
  total,
  busy,
  feishuEnabled,
  toggleAll,
  clear,
  apply,
}: {
  selected: number;
  total: number;
  busy: boolean;
  feishuEnabled: boolean;
  toggleAll: () => void;
  clear: () => void;
  apply: (action: TargetBatchAction, label: string) => void;
}) {
  return (
    <section className="batch-toolbar" aria-label="关注批量操作">
      <div className="batch-selection">
        <label>
          <input
            type="checkbox"
            aria-label="全选当前筛选结果"
            checked={total > 0 && selected === total}
            disabled={busy || total === 0}
            ref={(node) => {
              if (node) node.indeterminate = selected > 0 && selected < total;
            }}
            onChange={toggleAll}
          />
          全选当前结果（{total}）
        </label>
        <span aria-live="polite">已选 {selected} 个</span>
        <button disabled={busy || selected === 0} onClick={clear}>
          取消选择
        </button>
      </div>
      <div className="batch-buttons">
        {batchActions.map(({ action, label }) => (
          <button
            key={action}
            disabled={busy || selected === 0}
            onClick={() => apply(action, label)}
          >
            {label}
          </button>
        ))}
      </div>
      <p>
        仅操作当前筛选结果中勾选的关注。
        {!feishuEnabled && "飞书全局通知尚未开启，请先在设置中配置并开启。"}
      </p>
    </section>
  );
}
