<script setup lang="ts">
import { adminApps } from "./generated/adminSchema";
</script>

<template>
  <section class="container-fluid py-4 py-lg-5">
    <div class="mb-4">
      <p class="text-uppercase text-secondary fw-semibold small mb-1">Generated admin</p>
      <h1 class="display-6 fw-semibold mb-0">Models</h1>
    </div>

    <section v-for="app in adminApps" :key="app.name" class="mb-5">
      <div class="d-flex align-items-end gap-2 mb-3">
        <h2 class="h4 mb-0">{{ app.name }}</h2>
        <span class="badge text-bg-secondary">{{ app.models.length }} models</span>
      </div>

      <div class="row g-3">
        <div v-for="model in app.models" :key="model.resource" class="col-12 col-sm-6 col-xl-4">
          <RouterLink class="card h-100 text-decoration-none che-admin-model-card" :to="`/admin/${model.resource}`">
            <div class="card-body">
              <h3 class="h5 card-title text-body mb-1">{{ model.name }}</h3>
              <p class="card-text text-secondary mb-0">{{ model.resource }}</p>
            </div>
          </RouterLink>
        </div>
      </div>
    </section>
  </section>
</template>
