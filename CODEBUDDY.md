# CODEBUDDY.md

This file provides guidance to CodeBuddy Code when working with code in this repository.

## 项目概述

trae-signin-gui（TRAE 签到）：把 [Maquer/trae-signin](https://github.com/Maquer/trae-signin)（Go CLI：登录→签到→积分→Bark）实现为 **Windows 专用** Tauri 2 桌面应用。技术栈：Tauri 2 + React 19 + Vite 7 + Tailwind 4 + Radix UI + zustand，业务逻辑全部用 Rust 重写（不保留 Go、不用 sidecar）。交付物是**单 exe 便携文件**（不做安装包）。

**PLAN.md 是需求与拍板决策的权威**（19 条已拍板决策 + 第 8 节复核结论 + §9 遗留问题登记）。变更"单 exe""仅 Windows""auths 格式兼容""Rust 重写"等地基决策前必须先与用户确认。用户视角排障手册在 TROUBLESHOOTING.md；与代码/PLAN 冲突时以它们为准。

## 常用命令

```bash
npm install                                    # 前端依赖
npm run tauri dev                              # Tauri 开发模式（vite :5173 热更新）
npm run build                                  # 前端构建（tsc -b && vite build → dist/）

cargo test -p trae-signin-core                 # core 单元测试（54 项，含 httpmock）
cargo test -p trae-signin-core <test_name>     # 运行单个测试（如 signin_full_flow_mock）
cargo test --workspace                         # 全部测试（core 54 + 壳 7）
cargo clippy --workspace --all-targets         # lint（当前 0 警告，保持住）

# release 构建（必须带 --features custom-protocol，见下）
cargo build --release -p trae-signin-gui --features custom-protocol
# 产出 target/release/trae-signin-gui.exe

node scripts/gen-icons.mjs                     # 重新生成图标（纯 Node 无依赖）
```

Cargo workspace 根在仓库根目录，构建产物在 `target/`（**不是** `src-tauri/target/`）。

### 关键坑：release 构建必须带 `--features custom-protocol`

该 feature 把 `dist/` 前端资产嵌入 exe。手动 `cargo build --release` 不带它时，运行后 WebView 会去请求 devUrl（localhost:5173），出现"localhost 拒绝连接 ERR_CONNECTION_REFUSED"错误页。`npm run tauri build` 会自动加，手动 cargo 不会。

### 关键坑：应用开着时构建 = 白构建

Windows 锁住正在运行的 `trae-signin-gui.exe`，构建**只会刷新 `deps\*.rlib`，exe 保持旧内容**，链接步骤根本没产出新二进制。2026-09-15 15:13 的签到判据修复就是这样静默失效了一整天。

改完 Rust 侧要真正部署，顺序固定：

1. 停进程：`Stop-Process -Name trae-signin-gui -Force`
2. `npm run tauri build`
3. **核对 exe 的 `LastWriteTime` 确实前移**（没前移就是没链上）
4. 再启动

判据兜底：2026-09-16 起 exe 会往 `{数据目录}\app.log` 写日志；文件不存在或没有新行，说明跑的还是旧二进制。

### 关键坑：验证签到判据必须挑「今日未签」的时刻

只有 `checkedIn == false` 才会发 claim。当天已签后再点"全部签到"只走 ALREADY 分支，**完全绕开 claim 侧被改的逻辑，什么也证明不了**；同理，上游对已签状态的 claim 回幂等 `{"code":0,"message":"success"}`——用它验证 9074 修复是无效证据。要验证请改到次日定点时刻前，或用 httpmock 单测。

## 架构（三层）

```
src/（React 前端）  ←invoke/事件→  src-tauri/（Tauri 壳）  ←调用→  crates/trae-signin-core/（纯逻辑）
```

### 1. crates/trae-signin-core — 纯逻辑 crate（无 Tauri 依赖，全部单测在此）

- `upstream.rs`：**上游 API 唯一事实来源**。所有常量（双 host、ClientID、IDE 版本/UA、5 个端点路径）集中在文件顶部，上游变更只改这里。`Upstream::with_bases()` 可注入 base URL（httpmock 测试必需，否则测试会打到真实外网）。JSON 解析用 `find_key()` 宽容匹配——归一化比较（忽略大小写与 `_`/`-`，snake_case/camelCase 通吃），只取所需字段、不校验未知字段。
- `auth.rs`：凭证 `auths/trae-{uid}.json` 解析（兼容嵌套与扁平两种历史形态，对齐上游 Go `auth.Parse`）+ 原子写（tmp+rename）。过期时间秒/毫秒兼容（`>1e12` 视为毫秒）。
- `login.rs`：登录链接生成（query 参数逐项固定）+ 回调解析（refreshToken / userInfo / userJwt）、自带 urlencode/urldecode。`gen_device_id()` 生成 **16 位纯数字**设备号（官方 Aha 设备号格式）。
- `scheduler.rs`：纯时间计算（每日定点含跨天、周期间隔）。调度循环本体在壳里。
- `history.rs`：JSONL 追加/读取（按 ts 降序取最近 N 条，坏行跳过）。
- `settings.rs`：settings.json 读写，`sanitized()` 归一非法值。
- 错误模型：`CoreError::AuthExpired`（401/refreshToken 失效→标"需重新登录"）与 `Retryable`（网络/超时→标"刷新失败可重试"）**必须区分**（复核拍板 #8）。`SigninResult` 携带 `need_relogin`/`refresh_failed` 标志，禁止用错误消息字符串匹配来判定。

### 2. src-tauri/ — Tauri 壳

- `lib.rs`：插件链。**`tauri_plugin_single_instance` 必须第一个注册**（官方约定，否则拦截不可靠）；其回调唤起已有窗口，但 args 含 `--hidden`（开机自启二次触发）时不弹窗。
- `state.rs`：`AppState`（settings 缓存 / today 签到状态缓存 / signin 互斥锁 / 登录会话）。**数据目录引导指针**：`data-dir.txt` 存 exe 同级，启动先读它定位数据目录，再读其中的 settings.json（解决循环依赖）。
- `commands.rs`：全部 `#[tauri::command]` + 签到轮核心 `run_signin_round`（串行、逐账号 emit `signin://progress`）。互斥语义：`manual=true` 拿不到锁报"签到进行中"，`manual=false`（定时触发）静默跳过。**凭证写盘只发生在持 `signin_lock` 的轮内**——所以 `delete_account`、`change_data_dir` 也必须 `try_lock` 同一把锁（否则已删凭证会被进行中的轮回写复活、迁移会丢掉轮内写入的更新，2026-09-17 修复）。
- `scheduler_task.rs`：30 秒 tick 循环重读设置（启动补签 15s 后一次 / 每日定点 / 每日保活 / 周期检查）。**std MutexGuard 严禁跨 await**——guard 限制在独立块作用域内取值，await 在块外。
- `login_service.rs`：端口探测 18080-18089（全占报错中止）→ 生成链接 → opener 开浏览器 → `TcpListener` 手写极简 HTTP 捕获 `/authorize`（一次成功即回 HTML 并结束）→ ExchangeToken → GetUserInfo → 落盘。5 分钟超时，取消 = abort task。machine/device id 经 `stash_ids/take_ids` 全局暂存保证一致。**start_login 的「检查已有会话 → 开浏览器 → spawn → 注册句柄」必须在同一把 `state.login` 锁内完成**（`async_runtime::spawn` 是同步调用，锁内无 await 点）——拆开的话双击"添加账号"就能绕过单会话守卫（2026-09-17 修复）。
- `tray.rs`：托盘**只在代码里创建一次**（`TrayIconBuilder`）。**禁止在 tauri.conf.json 加 `app.trayIcon`**——会额外创建一个无菜单、点击无效的重复图标。两态图标用 `include_bytes!` 编译期嵌入（打包后无相对路径问题），`Image::from_bytes` 需要 tauri feature `"image-png"`。
- 窗口关闭：`close_to_tray=true` 拦截 CloseRequested 并隐藏；否则退出。

### 3. src/ — React 前端

- `lib/tauri.ts`：唯一前后端契约点——invoke 封装 + TS 类型（与 Rust serde 结构逐字段对齐，Rust 侧用 `#[serde(rename = "camelCase")]`）+ 事件订阅（`listen` 返回 `Promise<UnlistenFn>`，清理时需 await）。
- `stores/index.ts`：zustand（accounts/settings/ui）+ `useEventBridge()`（App 挂载时统一注册事件，toast 也在这里发）。
- 页面：AccountPage（卡片+批量进度）、HistoryPage（200 条+账号筛选）、SettingsPage；`components/ui.tsx` 为 Radix 封装的基础组件。
- tsconfig 开了 `verbatimModuleSyntax`：**类型导入必须用 `import type`**。

## 数据流与数据目录

- 启动：读 `data-dir.txt` → 无则前端显示首次引导卡（选目录，默认预填 exe 同级）→ `init_data_dir` 写指针+settings.json。运行中换目录走 `change_data_dir`（自动迁移 `auths/` 与 `history.jsonl`，成功后更新指针）。`init_data_dir` 遇目标目录已有 settings.json 时**读入保留、只补 data_dir**（重装/换机器后指针丢失、引导时选回原数据目录的场景，不能拿内存默认值覆盖用户配置）。
- 数据目录内容：`auths/trae-{uid}.json`（凭证，与上游 CLI / GitHub Actions secrets 互通）、`settings.json`、`history.jsonl`、`app.log`（Info 级运行日志，超 2 MiB 轮转为 `app.log.1`；上游原始响应体**只**落在这里）。
- 事件命名：`signin://progress`、`signin://done`、`login://progress|done|failed`、`tray://status`、`accounts://changed`、`settings://changed`。
- 凭证内的 machineId 每账号独立生成（32 位 hex）随文件保存；deviceId 为 **16 位纯数字**（官方 Aha 设备号格式），旧凭证的 32 位 hex deviceId 会在签到时自动迁移回写。

## 9074「网络拥挤/参与用户太多」风控（2026-09-16 修复）

上游 2026-09-04 起对 claim 收紧活动校验，伪造文案为 9074「当前参与用户太多」；status 只读不受校验。**不是真限流，不要往重试/退避方向修**。两个判据（均已实测/社区反编译实证）：

1. **请求体谱系**：`req_source` 是客户端产品谱系标识（TRAE 谱系配 `ono9krqynydwx5` 发 1，SOLO 谱系配 `en1oxy7wnw8j9n` 发 2，从不交叉）。本项目 OAuth 走 `en1oxy7wnw8j9n`，status/claim body 必须是 `{"req_source":2}`；空 body `{}`（对齐 Go 客户端的旧做法）会被拒。
2. **X-Device-Id 形态**：必须 16 位纯数字（Aha 设备号），32 位 hex UUID 一律 9074。

所以**不要**把签到 body 改回 `{}` 来"对齐 Go 源码"——Go 仓库无人维护、CI 失败也没人看，它的空 body 恰是 9074 诱因之一。

修复落点：常量 `CHECKIN_REQ_SOURCE`、deviceId 迁移 `ensure_device_id_format`、生成器 `gen_device_id()`（均在 core）；`signin_account` 每轮先迁移再签到，写盘失败不阻塞（本轮用内存值，下轮再回写）。**token 刷新（`ensure_fresh_token`）写盘失败同样不阻塞**——同一原则，写盘失败只 `log::warn`，内存中的新 token 本轮继续生效（2026-09-17 修复：原先 `?` 中断会让盘满/只读时本可成功的签到全部失败）。

## 约定

- 前端组件测试不写（拍板 #18）；Rust 改动必须带/更新单测，core 用内嵌 `#[cfg(test)]`。
- 与签到轮并发敏感的操作（删除账号、换数据目录）必须先 `try_lock` `signin_lock`；凭证写盘只发生在持锁轮内，这是防止"删了又复活/迁移丢更新"的唯一保证。
- 上游 API 无文档，端点/常量只出现在 `upstream.rs` 一处；响应解析保持宽容。
- `.gitignore` 已排除 `data/`、`auths/`、`data-dir.txt`、`target/`、`dist/`、`node_modules/`——凭证永不入库。
- 图标有改动：改 `scripts/gen-icons.mjs` 后重新运行生成，不要手工编辑 `src-tauri/icons/`。
