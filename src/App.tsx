import { ScannerSettings } from "./ScannerSettings";
import { TargetBatchToolbar } from "./TargetBatchToolbar";
import { FeishuSettings, TargetFeishu } from "./Feishu";
import { ScanToggle, scanSummary } from "./ScanToggle";
import { useCallback, useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  isPermissionGranted,
  requestPermission,
} from "@tauri-apps/plugin-notification";
import { enable, disable, isEnabled } from "@tauri-apps/plugin-autostart";
import {
  Activity,
  ArrowDownToLine,
  ArrowUpRight,
  Bell,
  Check,
  ChevronRight,
  CircleHelp,
  Clock3,
  Database,
  ExternalLink,
  History,
  LayoutGrid,
  LoaderCircle,
  Monitor,
  MoreHorizontal,
  Pause,
  Play,
  Plus,
  Radar,
  RefreshCw,
  Search,
  Settings2,
  ShieldCheck,
  Trash2,
  X,
} from "lucide-react";
import { command, native } from "./api";
import {
  dateTime,
  deliveryNames,
  errorText,
  eventNames,
  filterTargets,
  percentage,
  reasonNames,
  regionName,
  selectedVisibleIds,
  selectionProducts,
  stateNames,
  sourceName,
} from "./presentation";
import type {
  AddResult,
  Catalog,
  HistoryResult,
  Page,
  Settings,
  Snapshot,
  StockState,
  Target,
  TargetBatchAction,
} from "./types";

const navigation: { id: Page; name: string; icon: typeof Radar }[] = [
  { id: "targets", name: "我的关注", icon: LayoutGrid },
  { id: "add", name: "添加关注", icon: Plus },
  { id: "history", name: "历史分析", icon: History },
  { id: "settings", name: "设置", icon: Settings2 },
  { id: "diagnostics", name: "运行诊断", icon: Activity },
];
type Run = (
  action: () => Promise<unknown>,
  success?: string,
) => Promise<boolean>;

