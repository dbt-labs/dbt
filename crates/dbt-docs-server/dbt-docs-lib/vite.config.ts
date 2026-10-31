import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'
import dts from 'unplugin-dts/vite';
import { resolve } from 'node:path';
import tailwindcss from "@tailwindcss/vite";
import { libInjectCss } from "vite-plugin-lib-inject-css";

// https://vite.dev/config/
export default defineConfig({
plugins: [
    react(),
    tailwindcss(),
    dts({
      tsconfigPath: './tsconfig.lib.json', 
      include: ['lib/**/*.ts', 'lib/**/*.tsx'], 
    }),
    libInjectCss(),
  ],
  build: {
    lib: {
      entry: resolve(__dirname, 'lib/index.ts'),
      name: 'dbt-docs-lib',
      formats: ['es', 'cjs'],
      fileName: (format) => `index.${format}.js`,
    },
    rollupOptions: {
      // Ensure peer dependencies are not bundled into your final library
      external: ["react", "react-dom", "tailwindcss"],
        output: {
          dir: 'dist',
          globals: {
            react: "React",
            "react-dom": "ReactDOM",
            tailwindcss: "tailwindcss",
          },
        },
      },
      sourcemap: false,
      emptyOutDir: false,
  },
})
