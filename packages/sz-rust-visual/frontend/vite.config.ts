import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import { resolve } from "path";

export default defineConfig({
    plugins: [vue()],
    resolve: {
        alias: {
            "@": resolve(__dirname, "src"),
        },
    },
    clearScreen: false,
    server: {
        port: 1420,
        strictPort: true,
        watch: {
            ignored: ["**/src-tauri/**"],
        },
    },
    build: {
        outDir: "../dist",
        emptyOutDir: true,
        target: "es2021",
        minify: "esbuild",
    },
    test: {
        globals: true,
        environment: "jsdom",
    },
});