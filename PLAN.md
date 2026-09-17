# trae-signin-gui — 实施计划（PLAN）

> 状态：已复核通过（2026-09-14 六轮 grilling 拍板后落盘；同日复核确认，结论见第 8 节）
> 目标：把 [Maquer/trae-signin](https://github.com/Maquer/trae-signin)（纯 Go CLI：登录 → 签到 → 积分 → Bark）实现为 Windows 桌面 GUI 程序。
> UI/托盘参考 [changexbc/workbuddy-switch](https://github.com/changexbc/workbuddy-switch)，**仅参考视觉与页面组件组织，不照搬其业务**（账号切换/会话复制/Token 统计均不引入）。

---

## 1. 已拍板决策（不可擅自变更）

| # | 决策项 | 结论 |
|---|--------|------|
| 1 | 技术栈 | Tauri 2 + React 19 + Vite 7 + Tailwind 4 + Radix UI + zustand；**Rust 重写全部逻辑**（不保留 Go、不用 sidecar） |
| 2 | 平台 | 仅 Windows（Win10/11 x64） |
| 3 | 交付物 | **单 exe 便携文件**，不做安装包（跳过 NSIS/MSI bundle） |
| 4 | 项目名 | `trae-signin-gui`（安装显示名：TRAE 签到） |
| 5 | 登录 | 内置本地回调服务自动捕获（`127.0.0.1:{port}/authorize`），用户不粘贴链接 |
| 6 | 关闭窗口 | 可配置；默认隐藏到托盘常驻 |
| 7 | 定时 | 应用内调度：启动补签 + 每日定点（默认 08:00，可改）+ 周期检查；**不做** Windows 任务计划兜底 |
| 8 | 凭证格式 | 沿用 `auths/trae-{uid}.json` 嵌套格式，与 CLI 版、GitHub Actions secrets 互通 |
| 9 | 数据目录 | 首次启动用户自选，默认预填 exe 同级目录 |
| 10 | 通知 | Windows 系统通知（默认开）+ Bark 可选（设置页填 URL） |
| 11 | 托盘 | 菜单 4 项 + 图标状态反馈（有未签账号时红点） |
| 12 | 页面 | 侧边栏三页：账号 / 签到历史 / 设置 |
| 13 | 首次启动 | 空状态引导卡（选目录 / 登录账号 / 进入设置），不做向导 |
| 14 | 主题 | 暗色为主 + 可切浅色（跟随系统可选） |
| 15 | 启动行为 | 自启默认**关**（设置页开启）；自启时静默进托盘，手动双击显示窗口 |
| 16 | token 策略 | 签到前惰性刷新（2h 缓冲）+ 每日保活刷新一次 |
| 17 | 更新器 | 仅骨架占位（updater 插件配置 + 设置页"检查更新"按钮），暂不接 GitHub Releases |
| 18 | 测试 | Rust core 单元测试 + 手工冒烟；不写前端组件测试 |
| 19 | 开发顺序 | M1 骨架 → M2 core+单测 → M3 登录 → M4 账号页+签到 → M5 调度+托盘+通知 → M6 历史+设置 → M7 打包 |

### 明确不做（非目标）

积分趋势图、本机 Trae IDE 登录态导入、导出凭证给 Actions、Windows 任务计划兜底、安装包、macOS/Linux、前端组件测试、保留 Go/CLI。

---

## 2. 上游 API 事实（从 trae-signin 源码提取，集中管理）

> 全部常量集中在 `crates/trae-signin-core/src/upstream.rs` 顶部，上游变更只改一处。

| 项 | 值 |
|---|---|
| OAuth Host | `https://api.trae.com.cn` |
| UG Host（签到/积分） | `https://api.trae.cn` |
| ClientID | `en1oxy7wnw8j9n` |
| IDE 版本 / UA | `0.1.43` / `20260716`，UA = `Trae/0.1.43` |

**端点**

| 端点 | Host | 用途 |
|---|---|---|
| `POST /cloudide/api/v3/trae/oauth/ExchangeToken` | OAuth | refreshToken → accessToken |
| `POST /cloudide/api/v3/trae/GetUserInfo` | OAuth | 取 uid / nickname（header `x-cloudide-token`） |
| `POST /trae/api/v2/ug/checkin_credits/status` | UG | 查询签到状态 |
| `POST /trae/api/v2/ug/checkin_credits/claim` | UG | 执行签到 |
| `POST /trae/api/v2/pay/ide_user_ent_usage` | UG | 积分余额 |

**请求要点**

- ExchangeToken body：`{"ClientID","RefreshToken","ClientSecret":"-","UserID":""}`
- UG 请求 headers（Go `ugHeaders`）：`Authorization: Cloud-IDE-JWT {accessToken}`、`Accept: application/json`、`X-User-Region: CN`、`X-Device-Id`（**仅非空时发送**）、`User-Agent`；**无 `X-Machine-Id`、无版本头**（`IDE_BUILD` 仅为记录上游事实，不参与任何请求）
- 过期时间兼容秒/毫秒（`> 1e12` 视为毫秒除以 1000）；无 `TokenExpireAt` 时用 `TokenExpireDuration` 推算
- 签到判定顺序：`status.checked_in == true` → ALREADY；`enable == false` → DISABLED；否则 `claim`。错误信息含"已签到/already check"归为 ALREADY
- **claim 判定三分规则**（2026-09-15 用 `history.jsonl` 实测修正，2026-09-16 用 `app.log` 原始响应体收紧）：
  1. `HTTP >= 400` → 失败；文案含"已签到/already check"归 ALREADY；401/403 → `AuthExpired`，408/429/5xx → `Retryable`
  2. `HTTP 2xx` + **拥塞文案**（`太多/拥挤/繁忙/稍后/重试/too many/busy/later/retry`，**与 `code` 取值无关**）→ `Retryable`：签到**确实没生效**，退避重试后仍失败才算今日未签。证据：三次「当前参与用户太多，请稍后再试」后积分仍 5750，人工补签才 5900
  3. `HTTP 2xx` + 其余 → 成功（对齐 Go `doJSON`：它从不解析响应体）
  > 注意两个曾被当作"事实"的错判：旧实现把规则 2 的响应记成**不可重试的失败**（用户只能手动补签）；把它改成"2xx 即成功"则会记成**假成功**（积分没到账却显示已签）。两条路都错，正解是重试。
  > 规则 2 原先还要求 `code != 0`；`code: 0` 或无 `code` 字段回同样文案时仍会记假成功，故已去掉该前置条件。**不得**把匹配范围扩到原始响应体：成功报文里可能带 `needRetry` 之类的键，而特征表含 `retry`。
- **实测拥塞响应体**（2026-09-16 08:47 `app.log`，HTTP 200）：`{"code":9074,"message":"当前参与用户太多，请稍后再试"}` —— 仅此两字段，无 `data` 包装。`9074` 是**目前唯一实测到的拥塞码**，将来要换成 code 白名单就从这一条起步
- 判定性字段（`code`/`checkedIn`/`enable`）取值优先序：**顶层 → `data` 包装 → 全树递归**，避免兄弟子树按字典序抢先命中
- UG 瞬时错误按 `retry_backoff * 2^n` + 抖动重试，共 `UG_ATTEMPTS = 2` 次（基础退避 **20s**；`with_retry_backoff` 可注入，测试置 0）。2026-09-16 从「3 次 / 2s」收紧：9074 的实测行为更像**按客户端限流**（同账号同 IP，客户端三连拒后 1 分钟人工补签即成功），秒级密集重试等于在同一个拒绝窗口里自激
- 积分 = `sum(user_entitlement_pack_list[].entitlement_base_info.quota.credits_limit)`
- 刷新缓冲 2 小时（`expiresAt - now <= 2h` 即刷新）

**登录链接**（`https://www.trae.cn/authorization?` + query）

`login_version=1, auth_from=solo, login_channel=native_ide, plugin_version=2.3.62834, auth_type=local, client_id, redirect=0, login_trace_id=<hex16>, auth_callback_url=http://127.0.0.1:{port}/authorize, machine_id, device_id, x_device_id, x_machine_id, x_device_brand=PC, x_device_type=PC, x_os_version=1.0, x_app_version=0.1.43, x_app_type=stable`

- `machine_id`/`device_id`：32 位 hex（Rust 用 `Uuid::new_v4().simple()`），**每个账号独立生成并随凭证保存**
- 回调 query 关键参数：`refreshToken`、`userInfo`（JSON：`UserID`/`ScreenName`）、`userJwt`（JSON：`Token`/`RefreshToken`/`TokenExpireAt`）
- 优先走 `refreshToken` → ExchangeToken；无 refreshToken 时用 `userJwt.Token` 兜底

**凭证文件**（`auths/trae-{uid}.json`，嵌套形态，原子写：tmp + rename）

```json
{
  "auth": { "accessToken": "...", "refreshToken": "...", "expiresAt": 1786858238,
            "domain": "trae.cn", "apiHost": "https://api.trae.com.cn",
            "machineId": "...", "deviceId": "..." },
  "account": { "uid": "...", "enterpriseId": "", "nickname": "..." }
}
```

解析需兼容**嵌套**与**扁平**两种历史形态（对齐原 `auth.Parse`）。

---

## 3. 工程结构

```
AI-TraeQD/
├── PLAN.md
├── package.json / vite.config.ts / tsconfig.json / index.html
├── src/                          # React 前端
│   ├── main.tsx / App.tsx
│   ├── pages/                    # AccountPage / HistoryPage / SettingsPage
│   ├── components/               # ui 基础组件（参考 wb-switch 的 Radix 封装）+ 业务组件
│   ├── stores/                   # zustand：accounts / settings / signin / ui
│   ├── lib/                      # invoke 封装、事件订阅、格式化工具
│   └── styles/                   # Tailwind 入口 + 主题 CSS 变量
├── src-tauri/                    # Tauri 壳
│   ├── src/main.rs / lib.rs      # 命令注册、托盘、单实例、窗口行为、事件桥
│   ├── tauri.conf.json           # 单窗口；updater 占位配置
│   ├── icons/                    # 含托盘 normal / red-dot 两态 PNG
│   └── Cargo.toml
└── crates/
    └── trae-signin-core/         # 纯逻辑 crate（无 Tauri 依赖，可单测）
        └── src/{auth,upstream,login,scheduler,history,settings}.rs
```

**Rust 关键依赖**：`tauri`（features: `tray-icon`）、`tauri-plugin-{opener,notification,dialog,autostart,updater,single-instance}`、`reqwest`（rustls-tls）、`tokio`、`serde/serde_json`、`uuid`、`chrono`、`thiserror`；dev-deps：`httpmock`。

**前端关键依赖**：`react@19`、`@tauri-apps/api@2` + 各 plugin 前端包、Radix（dialog/alert-dialog/dropdown-menu/scroll-area/select/separator/switch/tabs/tooltip/label/slot）、`tailwindcss@4`、`lucide-react`、`sonner`、`zustand`、`clsx` + `tailwind-merge` + `class-variance-authority`。

---

## 4. 功能规格

### 4.1 登录（M3）

1. 前端点"添加账号" → Rust 生成 `machine_id`/`device_id`/`login_trace_id`，先探测可用端口（默认 18080，占用则 18081 起递增至 18089），**再**用该端口拼 `auth_callback_url` 生成登录链接
2. `opener` 打开系统默认浏览器；前端显示"等待浏览器授权…"状态卡（可取消）
3. 本地服务（`tokio::net::TcpListener` 手写极简 HTTP，只处理一次 GET `/authorize`）捕获回调 → 立即回一个"登录成功，请返回应用"的 HTML 页面 → 关停服务
4. 解析回调 → ExchangeToken → GetUserInfo → 原子落盘 `auths/trae-{uid}.json`（同名 uid 覆盖更新）→ 发 `login://done` 事件 → 前端刷新账号列表
5. 超时 5 分钟自动取消；取消/超时后关闭监听端口；18080-18089 全部被占用时**报错中止**登录（提示释放端口后重试）

### 4.2 账号页（M4）

- 顶部工具栏：`添加账号`、`导入凭证`（粘贴 JSON，弹窗 + 校验）、`全部签到`、`全部刷新`
- 账号卡片：昵称、UID（截断 + hover 全显）、积分、今日签到状态徽章（已签/未签/禁用/失败/未知）、token 有效期、操作（签到 / 刷新 / 删除[确认框]）
- 批量签到：**串行**执行，逐账号实时进度（待签 → 刷新 token → 签到 → 查积分 → 结果），行内状态流式更新；进行中禁用重复触发
- 单账号重签：卡片上的签到按钮
- 全部刷新：查询状态 + 积分（不签到）

### 4.3 调度与保活（M5）

- **启动补签**：应用启动后 30 秒内后台执行一轮"未签则签"检查
- **每日定点**：默认 `08:00`（设置可改 HH:MM），到点执行全部未签账号签到
  - 整轮**无进展**（每个账号都因上游限流/网络瞬时失败）时**不再把当天记为已完成**：按 `5/15/30/60` 分钟阶梯延后重试（`scheduler_task.rs · DAILY_RETRY_LADDER_SECS`），阶梯用尽才放弃今日定点轮、由周期检查兜底。2026-09-16 修：此前一次拥塞就把当天烧光（07:00 假成功后全天不再试）
  - 阶梯记在内存（`Instant`），**重启即重置**——重启后定点轮会立刻再跑一次
- **周期检查**：每 N 分钟（默认 30，可关）检查是否有账号今日未签且可签，有则补签
- **每日保活**：每天无条件刷新一次所有账号 token；失败**区分错误类型**——网络/超时类错误标"刷新失败"（可重试），401/refreshToken 无效才标"需重新登录"（历史记录失败原因，不阻塞其他账号）
- 全局互斥：任何一轮签到执行期间，定时任务顺延、手动触发提示"签到进行中"
- 调度用 `tokio` task + `chrono` 本地时区计算下一次触发时刻；设置变更后重建调度

### 4.4 托盘（M5）

- 常驻图标，菜单：`显示主窗口` / `立即签到全部` / `今日状态：已签 2/3`（只读，动态刷新） / `退出`
- 图标两态：正常 / 红点（存在今日未签账号时）；每次账号状态变化后更新图标与菜单文案
- 左键单击 → 显示/聚焦主窗口；点关闭按钮 → 按设置隐藏到托盘或退出
- 单实例（`single-instance` 插件）：二次启动唤起已有窗口

### 4.5 历史页（M6）

- JSONL 追加：`{ ts, uid, nickname, status, credits, message }`，文件 `history.jsonl` 存数据目录
- 列表：时间倒序，展示 时间 / 昵称 / 状态徽章 / 积分 / 详情；支持按账号筛选；默认加载最近 200 条

### 4.6 设置页（M6）

| 分区 | 项 |
|---|---|
| 数据与存储 | 数据目录（选择 + 打开；**重选后自动迁移** `auths/` 与 `history.jsonl` 到新目录，成功后更新 exe 同级指针，弹窗展示进度，失败保持原目录）、凭证文件数量 |
| 定时与保活 | 每日签到时间、周期检查开关+间隔、每日 token 保活开关 |
| 通知 | 系统通知开关、Bark URL（含"测试推送"按钮） |
| 启动与托盘 | 开机自启（默认关）、关闭窗口行为（托盘/退出，默认托盘） |
| 外观 | 主题（暗/亮/跟随系统） |
| 关于 | 版本号、"检查更新"按钮（占位：未配置更新源时提示） |

### 4.7 首次启动（M1/M6）

主界面空状态引导卡：`选择数据目录` / `登录第一个账号` / `导入凭证` 三个入口 + 一句说明（凭证存放位置）。

---

## 5. 状态与数据设计

**settings.json**（存数据目录）：

```json
{
  "data_dir": "D:/Pa/AI-TraeQD/data",
  "daily_time": "08:00",
  "periodic_check_minutes": 30,
  "keep_alive_daily": true,
  "notify_system": true,
  "bark_url": "",
  "close_to_tray": true,
  "autostart": false,
  "theme": "dark"
}
```

**数据目录引导指针**（解决循环依赖）：`settings.json`（含 `data_dir`）存数据目录内，但启动时需先定位数据目录才能读到它。方案：首次选定目录后，在 **exe 同级写入 `data-dir.txt`**（一行路径）；每次启动先读该指针定位数据目录，再读 `settings.json`。指针缺失/无效 → 视为首次启动走引导卡。

**运行日志**（`{数据目录}/app.log`，2026-09-16 加）：`logger.rs` 的极简 `log` 后端，Info 级、单条截断 600 字、超 2 MiB 轮转为 `app.log.1`；数据目录变更时跟随。存在的唯一理由：此前没有任何后端，所有 `log::` 静默丢弃，上游用 HTTP 200 + 业务码拒绝签到时**拿不到原始响应体**，判据只能靠猜。现在 `claim` 的完整响应体逐次落盘。注意它可能带 token 类字段，与 `auths/` 同目录、同一敏感级别。

**Tauri 命令**：`get_settings` / `set_settings` / `choose_data_dir` / `list_accounts` / `import_credential` / `start_login` / `cancel_login` / `signin_all` / `signin_one` / `refresh_all` / `refresh_account` / `delete_account` / `get_history` / `test_bark` / `check_update`(占位) / `open_data_dir` / `open_external`。

**事件**：`signin://progress`（逐账号阶段）、`signin://done`（汇总）、`login://done` / `login://failed`、`tray://status`（供前端同步今日状态）。

---

## 6. 里程碑与验收

| 里程碑 | 内容 | 验收标准 |
|---|---|---|
| **M1 骨架** | Tauri 2 工程初始化、三页空壳 + 侧边栏、托盘图标占位（仅显示/退出可用）、单实例、关闭到托盘可配 | `npm run tauri dev` 起窗口；托盘可见；关窗行为生效；二次启动唤起窗口 |
| **M2 core + 单测** | `trae-signin-core`：auth 解析/原子写回（嵌套+扁平）、upstream 全流程、login 链接生成与回调解析、scheduler 时间计算、history JSONL、settings | `cargo test` 全绿：覆盖字段兼容、秒/毫秒过期、签到四态分支、mock HTTP 全流程（httpmock）、JSONL 读写、定点计算（含跨天） |
| **M3 登录** | 端口选择 + 链接生成 + 浏览器打开 + 本地回调服务 + 换 token + GetUserInfo + 落盘 | 真实登录 1 个账号，`auths/trae-{uid}.json` 生成且被 CLI 版 `signin.sh` 认可 |
| **M4 账号页 + 签到** | 账号卡片列表、串行批量签到 + 实时进度、单账号重签、粘贴导入、删除确认 | 多账号签到结果与 CLI 输出一致（成功/已签/禁用/失败计数）；进度实时可见 |
| **M5 调度 + 托盘 + 通知** | 应用内调度（补签/定点/周期）、每日保活、托盘红点与菜单动态状态、系统通知、Bark 可选 | 把定点改到 1 分钟后，到点自动签到并弹通知；托盘红点随签到状态变化；Bark 测试推送成功 |
| **M6 历史 + 设置** | 历史列表与筛选、设置页全部项落位、空状态引导卡、主题切换、更新检查占位 | 手工冒烟全项；重启后设置与账号保留；主题切换即时生效 |
| **M7 打包** | release profile 优化（lto/strip/opt-level=s）、取单 exe、便携冒烟 | 单 exe 双击可用；换目录/移盘后数据目录选择正常；托盘、通知、自启在 exe 形态下均正常 |

**最终验收**（M7 后一次完整走查）：首次启动 → 选目录 → 浏览器登录 2 个真实账号 → 批量签到出结果 → 关窗到托盘 → 托盘"立即签到"可用 → 定点触发 + 通知 → 重启保留 → 开机自启（默认关，手动开）静默进托盘。

---

## 7. 风险与对策

| 风险 | 对策 |
|---|---|
| 上游 API 无文档、可能变更 | 常量与端点集中一处；解析对未知字段宽容（只取所需字段，不校验多余字段） |
| 无签名 exe 触发 SmartScreen | 已知限制，交付说明中注明"更多信息 → 仍要运行" |
| 回调端口被占用 | 18080 起递增探测（≤18089），先定端口再生成链接；全部占用则报错中止 |
| WebView2 缺失（老 Win10） | Win11 自带；缺失时 Tauri 弹引导，作为已知前提写入 README |
| refreshToken 失效需重新登录 | 卡片状态标"需重新登录"，历史记录失败原因；不阻塞其他账号 |
| 单 exe 首次运行需写数据目录 | 目录选择框默认 exe 同级，规避 Program Files 只读场景；写入失败给明确文案 |
| 每日保活/定时任务在窗口关闭后遗漏 | 关闭默认走托盘常驻；退出时若当日未签完，托盘菜单可见状态 |

---

## 8. 复核结论（2026-09-14 已确认）

### 8.1 原 5 个复核关注点（全部拍板）

1. 托盘"今日状态"文案：`已签 2/3`，无账号时 `未添加账号` —— ✅ 接受
2. 周期检查默认 30 分钟（可关/可改）—— ✅ 接受
3. 登录回调超时 5 分钟自动取消 —— ✅ 接受
4. 历史页默认加载最近 200 条，仅按账号筛选 —— ✅ 接受
5. 设置页 Bark "测试推送"按钮 —— ✅ 需要，保留（对应 `test_bark` 命令）

### 8.2 复核中新增拍板的设计补充

| # | 项 | 结论 |
|---|---|---|
| 6 | 数据目录引导指针 | 选定目录后写 **exe 同级 `data-dir.txt`**；启动先读指针 → 定位数据目录 → 读 settings.json（解决循环依赖） |
| 7 | 运行中更换数据目录 | **自动迁移** `auths/` 与 `history.jsonl`，成功后更新指针并弹窗展示进度；失败保持原目录不变 |
| 8 | 保活失败标记 | **区分错误类型**：网络/超时 → "刷新失败"可重试；401/refreshToken 无效 → 才标"需重新登录" |
| 9 | 回调端口全占用 | 18080-18089 全占 → **报错中止**登录 |

---

## 9. 遗留问题登记（2026-09-15 审查，尚未修）

2026-09-15 全面审查发现 20+ 项问题。已修的记录进上文第 2 节与代码；**本节只登记仍未修的**，按严重度排序。位置写「文件 · 符号」而非行号，避免行号漂移。

排障视角的说明见 [TROUBLESHOOTING.md](TROUBLESHOOTING.md)。

### High

| 项 | 位置 | 症状 / 风险 | 修复方向 |
|---|---|---|---|
| **拥塞识别仍靠文案匹配** | `upstream.rs` · `CONGESTION_HINTS` / `is_congestion` | 上游若改写「当前参与用户太多，请稍后再试」，响应会退回规则 3 被判为成功 → 假成功。**风险已缩小**：`app.log` 现在逐次落原始响应体，2026-09-16 实测到 `{"code":9074,"message":"当前参与用户太多，请稍后再试"}`（仅此两字段），不再靠猜 | 改为按 code 白名单判定（从 `9074` 起步），文案匹配退为兜底；每拿到一个新 code 就补进第 2 节。**白名单只能自建**：2026-09-16 复核，`Maquer/trae-signin` 末次提交 2026-08-19、父项目 `traework2api` 2026-08-02，均无更新；且 Go 侧 `doJSON` 从不解析响应体，同一条 9074 在它那里打印 `✅ OK`——它本身就是我们刚修掉的那个假成功。 |
| **9074 更像按客户端风控，不是真拥塞** | `upstream.rs` · `checkin_claim_once` 的上游语义 | 同账号同 IP：客户端 08:00–08:01 三连拒，用户手动在原程序/网页 08:02 **一次成功**。若判定条件在请求指纹上，**再调重试节奏也签不上**，用户会一直以为"程序坏了" | 与真机 TRAE IDE 的 claim 请求逐头比对（`X-Device-Id`/版本头/`Origin`/`Referer`/cookie），或换 `device_id` 观察是否解除；先拿 `app.log` 里的时间线做对照实验，再决定是否改请求构造 |
| **改完源码不等于改完程序（部署陷阱）** | 构建流程 | 2026-09-15 15:13 的拥塞修复**从未生效**：应用 15:03 启动后一直开着，Windows 锁住 `trae-signin-gui.exe`，后续构建只刷新了 `deps/*.rlib`，exe 仍是 15:00 那份。用户 09-16 一整天跑的是旧二进制，把一次假成功当成事实 | 每次修完 Rust 侧：先停 `trae-signin-gui` 进程 → `npm run tauri build` → **核对 exe mtime 前移** → 再启动。见 [TROUBLESHOOTING.md](TROUBLESHOOTING.md) |
| **登录回调无 state/nonce 绑定** | `login_service.rs` · `wait_authorize`；`login.rs` 生成了 `login_trace_id` 却从不校验 | 5 分钟窗口内，本机任意进程或用户访问的任意网页（`<img src="http://127.0.0.1:18080/authorize?…">`）都能注入一份攻击者的凭证，被当作你自己的账号保存并签到 | 生成 128 位 `state` 拼进 `auth_callback_url`，回调时严格比对；校验 `Host`；拒绝非 GET |
| **回调请求体单次 read，8192 上限** | `login_service.rs` · `wait_authorize` | `userJwt`+`userInfo` 百分号编码后可超 8 KiB → query 被截断。`parse_callback_query` 里 JSON 解析错误被 `if let Ok(v)` 吞掉 → 静默退回 `now+7d` 的假过期时间，或保存出 `refresh_token` 为空的残缺凭证 | 读到 `\r\n\r\n` 为止（带字节上限与显式"请求过大"错误）；解析失败必须上抛，不许静默兜底 |
| **machine/device id 用全局单槽暂存** | `login_service.rs` · `LAST_IDS_GLOBAL`/`stash_ids`/`take_ids` | 超时/解析失败/换取失败/取消等出口都会残留旧值；两会话重叠时 A 取到 B 的 ids 并写进 A 的凭证，而这些 id 是每次签到请求的头 | 删掉全局，把 ids 作为参数穿进 `run_callback_server`，或存进会话结构体 |
| **`tokenExpireDuration` 误用毫秒归一化** | `upstream.rs` · `exchange_token` | 该字段是**时长**不是时间戳。若上游回 `604800000`（7 天的毫秒），因 `<1e12` 不除 1000 → 算出约 +19 年的有效期 → 永不刷新 → 几天后每次签到 401 | duration 分支单独判位（`> 1e10` 才除 1000），并补毫秒时长测试 |
| **签到轮不过滤今日已签账号** | `commands.rs` · `run_signin_round`；`scheduler_task.rs` 各轮 | 每轮对**全部**账号发 status+credits 请求（周期检查设 1 分钟 = 每天 1440 轮全量查询），与第 2 节判据无关但会持续加大限流概率。违背 4.3「未签则签」 | 用 `today` 缓存跳过 `ok/already/disabled`；需同时决定「单账号重签」是否仍可强制（未拍板，见下） |

### Medium

| 项 | 位置 | 症状 / 风险 |
|---|---|---|
| `probe_port` 先 bind 后 drop、稍后再 bind | `login_service.rs` | TOCTOU：链接已按该端口生成，真 bind 失败时用户已授权完毕。`Err(_) => continue` 还把 `AccessDenied`（Hyper-V 保留端口段）误报成"均被占用"。修法：探测到的 listener 直接传给会话 |
| 登录会话槽检查与注册跨 await | `login_service.rs` · `start_login` | 两次 `start_login` 都能通过 `is_some()` 检查；结束时无条件 `*g = None` 会清掉新会话，导致取消按钮失效 |
| `open_external` 无 scheme 白名单 + CSP 为 null | `commands.rs` · `open_external`；`tauri.conf.json` · `app.security` | 任意字符串直交 `open_url`（Windows 下即 ShellExecute）：`file://`、UNC、`ms-*:` 均可触发。目前前端无调用点、上游昵称与 message 全走 JSX 转义（无 `dangerouslySetInnerHTML`），属**潜在**而非现存漏洞。修法：`url::Url` 解析后仅放行 `https`（可加 `mailto`），host 锁 `trae.cn`/`trae.com.cn`；配 CSP `script-src 'self'` |
| 前端事件桥卸载竞态 | `stores/index.ts` · `useEventBridge` | `unlisteners.push(await p)` 可能在 cleanup 之后才 resolve → StrictMode 下 dev 必现监听器泄漏与重复 toast。修法：`disposed` 标志，已卸载则立刻 unlisten |
| 红点分母用「今日处理过的账号数」 | `state.rs` · `today_summary` | 新导入账号未处理前不进分母 → 明明有未签账号却显示 `已签 2/2`、不亮红点。另：`disabled` 被计入"已签"，与账号卡的 ⛔ 语义冲突 |
| `bark_url` 不校验 | `notify.rs` · `bark_push`；`settings.rs` · `sanitized` | 前端与 `sanitized()` 都不校验 scheme/主机，`test_bark` 可把任意字符串发成 GET（含内网/链路本地地址）。urlencode 已防注入，无命令注入面 |
| 每轮都弹系统通知 | `commands.rs` · `run_signin_round` 末尾 | 周期检查开启时，即使是无进展的一轮也会弹 toast；`periodic_check_minutes` 允许设 1 → 每分钟一条。修法：全 `already` 或 `skipped` 的轮不通知 |
| `find_key` 兄弟子树字典序 | `upstream.rs` · `find_pref` | 本轮已加「顶层 → `data` → 全树」优先序，但**没有顶层也没有 `data` 时**仍是按字典序取第一个嵌套命中（如 `{"profile":{"checked_in":true}}` 会压过 `{"today":{"checked_in":false}}`）。需要向用户拿一份真实 status 响应体来定死路径 |
| 核心 `scheduler` 的 `next_daily_run`/`is_today` 无调用方 | `crates/trae-signin-core/src/scheduler.rs` | 壳层自己用 `now.time() >= daily` + 日期守卫重算了一套；有完整单测的 API 反而是死代码，两处逻辑将来会分叉 |

### Low

| 项 | 位置 | 说明 |
|---|---|---|
| 死依赖与死配置：updater | `src-tauri/Cargo.toml`、`tauri.conf.json` · `plugins.updater.active` | `tauri-plugin-updater` 从未 `register_plugin`；`plugins.updater.active` 是 Tauri **v1** 键，v2 直接忽略。`check_update` 是硬编码 JSON（决策 #17 有意如此）。建议删依赖与配置节点，避免下一个 reviewer 误判"接线断了" |
| `check_update` 文案像漏配 | `commands.rs` · `check_update`；`SettingsPage.tsx` | 「更新源未配置」读起来像用户没设好。建议改成「当前 v{version} 已是最新（更新通道暂未开放）」并显示版本号。`doCheckUpdate` 还缺 catch |
| 进度阶段是死代码 | `commands.rs` · `SigninProgress.stage` | 只 emit `checking`/`done`；前端 `STAGE_TEXT` 的 waiting/refreshing/claiming/querying 永不出现，PLAN 4.2 承诺的四段流式进度未落地 |
| capabilities 冗余授权 | `src-tauri/capabilities/default.json` | `opener/notification/dialog/autostart:default` 与四个 `core:window:*` 全部未使用——所有插件调用都在 Rust 侧，不经过 capabilities；前端只 import 了 `@tauri-apps/api/core` 与 `/event`。纯属多余攻击面 |
| 前端死导出 | `src/lib/tauri.ts` · `onSigninDone`/`onTrayStatus` | 订阅了 PLAN 5 列出的事件但无组件使用；`openExternal` 同样零调用 |
| DST 边界 | `state.rs` · `restore_today_from_history` | `and_hms_opt(0,0,0).unwrap()` 本身安全，但 `.single()` 在本地午夜不存在的时区返回 None → `today_start = 0` → 把**整个**历史文件当成"今日"回放 |
| 事件名超出 PLAN 5 | `login://progress`、`accounts://changed` | 实现在用、计划未列（PLAN 只写了 `signin://*`、`login://done|failed`、`tray://status`）。属良性超集，但 PLAN 应补记以免契约表失准 |

### 待你拍板

1. **签到轮是否跳过「今日已签」账号？**（High 表末行）跳过的代价：账号卡上的「重签」按钮语义要决定——保持强制重签（跳过只对定时轮生效）还是一并跳过。
2. ~~**拥塞重试的额度与节奏**~~ **已拍板并落地（2026-09-16）**：单轮降到 **2 次、基础退避 20s**（原 3 次 / 2s→4s）。日级兜底改由两处承担：定点轮的 `5/15/30/60` 分钟阶梯（见 4.3）+ 周期检查。理由见第 2 节「按客户端限流」实测。
3. 上述 High 项里，**登录回调的 state 绑定**与**凭证文件截断读取**是安全/数据完整性问题，但只在登录那一刻暴露（你已有可用凭证）——优先级是否排在「跳过已签账号」与「文案匹配换 code 白名单」之后？
