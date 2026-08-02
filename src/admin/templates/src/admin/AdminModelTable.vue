<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import { adminModels, type AdminFilter } from "./generated/adminSchema";

const route = useRoute();
const router = useRouter();
const rows = ref<Record<string, unknown>[]>([]);
const filterValues = ref<Record<string, string | number | boolean | null>>({});
const count = ref(0);
const loading = ref(false);
const error = ref("");

const resource = computed(() => String(route.params.resource ?? ""));
const model = computed(() => adminModels.find((item) => item.resource === resource.value));
const columns = computed(() => model.value?.fields.filter((field) => !field.writeOnly) ?? []);

function initFilters() {
  filterValues.value = Object.fromEntries(
    (model.value?.filters ?? []).map((filter) => [filter.name, filter.type === "boolean" ? null : ""]),
  );
}

async function loadRows() {
  if (!model.value) {
    rows.value = [];
    return;
  }

  loading.value = true;
  error.value = "";

  try {
    const response = await model.value.api.list({ limit: 100, ...cleanFilters() });
    rows.value = response.results;
    count.value = response.count;
  } catch (err) {
    error.value = err instanceof Error ? err.message : "Unable to load objects";
  } finally {
    loading.value = false;
  }
}

function filterInputType(filter: AdminFilter) {
  return filter.type === "integer" || filter.type === "real" ? "number" : "text";
}

function cleanFilters() {
  const params: Record<string, string | number | boolean | null> = {};

  for (const filter of model.value?.filters ?? []) {
    const value = filterValues.value[filter.name];
    if (value === "" || value === null || value === undefined) {
      continue;
    }

    params[filter.name] = filter.type === "integer" || filter.type === "real" ? Number(value) : value;
  }

  return params;
}

async function resetFilters() {
  initFilters();
  await loadRows();
}

async function removeRow(row: Record<string, unknown>) {
  if (!model.value || typeof row.id !== "number") {
    return;
  }

  if (!window.confirm(`Delete ${model.value.name} #${row.id}?`)) {
    return;
  }

  await model.value.api.remove(row.id);
  await loadRows();
}

function displayValue(value: unknown) {
  if (value === null || value === undefined) {
    return "-";
  }

  if (typeof value === "boolean") {
    return value ? "Yes" : "No";
  }

  if (typeof value === "object") {
    const objectValue = value as Record<string, unknown>;
    return objectValue.name ?? objectValue.title ?? objectValue.username ?? objectValue.email ?? objectValue.id ?? JSON.stringify(value);
  }

  return value;
}

watch(resource, async () => {
  initFilters();
  await loadRows();
}, { immediate: true });
</script>

<template>
  <section v-if="model" class="container-fluid py-4 py-lg-5">
    <div class="d-flex flex-column flex-lg-row align-items-lg-center justify-content-between gap-3 mb-4">
      <div>
        <p class="text-secondary mb-1">{{ model.resource }}</p>
        <h1 class="display-6 fw-semibold mb-0">{{ model.name }}</h1>
      </div>
      <div class="d-flex gap-2">
        <button class="btn btn-outline-secondary" type="button" @click="loadRows">Refresh</button>
        <RouterLink class="btn btn-primary" :to="`/admin/${model.resource}/new`">Create</RouterLink>
      </div>
    </div>

    <form v-if="model.filters.length > 0" class="card mb-4" @submit.prevent="loadRows">
      <div class="card-body">
        <div class="d-flex align-items-end gap-2 mb-3">
          <h2 class="h5 mb-0">Filters</h2>
          <span class="badge text-bg-secondary">{{ model.filters.length }} available</span>
        </div>

        <div class="row g-3">
          <div v-for="filter in model.filters" :key="filter.name" class="col-12 col-md-6 col-xl-3">
            <label class="form-label">{{ filter.label }}</label>
            <select v-if="filter.type === 'boolean'" v-model="filterValues[filter.name]" class="form-select">
              <option :value="null">Any</option>
              <option :value="true">Yes</option>
              <option :value="false">No</option>
            </select>
            <input
              v-else
              v-model="filterValues[filter.name]"
              class="form-control"
              :step="filter.type === 'real' ? 'any' : '1'"
              :type="filterInputType(filter)"
            />
          </div>
        </div>

        <div class="d-flex gap-2 mt-3">
          <button class="btn btn-primary" type="submit">Apply filters</button>
          <button class="btn btn-outline-secondary" type="button" @click="resetFilters">Reset</button>
        </div>
      </div>
    </form>

    <div v-if="loading" class="alert alert-secondary">Loading...</div>
    <div v-else-if="error" class="alert alert-danger">{{ error }}</div>

    <div v-else class="card">
      <div class="card-header bg-white text-secondary">{{ rows.length }} of {{ count }} objects</div>
      <div class="table-responsive">
        <table class="table table-hover align-middle mb-0">
          <thead class="table-light">
            <tr>
              <th v-for="field in columns" :key="field.name">{{ field.label }}</th>
              <th class="che-admin-actions-column">Actions</th>
            </tr>
          </thead>
          <tbody>
            <tr v-if="rows.length === 0">
              <td :colspan="columns.length + 1">No objects yet.</td>
            </tr>
            <tr v-for="row in rows" :key="String(row.id)">
              <td v-for="field in columns" :key="field.name">{{ displayValue(row[field.name]) }}</td>
              <td>
                <div class="d-flex gap-2">
                  <button class="btn btn-sm btn-outline-secondary" type="button" @click="router.push(`/admin/${model.resource}/${row.id}/edit`)">Edit</button>
                  <button class="btn btn-sm btn-outline-danger" type="button" @click="removeRow(row)">Delete</button>
                </div>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>
  </section>

  <div v-else class="container-fluid py-5">
    <div class="alert alert-danger">Unknown admin model: {{ resource }}</div>
  </div>
</template>
