<script setup lang="ts">
import { onMounted, ref, watch } from "vue";
import type { AdminField, AdminModel } from "../generated/adminSchema";

type RelationOption = { id: number; label: string };

const props = defineProps<{
  field: AdminField;
  model: AdminModel;
  modelValue: number | null | undefined;
  nullable: boolean;
}>();

const emit = defineEmits<{
  "update:modelValue": [value: number | null];
}>();

const search = ref("");
const options = ref<RelationOption[]>([]);
const loading = ref(false);
const error = ref("");
let searchTimer: ReturnType<typeof setTimeout> | undefined;

function labelFor(value: Record<string, unknown>) {
  const label = value.name ?? value.title ?? value.username ?? value.email;
  return label === undefined ? `#${value.id}` : String(label);
}

function searchParam() {
  return props.model.filters.find((filter) => filter.name === "search")?.name
    ?? props.model.filters.find((filter) => filter.name.endsWith("__contains") && filter.type === "text")?.name;
}

function mergeOption(option: RelationOption) {
  if (!options.value.some((current) => current.id === option.id)) {
    options.value = [option, ...options.value];
  }
}

async function loadOptions() {
  loading.value = true;
  error.value = "";

  try {
    const parameter = searchParam();
    const params = search.value && parameter ? { [parameter]: search.value } : {};
    const response = await props.model.api.list({ limit: 20, ...params });
    options.value = response.results
      .filter((value) => typeof value.id === "number")
      .map((value) => ({ id: value.id as number, label: labelFor(value) }));

    if (typeof props.modelValue === "number" && !options.value.some((option) => option.id === props.modelValue)) {
      const selected = await props.model.api.retrieve(props.modelValue);
      if (typeof selected.id === "number") {
        mergeOption({ id: selected.id, label: labelFor(selected) });
      }
    }
  } catch {
    error.value = "Unable to load related objects";
  } finally {
    loading.value = false;
  }
}

function scheduleLoad() {
  if (searchTimer) {
    clearTimeout(searchTimer);
  }
  searchTimer = setTimeout(loadOptions, 250);
}

function selectValue(event: Event) {
  const value = (event.target as HTMLSelectElement | null)?.value;
  emit("update:modelValue", value ? Number(value) : null);
}

watch(() => props.model.resource, loadOptions);
watch(search, scheduleLoad);
onMounted(loadOptions);
</script>

<template>
  <div class="che-admin-relation-select">
    <div class="input-group">
      <span class="input-group-text" aria-hidden="true">⌕</span>
      <input
        v-model="search"
        class="form-control"
        :placeholder="`Search ${model.name}`"
        type="search"
      />
    </div>
    <select
      class="form-select mt-2"
      :value="modelValue ?? ''"
      :required="field.required && !field.hasDefault && !nullable"
      @change="selectValue"
    >
      <option v-if="nullable" value="">No selection</option>
      <option v-for="option in options" :key="option.id" :value="option.id">
        {{ option.label }}
      </option>
    </select>
    <small v-if="loading" class="form-text">Loading...</small>
    <small v-else-if="error" class="form-text text-danger">{{ error }}</small>
  </div>
</template>
