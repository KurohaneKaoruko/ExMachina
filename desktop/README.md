# EXMACHINA 桌面端（Tauri v2）

桌面端为**对话优先**的交付形态：主窗口是壳内嵌的对话式界面（`ui/`，经 frontendDist 打进二进制，
无独立构建步骤）——打开即对话：会话列表、流式回答、思维链/工具轨迹回看、审批内联裁决。
捆绑的 `exm-gateway` 随应用启动充当后端（REST / WS / WebUI 同端口）；
同时支持 `--remote` 直连远程网关（服务器上的后台服务）。

> WebUI 管理控制台（个体编成 / 模型与提供商 / 通道 / 定时任务）收为**次级入口**：
> 托盘菜单「打开控制台（浏览器）」或对话页侧栏「⚙ 打开控制台」在系统浏览器中打开。
> 移动端不需要单独 App：手机浏览器 / PWA 直接打开网关地址即为同源客户端。

## 启动形态

| 形态 | 命令 | 行为 |
|------|------|------|
| 本机模式 | `exmachina-desktop` | 拉起捆绑网关（空闲端口 + `%LOCALAPPDATA%/ExMachina` 数据目录），主窗口进入对话界面；管理能力走原生设置 |
| 远程模式 | `exmachina-desktop --remote http://host:4173` | 主窗口对话界面直连远程网关（服务器上的后台服务） |
| 带密钥 | `--key <authKey>` | 密钥经初始化脚本注入 `window.__EXM_GATEWAY__`，免登录门 |

## 构建步骤

前置：Rust stable、Node ≥ 22、平台 WebView 运行时（Windows: WebView2 预装于 Win10/11）。

```bash
# 1. 构建网关（release）
cargo build -p exm-gateway --release

# 2. 放置捆绑资源（网关二进制 + WebUI 构建产物）
npm run build:webui                       # 仓库根执行，产出 webui/dist
mkdir -p desktop/src-tauri/binaries
cp target/release/exm-gateway.exe desktop/src-tauri/binaries/     # Windows
# cp target/release/exm-gateway desktop/src-tauri/binaries/       # Linux / macOS
cp -r webui/dist desktop/src-tauri/binaries/webui-dist            # 缺失时窗口无法访问（网关无 UI 可托管）

# 3. 安装依赖并构建安装包
cd desktop
npm install
npm run tauri build
```

产物：`desktop/src-tauri/target/release/bundle/` 下的 NSIS 安装包（Windows）/ AppImage（Linux）/ DMG（macOS）。

对话 UI 调试：浏览器直接打开 `desktop/ui/index.html?gw=http://127.0.0.1:<网关端口>`（`?gw=` 覆盖网关地址）。

## 设计说明

- **壳只做两件事**：拉起/连接网关 + 提供窗口。全部业务仍在网关进程（单进程交付原则不变）。
- **生命周期**：应用退出时终止网关子进程；网关崩溃时壳报错退出（不做静默重启，避免脏状态）。
- **数据隔离**：本机模式网关使用用户数据目录（`EXM_DATA_DIR`），与仓库工作区解耦；
  组工作区默认即该数据目录，可在 WebUI「智能体组」页改为任意项目路径。
- **远程模式零捆绑**：`--remote` 时不启动本地进程，等于一个连服务器的瘦客户端。
