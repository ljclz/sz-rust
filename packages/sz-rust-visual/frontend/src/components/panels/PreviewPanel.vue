<script setup lang="ts">
import { ref } from "vue";
import { invoke } from "@/api/tauri";

const previewUrl = ref("");
const device = ref<"Desktop" | "Tablet" | "Mobile">("Desktop");
const loading = ref(false);
const error = ref<string | null>(null);

async function handleStart() {
  loading.value = true;
  error.value = null;
  try {
    previewUrl.value = await invoke<string>("preview_start", {
      feature: "current",
      device: device.value,
    });
  } catch (e) {
    error.value = String(e);
  } finally {
    loading.value = false;
  }
}

async function handleStop() {
  if (!previewUrl.value) return;
  await invoke("preview_stop", { url: previewUrl.value });
  previewUrl.value = "";
}
</script>

<template>
  <div class="preview-panel">
    <h2>{{ $t("panel.preview") }}</h2>
    <div class="controls">
      <select v-model="device">
        <option value="Desktop">{{ $t("preview.device.desktop") }}</option>
        <option value="Tablet">{{ $t("preview.device.tablet") }}</option>
        <option value="Mobile">{{ $t("preview.device.mobile") }}</option>
      </select>
      <button :disabled="loading || !!previewUrl" @click="handleStart">
        {{ $t("preview.start") }}
      </button>
      <button :disabled="!previewUrl" @click="handleStop">
        {{ $t("preview.stop") }}
      </button>
    </div>
    <div v-if="error" class="error">{{ error }}</div>
    <iframe v-if="previewUrl" :src="previewUrl" class="preview-frame" />
  </div>
</template>

<style scoped>
.preview-panel {
  padding: 20px;
  height: 100%;
  display: flex;
  flex-direction: column;
}
.controls {
  display: flex;
  gap: 8px;
  margin-bottom: 12px;
}
select,
button {
  padding: 8px 16px;
  background: #2d2d2d;
  border: 1px solid #3c3c3c;
  color: #d4d4d4;
  border-radius: 4px;
  font-size: 13px;
}
button {
  background: #09977e;
  border: none;
  color: #fff;
  cursor: pointer;
}
button:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.error {
  color: #f48771;
  margin-bottom: 8px;
}
.preview-frame {
  flex: 1;
  border: 1px solid #3c3c3c;
  border-radius: 4px;
  background: #fff;
}
</style>