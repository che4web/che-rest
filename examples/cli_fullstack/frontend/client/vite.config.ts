import vue from "@vitejs/plugin-vue";
import { defineConfig, loadEnv } from "vite";

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), "");
  const apiTarget = env.VITE_API_TARGET ?? "http://127.0.0.1:3001";

  return {
    plugins: [vue()],
    server: {
      proxy: {
        "/api": apiTarget,
        "/api-session-auth": apiTarget,
      },
    },
  };
});
