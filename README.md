# Biu

原生 Rust + GPUI 打造的哔哩哔哩音乐播放器（已全面从 Electron / Tauri 迁移至纯 Rust GPU 原生架构）。

## 核心特性

- **极致轻量与流畅**：单进程纯 Rust + GPUI 原生渲染，无浏览器内核开销，常驻内存大幅降低，支持 macOS / Windows / Linux。
- **完整 B 站音乐生态**：推荐音乐、搜索自动补全、收藏夹与合集管理、稍后再看、历史记录、关注动态、UP 主主页、多 P 分集选集、点赞与 B 站原页跳转。
- **自研音频引擎（`biu-audio`）**：Symphonia + FFmpeg 流式边下边播、64 段实时频谱、倍速播放、单曲/列表/随机播放、OS 媒体控制与全局快捷键、系统托盘。
- **SmartLyrics v2 四源智能歌词引擎**：
  - 网易云音乐 + QQ 音乐 + LRCLIB + B 站 CC 字幕四源并行匹配；
  - 专为 B 站设计的标题清洗、多 P 分集识别、搬运号过滤与时长/内容验真；
  - 支持逐字 YRC、翻译+罗马音同时显示、桌面歌词、声纹自动对齐（Auto-Lyrics）与持久化时间偏移。
- **无缝自动更新**：支持正式版（Stable）与抢鲜版（Nightly）双通道，内置 SHA-256 校验、后台流式下载、原子替换并自动重启。
- **无缝数据兼容**：继续复用原有 `com.biu.wood3n` 数据目录，登录态、设置、歌词缓存与播放队列零迁移成本。

## 目录结构

```text
├── assets/icons/        # 应用图标与 SVG Logo
├── crates/
│   ├── biu-core/        # B 站 API、SmartLyrics、Auto-Lyrics、下载、缓存、自动更新器
│   ├── biu-audio/       # 原生音频解码、流式播放、频谱、播放队列与媒体键
│   └── biu-gpui/        # GPUI 原生桌面界面（主窗口、全屏歌词、桌面歌词、迷你播放器）
└── .github/workflows/   # CI 检查与全平台 Release / Nightly 自动发布
```

## 本地开发与构建

```bash
# 运行全部单元测试与 Clippy
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# 启动开发版
cargo run -p biu-gpui --bin biu

# 编译 Release 版本
cargo build --release -p biu-gpui --bin biu
```

## 许可协议

[MIT](./LICENSE)
