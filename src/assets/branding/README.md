# 应用图标

`norishell-app-icon-design.png` 是用户选定的粗体双色 N 与瓷白底座设计原图，由 ImageGen 生成。原图外部棋盘格是 RGB 像素，不可直接打包。

`norishell-app-icon.png` 是用于打包的 1024×1024 RGBA 母版。经用户批准，以原图 1254×1254 坐标中的 `(74, 74, 1180, 1166)`、圆角半径 220 裁切轮廓；蒙版使用 4 倍采样和 Lanczos 抗锯齿，再缩至 1024×1024。只处理外部透明区域，N 与白色底座沿用选定原图。

重新生成平台资源：

```sh
pnpm tauri icon src/assets/branding/norishell-app-icon.png --output src-tauri/icons
```

Tauri 配置继续消费 `src-tauri/icons` 中的 PNG、ICNS 和 ICO。Header 独立使用 `src/assets/norishell-mark.svg`。

Android（`src-tauri/gen/android/app/src/main/res`）不直接使用 `tauri icon` 的前景图：该前景把整块底座铺到 108dp 画布的 95dp，会被自适应蒙版裁掉圆角。Android 自适应图标以底座瓷白色作背景，前景只取母版中的 N（坐标 `(212, 190, 812, 812)`）缩放到 60dp 居中；旧版 `ic_launcher.png` / `ic_launcher_round.png` 直接复制 `tauri icon` 的 Android 输出；`monochrome` 层与通知小图标用 `norishell-mark.svg` 的轮廓。
