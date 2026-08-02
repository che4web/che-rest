<script setup lang="ts">
import axios from "axios";
import { ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { setAuthToken } from "../generated/api_client";

const route = useRoute();
const router = useRouter();
const username = ref("");
const password = ref("");
const loading = ref(false);
const error = ref("");

async function submit() {
  loading.value = true;
  error.value = "";

  try {
    const response = await axios.post<{ token: string }>(import.meta.env.VITE_AUTH_URL ?? "/api-token-auth/", {
      username: username.value,
      password: password.value,
    });
    window.localStorage.setItem("che_rest_admin_token", response.data.token);
    setAuthToken(response.data.token);
    router.push(typeof route.query.next === "string" ? route.query.next : "/admin");
  } catch (err) {
    error.value = err instanceof Error ? err.message : "Unable to log in";
  } finally {
    loading.value = false;
  }
}
</script>

<template>
  <main class="che-admin-login bg-body-tertiary">
    <form class="card shadow-sm che-admin-login-card" @submit.prevent="submit">
      <div class="card-body p-4 p-sm-5">
        <p class="text-uppercase text-secondary fw-semibold small mb-2">che-rest</p>
        <h1 class="h3 mb-4">Admin login</h1>

        <div v-if="error" class="alert alert-danger" role="alert">{{ error }}</div>

        <div class="mb-3">
          <label class="form-label" for="username">Username</label>
          <input id="username" v-model="username" autocomplete="username" class="form-control" required type="text" />
        </div>

        <div class="mb-4">
          <label class="form-label" for="password">Password</label>
          <input id="password" v-model="password" autocomplete="current-password" class="form-control" required type="password" />
        </div>

        <button class="btn btn-primary w-100" :disabled="loading" type="submit">
          {{ loading ? "Logging in..." : "Log in" }}
        </button>
      </div>
    </form>
  </main>
</template>
