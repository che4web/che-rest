import { createRouter, createWebHistory, type RouteLocationNormalized } from "vue-router";
import axios from "axios";
import AdminLogin from "./admin/AdminLogin.vue";
import AdminApp from "./admin/AdminApp.vue";
import { adminRoutes } from "./admin/generated/adminRoutes";

export const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: "/", redirect: "/admin" },
    { path: "/login", name: "login", component: AdminLogin },
    { path: "/admin", component: AdminApp, children: adminRoutes },
  ],
});

router.beforeEach(async (to: RouteLocationNormalized) => {
  let authenticated = false;
  if (to.path.startsWith("/admin") || to.path === "/login") {
    try {
      await axios.get("/api-session-auth/me/", { withCredentials: true });
      authenticated = true;
    } catch {
      authenticated = false;
    }
  }
  if (to.path.startsWith("/admin") && !authenticated) {
    return { path: "/login", query: { next: to.fullPath } };
  }
  if (to.path === "/login" && authenticated) {
    return "/admin";
  }
});