export function App() {
  const tray =
    new URLSearchParams(window.location.search).get("view") === "tray";
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [catalog, setCatalog] = useState<Catalog | null>(null);
  const [page, setPage] = useState<Page>("targets");
  useEffect(() => {
    window.scrollTo({ top: 0 });
  }, [page]);
  const [recording, setRecording] = useState<Target | null>(null);
  const [notice, setNotice] = useState<{ text: string; error: boolean } | null>(
    null,
  );
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(true);
  const locked = useRef(false);
  const sequence = useRef(0);
  const reload = useCallback(async () => {
    const version = ++sequence.current;
    const data = await command<Snapshot>("bootstrap");
    if (version === sequence.current) {
      setSnapshot((previous) =>
        JSON.stringify(previous) === JSON.stringify(data) ? previous : data,
      );
    }
  }, []);
  const run: Run = useCallback(
    async (action, success) => {
      if (locked.current) return false;
      locked.current = true;
      setBusy(true);
      setNotice(null);
      try {
        await action();
        await reload();
        if (success) setNotice({ text: success, error: false });
        return true;
      } catch (e) {
        setNotice({ text: errorText(e), error: true });
        return false;
      } finally {
        locked.current = false;
        setBusy(false);
      }
    },
    [reload],
  );
  useEffect(() => {
    let active = true;
    const unsubscribers: (() => void)[] = [];
    const report = (e: unknown) => {
      if (active) setNotice({ text: errorText(e), error: true });
    };
    Promise.all([
      reload(),
      command<Catalog>("catalog").then((c) => {
        if (active) setCatalog(c);
      }),
    ])
      .catch(report)
      .finally(() => {
        if (active) setLoading(false);
      });
    if (native) {
      const subscribe = async () => {
        for (const promise of [
          listen("radar:changed", () => {
            void reload().catch(report);
          }),
          listen<string>("radar:navigate", (event) => {
            if (navigation.some((n) => n.id === event.payload))
              setPage(event.payload as Page);
          }),
        ]) {
          const unlisten = await promise;
          if (active) unsubscribers.push(unlisten);
          else unlisten();
        }
      };
      void subscribe().catch(report);
    }
    const timer = window.setInterval(() => {
      if (native) void reload().catch(report);
    }, 15000);
    return () => {
      active = false;
      window.clearInterval(timer);
      unsubscribers.forEach((f) => f());
    };
  }, [reload]);
  useEffect(() => {
    if (!notice || notice.error) return;
    const timer = window.setTimeout(() => setNotice(null), 7000);
    return () => window.clearTimeout(timer);
  }, [notice]);
  const save = (patch: Partial<Settings>) =>
    run(() =>
      command("save_settings", {
        settings: { ...snapshot!.settings, ...patch },
      }),
    );
  const navigate = (next: Page) => {
    if (tray) void run(() => command("open_main", { page: next }));
    else setPage(next);
  };
  const changeTarget = (
    target: Target,
    patch: Partial<{ enabled: boolean; priority: number; archived: boolean }>,
  ) =>
    run(() =>
      command("update_target", {
        id: target.id,
        enabled: target.enabled,
        priority: target.priority,
        archived: false,
        ...patch,
      }),
    );
  const open = (target: Target) =>
    run(() => command("open_official", { url: target.product.official_url }));

  if (!snapshot)
    return (
      <div className="boot">
        <div className="brand-mark">
          <Radar size={30} />
        </div>
        <h1>库存雷达</h1>
        {loading ? (
          <p>
            <LoaderCircle className="spin" size={18} /> 正在读取本地数据…
          </p>
        ) : (
          <>
            <p role="alert">{notice?.text ?? "无法读取本地数据"}</p>
            <button onClick={() => window.location.reload()}>重新加载</button>
          </>
        )}
      </div>
    );
  const settings = snapshot.settings;
  const enabled = snapshot.targets.filter((t) => t.enabled);
  const stock = enabled.filter((t) => t.state === "IN_STOCK").length;
  const uncertain = enabled.filter((t) =>
    ["UNKNOWN", "STALE", "UNSUPPORTED"].includes(t.state),
  ).length;
  const latest =
    snapshot.targets
      .map((t) => t.observed_at)
      .filter((v): v is string => !!v)
      .sort()
      .at(-1) ?? null;
  const status = settings.paused
    ? "已暂停"
    : settings.mode === "mock"
      ? "模拟演示"
      : settings.mode === "apple"
        ? "Apple 查询"
        : "手动记录";
  return (
    <div className={tray ? "app tray-app" : "app"}>
      {!tray && (
        <aside className="sidebar">
          <div className="brand">
            <div className="brand-mark">
              <Radar size={25} />
            </div>
            <div>
              库存雷达<small>APPLE STORE · 个人版</small>
            </div>
          </div>
          <nav aria-label="主导航">
            {navigation.map(({ id, name, icon: Icon }) => (
              <button
                key={id}
                className={`nav-item ${page === id ? "active" : ""}`}
                onClick={() => setPage(id)}
              >
                <Icon size={18} />
                {name}
                {id === "targets" && (
                  <span className="nav-count">{snapshot.targets.length}</span>
                )}
              </button>
            ))}
          </nav>
          <div className="sidebar-bottom">
            <div className="local-tag">
              <ShieldCheck size={16} /> 数据仅保存在本机
            </div>
            <p>
              中国大陆 / 中国香港
              <br />
              本地关注 · 自己掌握
            </p>
            <span className="version">v0.6.0 · GPL-3.0-or-later</span>
          </div>
        </aside>
      )}
      <main>
        <header className="topbar">
          <span className="breadcrumb">
            {tray ? (
              <>
                <Radar size={19} /> 库存雷达
              </>
            ) : (
              <>
                工作空间 <ChevronRight size={14} />{" "}
                <strong>{navigation.find((n) => n.id === page)?.name}</strong>
              </>
            )}
          </span>
          <div className="top-actions">
            <span
              className={`mode-pill ${settings.mode === "mock" ? "mock" : ""}`}
            >
              <i />
              {status}
            </span>
            <button
              className="icon-button"
              disabled={busy}
              aria-label={settings.paused ? "恢复运行" : "暂停运行"}
              title={settings.paused ? "恢复运行" : "暂停运行"}
              onClick={() => void save({ paused: !settings.paused })}
            >
              {settings.paused ? <Play size={17} /> : <Pause size={17} />}
            </button>
            {tray && (
              <button
                className="icon-button"
                aria-label="打开主窗口"
                onClick={() => navigate("targets")}
              >
                <ArrowUpRight size={18} />
              </button>
            )}
          </div>
        </header>
        <div className="workspace">
          {notice && (
            <div
              role={notice.error ? "alert" : "status"}
              className={`notice ${notice.error ? "error" : ""}`}
            >
              <span>{notice.text}</span>
              <button
                className="icon-button"
                aria-label="关闭提示"
                onClick={() => setNotice(null)}
              >
                <X size={16} />
              </button>
            </div>
          )}
          {settings.paused && (
            <div className="banner paused">
              <Pause size={16} />
              <span>应用已暂停，停止记录、模拟和通知投递。</span>
            </div>
          )}
          {settings.mode === "mock" && (
            <div className="banner demo">
              <CircleHelp size={17} />
              <span>模拟演示 · 数据与手动记录隔离，不触发真实通知。</span>
            </div>
          )}
          {(page === "targets" || tray) && (
            <>
              <div className="page-heading">
                <div>
                  <p className="eyebrow">YOUR WATCHLIST</p>
                  <h1>{tray ? "我的关注" : "每一份期待，都有迹可循。"}</h1>
                  <p className="subtitle">
                    {tray
                      ? `${snapshot.targets.length} 个目标 · ${stock} 个可取货`
                      : "关注心仪的产品，记录每一次供货变化。"}
                  </p>
                </div>
                {!tray && (
                  <button className="primary" onClick={() => navigate("add")}>
                    <Plus size={17} />
                    添加关注
                  </button>
                )}
              </div>
              {!tray && (
                <div className="metrics">
                  <Metric
                    label="关注目标"
                    value={snapshot.targets.length}
                    note={`${enabled.length} 个已启用`}
                    icon={<Radar size={19} />}
                  />
                  <Metric
                    label="可取货"
                    value={stock}
                    note="当前来源的有效记录"
                    icon={<Check size={19} />}
                    green
                  />
                  <Metric
                    label="需要确认"
                    value={uncertain}
                    note="未知、过期或来源不支持"
                    icon={<CircleHelp size={19} />}
                  />
                  <Metric
                    label="最近记录"
                    value={latest ? dateTime(latest).slice(6) : "—"}
                    note={
                      latest ? dateTime(latest).slice(0, 5) : "等待第一次观测"
                    }
                    icon={<Clock3 size={19} />}
                  />
                </div>
              )}
              <div className="source-note">
                <ShieldCheck size={17} />
                <div>
                  <strong>
                    {settings.mode === "manual"
                      ? "当前使用手动记录"
                      : settings.mode === "apple"
                        ? "Apple 商店扫描"
                        : "当前使用离线模拟"}
                  </strong>
                  <span>
                    {settings.mode === "manual"
                      ? "打开 Apple 官网自行检查后录入。可在设置中启用个人 Apple 查询。"
                      : settings.mode === "apple"
                        ? snapshot.automatic_available
                          ? scanSummary(snapshot)
                          : "真实查询尚未启用。请在设置中允许个人查询并开启后台扫描。"
                        : "演示状态按固定序列变化，不代表 Apple 实际库存。"}
                  </span>
                </div>
                {settings.mode === "apple" && (
                  <ScanToggle snapshot={snapshot} busy={busy} run={run} />
                )}
                {settings.mode === "mock" && (
                  <button
                    disabled={busy || settings.paused}
                    onClick={() =>
                      void run(() => command("simulate"), "已生成一轮模拟观测")
                    }
                  >
                    <RefreshCw size={15} />
                    演示一轮
                  </button>
                )}
              </div>
              <TargetList
                snapshot={snapshot}
                run={run}
                compact={tray}
                busy={busy}
                onAdd={() => navigate("add")}
                update={changeTarget}
                open={open}
                record={(target) => {
                  if (tray) navigate("targets");
                  else setRecording(target);
                }}
              />
            </>
          )}
          {!tray && page === "add" && catalog && (
            <AddTargets
              catalog={catalog}
              busy={busy}
              run={run}
              done={() => setPage("targets")}
            />
          )}
          {!tray && page === "history" && (
            <HistoryPage snapshot={snapshot} busy={busy} run={run} />
          )}
          {!tray && page === "settings" && (
            <SettingsPage
              settings={settings}
              snapshot={snapshot}
              busy={busy}
              run={run}
              save={save}
            />
          )}
          {!tray && page === "diagnostics" && (
            <Diagnostics
              snapshot={snapshot}
              latest={latest}
              busy={busy}
              run={run}
            />
          )}
          {!tray && (
            <footer>
              <span>
                <span className="status-dot" /> 本地数据库已连接
              </span>
              <span>
                库存以 Apple 官方页面为准 <ExternalLink size={11} />
              </span>
            </footer>
          )}
        </div>
        {tray && (
          <div className="tray-bottom">
            <button onClick={() => navigate("add")}>
              <Plus size={16} />
              添加关注
            </button>
            <button onClick={() => navigate("history")}>
              历史分析
              <ChevronRight size={15} />
            </button>
          </div>
        )}
      </main>
      {recording && (
        <RecordDialog
          target={recording}
          source={settings.mode}
          busy={busy}
          run={run}
          close={() => setRecording(null)}
        />
      )}
    </div>
  );
}

