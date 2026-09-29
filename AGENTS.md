# AGENTS.md

trae-signin-gui（TRAE 签到）：把 Go CLI [Maquer/trae-signin](https://github.com/Maquer/trae-signin) 用 Rust 重写为 **Windows 专用** Tauri 2 桌面应用。功能只有五项：登录 → 签到 → 查积分 → Bark 通知 → 定时签到。**没有** OpenAI 反代/对话功能，交付物是**单 exe 便携文件**。
栈：Tauri 2 + React 19 + Vite 7 + Tailwind 4 + Radix + zustand。

## 文档分工（改东西前先看这几处）

| 文件 | 作用 |
|---|---|
| `PLAN.md` | 需求与**已拍板决策**（§1，19 条）、上游 API 事实（§2，含「设备号 / 账号-设备绑定」）、**§9 遗留问题登记**（2026-09-29 新增 5 项，3 项当日已修）、**§10 遗留修复拍板**（2026-09-23，11 条） |
| `TROUBLESHOOTING.md` | 用户视角排障、签到判据流程图、如何证明一次修复真的生效 |
| `CONTEXT.md` | 领域词汇表（签到/重签/跳过/静默轮/假成功/拥塞响应/设备绑定/真号/生成号/loginTraceID 等），**术语以它为准** |
| `README.md` | 面向使用者的功能说明与快速上手。**与代码/PLAN 冲突时以它们为准** |

## 常用命令

```bash
npm install                                 # 前端依赖
npm run tauri dev                           # 开发模式（vite :5173 热更新）
npm run build                               # 前端构建 = tsc -b && vite build（即前端类型检查）
npm run tauri build                         # release 构建（自动带 custom-protocol）

powershell -ExecutionPolicy Bypass -File scripts/build.ps1
                                            # ⭐ 构建推荐入口：停进程 → 构建 → 核对 exe mtime 前移（防白构建，见坑 1）

cargo test --workspace                      # 全部测试（core 63 + 壳 15）
cargo test -p trae-signin-core <test_name>  # 单个测试（如 signin_full_flow_mock）
cargo clippy --workspace --all-targets      # lint，当前 0 警告，保持住

node scripts/gen-icons.mjs                  # 图标有改动时重新生成，勿手工编辑 src-tauri/icons/
```

Cargo workspace 根在**仓库根**，产物在 `target/`（不是 `src-tauri/target/`）。

## 坑（按踩过的代价排序，全部实测得来）

1. **应用开着时构建 = 白构建。** Windows 锁住运行中的 `trae-signin-gui.exe`，构建只刷新 `deps\*.rlib`，**exe 内容不变**，修复静默不生效（2026-09-15 的签到判据修复就这样失效了一整天）。**已脚本化**（2026-09-23，PLAN §10 #3）：`powershell -ExecutionPolicy Bypass -File scripts/build.ps1` = 停进程 → 构建 → 核对 exe `LastWriteTime` 前移（未前移报错中止）。注意脚本文件必须存为 **UTF-8 with BOM**（PS 5.1 无 BOM 按 GBK 误读中文会解析失败）。
2. **手动 `cargo build --release` 必须带 `--features custom-protocol`**，否则 exe 会去请求 devUrl（localhost:5173）→ 白屏。`npm run tauri build` 自带。
2b. **改图标必须触发 build script 重跑**：`tauri_build` 在 build.rs 阶段把 `icons/icon.ico` 嵌入 exe 资源，`icons/` 变化默认**不会**让它重跑（2026-09-29 踩坑：图标换了、Rust 也重编译了，exe 里还是旧图标）。已在 `src-tauri/build.rs` 加 `cargo:rerun-if-changed=icons` 根治；验证方法用 `gen-icons.mjs` 的 PE 提取思路（临时脚本）比对 exe 内嵌 256px 条目。另外 Windows 资源管理器对 exe 文件图标有缓存，验证时以提取结果为准。
3. **验证签到判据必须挑「今日未签」的时刻。** 只有 `checkedIn == false` 才发 claim；定时轮与「全部签到」会**跳过**今日已签账号（连 status 都不发，2026-09-23 起），当天再点「全部签到」什么也证明不了。**单账号「重签」是强制路径**（忽略今日状态），任何时候都能验证 claim 判据。
4. **`{数据目录}\app.log` 是唯一能看到上游 HTTP 200 原始响应体的地方**（`claim` 与 `checkin status` 的响应体都逐次整包落日志——status 真实字段路径已现形：顶层 snake_case `checked_in`/`enable`/`did_checked_in`，`find_key` 忽略 `_` 恰好能命中，暂不硬编码）。改任何上游判据前先读它，不要靠猜。
5. Go 参考实现的 `doJSON` 只看状态码、从不解析响应体——**它本身就有假成功 bug**（同一条 `code=9074` 会打印 `✅ OK`）。不要用"和 Go 一致"当作改判据的理由。
6. **9074「当前参与用户太多」是风控不是拥塞**（2026-09-16 定位并修复）：签到 body 必须 `{"req_source":2}`（SOLO 谱系契约），`X-Device-Id` 必须纯数字 Aha 设备号（合法形态 12~20 位，生成器仍产 16 位）。不要改回空 body 或 32 位 hex deviceId。**且 2026-09-29 起语义升级：上游收紧了设备校验，随机生成的号过不了「新账号」首次绑定（status 可读、claim 必 9074），只有客户端注册过的真号能过**——解法是「设置设备号」/登录时自动探测本机客户端真号，见 PLAN §2「设备号 / 账号-设备绑定」。**拥塞判定 = code 白名单（`CONGESTION_CODES`，9074 起步）∪ 文案兜底**（2026-09-23 拍板 #5），拿到新拥塞 code 先补 `upstream.rs` 白名单再同步 PLAN §2。
7. **上游授权页不接受带 query 的回调地址**（2026-09-29 实测：`auth_callback_url` 拼 `?state=` 会让授权页直接报「登录失败/网络错误」）。回调地址必须保持裸 `http://127.0.0.1:{port}/authorize`；防注入比对走回调 query 里上游**原样带回**的 `loginTraceID`（强度不变）。同理 **9095「当前设备今日已经签到」是设备维度终局拒绝**（一号一天只签一个账号），必须记终局 `Failed`，不得进拥塞白名单、不得记成功（`DEVICE_ALREADY_SIGNED_CODE`）。

## 架构（三层）

```
src/（React 前端） ←invoke/事件→ src-tauri/（Tauri 壳） ←调用→ crates/trae-signin-core/（纯逻辑）
```

- **core**：全部单测在此。`upstream.rs` 是上游 API 唯一事实来源（双 host / ClientID / IDE 版本 / UA / 5 个端点全在文件顶部）；`Upstream::with_bases()` 注入 base URL 供 httpmock 用——**测试绝不打真实外网**。`login.rs` 含设备号三件套：`gen_device_id`（登录时随机生成）、`is_valid_device_id`（纯数字 12~20 位）、`detect_client_device_ids`（读本机客户端 `storage.json` 键名 `iCubeAuthInfo://icube-dc:{号}` 提取真号，**值可能含 token，绝不落日志**）。
- **壳**：`commands.rs`（`run_signin_round` 串行签到轮：`skip_signed=true` 跳过今日已签/禁用、`false` 强制重走（单账号重签）；互斥语义：manual 抢不到锁报错、定时抢不到静默顺延；通知门 = 有真实 claim 或有失败，全跳过轮零通知；`set_device_id` 手动改设备号、`detect_client_device_ids` 探测本机真号）、`scheduler_task.rs`（30s tick：启动补签 / 每日定点 + 限流重试阶梯 / 保活 / 周期检查；定点计算统一走 core 的 `next_daily_run`）、`state.rs`（`data-dir.txt` 引导指针破解决策循环依赖；`today_summary` 分母剔除 disabled）、`login_service.rs`（18080-18089 回调 HTTP：listener 随会话传递、**loginTraceID**/Host/GET 三重校验（上游回调原样带回发起时的 `login_trace_id`，回调地址本身不带 query，见坑 7）、循环读至 `\r\n\r\n` 上限 64 KiB；保存凭证前自动探测本机客户端真号——唯一且未被占用则替换生成号）、`logger.rs`、`tray.rs`。刷新轮（`run_refresh_round_inner`）只查 status+credits 不 claim，"未签且可签"记 `NotSigned` 并发 done 进度事件覆盖上一轮残留文案。
- **前端**：`src/lib/tauri.ts` 是**唯一前后端契约点**（invoke 封装 + 与 Rust serde 逐字段对齐的类型 + 事件订阅）。

## 代码约定

- 上游常量/端点**只出现在 `upstream.rs` 一处**；JSON 解析保持宽容（`find_key` 忽略大小写与 `_`/`-`，只取所需字段）。
- `CoreError::AuthExpired`（需重登）与 `Retryable`（可重试）**必须区分**，禁止用错误消息字符串判定。唯一例外是拥塞判定：**code 白名单（`CONGESTION_CODES`，9074 起步）为主、文案匹配兜底**（2026-09-23 拍板，见 PLAN §10 #5）。
- 一轮签到的终局状态必须如实：HTTP 2xx + 拥塞响应（**code 白名单或文案，任一命中**）→ `Retryable`，既不得记成 `ok`（骗用户），也不得记成不可重试失败（用户只能手动补签）；**`code=9095`（设备今日已签）→ 终局 `Failed`，先于拥塞与 4xx 分支拦截，200/4xx 两路径都拦**（`DEVICE_ALREADY_SIGNED_CODE`）。
- Rust 侧新增/改 `SigninSummary`、`SigninProgress`、`AccountView` 字段 → 同步 `src/lib/tauri.ts` 接口；**`CheckinStatus` 加变体 → 同步 `as_str`/`parse_status` + 前端 `CheckinStatus` 类型与 `STATUS_META`**（2026-09-29 加过 `NotSigned`，前端徽章"未签"）。
- **`std::sync::MutexGuard` 严禁跨 `await`**；guard 限制在独立块作用域内取值。
- **凭证写盘只发生在持 `signin_lock` 的轮内**：`delete_account`、`change_data_dir`、`set_device_id` 必须 `try_lock` 同一把锁（否则已删/改凭证被轮回写复活、迁移丢轮内更新）；`start_login` 的「检查 → 开浏览器 → spawn → 注册」必须在 `state.login` 一把锁内（spawn 是同步调用，锁内无 await）。均 2026-09-17 修复，`set_device_id` 2026-09-29 补。
- `ensure_fresh_token` / `ensure_device_id_format` **写盘失败不阻塞签到**：`log::warn` 后用内存值继续本轮，下轮再回写。
- 前端：`verbatimModuleSyntax` 开启 → 类型导入必须 `import type`；**不写组件测试**（拍板 #18）；样式走 `components/ui.tsx` 的 Radix 封装 + tailwind `var(--*)` 变量。
- `tauri_plugin_single_instance` 必须第一个注册；**禁止**在 `tauri.conf.json` 加 `app.trayIcon`（会多出一个无菜单、点击无效的重复托盘图标）。

## 数据与红线

- 数据目录 = exe 同级 `data-dir.txt` 指向的目录，内含 `auths/`、`settings.json`、`history.jsonl`、`app.log`。换目录走 `change_data_dir` 自动迁移。
- `auths/trae-{uid}.json` 是**真实凭证**（与上游 CLI / GitHub Actions secrets 互通），`app.log` 可能含 token 类字段——永不入库，外发前先打码。
- 读用户 TRAE 客户端的 `storage.json`（设备号自动检测）**只允许提取键名 `iCubeAuthInfo://icube-dc:` 后的数字**；该文件的**值**含 token，绝不读取、记录或落日志。
- 地基决策（单 exe、仅 Windows、Rust 重写不用 sidecar、auths 格式兼容、更新器只做占位）变更前**必须先与用户确认**。
