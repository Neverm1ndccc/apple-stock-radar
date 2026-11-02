# 库存雷达工程规则

- 本项目为 Tauri 2 + Rust + React/TypeScript 本地应用，GPL-3.0-or-later。
- 离线目录及图标派生自 Apple Pickup Watcher，归属和基线见 NOTICE。
- `crates/radar-core` 负责状态、目录、SQLite 与统计；`src-tauri` 负责系统能力；`src` 负责展示。
- 库存失败必须为 UNKNOWN；模拟、手动数据隔离；状态与告警判断在 Rust。
- 用户于 2026-10-25 明确同意本项目个人查询 Apple 商店非公开库存接口；该同意仅为项目使用授权，不得表述成 Apple 授权。其他自动源仍需单独确认。
- Apple 真实查询仅在本机用户启用后执行；拒绝、限流、策略阻断时立即停止该源，等待人工处理。不添加自动下单。
- 常规检查保持离线：cargo test、cargo fmt --all --check、cargo clippy --all-targets -- -D warnings、pnpm build、pnpm test。
- 不把 Apple live 请求加入自动化测试；允许用户明确授权的应用内实际查询与结果核对。不在日志、SQLite 和前端响应中暴露 Bark Key。
- 用户于2026-11-01要求飞书直接配置到程序中，避免每次发送请求钥匙串密码。飞书 Webhook 与签名密钥可存本机应用专用私有配置文件（目录0700、文件0600），不回显或写入日志、SQLite、备份；正常发送不得读取钥匙串。旧Keychain仅在用户明确点击导入时读取一次。只为用户开启的关注发送，模拟数据不发送。Bark仍只存Keychain。
- 不覆盖上游旧设置：本项目使用独立 identifier 和新数据库。
