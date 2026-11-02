# 本地应用架构

v0.3.2 的批量操作使用 TargetBatchAction 枚举与一次 batch_update_targets IPC。前端只传当前可见且勾选的目标 ID；Rust 限制数量、去重、验证全部未归档目标，并在 SQLite IMMEDIATE 事务内更新及取消相应待发消息。失败整批回滚，成功才更新扫描 generation；飞书偏好变更不取消当前库存请求。数据库锁在刷新原生 UI 前释放。

日期：2026-10-24

`crates/radar-core` 持有目录、状态归约、SQLite、事件、通知投递队列、统计、CSV/JSON 与备份恢复。`src-tauri` 负责 IPC、菜单栏、窗口生命周期、Keychain、本地通知、Bark 与离线模拟定时器。`src` 仅负责展示与用户输入，不在前端推断库存、创建事件或执行告警规则。

## 数据模型与时间

- 地区内 SKU 和地区内门店为独立实体；精确目标 `(variant_id, store_key)` 唯一。批量新增创建分组，通过成员表共享目标，不重复观测。
- 观测带 `manual` / `mock` 来源。同一目标按来源分别查询状态、历史、事件和统计；v0.2.0 增加 apple 独立分区，目标本身共用。
- UTC RFC3339 毫秒时间进入 SQLite，本地时区显示。统计按 UI 提供的时区偏移聚合；当前固定偏移适用于中港，尚未实现跨夏令时历史的 IANA 时区规则。
- 失败为 UNKNOWN，并保留上一有效记录；有效结果超过可配置阈值为 STALE。PENDING 与 UNSUPPORTED 单独展示。
- UNKNOWN、STALE 均打断连续可信观测；恢复有货不推断缺口中发生补货。首次有货和无货转有货分别记录。

## 存储与幂等

SQLite 开启 WAL、外键与 busy timeout，通过单个 Mutex 串行访问。观测 ID 唯一且同 ID 不同内容拒绝；事件关联唯一 observation；投递 `(event_id, channel)` 唯一。SENDING 进程中断后记为 UNCERTAIN，不盲目重发，重启前尚未发送的 PENDING 记为 CANCELLED，避免迟到的旧提醒。暂停或移除目标取消 PENDING 投递。全局暂停与模拟模式都不调度手动消息。

不得持有数据库或扫描器锁调用会同步等待主线程的原生 UI API。v0.2.4 的 tray_refresh 先生成拥有自身数据的展示快照，释放数据库锁后调用托盘 setter；并发回归模拟后台等待 UI 时主线程读取数据库，防止相互等待。

历史列表最多 5000，统计查询全量符合时间条件的样本，完整导出不受列表上限限制。保留期清理保留每目标每来源最新及最后有效样本，用于维持当前状态语义；因此完整导出可能含有早于保留期的这两类样本。

备份使用 SQLite backup API。恢复先检查完整性、版本、必要表、外键，在临时数据库验证设置、商品/门店目录与读取路径，再替换原库。Tauri 命令在恢复前自动备份。恢复后待投递标为 UNCERTAIN，Bark 与自动模拟关闭。高版本库拒绝打开，不覆盖版本号。

## 系统能力与凭证

Identifier 为 `local.dingdingguo.stockradar`，不读取或迁移上游设置。通知、登录启动权限仅授予本地主窗口和托盘卡片。前端不具备任意文件或通用链接打开权限；官方跳转由 Rust 验证 HTTPS、精确 Apple 主机、购物路径以及无凭证/端口/查询参数。

Bark 地址只写 Keychain service `local.dingdingguo.stockradar.bark`。前端只接收 configured 布尔值；HTTP 错误使用固定脱敏文案，不透出 reqwest URL。客户端只向 `https://api.day.app/push` 发送测试或启用的手动事件，禁重定向、带超时。模拟观测不创建投递。

定时器每秒检查离线模拟节奏，每 15 秒刷新托盘与过期状态，每小时清理历史。休眠不补发积压模拟；v0.2.0 新增 Apple 协议适配器和按地区/门店分批的调度、请求预算与失败退避；许可登记缺失时不发请求。接入和迁移说明见 APPLE_SCANNER.md。

