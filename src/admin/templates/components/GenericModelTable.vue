<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useRouter } from "vue-router";
import type { AdminFilter, AdminModel } from "../generated/adminSchema";

const props = defineProps<{
  model: AdminModel;
}>();

const router = useRouter();
const rows = ref<Record<string, unknown>[]>([]);
const filterValues = ref<Record<string, string | number | boolean | null>>({});
const count = ref(0);
const page = ref(1);
const pageSize = 20;
const loading = ref(false);
const error = ref("");

const columns = computed(() => props.model.fields.filter((field) => !field.writeOnly));
const pageCount = computed(() => Math.max(1, Math.ceil(count.value / pageSize)));
const firstRow = computed(() => (count.value === 0 ? 0 : (page.value - 1) * pageSize + 1));
const lastRow = computed(() => Math.min(page.value * pageSize, count.value));

function initFilters() {
  filterValues.value = Object.fromEntries(
    props.model.filters.map((filter) => [filter.name, filter.type === "boolean" ? null : ""]),
  );
}

async function loadRows() {
  loading.value = true;
  error.value = "";

  try {
    const response = await props.model.api.list({
      limit: pageSize,
      offset: (page.value - 1) * pageSize,
      ...cleanFilters(),
    });
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

  for (const filter of props.model.filters) {
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
  page.value = 1;
  await loadRows();
}

async function applyFilters() {
  page.value = 1;
  await loadRows();
}

async function goToPage(nextPage: number) {
  if (nextPage < 1 || nextPage > pageCount.value || nextPage === page.value) {
    return;
  }

  page.value = nextPage;
  await loadRows();
}

async function removeRow(row: Record<string, unknown>) {
  if (typeof row.id !== "number") {
    return;
  }

  if (!window.confirm(`Delete ${props.model.name} #${row.id}?`)) {
    return;
  }

  await props.model.api.remove(row.id);
  if (rows.value.length === 1 && page.value > 1) {
    page.value -= 1;
  }
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

watch(() => props.model.resource, async () => {
  initFilters();
  page.value = 1;
  await loadRows();
}, { immediate: true });
</script>

<template>
  <section class="container-fluid py-4 py-lg-5">
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

    <div class="che-admin-data-layout">
      <form v-if="model.filters.length > 0" class="card che-admin-filter-panel" @submit.prevent="applyFilters">
        <div class="card-body">
          <div class="d-flex align-items-end gap-2 mb-3">
            <h2 class="h5 mb-0">Filters</h2>
            <span class="badge text-bg-secondary">{{ model.filters.length }}</span>
          </div>

          <div class="d-grid gap-2">
            <div v-for="filter in model.filters" :key="filter.name">
              <label class="form-label">{{ filter.label }}</label>
              <select v-if="filter.choices" v-model="filterValues[filter.name]" class="form-select">
                <option value="">Any</option>
                <option v-for="choice in filter.choices" :key="choice" :value="choice">{{ choice }}</option>
              </select>
              <select v-else-if="filter.type === 'boolean'" v-model="filterValues[filter.name]" class="form-select">
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

          <div class="d-flex flex-column gap-2 mt-3">
            <button class="btn btn-primary" type="submit">Apply filters</button>
            <button class="btn btn-outline-secondary" type="button" @click="resetFilters">Reset</button>
          </div>
        </div>
      </form>

      <div class="che-admin-table-column">
        <div v-if="loading" class="alert alert-secondary">Loading...</div>
        <div v-else-if="error" class="alert alert-danger">{{ error }}</div>

        <div v-else class="card che-admin-table-card">
          <div class="card-header bg-white text-secondary">{{ firstRow }}–{{ lastRow }} of {{ count }} objects</div>
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
          <div class="che-admin-table-footer">
            <span class="text-secondary">Page {{ page }} of {{ pageCount }}</span>
            <div class="btn-group" role="group" aria-label="Pagination">
              <button
                class="btn btn-sm btn-outline-secondary"
                type="button"
                :disabled="page === 1 || loading"
                @click="goToPage(page - 1)"
              >
                Previous
              </button>
              <button
                class="btn btn-sm btn-outline-secondary"
                type="button"
                :disabled="page === pageCount || loading"
                @click="goToPage(page + 1)"
              >
                Next
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>
  </section>
</template>
