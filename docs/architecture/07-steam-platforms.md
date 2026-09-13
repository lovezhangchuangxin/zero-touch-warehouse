# Steam 集成与平台支持

## 平台范围

已定：仅支持 Windows 与 macOS，不支持 Linux（含 Steam Deck）。

| 平台 | 支持范围 |
| --- | --- |
| Windows | Windows 10（64 位）及以上与 Windows 11；x86_64 |
| macOS | 最低 macOS 14（随原型校准后冻结）；universal bundle（arm64 + x86_64） |

- 不承诺 Windows on ARM。
- 前端只使用 WebView2 与 WKWebView 都稳定支持的能力（渲染决策见 [05](05-frontend.md)）。
- 测试矩阵覆盖最低支持版本与最新正式版本。

## Steamworks 集成

- `steamworks` crate 封装在 `crates/desktop` 的独立 adapter 中；`sim` 与 `api` 不依赖 Steam，游戏可在无 Steam 环境下运行（开发与测试）。
- 首期使用：成就、Steam Cloud 存档。
- 不集成 Tauri updater 插件，版本更新全部走 Steam。
- 创意工坊（分享玩家程序）留作后续扩展，不纳入首期；上线前须先建立安全边界（见 [03](03-player-runtime.md)）。

## Windows

- 安装包确保 WebView2 runtime 可用（捆绑引导器或固定版本）。
- Steam Overlay 在 WebView2 上预期不可用（overlay 挂载主进程图形上下文，而 WebView2 渲染在独立浏览器进程）。作为已知取舍接受：影响 Shift+Tab 与截图弹层，不影响游戏功能；上线前实测确认。

## macOS

- 产出 universal bundle（arm64 + x86_64）。python-build-standalone 只提供单架构发行物，universal 需用 lipo 合并两个构建（在原型 D 验证）；须枚举两个发行物的动态库、原生模块和依赖布局，逐项验证架构兼容与加载路径；标准库白名单不等于发行物没有原生依赖。
- 全部二进制（含嵌入的 CPython 动态库）纳入签名与公证流程；hardened runtime 要求嵌入的动态库以同一开发者身份由内向外完成签名。
- 不得启用 App Sandbox：Steamworks 与 `com.apple.security.app-sandbox` 不兼容。

## 构建与发布

- CI 矩阵：Windows（msvc、x86_64）与 macOS（arm64、x86_64），从第一天跑通；rquickjs 在 Windows msvc 需要尽早验证构建与测试。
- 发布产物包含主程序与宿主进程（runtime host）两个二进制，均纳入 Windows 安装包与 macOS 签名公证。
- CPython 与 quickjs-ng 版本随构建锁定，写入构建元数据与存档兼容信息。

## 待定

- Steam 成就清单与云存档大小预算。
- 最低硬件配置（内存、显卡）。
- CPython universal 合并的构建细节（依赖布局与 home 解析）。
