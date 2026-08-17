<script setup lang="ts">
import { useRouter } from "vue-router";
import axios from "axios";
import { adminApps } from "./generated/adminSchema";

const router = useRouter();

async function logout() {
  await axios.post(import.meta.env.VITE_LOGOUT_URL ?? "/api-session-auth/logout/", null, {
    withCredentials: true,
    xsrfCookieName: "csrf_token",
    xsrfHeaderName: "X-CSRF-Token",
  });
  router.push("/login");
}
</script>

<template>
  <div class="che-admin-layout">
    <aside class="che-admin-sidebar bg-dark text-white">
      <div class="d-flex align-items-center justify-content-between mb-4">
        <RouterLink class="che-admin-brand text-white text-decoration-none" to="/admin">Admin</RouterLink>
        <button class="btn btn-sm btn-outline-light d-lg-none" type="button" @click="logout">Logout</button>
      </div>

      <nav class="che-admin-nav">
        <section v-for="app in adminApps" :key="app.name" class="mb-4">
          <p class="che-admin-nav-title text-uppercase text-white-50 mb-2">{{ app.name }}</p>
          <div class="list-group list-group-flush">
            <RouterLink
              v-for="model in app.models"
              :key="model.resource"
              class="list-group-item list-group-item-action che-admin-nav-link"
              :to="`/admin/${model.resource}`"
            >
              {{ model.name }}
            </RouterLink>
          </div>
        </section>
      </nav>

      <button class="btn btn-outline-light w-100 mt-auto d-none d-lg-block" type="button" @click="logout">Logout</button>
    </aside>

    <main class="che-admin-main bg-body-tertiary">
      <RouterView />
    </main>
  </div>
</template>