v0.2.1 增加用户明确启用的 Personal 访问配置，与提供方 Licensed 配置分开。v0.2.2 将累计预算、失败计数、Retry-After 与保护等待写入普通设置；发请求前使用 SQLite IMMEDIATE 事务预约额度。恢复备份保留当前与备份中更严格的保护状态。原生调度与立即扫描共用此路径，前端只展示预算与等待时间。拒绝/限流持久停扫，保护期满仍不自动探测。

v0.3.0 增加飞书群 Webhook：radar-core::feishu 负责官方地址校验、文本/签名协议、大小、成功码与状态文本；target_notifications 保存每目标选择，Settings 保存全局开关，deliveries 增加 feishu 渠道，使用既有事件/渠道去重。src-tauri::feishu_notify 负责独立 Keychain 配置、共享异步串行发送（至少 700ms）、实际 HTTP 和固定脱敏错误；配置变更与发送互斥，避免旧队列换目的地。HTTP 或等待期间不持有数据库/扫描器锁，原生 UI 更新也不在锁内执行。Schema v3 迁移前备份，旧备份恢复关闭全局飞书，保留目标选择。配置说明见 FEISHU.md。

## v0.4.0 连接与扫描健康

apple_transport 单独构造普通 reqwest 客户端，开启标准 HTTP/2 协商，与通知客户端隔离；固定的协议名和毫秒耗时进入 Settings。scan_connection_protection 由核心计算有效间隔，Scanner 和 SQLite 原子预约共享，保存设置不能改写运行时诊断。

AppleBatch::parse_coverage 解析一次响应，严格复用同地区已关注门店的本次请求 SKU；next_batch_index 根据持久化最旧观测选批，游标用于同龄批次公平轮转。没有返回的额外目标不产生记录，失败仍为 UNKNOWN。

scan_health 在 Settings 保存最近一次事故的 ID/原因/HTTP/时间及状态，原子 SQL 条件领取与完成避免并发重复或旧确认覆盖新事故。feishu_notify 与有货消息共享速率锁，短数据库读取后释放再网络等待。升级沿用 schema v3 和 serde 缺省字段，不重置预算、关注或用户通知设置。

2026-10-25 补充：后台扫描与通知投递使用独立任务，钥匙串读取在 blocking 线程执行，通知等待系统授权不能阻塞下一批库存扫描。新增并发回归验证通知不完成时扫描仍连续运行；Rust 测试总计 65 项。

## v0.5.0 历史组合筛选

radar-core::history::HistoryFilter 接收库存状态、目标、地区、空格分隔关键字、事件及时间范围，校验后以绑定参数构造共用SQL条件。列表先筛选再取最新5000条，汇总数量与本地时区小时格子按完整匹配集合聚合；不从截断后的前端列表计算。query_history IPC在一次数据库锁内返回rows/summary/since/until，异步响应失效后前端丢弃。

自定义时间以带偏移的RFC3339规范化到UTC毫秒，含两端，最大180天。未知观测保持UNKNOWN，不混入有效分母；状态筛选后占比只代表该匹配集合。历史包含已移除目标，来源仍取当前模式隔离。SQL关键字按字面子串匹配，百分号/下划线不会成为通配符。既有完整CSV/JSON导出保持完整来源语义，界面明确说明。

## v0.6.0 飞书本机配置

按用户2026-11-01要求，feishu_config在独立private目录以0700/0600保存未加密JSON，正常飞书流程只使用该文件。保存使用随机create_new临时文件、sync_all、原子rename；读取上限8KB、拒绝符号链接/硬链接/特殊文件，错误不含凭证。Credentials仅进入Rust HTTP逻辑，configured只返回布尔值；文件排除SQLite备份/导出和源代码提交。

set_feishu/import_feishu_keychain/remove_feishu仍与发送共享速率锁，配置变化先关闭全局并取消旧待发消息。只有用户主动导入命令可调用旧Keychain，正常配置检查/开启/发送不调用。无本机配置时自动投递不领取消息，避免后台触发旧密码提示；重启仍取消旧PENDING/标记中断SENDING，不自动补发。Bark的Keychain逻辑保持。历史章节的Keychain说明代表旧版本。