function Metric({
  label,
  value,
  note,
  icon,
  green = false,
}: {
  label: string;
  value: ReactNode;
  note: string;
  icon: ReactNode;
  green?: boolean;
}) {
  return (
    <div className={`metric ${green ? "green" : ""}`}>
      <div className="metric-label">
        {label}
        {icon}
      </div>
      <strong>{value}</strong>
      <small>{note}</small>
    </div>
  );
}
function Status({
  state,
  enabled = true,
}: {
  state: string;
  enabled?: boolean;
}) {
  return (
    <span
      className={`stock-badge ${enabled ? state.toLowerCase() : "disabled"}`}
    >
      <i />
      {enabled ? (stateNames[state] ?? state) : "已暂停"}
    </span>
  );
}
function Empty({
  title,
  body,
  action,
}: {
  title: string;
  body: string;
  action?: ReactNode;
}) {
  return (
    <div className="empty">
      <div className="empty-icon">
        <Radar size={35} strokeWidth={1.2} />
      </div>
      <h3>{title}</h3>
      <p>{body}</p>
      {action}
    </div>
  );
}
function TargetList({
  snapshot,
  run,
  compact,
  busy,
  onAdd,
  update,
  open,
  record,
}: {
  snapshot: Snapshot;
  run: Run;
  compact: boolean;
  busy: boolean;
  onAdd: () => void;
  update: (
    target: Target,
    patch: Partial<{ enabled: boolean; priority: number; archived: boolean }>,
  ) => Promise<boolean>;
  open: (target: Target) => Promise<boolean>;
  record: (target: Target) => void;
}) {
  const [query, setQuery] = useState("");
  const [locale, setLocale] = useState("");
  const [state, setState] = useState("");
  const [group, setGroup] = useState("");
  const [removing, setRemoving] = useState<Target | null>(null);
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  useEffect(() => {
    setSelectedIds([]);
  }, [query, locale, state, group]);
  const members = snapshot.groups.find((g) => g.id === group)?.target_ids;
  const targets = filterTargets(
    snapshot.targets,
    query,
    locale,
    state,
    members,
  );
  const visibleIds = targets.map((target) => target.id);
  const selected = selectedVisibleIds(selectedIds, visibleIds);
  const applyBatch = async (action: TargetBatchAction, label: string) => {
    if (!selected.length) return;
    if (
      await run(
        () => command("batch_update_targets", { ids: selected, action }),
        `已为 ${selected.length} 个关注${label}`,
      )
    ) {
      setSelectedIds([]);
    }
  };
  return (
    <>
      <div className="list-toolbar">
        <label className="search">
          <Search size={16} />
          <input
            aria-label="搜索关注目标"
            placeholder="搜索产品、颜色或门店…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </label>
        <select
          aria-label="地区筛选"
          value={locale}
          onChange={(e) => setLocale(e.target.value)}
        >
          <option value="">全部地区</option>
          <option value="zh_CN">中国大陆</option>
          <option value="zh_HK">中国香港</option>
        </select>
        {!compact && (
          <>
            <select
              aria-label="状态筛选"
              value={state}
              onChange={(e) => setState(e.target.value)}
            >
              <option value="">全部状态</option>
              {Object.entries(stateNames).map(([key, value]) => (
                <option key={key} value={key}>
                  {value}
                </option>
              ))}
              <option value="PAUSED">已暂停</option>
            </select>
            <select
              aria-label="分组筛选"
              value={group}
              onChange={(e) => setGroup(e.target.value)}
            >
              <option value="">全部分组</option>
              {snapshot.groups.map((g) => (
                <option key={g.id} value={g.id}>
                  {g.title || "未命名分组"} · {g.target_ids.length}
                </option>
              ))}
            </select>
          </>
        )}
      </div>
      {snapshot.targets.length > 0 && (
        <TargetBatchToolbar
          selected={selected.length}
          total={targets.length}
          busy={busy}
          feishuEnabled={snapshot.settings.feishu_enabled}
          toggleAll={() =>
            setSelectedIds(selected.length === targets.length ? [] : visibleIds)
          }
          clear={() => setSelectedIds([])}
          apply={(action, label) => void applyBatch(action, label)}
        />
      )}
      {!snapshot.targets.length ? (
        <Empty
          title="从一个心仪目标开始"
          body="选择产品与门店，建立你的第一份库存关注。"
          action={
            <button className="primary" onClick={onAdd}>
              <Plus size={16} />
              添加关注
            </button>
          }
        />
      ) : !targets.length ? (
        <Empty title="没有匹配的关注" body="调整搜索内容或筛选条件后再试。" />
      ) : (
        <div className={compact ? "compact-targets" : "target-grid"}>
          {targets.map((target) => (
            <article
              key={target.id}
              className={`target-card ${!target.enabled ? "target-disabled" : ""} ${selected.includes(target.id) ? "target-selected" : ""}`}
            >
              <div className="card-top">
                <label className="card-selection">
                  <input
                    type="checkbox"
                    disabled={busy}
                    checked={selected.includes(target.id)}
                    aria-label={`选择关注 · ${target.product.model} · ${target.product.capacity} · ${target.product.color} · ${target.store.name}`}
                    onChange={() =>
                      setSelectedIds(
                        selected.includes(target.id)
                          ? selected.filter((id) => id !== target.id)
                          : [...selected, target.id],
                      )
                    }
                  />
                  <span className="region-label">
                    {regionName(target.product.locale)}
                    {target.priority > 0 && (
                      <span className="priority">
                        {target.priority === 2 ? "高优先" : "优先"}
                      </span>
                    )}
                  </span>
                </label>
                <Status state={target.state} enabled={target.enabled} />
              </div>
              <div className="product-title">
                <div className="phone-art" aria-hidden="true">
                  <div />
                </div>
                <div>
                  <h3>{target.product.model}</h3>
                  <p>
                    {target.product.capacity} <span>·</span>{" "}
                    {target.product.color}
                  </p>
                  {!compact && <small>{target.product.sku}</small>}
                </div>
              </div>
              <div className="store-line">
                <span className="store-symbol">A</span>
                <div>
                  <strong>{target.store.name}</strong>
                  <small>{target.store.city}</small>
                </div>
              </div>
              <div className="record-meta">
                <Clock3 size={13} />
                <span>{dateTime(target.observed_at)}</span>
                <span className="source-label">
                  {sourceName(target.source)}
                </span>
              </div>
              {target.reason && (
                <p className="target-reason">
                  {reasonNames[target.reason] ?? target.reason}
                </p>
              )}
              {["UNKNOWN", "STALE", "UNSUPPORTED"].includes(target.state) &&
                target.last_state && (
                  <p className="last-valid">
                    上次有效：{stateNames[target.last_state]} ·{" "}
                    {dateTime(target.last_valid_at)}
                  </p>
                )}
              <TargetFeishu
                target={target}
                settings={snapshot.settings}
                busy={busy}
                run={run}
              />
              <div className="card-actions">
                <button disabled={busy} onClick={() => void open(target)}>
                  <ExternalLink size={14} />
                  官网检查
                </button>
                <button
                  className="record-button"
                  disabled={
                    busy ||
                    !target.enabled ||
                    snapshot.settings.paused ||
                    snapshot.settings.mode === "apple"
                  }
                  onClick={() => record(target)}
                >
                  {snapshot.settings.mode === "apple" ? "自动记录" : "录入结果"}
                  <ChevronRight size={14} />
                </button>
                {!compact && (
                  <details className="target-menu">
                    <summary aria-label={`${target.product.model} 目标操作`}>
                      <MoreHorizontal size={17} />
                    </summary>
                    <div>
                      <button
                        disabled={busy}
                        onClick={() =>
                          void update(target, { enabled: !target.enabled })
                        }
                      >
                        {target.enabled ? (
                          <Pause size={14} />
                        ) : (
                          <Play size={14} />
                        )}
                        {target.enabled ? "暂停目标" : "恢复目标"}
                      </button>
                      <label>
                        优先级
                        <select
                          aria-label="目标优先级"
                          value={target.priority}
                          disabled={busy}
                          onChange={(e) =>
                            void update(target, {
                              priority: Number(e.target.value),
                            })
                          }
                        >
                          <option value={0}>普通</option>
                          <option value={1}>优先</option>
                          <option value={2}>高优先</option>
                        </select>
                      </label>
                      <button
                        className="danger-text"
                        onClick={() => setRemoving(target)}
                      >
                        <Trash2 size={14} />
                        移除关注
                      </button>
                    </div>
                  </details>
                )}
              </div>
            </article>
          ))}
        </div>
      )}
      {!compact && snapshot.targets.length > 0 && (
        <p className="list-caption">
          显示 {targets.length} / {snapshot.targets.length} 个关注目标 · 同一
          SKU 与门店只保留一个目标
        </p>
      )}
      {removing && (
        <Confirm
          title="移除这个关注？"
          body={`${removing.product.model} · ${removing.store.name}。历史观测会保留，重新添加可恢复关注。`}
          busy={busy}
          close={() => setRemoving(null)}
          confirm={async () => {
            if (await update(removing, { archived: true })) setRemoving(null);
          }}
        />
      )}
    </>
  );
}
function AddTargets({
  catalog,
  busy,
  run,
  done,
}: {
  catalog: Catalog;
  busy: boolean;
  run: Run;
  done: () => void;
}) {
  const [locale, setLocale] = useState("zh_CN");
  const [model, setModel] = useState("");
  const [capacity, setCapacity] = useState("");
  const [city, setCity] = useState("");
  const [productIds, setProductIds] = useState<string[]>([]);
  const [storeIds, setStoreIds] = useState<string[]>([]);
  const [title, setTitle] = useState("");
  const products = selectionProducts(catalog.products, locale, model, capacity);
  const stores = catalog.stores.filter(
    (s) => s.locale === locale && (!city || s.city === city),
  );
  const unique = (values: string[]) => [...new Set(values)];
  const models = unique(
    catalog.products.filter((p) => p.locale === locale).map((p) => p.model),
  );
  const capacities = unique(
    selectionProducts(catalog.products, locale, model, "").map(
      (p) => p.capacity,
    ),
  );
  const cities = unique(
    catalog.stores.filter((s) => s.locale === locale).map((s) => s.city),
  );
  const toggle = (list: string[], id: string) =>
    list.includes(id) ? list.filter((x) => x !== id) : [...list, id];
  const selectedProducts = selectedVisibleIds(
    productIds,
    products.map((p) => p.id),
  );
  const selectedStores = selectedVisibleIds(
    storeIds,
    stores.map((s) => s.id),
  );
  const combinations = selectedProducts.length * selectedStores.length;
  const submit = async () => {
    let result: AddResult | null = null;
    const ok = await run(async () => {
      result = await command<AddResult>("add_targets", {
        productIds: selectedProducts,
        storeIds: selectedStores,
        title:
          title.trim() ||
          `${model || "iPhone"} · ${city || regionName(locale)}`,
      });
    });
    if (ok && result) {
      setAdded(result);
    }
  };
  const [added, setAdded] = useState<AddResult | null>(null);
  return (
    <>
      <div className="page-heading">
        <div>
          <p className="eyebrow">BUILD YOUR WATCHLIST</p>
          <h1>把期待，加入关注。</h1>
          <p className="subtitle">选择真实 SKU 与门店，批量建立精确目标。</p>
        </div>
      </div>
      <div className="banner">
        <Database size={17} />
        <span>
          离线目录：{catalog.version}。下单前请在官网核对当前在售规格。
        </span>
      </div>
      <fieldset disabled={busy} className="add-layout">
        <section className="panel">
          <div className="section-title">
            <span className="step">01</span>
            <h2>选择产品</h2>
            <span className="muted">iPhone</span>
          </div>
          <div className="region-tabs">
            {["zh_CN", "zh_HK"].map((value) => (
              <button
                type="button"
                key={value}
                className={locale === value ? "selected" : ""}
                onClick={() => {
                  setLocale(value);
                  setModel("");
                  setCapacity("");
                  setCity("");
                  setProductIds([]);
                  setStoreIds([]);
                }}
              >
                {regionName(value)}
              </button>
            ))}
          </div>
          <div className="form-grid">
            <label>
              型号
              <select
                value={model}
                onChange={(e) => {
                  setModel(e.target.value);
                  setCapacity("");
                  setProductIds([]);
                }}
              >
                <option value="">请选择型号</option>
                {models.map((m) => (
                  <option key={m}>{m}</option>
                ))}
              </select>
            </label>
            <label>
              容量
              <select
                value={capacity}
                onChange={(e) => {
                  setCapacity(e.target.value);
                  setProductIds([]);
                }}
                disabled={!model}
              >
                <option value="">全部容量</option>
                {capacities.map((c) => (
                  <option key={c}>{c}</option>
                ))}
              </select>
            </label>
          </div>
          <div className="selection-heading">
            <strong>具体规格</strong>
            <span>已选 {selectedProducts.length} 个 SKU</span>
          </div>
          {model ? (
            <div className="choice-list">
              {products.map((p) => (
                <label
                  key={p.id}
                  className={`choice ${selectedProducts.includes(p.id) ? "checked" : ""}`}
                >
                  <input
                    type="checkbox"
                    checked={selectedProducts.includes(p.id)}
                    onChange={() =>
                      setProductIds(toggle(selectedProducts, p.id))
                    }
                  />
                  <div>
                    <strong>
                      {p.color} · {p.capacity}
                    </strong>
                    <small>{p.sku}</small>
                  </div>
                </label>
              ))}
            </div>
          ) : (
            <p className="inline-empty">选择型号后显示可用的颜色与容量组合。</p>
          )}
        </section>
        <section className="panel">
          <div className="section-title">
            <span className="step">02</span>
            <h2>选择门店</h2>
          </div>
          <label className="field-label">
            城市
            <select
              value={city}
              onChange={(e) => {
                setCity(e.target.value);
                setStoreIds([]);
              }}
            >
              <option value="">全部城市</option>
              {cities.map((c) => (
                <option key={c}>{c}</option>
              ))}
            </select>
          </label>
          <div className="selection-heading">
            <strong>Apple Store</strong>
            <span>已选 {selectedStores.length} 家</span>
          </div>
          <div className="choice-list stores">
            {stores.map((s) => (
              <label
                key={s.id}
                className={`choice ${selectedStores.includes(s.id) ? "checked" : ""}`}
              >
                <input
                  type="checkbox"
                  checked={selectedStores.includes(s.id)}
                  onChange={() => setStoreIds(toggle(selectedStores, s.id))}
                />
                <div>
                  <strong>{s.name}</strong>
                  <small>
                    {s.city} · {s.id}
                  </small>
                </div>
              </label>
            ))}
          </div>
        </section>
      </fieldset>
      <section className="panel batch-preview">
        <div>
          <h2>
            组合预览 <strong>{combinations}</strong>
            <span>个关注目标</span>
          </h2>
          <p>
            {selectedProducts.length} 个 SKU × {selectedStores.length} 家门店 ·
            重复目标自动跳过
          </p>
        </div>
        <label>
          分组名称
          <input
            maxLength={80}
            placeholder="例如：我的 iPhone"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
          />
        </label>
        <button
          className="primary"
          disabled={busy || !combinations || combinations > 500}
          onClick={() => void submit()}
        >
          <Plus size={16} />
          {busy ? "保存中…" : "保存关注"}
        </button>
      </section>
      {combinations > 500 && (
        <div className="notice error">
          单次最多创建 500 个组合，请缩小选择范围。
        </div>
      )}
      {added && (
        <div className="modal-backdrop">
          <section
            className="dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="added-title"
          >
            <div className="success-icon">
              <Check size={27} />
            </div>
            <h2 id="added-title">关注已保存</h2>
            <p>
              新增或恢复 {added.added} 个目标，跳过 {added.duplicates}{" "}
              个重复组合。
            </p>
            <button autoFocus className="primary" onClick={done}>
              查看我的关注
              <ChevronRight size={16} />
            </button>
          </section>
        </div>
      )}
    </>
  );
}
function Modal({
  title,
  children,
  close,
  busy,
}: {
  title: string;
  children: ReactNode;
  close: () => void;
  busy: boolean;
}) {
  const dialog = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    dialog.current?.focus();
    return () => previous?.focus();
  }, []);
  return (
    <div
      className="modal-backdrop"
      onClick={(e) => {
        if (e.target === e.currentTarget && !busy) close();
      }}
    >
      <div
        className="dialog"
        role="dialog"
        aria-modal="true"
        aria-label={title}
        ref={dialog}
        tabIndex={-1}
        onKeyDown={(e) => {
          if (e.key === "Escape" && !busy) close();
          if (e.key === "Tab") {
            const elements = dialog.current?.querySelectorAll<HTMLElement>(
              "button:not(:disabled), select:not(:disabled), input:not(:disabled)",
            );
            if (!elements?.length) return;
            const first = elements[0];
            const last = elements[elements.length - 1];
            if (
              e.shiftKey &&
              (document.activeElement === first ||
                document.activeElement === dialog.current)
            ) {
              e.preventDefault();
              last.focus();
            } else if (!e.shiftKey && document.activeElement === last) {
              e.preventDefault();
              first.focus();
            }
          }
        }}
      >
        <div className="dialog-heading">
          <h2>{title}</h2>
          <button
            disabled={busy}
            className="icon-button"
            aria-label="关闭对话框"
            onClick={close}
          >
            <X size={18} />
          </button>
        </div>
        {children}
      </div>
    </div>
  );
}
function RecordDialog({
  target,
  source,
  busy,
  run,
  close,
}: {
  target: Target;
  source: string;
  busy: boolean;
  run: Run;
  close: () => void;
}) {
  const [state, setState] = useState<StockState>("UNKNOWN");
  const [reason, setReason] = useState("USER_UNCERTAIN");
  return (
    <Modal
      title={source === "mock" ? "录入模拟结果" : "录入人工检查结果"}
      close={close}
      busy={busy}
    >
      <p>
        {target.product.model} · {target.product.capacity} ·{" "}
        {target.product.color}
        <br />
        {target.store.city} / {target.store.name}
      </p>
      <div className="banner">
        {source === "mock"
          ? "本次记录仅用于模拟，不触发真实通知。"
          : "请先亲自在 Apple 官网查看供货情况；此操作不会自动查询库存。"}
      </div>
      <label className="field-label">
        观测结果
        <select
          value={state}
          onChange={(e) => setState(e.target.value as StockState)}
        >
          {["UNKNOWN", "IN_STOCK", "OUT_OF_STOCK"].map((value) => (
            <option key={value} value={value}>
              {stateNames[value]}
            </option>
          ))}
        </select>
      </label>
      {state === "UNKNOWN" && (
        <label className="field-label">
          无法确认的原因
          <select value={reason} onChange={(e) => setReason(e.target.value)}>
            {Object.entries(reasonNames).map(([value, name]) => (
              <option value={value} key={value}>
                {name}
              </option>
            ))}
          </select>
        </label>
      )}
      <div className="dialog-actions">
        <button disabled={busy} onClick={close}>
          取消
        </button>
        <button
          className="primary"
          disabled={busy}
          onClick={() =>
            void run(
              () =>
                command("record_observation", {
                  input: {
                    id: crypto.randomUUID(),
                    target_id: target.id,
                    source,
                    state,
                    reason: state === "UNKNOWN" ? reason : null,
                  },
                }),
              "观测结果已保存",
            ).then((ok) => {
              if (ok) close();
            })
          }
        >
          保存记录
        </button>
      </div>
    </Modal>
  );
}
function Confirm({
  title,
  body,
  busy,
  close,
  confirm,
}: {
  title: string;
  body: string;
  busy: boolean;
  close: () => void;
  confirm: () => Promise<unknown>;
}) {
  return (
    <Modal title={title} close={close} busy={busy}>
      <p>{body}</p>
      <div className="dialog-actions">
        <button disabled={busy} onClick={close}>
          取消
        </button>
        <button
          className="danger"
          disabled={busy}
          onClick={() => void confirm()}
        >
          确认
        </button>
      </div>
    </Modal>
  );
}

