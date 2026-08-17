<script setup lang="ts">
import { onMounted, ref } from "vue";

import { taskApi } from "./generated/api";
import { sessionAuth, type AuthUser } from "./generated/auth";
import type { TaskCreate } from "./generated/models";
import { useModelList } from "./generated/useModelList";

const user = ref<AuthUser | null>(null);
const newTask = ref("");
const newTaskStatus = ref<TaskCreate["status"]>("draft");
const username = ref("admin");
const password = ref("secret");
const loginError = ref("");
const { items: tasks, filters, loading, error, load } = useModelList(taskApi, {
  autoLoad: false,
  defaultFilters: { ordering: "-created_at", limit: 50 },
  reloadOnFilterChange: true,
});

async function login() {
  loginError.value = "";
  try {
    const response = await sessionAuth.login({ username: username.value, password: password.value });
    user.value = response.user;
    await load();
  } catch {
    loginError.value = "Login failed. Check the username and password.";
  }
}

async function logout() {
  await sessionAuth.logout();
  user.value = null;
  tasks.value = [];
}

async function createTask() {
  const name = newTask.value.trim();
  if (!name) return;

  try {
    await taskApi.create({ name, status: newTaskStatus.value });
    await load();
    newTask.value = "";
  } catch {
    loginError.value = "Unable to create the task.";
  }
}

onMounted(async () => {
  try {
    const response = await sessionAuth.me();
    user.value = response.user;
    await load();
  } catch {
    // An anonymous visitor sees the sign-in panel.
  }
});
</script>

<template>
  <main class="shell">
    <section class="intro">
      <p class="eyebrow">CLI fullstack example</p>
      <h1>Task desk</h1>
      <p class="lede">A small Vue client using the generated REST API and cookie session auth.</p>
    </section>

    <section v-if="!user" class="login-card">
      <h2>Sign in</h2>
      <p>Use the account created with the management command.</p>
      <form @submit.prevent="login">
        <label>
          Username
          <input v-model="username" autocomplete="username" required />
        </label>
        <label>
          Password
          <input v-model="password" type="password" autocomplete="current-password" required />
        </label>
        <button type="submit">Open task desk</button>
      </form>
    </section>

    <section v-else class="board">
      <header class="board-header">
        <div>
          <p class="eyebrow">Signed in as {{ user.username }}</p>
          <h2>Active work</h2>
        </div>
        <button class="quiet" type="button" @click="logout">Sign out</button>
      </header>

      <form class="composer" @submit.prevent="createTask">
        <input v-model="newTask" placeholder="What needs doing?" aria-label="New task" />
        <select v-model="newTaskStatus" aria-label="New task status">
          <option value="draft">Draft</option>
          <option value="in_progress">In progress</option>
          <option value="done">Done</option>
        </select>
        <button type="submit">Add task</button>
      </form>

      <div class="toolbar">
        <input v-model="filters.name__contains" placeholder="Filter tasks" aria-label="Filter tasks" />
        <button class="quiet" type="button" :disabled="loading" @click="load()">
          {{ loading ? "Refreshing..." : "Refresh" }}
        </button>
      </div>

      <p v-if="error" class="error">{{ error }}</p>
      <ol class="task-list">
        <li v-for="task in tasks" :key="task.id">
          <span class="task-id">#{{ task.id }}</span>
          <span>{{ task.name }}</span>
          <small>by {{ task.author.username }}</small>
        </li>
        <li v-if="!loading && tasks.length === 0" class="empty">No matching tasks.</li>
      </ol>
    </section>

    <p v-if="loginError && !user" class="error">{{ loginError }}</p>
  </main>
</template>
