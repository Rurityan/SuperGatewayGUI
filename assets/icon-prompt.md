# 图标生成记录

通过 Image MCP 的 `image_gen` 生图工具生成；请求透明 PNG、1024×1024。服务实际返回 1254×1254、具有真实 alpha 的 PNG，原样保存在 `icon-original.png`。

`scripts/build-icon.ps1` 保留原始文件，去除极低 alpha 噪点，按主体边界居中并留出约 12% 安全边距，生成 1024×1024 的 `icon-source.png`、九尺寸 `app.ico` 和浅色/深色背景预览。无需重新调用生图即可重建 ICO。

## 原始提示词

```text
Use case: logo-brand
Asset type: Windows desktop application icon for SuperGateway, used in EXE, title bar, taskbar and system tray.
Primary request: a simple, distinctive gateway-and-connection-nodes symbol. One bold central open gateway connects to two or three large round network nodes. Communicate bridging and connectivity.
Style: crisp, minimal flat graphic, strong simple silhouette, high contrast, restrained cohesive colors, no fine outlines or tiny details. Remain recognizable at 16x16 pixels.
Composition: square 1024x1024 canvas, centered single symbol, approximately 12 percent safe padding on every edge, balanced visual weight.
Background: genuinely transparent with clean smooth alpha edges, no background tile, no checkerboard painted into the image.
Constraints: no text, no letters, no numbers, no watermark, no mockup, no photo, no elaborate circuitry, no glow, no drop shadow.
```
