<script setup lang="ts">
import { taskAdminModel } from "../generated/adminSchema";
import GenericModelTable from "../components/GenericModelTable.vue";
</script>

<template>
  <GenericModelTable :model="taskAdminModel" />
</template>
