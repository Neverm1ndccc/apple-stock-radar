import { invoke, isTauri } from "@tauri-apps/api/core";

export const native = isTauri();
export function command<T>(
  name: string,
  args?: Record<string, unknown>,
): Promise<T> {
  if (!native)
    return Promise.reject(
      "请在库存雷达桌面应用中使用此功能。运行 pnpm tauri dev 可启动。",
    );
  return invoke<T>(name, args);
}
