import { createRouter, createWebHistory, type RouteLocationNormalized } from "vue-router";
import AdminLogin from "./admin/AdminLogin.vue";
import AdminApp from "./admin/AdminApp.vue";
import { adminRoutes } from "./admin/generated/adminRoutes";

const TOKEN_KEY = "che_rest_admin_token";

export const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: "/", redirect: "/admin" },
    { path: "/login", name: "login", component: AdminLogin },
    { path: "/admin", component: AdminApp, children: adminRoutes },
  ],
});

router.beforeEach((to: RouteLocationNormalized) => {
  const hasToken = Boolean(window.localStorage.getItem(TOKEN_KEY));
  if (to.path.startsWith("/admin") && !hasToken) {
    return { path: "/login", query: { next: to.fullPath } };
  }
  if (to.path === "/login" && hasToken) {
    return "/admin";
  }
});
