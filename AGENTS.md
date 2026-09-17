# AGENTS.md

trae-signin-gui（TRAE 签到）：把 Go CLI [Maquer/trae-signin](https://github.com/Maquer/trae-signin) 用 Rust 重写为 **Windows 专用** Tauri 2 桌面应用。功能只有五项：登录 → 签到 → 查积分 → Bark 通知 → 定时签到。**没有** OpenAI 反代/对话功能，交付物是**单 exe 便携文件**。
栈：Tauri 2 + React 19 + Vite 7 + Tailwind 4 + Radix + zustand。

## 文档分工（改东西前先看这三处）

| 文件 | 作用 |
|---|---|
| `PLAN.md` | 需求与**已拍板决策**（§1，19 条）、上游 API 事实（§2）、**§9 遗留问题登记** |
| `TROUBLESHOOTING.md` | 用户视角排障、签到判据流程图、如何证明一次修复真的生效 |
| `CODEBUDDY.md` | 本文件的长文版（同一批事实的展开说明）。**与代码/PLAN 冲突时以它们为准** |

## 常用命令

```bash
npm install                                 # 前端依赖
npm run tauri dev                           # 开发模式（vite :5173 热更新）
npm run build                               # 前端构建 = tsc -b && vite build（即前端类型检查）
npm run tauri build                         # release 构建（推荐入口，自动带 custom-protocol）

cargo test --workspace                      # 全部测试（core 54 + 壳 7）
cargo test -p trae-signin-core <test_name>  # 单个测试（如 signin_full_flow_mock）
cargo clippy --workspace --all-targets      # lint，当前 0 警告，保持住

node scripts/gen-icons.mjs                  # 图标有改动时重新生成，勿手工编辑 src-tauri/icons/
```

Cargo workspace 根在**仓库根**，产物在 `target/`（不是 `src-tauri/target/`）。

## 坑（按踩过的代价排序，全部实测得来）

1. **应用开着时构建 = 白构建。** Windows 锁住运行中的 `trae-signin-gui.exe`，构建只刷新 `deps\*.rlib`，**exe 内容不变**，修复静默不生效（2026-09-15 的签到判据修复就这样失效了一整天）。固定顺序：
   `Stop-Process -Name trae-signin-gui -Force` → `npm run tauri build` → **核对 exe 的 `LastWriteTime` 确实前移** → 再启动。
2. **手动 `cargo build --release` 必须带 `--features custom-protocol`**，否则 exe 会去请求 devUrl（localhost:5173）→ 白屏。`npm run tauri build` 自带。
3. **验证签到判据必须挑「今日未签」的时刻。** 只有 `checkedIn == false` 才发 claim；当天再点「全部签到」只走 ALREADY 分支，**完全绕开被改的逻辑，什么也证明不了**。
4. **`{数据目录}\app.log` 是唯一能看到上游 HTTP 200 原始响应体的地方**（`claim HTTP 200 OK 响应体: {…}`）。改任何上游判据前先读它，不要靠猜。
5. Go 参考实现的 `doJSON` 只看状态码、从不解析响应体——**它本身就有假成功 bug**（同一条 `code=9074` 会打印 `✅ OK`）。不要用"和 Go 一致"当作改判据的理由。
6. **9074「当前参与用户太多」是风控不是拥塞**（2026-09-16 定位并修复，详见 CODEBUDDY.md 同名章节）：签到 body 必须 `{"req_source":2}`（SOLO 谱系契约），`X-Device-Id` 必须 16 位纯数字（Aha 设备号）。不要改回空 body 或 32 位 hex deviceId。

## 架构（三层）

```
src/（React 前端） ←invoke/事件→ src-tauri/（Tauri 壳） ←调用→ crates/trae-signin-core/（纯逻辑）
```

- **core**：全部单测在此。`upstream.rs` 是上游 API 唯一事实来源（双 host / ClientID / IDE 版本 / UA / 5 个端点全在文件顶部）；`Upstream::with_bases()` 注入 base URL 供 httpmock 用——**测试绝不打真实外网**。
- **壳**：`commands.rs`（`run_signin_round` 串行签到轮 + 互斥语义：manual 抢不到锁报错、定时抢不到静默顺延）、`scheduler_task.rs`（30s tick：启动补签 / 每日定点 + 限流重试阶梯 / 保活 / 周期检查）、`state.rs`（`data-dir.txt` 引导指针破解决策循环依赖）、`login_service.rs`（18080-18089 手写回调 HTTP）、`logger.rs`、`tray.rs`。
- **前端**：`src/lib/tauri.ts` 是**唯一前后端契约点**（invoke 封装 + 与 Rust serde 逐字段对齐的类型 + 事件订阅）。

## 代码约定

- 上游常量/端点**只出现在 `upstream.rs` 一处**；JSON 解析保持宽容（`find_key` 忽略大小写与 `_`/`-`，只取所需字段）。
- `CoreError::AuthExpired`（需重登）与 `Retryable`（可重试）**必须区分**，禁止用错误消息字符串判定。唯一例外是拥塞文案匹配（`CONGESTION_HINTS`），那是权宜，见 PLAN §9。
- 一轮签到的终局状态必须如实：HTTP 2xx + 拥塞文案 → `Retryable`（**与 `code` 取值无关**），既不得记成 `ok`（骗用户），也不得记成不可重试失败（用户只能手动补签）。
- Rust 侧新增/改 `SigninSummary`、`SigninProgress`、`AccountView` 字段 → 同步 `src/lib/tauri.ts` 接口。
- **`std::sync::MutexGuard` 严禁跨 `await`**；guard 限制在独立块作用域内取值。
- **凭证写盘只发生在持 `signin_lock` 的轮内**：`delete_account`、`change_data_dir` 必须 `try_lock` 同一把锁（否则已删凭证被轮回写复活、迁移丢轮内更新）；`start_login` 的「检查 → 开浏览器 → spawn → 注册」必须在 `state.login` 一把锁内（spawn 是同步调用，锁内无 await）。均 2026-09-17 修复。
- `ensure_fresh_token` / `ensure_device_id_format` **写盘失败不阻塞签到**：`log::warn` 后用内存值继续本轮，下轮再回写。
- 前端：`verbatimModuleSyntax` 开启 → 类型导入必须 `import type`；**不写组件测试**（拍板 #18）；样式走 `components/ui.tsx` 的 Radix 封装 + tailwind `var(--*)` 变量。
- `tauri_plugin_single_instance` 必须第一个注册；**禁止**在 `tauri.conf.json` 加 `app.trayIcon`（会多出一个无菜单、点击无效的重复托盘图标）。

## 数据与红线

- 数据目录 = exe 同级 `data-dir.txt` 指向的目录，内含 `auths/`、`settings.json`、`history.jsonl`、`app.log`。换目录走 `change_data_dir` 自动迁移。
- `auths/trae-{uid}.json` 是**真实凭证**（与上游 CLI / GitHub Actions secrets 互通），`app.log` 可能含 token 类字段——永不入库，外发前先打码。
- 地基决策（单 exe、仅 Windows、Rust 重写不用 sidecar、auths 格式兼容、更新器只做占位）变更前**必须先与用户确认**。
