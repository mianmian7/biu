# Mac S2 自定义协议 SIGABRT：根因与 panic=unwind 对照（GitHub Actions macos-26 arm64）

- 运行：https://github.com/mianmian7/biu/actions/runs/41772631643 （分支 `spike/tauri-m0-ci` 提交 `daf197bf`，只含 `spikes/` 代码；夹具为 afconvert 编码的原创 440 Hz 正弦波）。
- 条件：`BIU_MODE=custom`、`BIU_SOAK_SECONDS=600`、`RUST_BACKTRACE=full`、release + line-tables。矩阵只改 `CARGO_PROFILE_RELEASE_PANIC`。

| panic | exit | 事件计数 |
|---|---|---|
| abort | 134（SIGABRT） | {'startup': 1, 'transport': 1, 'environment': 1, 'dependency_probe': 1, 'lyrics_api': 1, 'url_refresh': 4, 'range': 4} |
| unwind | 0 | {'startup': 1, 'transport': 1, 'environment': 1, 'dependency_probe': 1, 'lyrics_api': 1, 'url_refresh': 110, 'range': 1789, 'aac_playing': 1, 'seek_seed': 1, 'seek': 10, 'waveform': 1, 'flac_playing': 1, 'soak_start': 1, 'soak_reload': 20, 'soak': 60, 'lyrics_poll_alive': 10, 'lyrics_frames': 1, 'complete': 1} |

## 根因

abort 组的 backtrace（节选）：

```
  18:        core[2525d0a98ae2f2bb]::panicking::panic_cannot_unwind
  19:        __invoke<()>
  20:        send<(), ()>
  21:        send_message<*mut objc2::runtime::AnyObject, (), ()>
  22:        send_message<&objc2::runtime::protocol_object::ProtocolObject<dyn objc2_web_kit::generated::__WKURLSchemeTask::WKURLSchemeTask>, (), objc2::__macro
  23:        didFinish<objc2::runtime::protocol_object::ProtocolObject<dyn objc2_web_kit::generated::__WKURLSchemeTask::WKURLSchemeTask>>
  24:        {closure#4}
  25:        call_once<wry::wkwebview::class::url_scheme_handler::start_task::{closure#2}::response::{closure_env#4}, ()>
  26:        call_once<(), wry::wkwebview::class::url_scheme_handler::start_task::{closure#2}::response::{closure_env#4}>
  27:        {closure#0}<(), core::panic::unwind_safe::AssertUnwindSafe<wry::wkwebview::class::url_scheme_handler::start_task::{closure#2}::response::{closure_e
  28:        try_objc_execute_closure<objc2::exception::catch::{closure_env#0}<(), core::panic::unwind_safe::AssertUnwindSafe<wry::wkwebview::class::url_scheme_
  29:        _objc2_exception_helper_0_1_try_catch
  30:        try_no_ret<objc2::exception::catch::{closure_env#0}<(), core::panic::unwind_safe::AssertUnwindSafe<wry::wkwebview::class::url_scheme_handler::start
  31:        catch<(), core::panic::unwind_safe::AssertUnwindSafe<wry::wkwebview::class::url_scheme_handler::start_task::{closure#2}::response::{closure_env#4}>
  32:        response
  33:        {closure#2}
  34:        call_once<wry::wkwebview::class::url_scheme_handler::start_task::{closure_env#2}, (http::response::Response<alloc::borrow::Cow<[u8]>>)>
 35:        call_once<(http::response::Response<alloc::borrow::Cow<[u8]>>), dyn core::ops::function::FnOnce<(http::response::Response<alloc::borrow::Cow<[u8]>>)
  36:        respond<alloc::borrow::Cow<[u8]>>
  37:        {closure#0}<tauri::EventLoopMessage>
  38:        call_once<tauri_runtime_wry::create_webview::{closure#8}::{closure_env#0}<tauri::EventLoopMessage>, (http::response::Response<alloc::borrow::Cow<[u
  39:        call_once<(http::response::Response<alloc::borrow::Cow<[u8]>>), (dyn core::ops::function::FnOnce<(http::response::Response<alloc::borrow::Cow<[u8]>
  40:        respond<alloc::vec::Vec<u8, alloc::alloc::Global>>
  41:        {async_block#0}
```

seek 取消旧 Range 请求后，WebKit 停掉该 `WKURLSchemeTask`；wry 0.57.1 的检查与 `didFinish` 之间有竞态，`didFinish` 抛 NSException。wry 用 `objc2::exception::catch` 接，但异常要穿过 Rust 闭包 `try_objc_execute_closure`；`panic = "abort"` 下该帧不能 unwind，于是 `panic_cannot_unwind` → abort。`panic = "unwind"` 时异常被 wry 接住并丢弃该任务，播放继续。

## 结论

- Mac 自定义协议本身可用：unwind 组 10 次 seek、1789 次 206 Range、20 轮 soak reload、600 s soak 均完成。
- M0 工程已删掉 `[profile.release] panic = "abort"`。正式工程（M1 起）在 macOS 上禁止 `panic = "abort"`。
- 未覆盖：3600 s soak 与内存斜率（runner 未跑满 60 min）、体积增量（unwind 多出的 landing pad，待 S4 重测）。

## 补测：panic=unwind 下 3600 s soak 与 S4（运行 41775659926，提交 5630de32）

- S2：exit 0，`complete`。10 次 seek 恢复 606/471/304/482/466/453/471/477/382/475 ms（均 ≤1500 ms）；10158 次 Range；110 次 url_refresh；soak 3600 s、120 轮 reload-and-range。
- 内存（app 进程树 + 启动后新出现的 WebKit XPC：Networking、2×WebContent、GPU；10 s 采样 357 点）：起点 17 MB，最大 428 MB，结束 427 MB。600 s warmup 后 OLS **+1.03 MB/min**，最后 30 min **+0.57 MB/min**。按预设规则（两段均 ≤0）**不通过，记待定**。runner 无音频设备、无真实 CDN，结论只代表本夹具。
- S4：`Biu M0_0.0.1_aarch64.dmg` **3.89 MiB（≈4.08 MB）**，满足 ≤8 MB；`.app` 被 dmg 打包步骤清理，未取得本次 `.app` 字节数。
