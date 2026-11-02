import type { Product, Target } from "./types.ts";

export function sourceName(source: string): string {
  return source === "apple"
    ? "Apple 查询"
    : source === "mock"
      ? "模拟"
      : "手动";
}

export const regionName = (locale: string) =>
  locale === "zh_HK" ? "中国香港" : "中国大陆";
export const stateNames: Record<string, string> = {
  IN_STOCK: "可取货",
  OUT_OF_STOCK: "暂无货",
  UNKNOWN: "无法确认",
  PENDING: "待记录",
  STALE: "记录已过期",
  UNSUPPORTED: "来源不支持",
};
export const eventNames: Record<string, string> = {
  FIRST_IN_STOCK: "首次观察到有货",
  RESTOCK_DETECTED: "从无货变为有货",
  RECOVERED_IN_STOCK: "恢复记录后有货",
  HEALTH_ALERT: "状态无法确认",
};
export const reasonNames: Record<string, string> = {
  SOURCE_RECHECK: "来源恢复后等待重新验证",
  SERVICE_UNAVAILABLE: "Apple 服务暂时不可用，已退避",
  STORAGE_ERROR: "本地记录写入失败，请检查数据库",
  TIMEOUT: "请求超时",
  NETWORK_ERROR: "网络不可用",
  BLOCKED: "来源拒绝访问",
  RATE_LIMITED: "来源限流",
  PARSE_ERROR: "结果无法解析",
  UNAUTHORIZED: "访问未启用或服务拒绝授权",
  UNSUPPORTED: "来源不支持",
  NO_MATCH: "未匹配到目标",
  USER_UNCERTAIN: "人工无法确认",
};
export const deliveryNames: Record<string, string> = {
  CANCELLED: "已取消",
  PENDING: "等待投递",
  SENDING: "投递中",
  SENT: "已提交",
  FAILED: "投递失败",
  UNCERTAIN: "结果未确认",
};
export function dateTime(value: string | null): string {
  if (!value) return "尚无记录";
  const date = new Date(value);
  return Number.isNaN(date.getTime())
    ? "时间无效"
    : new Intl.DateTimeFormat("zh-CN", {
        month: "2-digit",
        day: "2-digit",
        hour: "2-digit",
        minute: "2-digit",
        second: "2-digit",
        hour12: false,
      }).format(date);
}
export function percentage(numerator: number, denominator: number): string {
  return denominator === 0
    ? "—"
    : `${((100 * numerator) / denominator).toFixed(1)}%`;
}
export function filterTargets(
  targets: Target[],
  query: string,
  locale: string,
  state: string,
  members?: string[],
): Target[] {
  const terms = query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
  return targets.filter(
    (t) =>
      (!locale || t.product.locale === locale) &&
      (!state ||
        (state === "PAUSED" ? !t.enabled : t.enabled && t.state === state)) &&
      (!members || members.includes(t.id)) &&
      terms.every((term) =>
        `${t.product.model} ${t.product.capacity} ${t.product.color} ${t.product.sku} ${t.store.city} ${t.store.name}`
          .toLocaleLowerCase()
          .includes(term),
      ),
  );
}
export function selectionProducts(
  products: Product[],
  locale: string,
  model: string,
  capacity: string,
): Product[] {
  return products.filter(
    (p) =>
      p.locale === locale &&
      (!model || p.model === model) &&
      (!capacity || p.capacity === capacity),
  );
}
export function selectedVisibleIds(
  ids: string[],
  visibleIds: string[],
): string[] {
  const allowed = new Set(visibleIds);
  return [...new Set(ids)].filter((id) => allowed.has(id));
}
export function errorText(error: unknown): string {
  // Last line of defense for accidental credential-bearing exception strings.
  return String(error)
    .replace(/https?:\/\/api\.day\.app[^\s"'<>]*/gi, "[Bark 地址已隐藏]")
    .replace(
      /https?:\/\/open\.feishu\.cn\/open-apis\/bot\/[^\s"'<>]*/gi,
      "[飞书 Webhook 已隐藏]",
    )
    .slice(0, 400);
}
