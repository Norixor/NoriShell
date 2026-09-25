import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const host = process.env.TAURI_DEV_HOST;
const projectRoot = fileURLToPath(new URL(".", import.meta.url));

export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
  server: {
    port: 1430,
    strictPort: true,
    host: host ?? "127.0.0.1",
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1431,
        }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**", "**/target/**"],
    },
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    // macOS 13 ships a Safari 16-era WebKit; targeting Safari 13 makes current
    // Vue output depend on transforms that esbuild intentionally no longer emits.
    target: process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari16",
    minify: process.env.TAURI_ENV_DEBUG ? false : "esbuild",
    sourcemap: Boolean(process.env.TAURI_ENV_DEBUG),
    rollupOptions: {
      input: {
        main: resolve(projectRoot, "index.html"),
        toolWindow: resolve(projectRoot, "tool-window.html"),
        secureVault: resolve(projectRoot, "secure-vault.html"),
        secureBackup: resolve(projectRoot, "secure-backup.html"),
        secureCredential: resolve(projectRoot, "secure-credential.html"),
        secureSshChallenge: resolve(projectRoot, "secure-ssh-challenge.html"),
        trayPanel: resolve(projectRoot, "tray-panel.html"),
        secureDesktop: resolve(projectRoot, "secure-desktop.html"),
        securePluginHostApproval: resolve(projectRoot, "secure-plugin-host-approval.html"),
        securePluginRemoteApproval: resolve(projectRoot, "secure-plugin-remote-approval.html"),
        securePluginPermission: resolve(projectRoot, "secure-plugin-permission.html"),
        securePluginTerminalInput: resolve(projectRoot, "secure-plugin-terminal-input.html"),
        secureSshSync: resolve(projectRoot, "secure-ssh-sync.html"),
        pluginIsolated: resolve(projectRoot, "plugin-isolated.html"),
      },
    },
  },
});
