# 内嵌运行时许可文本

游戏分发的二进制内嵌两个语言运行时，其许可文本随构建锁定版本记录：

## CPython（python-build-standalone 发行物）

- 钉版：`cpython-3.13.15+20260901`（scripts/fetch-python.mjs）。
- Python 本体：Python Software Foundation License Version 2（PSF-2.0），
  全文见发行物 `install/lib/python3.13/LICENSE.txt`（Windows 为
  `install/Lib/LICENSE.txt`），发行时须随附。
- python-build-standalone 构建脚本（astral-sh）：Apache-2.0 / MIT，
  只在构建期使用，不进入发行物。
- 发行物内含 pip 与 site-packages：宿主侧能力收窄（meta_path 白名单 +
  sys.path 清理）后玩家不可达，发布打包（原型 D）时评估是否裁剪以缩小
  发行体积。

## quickjs-ng（rquickjs）

- 随 rquickjs crate 的构建 vendored，许可文本见其源码仓库；发布工程
  （原型 D）统一收编随附。
