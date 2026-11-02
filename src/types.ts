export type Source = "manual" | "mock" | "apple";
export type StockState = "IN_STOCK" | "OUT_OF_STOCK" | "UNKNOWN";
export type Page = "targets" | "add" | "history" | "settings" | "diagnostics";
export type TargetBatchAction =
  | "enable_feishu"
  | "disable_feishu"
  | "pause"
  | "resume"
  | "high_priority"
  | "normal_priority";
export interface Product {
  id: string;
  locale: string;
  category: string;
  model: string;
  capacity: string;
  color: string;
  sku: string;
  official_url: string;
}
export interface Store {
  id: string;
  locale: string;
  city: string;
  name: string;
}
export interface Catalog {
  products: Product[];
  stores: Store[];
  version: string;
}
export interface Settings {
  mode: Source;
  paused: boolean;
  stale_minutes: number;
  retention_days: number;
  local_notifications: boolean;
  bark_enabled: boolean;
  feishu_enabled: boolean;
  first_alert: boolean;
  recovery_alert: boolean;
  mock_running: boolean;
  mock_interval_seconds: number;
  scan_running: boolean;
  scan_interval_seconds: number;
  scan_last_request_at: string | null;
  apple_personal_access: boolean;
  scan_stop_reason: string | null;
  scan_invalidated_at: string | null;
  scan_last_http_status: number | null;
  scan_budget: { credit_ms: number; updated_at: string } | null;
  scan_retry_after: string | null;
  scan_failure_count: number;
  scan_connection_protection: boolean;
  scan_failure_notifications: boolean;
  scan_last_http_version: string | null;
  scan_last_duration_ms: number | null;
  scan_alert: {
    id: string;
    at: string;
    reason: string;
    http_status: number | null;
    status:
      | "PENDING"
      | "SENDING"
      | "SENT"
      | "FAILED"
      | "CANCELLED"
      | "UNCERTAIN";
    error: string | null;
  } | null;
}
export interface Target {
  id: string;
  product: Product;
  store: Store;
  enabled: boolean;
  priority: number;
  feishu_enabled: boolean;
  state: string;
  last_state: string | null;
  observed_at: string | null;
  last_valid_at: string | null;
  reason: string | null;
  source: Source;
}
export interface Delivery {
  id: string;
  target_id: string;
  event: string;
  title: string;
  body: string;
  url: string;
  channel: string;
  status: string;
  attempted_at: string | null;
  error: string | null;
  source: Source;
}
export interface WatchGroup {
  id: string;
  title: string;
  target_ids: string[];
}
export interface Snapshot {
  targets: Target[];
  settings: Settings;
  deliveries: Delivery[];
  groups: WatchGroup[];
  catalog_version: string;
  automatic_available: boolean;
  scanner: ScanStatus;
}
export interface ScanStatus {
  running: boolean;
  last_started_at: string | null;
  last_finished_at: string | null;
  last_success_at: string | null;
  next_scan_at: string | null;
  consecutive_failures: number;
  stop_reason: string | null;
  checked_targets: number;
  batch_count: number;
  minimum_interval_seconds: number | null;
  allowed_locales: string[];
  invalidated_at: string | null;
  access_kind: "disabled" | "personal" | "licensed" | "";
  last_http_status: number | null;
  budget_remaining: number;
  budget_capacity: number;
  budget_refill_seconds: number;
  budget_ready_at: string | null;
  resume_after: string | null;
}
export interface HistoryRow {
  id: string;
  target_id: string;
  product: string;
  store: string;
  locale: string;
  state: string;
  observed_at: string;
  source: string;
  reason: string | null;
  event: string | null;
}
export interface HeatCell {
  day: string;
  hour: number;
  valid: number;
  in_stock: number;
  total: number;
}
export interface Summary {
  total: number;
  valid: number;
  in_stock: number;
  events: number;
  first_seen: string | null;
  last_seen: string | null;
  cells: HeatCell[];
}
export interface AddResult {
  added: number;
  duplicates: number;
  combinations: number;
}

export interface HistoryFilter {
  days: number;
  targetId: string | null;
  state: StockState | null;
  locale: string | null;
  query: string;
  event: string | null;
  from: string | null;
  to: string | null;
}
export interface HistoryResult {
  rows: HistoryRow[];
  summary: Summary;
  since: string;
  until: string;
}
