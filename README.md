# PCL-RS

**用 Rust 重写的 PCL（Plain Craft Launcher）风格 Minecraft 启动器。**

PCL 原有的下载功能被举报后停止服务，社区留下了一个明显的空位：一个能正常、快速下载并启动 Minecraft 的启动器。
PCL-RS 是针对这件事的第三方独立实现 —— **下载链路重新设计**：元数据与文件全部来自官方源与授权 CDN 镜像（BMCLAPI），Mod 直接对接 Modrinth 与 CurseForge 官方 API，不内置任何第三方下载站。

> 本项目是第三方基于 PCL 独立进行的二次创作，**并非** PCL 官方发布。
> 依据 PCL 的《PCL 存储库合理使用指南》，署名与致谢见「[关于与署名](#关于与署名)」。

---

## 已实现

| 模块 | 状态 | 说明 |
| --- | --- | --- |
| 版本清单 | ✅ | `version_manifest_v2.json`，多源回退 + 自适应测速 |
| 版本安装 | ✅ | 客户端 jar、libraries、natives 解压、资源索引与全部 assets、log4j 配置 |
| 完整性校验 | ✅ | 逐文件 SHA-1，缺失/损坏可自动补全 |
| 启动游戏 | ✅ | 现代 `arguments`（1.13+）与旧版 `minecraftArguments`（1.6–1.12）双格式 |
| Java 探测 | ✅ | JAVA_HOME / PATH / 注册表 / 常见目录，按版本匹配最合适的 JDK |
| 离线账户 | ✅ | UUID 与官方 `OfflinePlayer` 算法完全一致 |
| 微软账户 | ✅ | **浏览器登录**（OAuth 授权码 + PKCE + 本地回环回调）为主，设备码为备选；之后走 Xbox Live → XSTS → Minecraft Services |
| Mod 搜索/下载 | ✅ | Modrinth 与 CurseForge **官方 API** |
| 命令行 | ✅ | 全部核心能力可脚本化，便于 CI 与排障 |
| 桌面 GUI | ✅ | Tauri 2 + 原生 Web 前端（无 Node 打包步骤），PCL 式左侧导航 + 下载页内分栏 |
| 联机（陶瓦） | ✅ | 一键安装 [Terracotta](https://github.com/burningtnt/Terracotta)，通过其本地 HTTP API 建房/加入/**直接显示房间码**；不重新分发其代码 |
| 模组加载器安装 | 🚧 | 已能加载 `inheritsFrom` 的实例并启动；自动安装加载器待做 |
| 整合包 / 资源包 / 光影 | 🚧 | 规划中 |
| 崩溃分析 | 🚧 | 规划中 |

## 下载线路

这是本项目与 PCL 现状最主要的差异点。默认线路会**先并发测速**，再按实测延迟选择源，失败自动回退：

```
        ┌────────── 官方源（piston-meta / libraries / resources）──────────┐
请求 →  │                                                                  │
        └────────── CDN 加速（BMCLAPI，路径一一对应）─────────────────────┘
                     ↑ 默认优先（中国大陆友好）      ↑ 失败自动回退
```

* `cn`（默认）：CDN ↔ 官方，按延迟排序，失败自动切换；
* `global`：官方优先，CDN 回退；
* `official`：仅官方源。

每个 URL 都会被重写映射（`libraries.minecraft.net → bmclapi2.bangbang93.com/maven` 等），
下载器对每个文件在多个源之间重试，**落地前一定校验 SHA-1**，并用 `Range` 断点续传。

Mod 下载只走官方 API：

* **Modrinth** — `https://api.modrinth.com/v2`，无需 Key；
* **CurseForge** — `https://api.curseforge.com/v1`，需要免费 API Key（设置页填写）。
  作者关闭第三方下载的文件会明确提示手动下载，而不是绕过。

## 界面

沿用 PCL 的信息架构：左侧竖向导航，页面内再分栏。主色相为蓝色（指南要求：Windows 上不得出现类似 PCL 赞助换色的功能）：

* **主页**：选实例、账户、启动、实时日志、正版提示
* **下载**：左侧分栏统一承载「游戏本体 / Mod / 整合包 / 资源包 / 光影 / 数据包 / 世界」；
  右上角先选**目标实例**，Mod 搜索会自动用该实例的 Minecraft 版本与加载器过滤
  （未选实例时不能搜 Mod——这是刻意模仿 PCL 的行为）
* **实例**：已安装版本、校验、补全、设为下载目标
* **联机**：陶瓦联机（Terracotta）一键安装、启动、创建/加入房间，**房间码直接显示并一键复制**（对接其官方 HTTP API），成员与直连地址同屏可见
* **设置**：游戏目录（可点「浏览…」用系统目录选择器更换）、内存、窗口、线路、CurseForge Key、Java 路径
* **更多**：版本信息、署名与许可

动效参考 PCL：页面切换淡入上移、导航左侧高亮条、列表逐条错峰入场、卡片悬浮抬升、
按钮按下微缩、进度条平滑推进（全部为 CSS 过渡，无第三方动画库）。

## 构建

```bash
# 依赖：Rust stable ≥ 1.85；Windows 需 MSVC 或 MinGW-w64
git clone https://github.com/mc-githu/pcl-rs
cd pcl-rs

cargo test --workspace          # 单元测试
cargo build --release           # 核心库 + pcl CLI
```

桌面端（需要 [Tauri 2 前置条件](https://tauri.app/start/prerequisites/)，即 WebView2 与 MSVC）：

```bash
cd src-tauri
cargo build --release
```

前端在 `web/`，是纯静态 HTML/CSS/JS，无打包步骤；`tauri.conf.json` 直接把它作为 `frontendDist`。

## 命令行

```bash
pcl list                        # 可安装版本
pcl install 1.20.1 --dry-run    # 只打印下载计划
pcl install 1.20.1              # 安装（CDN 加速 + 官方回退）
pcl verify 1.20.1               # 校验完整性
pcl java                        # 本机 Java
pcl account offline Steve       # 离线账户
pcl account ms                  # 微软设备码登录
pcl launch 1.20.1 --memory 4096 # 启动（缺失文件自动补全）
pcl launch 1.20.1 --dry-run     # 只打印启动命令行
pcl mods search sodium --version 1.20.1 --loader fabric
pcl mods download sodium --version 1.20.1 --loader fabric
```

> 微软登录需要自己的 Azure 应用 ID（公共客户端 + 启用设备代码流）。
> 本项目**不内置**共享 client id：通过 `PCL_MS_CLIENT_ID` 环境变量或 `--client-id` 传入。

## 架构

```
pcl-rs/
├── crates/
│   ├── core/     版本 JSON 模型、规则求值、Maven 坐标、目录布局、离线 UUID
│   ├── net/      CDN 镜像重写、自适应测速、多源回退 HTTP、并发下载器
│   ├── auth/     离线账户、微软设备码登录、账户存储
│   ├── install/  安装计划与执行：libraries / natives / assets / log4j
│   ├── launch/   Java 探测、命令行构造、进程启动与日志泵
│   ├── mods/     Modrinth 与 CurseForge 客户端
│   └── cli/      `pcl` 命令行
├── src-tauri/    Tauri 2 桌面壳（独立 workspace）
└── web/          静态前端
```

设计要点：

1. **计划与执行分离** —— `Installer::plan_full()` 产出完整下载计划，GUI 与 `--dry-run` 复用同一份逻辑。
2. **单一事实来源** —— 版本元数据只在 `pcl-core` 建模一次。
3. **不信任镜像** —— 文件落地前校验 SHA-1，失败即丢弃 `.part` 重下。
4. **GUI 是薄层** —— 每个 GUI 命令都有等价 CLI 能力，核心逻辑可脱离窗口测试。

## 关于与署名

* 本项目是**第三方独立实现**，与 PCL 官方无关。名称以 PCL 开头并带「RS（Rust 重制）」后缀。
* 致敬并署名：**龙腾猫跃**（PCL / Plain Craft Launcher 作者）
  * PCL 仓库：<https://github.com/Meloong-Git/PCL>
  * 赞助支持 PCL：<https://afdian.com/a/LTCat>
* 遵守 [《PCL 存储库合理使用指南》](LICENCE-PCL.md)：已署名并附赞助链接、源码公开、保留指南文件、
  启动前给出正版购买劝导、界面主色相为蓝色且不含赞助解锁类功能。

## 许可证

* 本项目代码：**GPL-3.0-or-later**（见 [LICENSE](LICENSE)）
* PCL 相关署名与指南：[LICENCE-PCL.md](LICENCE-PCL.md)

Minecraft 是 Mojang Studios 的商标。本项目不包含、也不重新分发任何 Minecraft 游戏文件；
所有游戏内容均由用户在本地从官方源或授权 CDN 镜像下载。
