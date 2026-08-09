<script setup lang="ts">
import { taskAdminModel } from "../generated/adminSchema";
import GenericModelForm from "../components/GenericModelForm.vue";
</script>

<template>
  <GenericModelForm :model="taskAdminModel" />
</template>