function HistoryPage({
  snapshot,
  busy,
  run,
}: {
  snapshot: Snapshot;
  busy: boolean;
  run: Run;
}) {
  const [days, setDays] = useState(7);
  const [target, setTarget] = useState("");
  const [stock, setStock] = useState("");
  const [locale, setLocale] = useState("");
  const [query, setQuery] = useState("");
  const [search, setSearch] = useState("");
  const [event, setEvent] = useState("");
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  const [data, setData] = useState<HistoryResult | null>(null);
  useEffect(() => {
    const timer = window.setTimeout(() => setSearch(query), 300);
    return () => window.clearTimeout(timer);
  }, [query]);
  const [error, setError] = useState("");
  const [path, setPath] = useState("");
  useEffect(() => {
    let active = true;
    setError("");
    setData(null);
    const filter = {
      days,
      targetId: target || null,
      state: stock || null,
      locale: locale || null,
      query: search,
      event: event || null,
      from: from ? new Date(from).toISOString() : null,
      to: to ? new Date(to).toISOString() : null,
    };
    command<HistoryResult>("query_history", {
      filter,
      offsetMinutes: -new Date().getTimezoneOffset(),
    })
      .then((result) => {
        if (active) setData(result);
      })
      .catch((e) => {
        if (active) setError(errorText(e));
      });
    return () => {
      active = false;
    };
  }, [days, target, stock, locale, search, event, from, to, snapshot]);
  const summary = data?.summary;
  const dates: string[] = [];
  if (data) {
    const end = new Date(data.until);
    const day = new Date(data.since);
    day.setHours(0, 0, 0, 0);
    while (day <= end && dates.length <= 181) {
      dates.push(
        `${day.getFullYear()}-${String(day.getMonth() + 1).padStart(2, "0")}-${String(day.getDate()).padStart(2, "0")}`,
      );
      day.setDate(day.getDate() + 1);
    }
  }
  const resetFilters = () => {
    setDays(7);
    setTarget("");
    setStock("");
    setLocale("");
    setQuery("");
    setSearch("");
    setEvent("");
    setFrom("");
    setTo("");
  };
  const cellMap = new Map(summary?.cells.map((c) => [`${c.day}:${c.hour}`, c]));
  return (
    <>
      <div className="page-heading">
        <div>
          <p className="eyebrow">OBSERVE, DON'T GUESS</p>
          <h1>让每一次观测，有据可查。</h1>
          <p className="subtitle">
            {sourceName(snapshot.settings.mode)}
            历史独立统计，未知样本不计入有货比例分母。
          </p>
        </div>
        <div className="button-row">
          <button
            disabled={busy}
            onClick={() =>
              void run(async () => {
                setPath(
                  await command<string>("export_data", { format: "csv" }),
                );
              })
            }
          >
            <ArrowDownToLine size={16} />
            CSV
          </button>
          <button
            disabled={busy}
            onClick={() =>
              void run(async () => {
                setPath(
                  await command<string>("export_data", { format: "json" }),
                );
              })
            }
          >
            JSON
          </button>
        </div>
      </div>
      {path && (
        <div className="notice">
          <span>完整来源历史已导出：{path}</span>
          <button
            onClick={() => void run(() => command("open_data_directory"))}
          >
            打开目录
          </button>
        </div>
      )}
      <div className="history-filters">
        <div className="segmented">
          {[1, 7, 30].map((value) => (
            <button
              key={value}
              className={!from && !to && days === value ? "selected" : ""}
              onClick={() => {
                setDays(value);
                setFrom("");
                setTo("");
              }}
            >
              {value === 1 ? "24 小时" : `${value} 天`}
            </button>
          ))}
        </div>
        <div className="button-row">
          <button
            className={stock === "IN_STOCK" ? "primary" : ""}
            aria-pressed={stock === "IN_STOCK"}
            onClick={() => setStock(stock === "IN_STOCK" ? "" : "IN_STOCK")}
          >
            只看有货
          </button>
          <button onClick={resetFilters}>重置筛选</button>
        </div>
      </div>
      <div className="history-filter-grid">
        <label>
          库存状态
          <select
            aria-label="历史库存状态"
            value={stock}
            onChange={(e) => setStock(e.target.value)}
          >
            <option value="">全部状态</option>
            <option value="IN_STOCK">有货 / 可取货</option>
            <option value="OUT_OF_STOCK">暂无货</option>
            <option value="UNKNOWN">未知 / 无法确认</option>
          </select>
        </label>
        <label>
          地区
          <select
            aria-label="历史地区"
            value={locale}
            onChange={(e) => setLocale(e.target.value)}
          >
            <option value="">全部地区</option>
            <option value="zh_CN">中国大陆</option>
            <option value="zh_HK">中国香港</option>
          </select>
        </label>
        <label>
          关键字
          <input
            aria-label="历史关键字"
            placeholder="型号、容量、颜色、SKU、城市或门店"
            value={query}
            maxLength={200}
            onChange={(e) => setQuery(e.target.value)}
          />
        </label>
        <label>
          观测事件
          <select
            aria-label="历史观测事件"
            value={event}
            onChange={(e) => setEvent(e.target.value)}
          >
            <option value="">全部记录</option>
            <option value="EVENTS_ONLY">仅有事件的记录</option>
            <option value="FIRST_IN_STOCK">首次有货</option>
            <option value="RESTOCK_DETECTED">无货转有货</option>
            <option value="RECOVERED_IN_STOCK">恢复后有货</option>
            <option value="HEALTH_ALERT">来源异常事件</option>
          </select>
        </label>
        <label>
          开始时间
          <input
            aria-label="历史开始时间"
            type="datetime-local"
            value={from}
            onChange={(e) => setFrom(e.target.value)}
          />
        </label>
        <label>
          结束时间
          <input
            aria-label="历史结束时间"
            type="datetime-local"
            value={to}
            onChange={(e) => setTo(e.target.value)}
          />
        </label>
        <label className="history-target-filter">
          关注目标
          <select
            aria-label="历史目标筛选"
            value={target}
            onChange={(e) => setTarget(e.target.value)}
          >
            <option value="">全部目标（含已移除）</option>
            {snapshot.targets.map((t) => (
              <option key={t.id} value={t.id}>
                {t.product.model} / {t.product.capacity} / {t.product.color} /{" "}
                {t.store.name}
              </option>
            ))}
          </select>
        </label>
      </div>
      <p className="panel-caption">
        各筛选条件同时生效，关键字可用空格组合。时间按本地时区填写，最多180天。下方统计和热力图仅统计筛选后的记录；只看有货时占比为100%，不代表整体供货率。CSV
        / JSON 仍导出当前来源的完整历史。
      </p>
      {error ? (
        <div className="notice error" role="alert">
          {error}
        </div>
      ) : !data ? (
        <div className="inline-empty">
          <LoaderCircle className="spin" size={18} /> 正在汇总本地历史…
        </div>
      ) : (
        <>
          <div className="metrics">
            <Metric
              label="筛选后有货观测占比"
              value={percentage(data.summary.in_stock, data.summary.valid)}
              note={`${data.summary.in_stock} / ${data.summary.valid} 个有效样本`}
              icon={<Check size={18} />}
              green
            />
            <Metric
              label="有效样本覆盖"
              value={percentage(data.summary.valid, data.summary.total)}
              note={`${data.summary.valid} / ${data.summary.total} 次观测`}
              icon={<Activity size={18} />}
            />
            <Metric
              label="有货观测事件"
              value={data.summary.events}
              note="包含首次、恢复和无货转有货"
              icon={<Bell size={18} />}
            />
            <Metric
              label="未知样本"
              value={data.summary.total - data.summary.valid}
              note="保留缺口，不推断实际库存"
              icon={<CircleHelp size={18} />}
            />
          </div>
          <section className="panel">
            <div className="section-title">
              <h2>按小时的观测分布</h2>
              <span className="muted">
                本地时区 UTC{new Date().getTimezoneOffset() <= 0 ? "+" : "−"}
                {Math.abs(new Date().getTimezoneOffset() / 60)}
              </span>
            </div>
            <p className="panel-caption">
              颜色表示有货 /
              有效样本比例。悬停查看分子、分母与覆盖；空白表示没有观测。
            </p>
            <div className="heatmap-scroll">
              <div className="heatmap">
                <div className="heat-row heat-labels">
                  <span />
                  {Array.from({ length: 24 }, (_, hour) => (
                    <span key={hour}>{hour % 3 === 0 ? `${hour}时` : ""}</span>
                  ))}
                </div>
                {dates.map((day) => (
                  <div className="heat-row" key={day}>
                    <span className="heat-day">{day.slice(5)}</span>
                    {Array.from({ length: 24 }, (_, hour) => {
                      const cell = cellMap.get(`${day}:${hour}`);
                      const ratio = cell?.valid
                        ? cell.in_stock / cell.valid
                        : 0;
                      const label = `${day} ${hour}时：有货 ${cell?.in_stock ?? 0} / 有效 ${cell?.valid ?? 0} / 全部 ${cell?.total ?? 0}`;
                      return (
                        <div
                          key={hour}
                          className={`heat-cell ${cell ? (cell.valid ? "sampled" : "unknown") : ""}`}
                          style={
                            cell?.valid
                              ? {
                                  backgroundColor: `rgba(38, 133, 91, ${0.12 + ratio * 0.88})`,
                                }
                              : undefined
                          }
                          tabIndex={0}
                          aria-label={label}
                          title={label}
                        />
                      );
                    })}
                  </div>
                ))}
              </div>
            </div>
            <div className="heat-legend">
              <span>
                无观测 <i />
              </span>
              <span>
                仅未知 <i className="unknown" />
              </span>
              <span>
                有货占比低 <i className="low" />
                <i className="high" /> 高
              </span>
            </div>
          </section>
          <section className="panel">
            <div className="section-title">
              <h2>观测时间线</h2>
              <span className="muted">
                筛选匹配 {data.summary.total} 条 · 显示最新 {data.rows.length}{" "}
                条（最多 5000 条）
              </span>
            </div>
            {!data.rows.length ? (
              <Empty
                title="没有符合条件的记录"
                body="可以切换库存状态、扩大时间范围，或点击“重置筛选”。"
              />
            ) : (
              <div className="table-scroll">
                <table>
                  <thead>
                    <tr>
                      <th>记录时间</th>
                      <th>产品 / 门店</th>
                      <th>状态</th>
                      <th>观测事件 / 原因</th>
                    </tr>
                  </thead>
                  <tbody>
                    {data.rows.map((row) => (
                      <tr key={row.id}>
                        <td className="mono">
                          {dateTime(row.observed_at)}
                          <small>{sourceName(row.source)}</small>
                        </td>
                        <td>
                          <strong>{row.product}</strong>
                          <small>
                            {regionName(row.locale)} · {row.store}
                          </small>
                        </td>
                        <td>
                          <Status state={row.state} />
                        </td>
                        <td>
                          {(row.event && eventNames[row.event]) || "—"}
                          {row.reason && (
                            <small>
                              {reasonNames[row.reason] ?? row.reason}
                            </small>
                          )}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
            <p className="panel-caption">
              观测时刻不等于实际放货时刻。缺失与过期记录之间不推断连续有货区间。导出包含当前来源全部保留历史，不受列表上限和筛选影响。
            </p>
          </section>
        </>
      )}
    </>
  );
}
function Toggle({
  label,
  description,
  checked,
  disabled,
  onChange,
}: {
  label: string;
  description: string;
  checked: boolean;
  disabled: boolean;
  onChange: () => void;
}) {
  return (
    <div className="setting-row">
      <div>
        <strong>{label}</strong>
        <p>{description}</p>
      </div>
      <button
        type="button"
        className={`toggle ${checked ? "on" : ""}`}
        role="switch"
        aria-label={label}
        aria-checked={checked}
        disabled={disabled}
        onClick={onChange}
      >
        <span />
      </button>
    </div>
  );
}
function SettingsPage({
  settings,
  snapshot,
  busy,
  run,
  save,
}: {
  settings: Settings;
  snapshot: Snapshot;
  busy: boolean;
  run: Run;
  save: (patch: Partial<Settings>) => Promise<boolean>;
}) {
  const [bark, setBark] = useState("");
  const [configured, setConfigured] = useState(false);
  const [autostart, setAutostart] = useState(false);
  const [backups, setBackups] = useState<string[]>([]);
  const [backupName, setBackupName] = useState("");
  const [confirmation, setConfirmation] = useState<"restore" | "clear" | null>(
    null,
  );
  const [output, setOutput] = useState("");
  const [loadError, setLoadError] = useState("");
  useEffect(() => {
    let active = true;
    Promise.all([
      command<boolean>("bark_configured"),
      isEnabled(),
      command<string[]>("list_backups"),
    ])
      .then(([b, a, names]) => {
        if (active) {
          setConfigured(b);
          setAutostart(a);
          setBackups(names);
        }
      })
      .catch((e) => {
        if (active) setLoadError(errorText(e));
      });
    return () => {
      active = false;
    };
  }, []);
  const notifications = () =>
    run(async () => {
      if (!settings.local_notifications) {
        const granted =
          (await isPermissionGranted()) ||
          (await requestPermission()) === "granted";
        if (!granted)
          throw new Error(
            "系统通知权限未开启。请在 macOS 系统设置 → 通知中允许库存雷达。",
          );
      }
      await command("save_settings", {
        settings: {
          ...settings,
          local_notifications: !settings.local_notifications,
        },
      });
    });
  return (
    <>
      <div className="page-heading">
        <div>
          <p className="eyebrow">MAKE IT YOURS</p>
          <h1>按你的节奏，保持关注。</h1>
          <p className="subtitle">配置来源、提醒与本地数据管理。</p>
        </div>
      </div>
      {loadError && <div className="notice error">{loadError}</div>}
      {output && (
        <div className="notice">
          <span>{output}</span>
          <button
            onClick={() => void run(() => command("open_data_directory"))}
          >
            打开目录
          </button>
        </div>
      )}
      <ScannerSettings snapshot={snapshot} busy={busy} save={save} run={run} />
      <FeishuSettings settings={settings} busy={busy} run={run} />
      <section className="panel settings-panel">
        <div className="section-title">
          <Radar size={19} />
          <h2>来源与记录</h2>
        </div>
        <div className="setting-row">
          <div>
            <strong>工作模式</strong>
            <p>切换来源会同步切换当前状态与历史。关注目标共用。</p>
          </div>
          <select
            aria-label="工作模式"
            value={settings.mode}
            disabled={busy}
            onChange={(e) =>
              void save({ mode: e.target.value as Settings["mode"] })
            }
          >
            <option value="manual">手动记录</option>
            <option value="mock">模拟演示</option>
            <option value="apple">
              Apple 商店查询{snapshot.automatic_available ? "" : "（未启用）"}
            </option>
          </select>
        </div>
        <div className="banner">
          {snapshot.automatic_available
            ? "Apple 查询已允许，可切换 Apple 模式后开启定时扫描。"
            : "Apple 真实查询尚未启用，请先允许个人使用非公开接口。"}
        </div>
        <div className="setting-row">
          <div>
            <strong>记录有效时间</strong>
            <p>超过此时间后标记过期，不继续显示为当前可取货。</p>
          </div>
          <select
            aria-label="记录有效时间"
            value={settings.stale_minutes}
            disabled={busy}
            onChange={(e) =>
              void save({ stale_minutes: Number(e.target.value) })
            }
          >
            {[5, 10, 15, 30, 60].map((v) => (
              <option key={v} value={v}>
                {v} 分钟
              </option>
            ))}
          </select>
        </div>
        <Toggle
          label="后台模拟演示"
          description="仅在模拟模式下运行，关闭窗口仍会继续；Mac 休眠期间不运行。"
          checked={settings.mock_running}
          disabled={busy}
          onChange={() => void save({ mock_running: !settings.mock_running })}
        />
        <div className="setting-row">
          <div>
            <strong>模拟间隔</strong>
            <p>仅影响演示数据，与真实库存更新频率无关。</p>
          </div>
          <select
            aria-label="模拟间隔"
            disabled={busy}
            value={settings.mock_interval_seconds}
            onChange={(e) =>
              void save({ mock_interval_seconds: Number(e.target.value) })
            }
          >
            {[30, 60, 120, 300, 600, 3600].map((v) => (
              <option key={v} value={v}>
                {v < 60 ? `${v} 秒` : `${v / 60} 分钟`}
              </option>
            ))}
          </select>
        </div>
      </section>
      <section className="panel settings-panel">
        <div className="section-title">
          <Bell size={19} />
          <h2>通知</h2>
        </div>
        <Toggle
          label="macOS 系统通知"
          description="手动与 Apple 查询的状态变化可触发通知，模拟观测不会投递。"
          checked={settings.local_notifications}
          disabled={busy}
          onChange={() => void notifications()}
        />
        <button
          disabled={busy}
          onClick={() =>
            void run(async () => {
              if (
                !(await isPermissionGranted()) &&
                (await requestPermission()) !== "granted"
              )
                throw new Error("请先允许系统通知权限");
              await command("test_notification", { channel: "local" });
            }, "测试通知已提交给 macOS，请检查通知中心")
          }
        >
          <Monitor size={15} />
          测试本机通知
        </button>
        <Toggle
          label="首次有货提醒"
          description="第一次观察到可取货时提醒。"
          checked={settings.first_alert}
          disabled={busy}
          onChange={() => void save({ first_alert: !settings.first_alert })}
        />
        <Toggle
          label="恢复后有货提醒"
          description="未知或过期后恢复为有货，单独标为恢复事件。"
          checked={settings.recovery_alert}
          disabled={busy}
          onChange={() =>
            void save({ recovery_alert: !settings.recovery_alert })
          }
        />
        <Toggle
          label="Bark 推送到 iPhone"
          description="启用后向 api.day.app 发送产品与门店摘要，Mac 需要在线。"
          checked={settings.bark_enabled}
          disabled={busy || !configured}
          onChange={() => void save({ bark_enabled: !settings.bark_enabled })}
        />
        <div className="bark-config">
          <label>
            Bark 地址{" "}
            {configured && (
              <span className="green-text">· 已保存在 Keychain</span>
            )}
            <input
              aria-label="Bark 地址"
              type="password"
              autoComplete="off"
              spellCheck={false}
              value={bark}
              placeholder="https://api.day.app/你的Key"
              onChange={(e) => setBark(e.target.value)}
            />
          </label>
          <div className="button-row">
            <button
              disabled={busy || !bark.trim()}
              onClick={() => {
                const value = bark;
                setBark("");
                void run(async () => {
                  await command("set_bark", { value });
                  setConfigured(true);
                }, "Bark 地址已保存至 Keychain");
              }}
            >
              保存到 Keychain
            </button>
            <button
              disabled={busy || !configured}
              onClick={() =>
                void run(
                  () => command("test_notification", { channel: "bark" }),
                  "Bark 服务已确认接收测试消息",
                )
              }
            >
              测试 Bark
            </button>
            <button
              className="danger-text"
              disabled={busy || !configured}
              onClick={() =>
                void run(async () => {
                  await command("remove_bark");
                  setConfigured(false);
                })
              }
            >
              移除
            </button>
          </div>
          <p>
            首版支持 api.day.app。地址不会从 Keychain 回显，也不会写入 SQLite
            或导出文件。
          </p>
        </div>
      </section>
      <section className="panel settings-panel">
        <div className="section-title">
          <Settings2 size={19} />
          <h2>系统行为</h2>
        </div>
        <Toggle
          label="登录时启动"
          description="登录当前 macOS 账户后启动；默认关闭。"
          checked={autostart}
          disabled={busy}
          onChange={() =>
            void run(async () => {
              if (autostart) await disable();
              else await enable();
              setAutostart(await isEnabled());
            })
          }
        />
        <p className="panel-caption">
          可通过 Dock 或 ⌘Tab 返回应用。关闭窗口后继续后台运行，可点击 Dock
          图标重新打开；要结束后台任务，请退出应用。
        </p>
      </section>
      <section className="panel settings-panel">
        <div className="section-title">
          <Database size={19} />
          <h2>本地数据</h2>
        </div>
        <div className="setting-row">
          <div>
            <strong>历史保留期</strong>
            <p>自动清理旧样本，保留各目标最后一条和最后有效记录。</p>
          </div>
          <select
            aria-label="历史保留期"
            disabled={busy}
            value={settings.retention_days}
            onChange={(e) =>
              void save({ retention_days: Number(e.target.value) })
            }
          >
            {[7, 30, 90, 180].map((v) => (
              <option key={v} value={v}>
                {v} 天
              </option>
            ))}
          </select>
        </div>
        <div className="button-row">
          <button
            disabled={busy}
            onClick={() =>
              void run(async () => {
                setOutput(
                  `备份已保存：${await command<string>("backup_data")}`,
                );
                setBackups(await command<string[]>("list_backups"));
              })
            }
          >
            <Database size={15} />
            创建完整备份
          </button>
          <button
            disabled={busy}
            onClick={() => void run(() => command("open_data_directory"))}
          >
            打开数据目录
          </button>
          <button
            disabled={busy}
            onClick={() =>
              void run(async () => {
                setOutput(
                  `已清理 ${await command<number>("prune_history")} 条过期历史。`,
                );
              })
            }
          >
            立即清理过期记录
          </button>
        </div>
        <div className="restore-row">
          <select
            aria-label="选择备份"
            value={backupName}
            onChange={(e) => setBackupName(e.target.value)}
          >
            <option value="">选择要恢复的备份</option>
            {backups.map((name) => (
              <option key={name}>{name}</option>
            ))}
          </select>
          <button
            disabled={busy || !backupName}
            onClick={() => setConfirmation("restore")}
          >
            恢复备份
          </button>
        </div>
        <p className="panel-caption">
          备份包含关注、设置、手动与模拟历史，不包含飞书私有配置或 Keychain
          凭证。恢复前自动备份当前库；待投递消息不会自动重发。
        </p>
        <div className="danger-zone">
          <div>
            <strong>清空当前来源历史</strong>
            <p>
              只清空{sourceName(settings.mode)}
              观测、事件和投递记录；保留关注与另一来源。
            </p>
          </div>
          <button
            className="danger-text"
            disabled={busy}
            onClick={() => setConfirmation("clear")}
          >
            <Trash2 size={15} />
            清空历史
          </button>
        </div>
      </section>
      {confirmation && (
        <Confirm
          title={
            confirmation === "restore" ? "恢复这个备份？" : "清空当前来源历史？"
          }
          body={
            confirmation === "restore"
              ? `将用 ${backupName} 替换当前关注、设置和全部历史。当前库会先自动备份。`
              : `将永久清空${sourceName(settings.mode)}历史，建议先创建备份。`
          }
          busy={busy}
          close={() => setConfirmation(null)}
          confirm={async () => {
            const action = confirmation;
            const ok = await run(
              async () => {
                await command(
                  action === "restore" ? "restore_data" : "clear_history",
                  action === "restore" ? { name: backupName } : undefined,
                );
                if (action === "restore")
                  setBackups(await command<string[]>("list_backups"));
              },
              action === "restore"
                ? "备份已恢复，请核对通知开关"
                : "当前来源历史已清空",
            );
            if (ok) setConfirmation(null);
          }}
        />
      )}
    </>
  );
}
function Diagnostics({
  snapshot,
  latest,
  busy,
  run,
}: {
  snapshot: Snapshot;
  latest: string | null;
  busy: boolean;
  run: Run;
}) {
  return (
    <>
      <div className="page-heading">
        <div>
          <p className="eyebrow">EVERYTHING, IN VIEW</p>
          <h1>了解你的雷达。</h1>
          <p className="subtitle">数据来源、最近记录与通知投递状态。</p>
        </div>
        <button
          disabled={busy}
          onClick={() => void run(() => command("open_data_directory"))}
        >
          打开本地目录
          <ArrowUpRight size={15} />
        </button>
      </div>
      <div className="diagnostic-grid">
        <section className="panel">
          <div className="section-title">
            <ShieldCheck size={19} />
            <h2>来源状态</h2>
          </div>
          <dl>
            <dt>自动库存来源</dt>
            <dd>
              {snapshot.automatic_available
                ? "Apple · 查询访问已启用"
                : "Apple · 尚未启用"}
            </dd>
            <dt>当前模式</dt>
            <dd>{sourceName(snapshot.settings.mode)}</dd>
            <dt>最近观测</dt>
            <dd>{dateTime(latest)}</dd>
            <dt>运行状态</dt>
            <dd>{snapshot.settings.paused ? "已暂停" : "运行中"}</dd>
          </dl>
          <p className="panel-caption">
            {snapshot.automatic_available
              ? "扫描按设置间隔分批执行；拒绝与限流会停止来源，请在设置中查看停止原因。"
              : "Apple 查询尚未启用。可在设置中允许个人查询，模拟场景只影响演示数据。"}
          </p>
        </section>
        <section className="panel">
          <div className="section-title">
            <Database size={19} />
            <h2>目录与存储</h2>
          </div>
          <dl>
            <dt>目录版本</dt>
            <dd>{snapshot.catalog_version}</dd>
            <dt>本地存储</dt>
            <dd>SQLite · 独立数据库</dd>
            <dt>凭证存储</dt>
            <dd>飞书：本机私有文件 · Bark：macOS Keychain</dd>
            <dt>时间记录</dt>
            <dd>UTC 保存 / 本地时区显示</dd>
          </dl>
          <p className="panel-caption">
            目录为上游离线快照，尚未在线核验 SKU 与门店当前可用性。
          </p>
        </section>
      </div>
      <section className="panel">
        <div className="section-title">
          <Bell size={19} />
          <h2>最近通知投递</h2>
          <span className="muted">最多 100 条</span>
        </div>
        {snapshot.deliveries.length ? (
          <div className="table-scroll">
            <table>
              <thead>
                <tr>
                  <th>时间</th>
                  <th>事件</th>
                  <th>渠道</th>
                  <th>结果</th>
                </tr>
              </thead>
              <tbody>
                {snapshot.deliveries.map((d) => (
                  <tr key={d.id}>
                    <td>{dateTime(d.attempted_at)}</td>
                    <td>
                      {d.title}
                      <small>{d.error}</small>
                    </td>
                    <td>
                      {d.channel === "local"
                        ? "macOS"
                        : d.channel === "feishu"
                          ? "飞书"
                          : "Bark"}
                    </td>
                    <td>
                      <span
                        className={`delivery ${d.status === "FAILED" ? "danger-text" : ""}`}
                      >
                        {deliveryNames[d.status] ?? d.status}
                      </span>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : (
          <Empty
            title="暂无通知投递"
            body="手动与 Apple 查询事件可产生投递记录。模拟观测不触发通知。"
          />
        )}
        <p className="panel-caption">
          「已提交」表示系统或服务已接收，不保证设备已展示。进程退出时结果不确定的消息不会自动重发。
        </p>
      </section>
    </>
  );
}
