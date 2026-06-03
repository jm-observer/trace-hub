import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import { viteSingleFile } from 'vite-plugin-singlefile'

// 产物：单个自包含 index.html（JS/CSS 全内联），直接写入 Rust 的 ui 目录，
// 由 `include_str!("ui/index.html")` 嵌进二进制——保住「单二进制 + 离线自包含」。
export default defineConfig({
  plugins: [react(), viteSingleFile()],
  build: {
    outDir: '../crates/trace-hub/src/ui',
    emptyOutDir: false, // 不清空（与 Rust 源码同目录）
    assetsInlineLimit: 100000000,
    chunkSizeWarningLimit: 100000000,
    cssCodeSplit: false,
  },
})
