<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import { adminModels } from "../generated/adminSchema";
import type { AdminField, AdminModel } from "../generated/adminSchema";
import AsyncRelationSelect from "./AsyncRelationSelect.vue";

const props = defineProps<{
  model: AdminModel;
}>();

const route = useRoute();
const router = useRouter();
const form = ref<Record<string, string | number | boolean | null>>({});
const loading = ref(false);
const saving = ref(false);
const error = ref("");

const id = computed(() => Number(route.params.id));
const isEdit = computed(() => Number.isFinite(id.value));
const fields = computed(() => props.model.fields.filter((field) => !field.readOnly));

function modelFor(field: AdminField) {
  return adminModels.find((model) => model.name === field.relatedModel);
}

function relationValue(field: AdminField) {
  const value = form.value[field.name];
  return typeof value === "number" ? value : null;
}

function setRelationValue(field: AdminField, value: number | null) {
  form.value[field.name] = value;
}

function emptyValue(field: AdminField) {
  return field.type === "boolean" ? false : "";
}

function inputType(field: AdminField) {
  return field.type === "integer" || field.type === "real" ? "number" : "text";
}

async function loadObject() {
  form.value = Object.fromEntries(fields.value.map((field) => [field.name, emptyValue(field)]));

  if (!isEdit.value) {
    return;
  }

  loading.value = true;
  error.value = "";

  try {
    const object = await props.model.api.retrieve(id.value);
    const nextForm: Record<string, string | number | boolean | null> = {};
    for (const field of fields.value) {
      const directValue = object[field.name];
      const relationValue = field.relationField ? object[field.relationField] : undefined;
      const relationId = relationValue && typeof relationValue === "object"
        ? (relationValue as Record<string, unknown>).id
        : undefined;
      nextForm[field.name] = (directValue ?? relationId ?? emptyValue(field)) as string | number | boolean | null;
    }
    form.value = nextForm;
  } catch (err) {
    error.value = err instanceof Error ? err.message : "Unable to load object";
  } finally {
    loading.value = false;
  }
}

function normalizeValue(field: AdminField) {
  const value = form.value[field.name];

  if (field.type === "boolean") {
    return Boolean(value);
  }

  if (value === "") {
    if (field.nullable) {
      return null;
    }
    if (field.type === "text") {
      return "";
    }
    return undefined;
  }

  if (field.type === "integer" || field.type === "real") {
    return Number(value);
  }

  return value;
}

async function submit() {
  saving.value = true;
  error.value = "";

  const payload: Record<string, unknown> = {};
  for (const field of fields.value) {
    const value = normalizeValue(field);
    if (value !== undefined) {
      payload[field.name] = value;
    }
  }

  try {
    if (isEdit.value) {
      await props.model.api.update(id.value, payload);
    } else {
      await props.model.api.create(payload);
    }
    router.push(`/admin/${props.model.resource}`);
  } catch (err) {
    error.value = err instanceof Error ? err.message : "Unable to save object";
  } finally {
    saving.value = false;
  }
}

watch(() => [props.model.resource, route.params.id], loadObject, { immediate: true });
</script>

<template>
  <section class="container-fluid py-4 py-lg-5">
    <div class="d-flex flex-column flex-lg-row align-items-lg-center justify-content-between gap-3 mb-4">
      <div>
        <p class="text-secondary mb-1">{{ model.resource }}</p>
        <h1 class="display-6 fw-semibold mb-0">{{ isEdit ? `Edit ${model.name}` : `Create ${model.name}` }}</h1>
      </div>
      <RouterLink class="btn btn-outline-secondary" :to="`/admin/${model.resource}`">Back to list</RouterLink>
    </div>

    <div v-if="loading" class="alert alert-secondary">Loading...</div>

    <form v-else class="card che-admin-form-card" @submit.prevent="submit">
      <div class="card-body">
        <div class="row g-3">
          <div v-for="field in fields" :key="field.name" class="col-12">
            <div v-if="field.type === 'boolean'" class="form-check">
              <input :id="field.name" v-model="form[field.name]" class="form-check-input" type="checkbox" />
              <label class="form-check-label" :for="field.name">{{ field.label }}</label>
            </div>
            <select
              v-else-if="field.choices"
              :id="field.name"
              v-model="form[field.name]"
              class="form-select"
              :required="field.required && !field.hasDefault && !field.nullable"
            >
              <option v-if="field.nullable" :value="null">No selection</option>
              <option v-for="choice in field.choices" :key="choice" :value="choice">{{ choice }}</option>
            </select>
            <AsyncRelationSelect
              v-else-if="field.relatedModel && modelFor(field)"
              :model-value="relationValue(field)"
              :field="field"
              :model="modelFor(field)!"
              :nullable="field.nullable"
              @update:model-value="setRelationValue(field, $event)"
            />
            <template v-else>
              <label class="form-label" :for="field.name">{{ field.label }}</label>
              <input
                :id="field.name"
                v-model="form[field.name]"
                class="form-control"
                :required="field.required && !field.hasDefault && !field.nullable"
                :step="field.type === 'real' ? 'any' : '1'"
                :type="inputType(field)"
              />
              <div v-if="field.relatedModel" class="form-text">{{ field.relatedModel }} id</div>
            </template>
          </div>
        </div>

        <div v-if="error" class="alert alert-danger mt-4">{{ error }}</div>

        <div class="d-flex gap-2 mt-4">
          <button class="btn btn-primary" :disabled="saving" type="submit">{{ saving ? "Saving..." : "Save" }}</button>
          <RouterLink class="btn btn-outline-secondary" :to="`/admin/${model.resource}`">Cancel</RouterLink>
        </div>
      </div>
    </form>
  </section>
</template>
