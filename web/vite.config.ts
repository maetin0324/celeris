import tailwindcss from "@tailwindcss/vite";
import { tanstackRouter } from "@tanstack/router-plugin/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// SPA の build。asset は Vite の既定で内容の hash 付きの名前（dist/assets/<name>-<hash>.<ext>）になり、長期 cache できる。
export default defineConfig({
  plugins: [
    tanstackRouter({
      target: "react",
      routesDirectory: "./routes",
      generatedRouteTree: "./routeTree.gen.ts",
    }),
    react(),
    tailwindcss(),
  ],
  build: {
    outDir: "dist",
    assetsDir: "assets",
  },
});
