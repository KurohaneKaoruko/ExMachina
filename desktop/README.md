# EXMACHINA 桌面端（Tauri v2）

桌面端为**自带本地服务器**的单端口交付形态：捆绑的 `exm-gateway` 随应用启动，
REST API / WebSocket / WebUI 共用一个端口；同时支持 `--remote` 直连远程网关（服务器上的后台服务）。

> WebUI 由网关同端口托管，客户端「打开即连接」，无需在页面里配置服务器地址。
> 移动端不需要单独 App：手机浏览器 / PWA 直接打开网关地址（桌面端的局域网地址或远程服务器地址）即为同源客户端。

## 启动形态

| 形态 | 命令 | 行为 |
|------|------|------|
| 本机模式 | `exmachina-desktop` | 拉起捆绑网关（空闲端口 + `%LOCALAPPDATA%/ExMachina` 数据目录），打开窗口加载 `http://127.0.0.1:<port>` |
| 远程模式 | `exmachina-desktop --remote http://host:4173` | 窗口直接加载远程网关（服务器上的后台服务） |
| 带密钥 | `--key <authKey>` | 密钥经初始化脚本注入 localStorage，免登录门 |

## 构建步骤

前置：Rust stable、Node ≥ 22、平台 WebView 运行时（Windows: WebView2 预装于 Win10/11）。

```bash
# 1. 构建网关（release）
cargo build -p exm-gateway --release

# 2. 放置捆绑二进制
mkdir -p desktop/src-tauri/binaries
cp target/release/exm-gateway.exe desktop/src-tauri/binaries/     # Windows
# cp target/release/exm-gateway desktop/src-tauri/binaries/       # Linux / macOS

# 3. 安装依赖并构建安装包（首次运行 tauri icon 生成图标）
cd desktop
npm install
npm run tauri icon path/to/icon-512.png   # 生成 src-tauri/icons/（仓库已附一份占位图标）
npm run tauri build
```

产物：`desktop/src-tauri/target/release/bundle/` 下的 NSIS 安装包（Windows）/ AppImage（Linux）/ DMG（macOS）。

## 设计说明

- **壳只做两件事**：拉起/连接网关 + 提供窗口。全部业务仍在网关进程（单进程交付原则不变）。
- **生命周期**：应用退出时终止网关子进程；网关崩溃时壳报错退出（不做静默重启，避免脏状态）。
- **数据隔离**：本机模式网关使用用户数据目录（`EXM_DATA_DIR`），与仓库工作区解耦；
  组工作区默认即该数据目录，可在 WebUI「智能体组」页改为任意项目路径。
- **远程模式零捆绑**：`--remote` 时不启动本地进程，等于一个连服务器的瘦客户端。
