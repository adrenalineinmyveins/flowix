import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// 安卓壳前端：独立入口，产物到 .build/web-mobile-dist，供 tauri.conf.json 的
// frontendDist 引用。依赖解析向上走仓库根 node_modules，与桌面端同版本。
const frontendDir = path.dirname(fileURLToPath(import.meta.url));

export default defineConfig({
  root: frontendDir,
  plugins: [react()],
  build: {
    outDir: path.resolve(frontendDir, '../../../.build/web-mobile-dist'),
    emptyOutDir: true,
    target: 'es2020',
  },
});
